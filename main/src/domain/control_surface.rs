use crate::domain::{DomainEventHandler, Global, MainProcessor};
use crossbeam_channel::Receiver;
use reaper_high::{
    ChangeDetector, ControlSurfaceEvent, ControlSurfaceMiddleware, MeterControlSurfaceMiddleware,
};
use reaper_rx::ControlSurfaceRxDriver;
use std::collections::HashMap;
use std::time::Duration;
use wrap_debug::WrapDebug;

#[derive(Debug)]
pub struct RealearnControlSurfaceMiddleware<EH: DomainEventHandler> {
    logger: slog::Logger,
    change_detector: ChangeDetector,
    rx_driver: ControlSurfaceRxDriver,
    main_processors: Vec<MainProcessor<EH>>,
    main_task_receiver: Receiver<RealearnControlSurfaceMainTask<EH>>,
    server_task_receiver: Receiver<RealearnControlSurfaceServerTask>,
    meter_middleware: MeterControlSurfaceMiddleware,
    counter: u64,
}

pub enum RealearnControlSurfaceMainTask<EH: DomainEventHandler> {
    AddMainProcessor(MainProcessor<EH>),
    RemoveMainProcessor(String),
    LogDebugInfo,
}

pub enum RealearnControlSurfaceServerTask {
    ProvidePrometheusMetrics(tokio::sync::oneshot::Sender<String>),
}

impl<EH: DomainEventHandler> RealearnControlSurfaceMiddleware<EH> {
    pub fn new(
        parent_logger: &slog::Logger,
        main_task_receiver: Receiver<RealearnControlSurfaceMainTask<EH>>,
        server_task_receiver: Receiver<RealearnControlSurfaceServerTask>,
    ) -> Self {
        let logger = parent_logger.new(slog::o!("struct" => "RealearnControlSurfaceMiddleware"));
        Self {
            logger: logger.clone(),
            change_detector: ChangeDetector::new(),
            rx_driver: ControlSurfaceRxDriver::new(Global::control_surface_rx().clone()),
            main_processors: Default::default(),
            main_task_receiver,
            server_task_receiver,
            meter_middleware: MeterControlSurfaceMiddleware::new(logger),
            counter: 0,
        }
    }

    pub fn reset(&self) {
        self.change_detector.reset(|e| {
            self.rx_driver.handle_change(e);
        });
    }
}

impl<EH: DomainEventHandler> ControlSurfaceMiddleware for RealearnControlSurfaceMiddleware<EH> {
    fn run(&mut self) {
        let elapsed = MeterControlSurfaceMiddleware::measure(|| {
            for t in self.main_task_receiver.try_iter().take(10) {
                use RealearnControlSurfaceMainTask::*;
                match t {
                    AddMainProcessor(p) => {
                        self.main_processors.push(p);
                    }
                    RemoveMainProcessor(id) => {
                        self.main_processors.retain(|p| p.instance_id() != id);
                    }
                    LogDebugInfo => {
                        self.meter_middleware.log_metrics();
                    }
                }
            }
            for t in self.server_task_receiver.try_iter().take(10) {
                use RealearnControlSurfaceServerTask::*;
                match t {
                    ProvidePrometheusMetrics(sender) => {
                        let text = serde_prometheus::to_string(
                            self.meter_middleware.metrics(),
                            None,
                            HashMap::new(),
                        )
                        .unwrap();
                        sender.send(text);
                    }
                }
            }
            for mut p in &mut self.main_processors {
                p.run();
            }
            // Roughly each 10 second
            if self.counter == 30 * 10 {
                self.meter_middleware.warn_about_critical_metrics();
            } else {
                self.counter += 1;
            }
        });
        self.meter_middleware.record_run(elapsed);
    }

    fn handle_event(&self, event: ControlSurfaceEvent) {
        let elapsed = MeterControlSurfaceMiddleware::measure(|| {
            self.change_detector.process(event, |e| {
                self.rx_driver.handle_change(e);
            });
        });
        self.meter_middleware.record_event(event, elapsed);
    }
}
