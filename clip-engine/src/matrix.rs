use crate::{
    clip_timeline, keep_stretching, Clip, ClipChangedEvent, ClipContent, ClipSlot, RecordArgs,
    SharedRegister, SlotPlayOptions, SlotStopBehavior, StretchWorkerRequest, Timeline,
    TransportChange,
};
use crossbeam_channel::Sender;
use helgoboss_learn::UnitValue;
use reaper_high::{Item, Project, Track};
use reaper_medium::{Bpm, PositionInSeconds, ReaperVolumeValue};
use serde::{Deserialize, Serialize};
use std::error::Error;

#[derive(Debug)]
pub struct ClipMatrix<H> {
    handler: H,
    clip_slots: Vec<ClipSlot>,
    /// To communicate with the time stretching worker.
    stretch_worker_sender: Sender<StretchWorkerRequest>,
}

impl<H: ClipMatrixHandler> ClipMatrix<H> {
    pub fn new(handler: H) -> Self {
        let (stretch_worker_sender, stretch_worker_receiver) = crossbeam_channel::bounded(500);
        std::thread::spawn(move || {
            keep_stretching(stretch_worker_receiver);
        });
        Self {
            handler,
            clip_slots: (0..8).map(ClipSlot::new).collect(),
            stretch_worker_sender,
        }
    }

    pub fn process_transport_change(&mut self, change: TransportChange, project: Option<Project>) {
        let timeline = clip_timeline(project, true);
        let moment = timeline.capture_moment();
        for slot in self.clip_slots.iter_mut() {
            slot.process_transport_change(change, moment, &timeline)
                .unwrap();
        }
    }

    /// Detects clips that are finished playing and invokes a stop feedback event if not looped.
    pub fn poll_slot(
        &mut self,
        slot_index: usize,
        timeline_cursor_pos: PositionInSeconds,
        timeline_tempo: Bpm,
    ) -> Option<ClipChangedEvent> {
        self.clip_slots
            .get_mut(slot_index)
            .expect("no such slot")
            .poll(timeline_cursor_pos, timeline_tempo)
    }

    pub fn filled_slot_descriptors(&self) -> Vec<QualifiedSlotDescriptor> {
        self.clip_slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.is_filled())
            .map(|(i, s)| QualifiedSlotDescriptor {
                index: i,
                descriptor: s.descriptor().clone(),
            })
            .collect()
    }

    pub fn load_slots(
        &mut self,
        descriptors: Vec<QualifiedSlotDescriptor>,
        project: Option<Project>,
    ) -> Result<(), &'static str> {
        for slot in &mut self.clip_slots {
            let _ = slot.reset();
        }
        for desc in descriptors {
            let events = {
                let slot = get_slot_mut(&mut self.clip_slots, desc.index)?;
                slot.load(desc.descriptor, project, &self.stretch_worker_sender)?
            };
            for e in events {
                self.handler.notify_clip_changed(desc.index, e);
            }
        }
        self.handler.notify_slot_contents_changed();
        Ok(())
    }

    pub fn fill_slot_by_user(
        &mut self,
        slot_index: usize,
        content: ClipContent,
        project: Option<Project>,
    ) -> Result<(), &'static str> {
        get_slot_mut(&mut self.clip_slots, slot_index)?.fill_by_user(
            content,
            project,
            &self.stretch_worker_sender,
        )?;
        self.handler.notify_slot_contents_changed();
        Ok(())
    }

    pub fn fill_slot_with_item_source(
        &mut self,
        slot_index: usize,
        item: Item,
    ) -> Result<(), Box<dyn Error>> {
        let slot = get_slot_mut(&mut self.clip_slots, slot_index)?;
        let content = ClipContent::from_item(item, false)?;
        slot.fill_by_user(content, item.project(), &self.stretch_worker_sender)?;
        self.handler.notify_slot_contents_changed();
        Ok(())
    }

    pub fn play_clip(
        &mut self,
        project: Project,
        slot_index: usize,
        track: Option<Track>,
        options: SlotPlayOptions,
    ) -> Result<(), &'static str> {
        self.get_slot_mut(slot_index)?.play(
            project,
            track,
            options,
            clip_timeline(Some(project), false).capture_moment(),
        )
    }

    /// If repeat is not enabled and `immediately` is false, this has essentially no effect.
    pub fn stop_clip(
        &mut self,
        slot_index: usize,
        stop_behavior: SlotStopBehavior,
        project: Project,
    ) -> Result<(), &'static str> {
        self.get_slot_mut(slot_index)?.stop(
            stop_behavior,
            clip_timeline(Some(project), false).capture_moment(),
        )
    }

    pub fn record_clip(
        &mut self,
        slot_index: usize,
        project: Project,
        args: RecordArgs,
    ) -> Result<(), &'static str> {
        let slot = get_slot_mut(&mut self.clip_slots, slot_index)?;
        let register = slot.record(project, &self.stretch_worker_sender, args)?;
        let task = ClipRecordTask { register, project };
        self.handler.request_recording_input(task);
        Ok(())
    }

    pub fn pause_clip(&mut self, slot_index: usize) -> Result<(), &'static str> {
        self.get_slot_mut(slot_index)?.pause()
    }

    pub fn toggle_repeat(&mut self, slot_index: usize) -> Result<(), &'static str> {
        let event = self.get_slot_mut(slot_index)?.toggle_repeat();
        self.handler.notify_clip_changed(slot_index, event);
        Ok(())
    }

    pub fn seek_slot(
        &mut self,
        slot_index: usize,
        position: UnitValue,
    ) -> Result<(), &'static str> {
        let event = self
            .get_slot_mut(slot_index)?
            .set_proportional_position(position)?;
        if let Some(event) = event {
            self.handler.notify_clip_changed(slot_index, event);
        }
        Ok(())
    }

    pub fn set_volume(
        &mut self,
        slot_index: usize,
        volume: ReaperVolumeValue,
    ) -> Result<(), &'static str> {
        let event = self.get_slot_mut(slot_index)?.set_volume(volume);
        self.handler.notify_clip_changed(slot_index, event);
        Ok(())
    }

    pub fn set_clip_tempo_factor(
        &mut self,
        slot_index: usize,
        tempo_factor: f64,
    ) -> Result<(), &'static str> {
        self.get_slot_mut(slot_index)?
            .set_tempo_factor(tempo_factor);
        Ok(())
    }

    pub fn get_slot(&self, slot_index: usize) -> Result<&ClipSlot, &'static str> {
        self.clip_slots.get(slot_index).ok_or("no such slot")
    }

    fn get_slot_mut(&mut self, slot_index: usize) -> Result<&mut ClipSlot, &'static str> {
        self.clip_slots.get_mut(slot_index).ok_or("no such slot")
    }
}

fn get_slot_mut(slots: &mut [ClipSlot], index: usize) -> Result<&mut ClipSlot, &'static str> {
    slots.get_mut(index).ok_or("no such slot")
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct QualifiedSlotDescriptor {
    #[serde(rename = "index")]
    pub index: usize,
    #[serde(flatten)]
    pub descriptor: Clip,
}

#[derive(Debug)]
pub struct ClipRecordTask {
    pub register: SharedRegister,
    pub project: Project,
}

pub trait ClipMatrixHandler {
    fn request_recording_input(&self, task: ClipRecordTask);
    fn notify_slot_contents_changed(&mut self);
    fn notify_clip_changed(&self, slot_index: usize, event: ClipChangedEvent);
}