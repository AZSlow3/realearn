use crate::domain::{OscDeviceId, OscOutputDevice, RealearnTargetContext, ReaperTarget};
use rosc::{OscMessage, OscPacket};
use std::cell::RefCell;

make_available_globally_in_main_thread!(DomainGlobal);

#[derive(Default)]
pub struct DomainGlobal {
    target_context: RefCell<RealearnTargetContext>,
    last_touched_target: RefCell<Option<ReaperTarget>>,
    osc_output_devices: RefCell<Vec<OscOutputDevice>>,
}

impl DomainGlobal {
    pub fn target_context() -> &'static RefCell<RealearnTargetContext> {
        &DomainGlobal::get().target_context
    }

    pub fn last_touched_target(&self) -> Option<ReaperTarget> {
        self.last_touched_target.borrow().clone()
    }

    pub fn send_osc_feedback(&self, dev_id: &OscDeviceId, msg: OscMessage) {
        let devices = self.osc_output_devices.borrow();
        if let Some(dev) = devices.iter().find(|d| d.id() == dev_id) {
            let _ = dev.send(&OscPacket::Message(msg));
        }
    }

    pub fn set_osc_output_devices(&self, devices: Vec<OscOutputDevice>) {
        *self.osc_output_devices.borrow_mut() = devices;
    }

    pub fn clear_osc_output_devices(&self) {
        self.osc_output_devices.borrow_mut().clear();
    }

    pub(super) fn set_last_touched_target(&self, target: ReaperTarget) {
        *self.last_touched_target.borrow_mut() = Some(target);
    }
}