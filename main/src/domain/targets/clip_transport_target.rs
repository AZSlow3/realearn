use crate::domain::{
    clip_play_state_unit_value, format_value_as_on_off, get_effective_tracks,
    transport_is_enabled_unit_value, CompoundChangeEvent, ControlContext, ExtendedProcessorContext,
    HitInstructionReturnValue, InstanceStateChanged, MappingCompartment, MappingControlContext,
    RealTimeControlContext, RealTimeReaperTarget, RealearnTarget, ReaperTarget, ReaperTargetType,
    TargetCharacter, TargetTypeDef, TrackDescriptor, TransportAction, UnresolvedReaperTargetDef,
    DEFAULT_TARGET,
};
use helgoboss_learn::{AbsoluteValue, ControlType, ControlValue, Target, UnitValue};
use playtime_clip_engine::main::{ClipRecordTiming, RecordArgs, RecordKind, SlotPlayOptions};
use playtime_clip_engine::rt::ClipChangedEvent;
use playtime_clip_engine::{clip_timeline, Timeline};
use reaper_high::{Project, Track};

#[derive(Debug)]
pub struct UnresolvedClipTransportTarget {
    pub track_descriptor: Option<TrackDescriptor>,
    pub slot_index: usize,
    pub action: TransportAction,
    pub play_options: SlotPlayOptions,
}

impl UnresolvedReaperTargetDef for UnresolvedClipTransportTarget {
    fn resolve(
        &self,
        context: ExtendedProcessorContext,
        compartment: MappingCompartment,
    ) -> Result<Vec<ReaperTarget>, &'static str> {
        let project = context.context.project_or_current_project();
        let basics = ClipTransportTargetBasics {
            slot_index: self.slot_index,
            action: self.action,
            play_options: self.play_options,
        };
        let targets = if let Some(desc) = self.track_descriptor.as_ref() {
            get_effective_tracks(context, &desc.track, compartment)?
                .into_iter()
                .map(|track| {
                    ReaperTarget::ClipTransport(ClipTransportTarget {
                        project,
                        track: Some(track),
                        basics: basics.clone(),
                    })
                })
                .collect()
        } else {
            vec![ReaperTarget::ClipTransport(ClipTransportTarget {
                project,
                track: None,
                basics,
            })]
        };
        Ok(targets)
    }

    fn track_descriptor(&self) -> Option<&TrackDescriptor> {
        self.track_descriptor.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClipTransportTarget {
    pub project: Project,
    pub track: Option<Track>,
    pub basics: ClipTransportTargetBasics,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClipTransportTargetBasics {
    pub slot_index: usize,
    pub action: TransportAction,
    pub play_options: SlotPlayOptions,
}

impl RealearnTarget for ClipTransportTarget {
    fn control_type_and_character(&self, _: ControlContext) -> (ControlType, TargetCharacter) {
        self.basics.action.control_type_and_character()
    }

    fn format_value(&self, value: UnitValue, _: ControlContext) -> String {
        format_value_as_on_off(value).to_string()
    }

    fn hit(
        &mut self,
        value: ControlValue,
        context: MappingControlContext,
    ) -> Result<HitInstructionReturnValue, &'static str> {
        use TransportAction::*;
        let on = value.is_on();
        let mut instance_state = context.control_context.instance_state.borrow_mut();
        let clip_matrix = instance_state.require_clip_matrix_mut();
        match self.basics.action {
            PlayStop => {
                if on {
                    clip_matrix.play_clip(self.basics.slot_index)?;
                } else {
                    clip_matrix.stop_clip(self.basics.slot_index)?;
                }
            }
            PlayPause => {
                if on {
                    clip_matrix.play_clip(self.basics.slot_index)?;
                } else {
                    clip_matrix.pause_clip_legacy(self.basics.slot_index)?;
                }
            }
            Stop => {
                if on {
                    clip_matrix.stop_clip(self.basics.slot_index)?;
                }
            }
            Pause => {
                if on {
                    clip_matrix.pause_clip_legacy(self.basics.slot_index)?;
                }
            }
            RecordStop => {
                if on {
                    let timing = {
                        let timeline = clip_timeline(Some(self.project), false);
                        let next_bar = timeline.next_bar_at(timeline.cursor_pos());
                        ClipRecordTiming::StartOnBarStopOnDemand {
                            start_bar: next_bar,
                        }
                    };
                    clip_matrix.record_clip_legacy(
                        self.basics.slot_index,
                        RecordArgs {
                            kind: RecordKind::Normal {
                                play_after: true,
                                timing,
                                detect_downbeat: true,
                            },
                        },
                    )?;
                } else {
                    clip_matrix.stop_clip(self.basics.slot_index)?;
                }
            }
            Repeat => {
                clip_matrix.toggle_repeat_legacy(self.basics.slot_index)?;
            }
        };
        Ok(None)
    }

    fn is_available(&self, _: ControlContext) -> bool {
        // TODO-medium With clip targets we should check the control context (instance state) if
        //  slot filled.
        if let Some(t) = &self.track {
            if !t.is_available() {
                return false;
            }
        }
        true
    }

    fn project(&self) -> Option<Project> {
        self.track.as_ref().map(|t| t.project())
    }

    fn track(&self) -> Option<&Track> {
        self.track.as_ref()
    }

    fn process_change_event(
        &self,
        evt: CompoundChangeEvent,
        _: ControlContext,
    ) -> (bool, Option<AbsoluteValue>) {
        match evt {
            CompoundChangeEvent::Instance(InstanceStateChanged::Clip {
                slot_index: si,
                event,
            }) if *si == self.basics.slot_index => {
                use TransportAction::*;
                match self.basics.action {
                    PlayStop | PlayPause | Stop | Pause | RecordStop => match event {
                        ClipChangedEvent::PlayState(new_state) => {
                            let uv = clip_play_state_unit_value(self.basics.action, *new_state);
                            (true, Some(AbsoluteValue::Continuous(uv)))
                        }
                        _ => (false, None),
                    },
                    Repeat => match event {
                        ClipChangedEvent::ClipRepeat(new_state) => (
                            true,
                            Some(AbsoluteValue::Continuous(transport_is_enabled_unit_value(
                                *new_state,
                            ))),
                        ),
                        _ => (false, None),
                    },
                }
            }
            _ => (false, None),
        }
    }

    fn text_value(&self, context: ControlContext) -> Option<String> {
        Some(format_value_as_on_off(self.current_value(context)?.to_unit_value()).to_string())
    }

    fn reaper_target_type(&self) -> Option<ReaperTargetType> {
        Some(ReaperTargetType::ClipTransport)
    }

    fn splinter_real_time_target(&self) -> Option<RealTimeReaperTarget> {
        use TransportAction::*;
        if matches!(self.basics.action, RecordStop | Repeat) {
            // These are not for real-time usage.
            return None;
        }
        let t = RealTimeClipTransportTarget {
            project: self.project,
            basics: self.basics.clone(),
        };
        Some(RealTimeReaperTarget::ClipTransport(t))
    }
}

impl<'a> Target<'a> for ClipTransportTarget {
    type Context = ControlContext<'a>;

    fn current_value(&self, context: ControlContext<'a>) -> Option<AbsoluteValue> {
        let instance_state = context.instance_state.borrow();
        use TransportAction::*;
        let val = match self.basics.action {
            PlayStop | PlayPause | Stop | Pause | RecordStop => {
                let play_state = instance_state
                    .clip_matrix()?
                    .clip_play_state(self.basics.slot_index)?;
                clip_play_state_unit_value(self.basics.action, play_state)
            }
            Repeat => {
                let is_looped = instance_state
                    .clip_matrix()?
                    .clip_repeated(self.basics.slot_index)?;
                transport_is_enabled_unit_value(is_looped)
            }
        };
        Some(AbsoluteValue::Continuous(val))
    }

    fn control_type(&self, context: Self::Context) -> ControlType {
        self.control_type_and_character(context).0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RealTimeClipTransportTarget {
    pub project: Project,
    pub basics: ClipTransportTargetBasics,
}

impl RealTimeClipTransportTarget {
    pub fn hit(
        &mut self,
        value: ControlValue,
        context: RealTimeControlContext,
    ) -> Result<(), &'static str> {
        use TransportAction::*;
        let on = value.is_on();
        let matrix = context.clip_matrix()?;
        match self.basics.action {
            PlayStop => {
                if on {
                    matrix.play_clip(self.basics.slot_index)
                } else {
                    matrix.stop_clip(self.basics.slot_index)
                }
            }
            PlayPause => {
                if on {
                    matrix.play_clip(self.basics.slot_index)
                } else {
                    matrix.pause_clip(self.basics.slot_index)
                }
            }
            Stop => {
                if on {
                    matrix.stop_clip(self.basics.slot_index)
                } else {
                    Ok(())
                }
            }
            Pause => {
                if on {
                    matrix.pause_clip(self.basics.slot_index)
                } else {
                    Ok(())
                }
            }
            RecordStop => Err("record not supported for real-time target"),
            Repeat => Err("setting repeated not supported for real-time target"),
        }
    }
}

impl<'a> Target<'a> for RealTimeClipTransportTarget {
    type Context = RealTimeControlContext<'a>;

    fn current_value(&self, context: RealTimeControlContext<'a>) -> Option<AbsoluteValue> {
        let column = context
            .clip_matrix()
            .ok()?
            .column(self.basics.slot_index)
            .ok()?;
        let column = column.lock();
        let clip = column.slot(0).ok()?.clip().ok()?;
        use TransportAction::*;
        let val = match self.basics.action {
            PlayStop | PlayPause | Stop | Pause | RecordStop => {
                clip_play_state_unit_value(self.basics.action, clip.play_state())
            }
            Repeat => transport_is_enabled_unit_value(clip.looped()),
        };
        Some(AbsoluteValue::Continuous(val))
    }

    fn control_type(&self, _: RealTimeControlContext<'a>) -> ControlType {
        self.basics.action.control_type_and_character().0
    }
}

pub const CLIP_TRANSPORT_TARGET: TargetTypeDef = TargetTypeDef {
    name: "Clip: Invoke transport action",
    short_name: "Clip transport",
    hint: "Experimental target, record not supported",
    supports_track: true,
    supports_slot: true,
    ..DEFAULT_TARGET
};
