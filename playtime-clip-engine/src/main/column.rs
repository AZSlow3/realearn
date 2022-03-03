use crate::main::{Clip, ClipRecordTask, MatrixSettings, Slot};
use crate::rt::supplier::RecorderEquipment;
use crate::rt::{
    ClipChangedEvent, ClipPlayState, ColumnCommandSender, ColumnEvent, ColumnFillSlotArgs,
    ColumnPlayClipArgs, ColumnSetClipRepeatedArgs, ColumnStopClipArgs, RecordBehavior,
    SharedColumn, WeakColumn,
};
use crate::{clip_timeline, rt, ClipEngineResult};
use crossbeam_channel::Receiver;
use enumflags2::BitFlags;
use helgoboss_learn::UnitValue;
use playtime_api as api;
use playtime_api::{
    AudioCacheBehavior, AudioTimeStretchMode, ColumnClipPlayAudioSettings, ColumnClipPlaySettings,
    ColumnClipRecordSettings, TrackRecordOrigin, VirtualResampleMode,
};
use reaper_high::{Guid, OrCurrentProject, Project, Reaper, Track};
use reaper_low::raw::preview_register_t;
use reaper_medium::{
    create_custom_owned_pcm_source, Bpm, CustomPcmSource, FlexibleOwnedPcmSource, MeasureAlignment,
    OwnedPreviewRegister, PositionInSeconds, ReaperMutex, ReaperVolumeValue,
};
use std::ptr::NonNull;
use std::sync::Arc;

pub type SharedRegister = Arc<ReaperMutex<OwnedPreviewRegister>>;

#[derive(Clone, Debug)]
pub struct Column {
    settings: ColumnSettings,
    rt_settings: rt::ColumnSettings,
    rt_command_sender: ColumnCommandSender,
    column_source: SharedColumn,
    preview_register: Option<PlayingPreviewRegister>,
    slots: Vec<Slot>,
    event_receiver: Receiver<ColumnEvent>,
    project: Option<Project>,
}

#[derive(Clone, Debug, Default)]
pub struct ColumnSettings {
    pub audio_resample_mode: Option<VirtualResampleMode>,
    pub audio_time_stretch_mode: Option<AudioTimeStretchMode>,
    pub audio_cache_behavior: Option<AudioCacheBehavior>,
}

#[derive(Clone, Debug)]
struct PlayingPreviewRegister {
    _preview_register: SharedRegister,
    play_handle: NonNull<preview_register_t>,
    track: Option<Track>,
}

impl Column {
    pub fn new(permanent_project: Option<Project>) -> Self {
        let (command_sender, command_receiver) = crossbeam_channel::bounded(500);
        let (event_sender, event_receiver) = crossbeam_channel::bounded(500);
        let source = rt::Column::new(permanent_project, command_receiver, event_sender);
        let shared_source = SharedColumn::new(source);
        Self {
            settings: Default::default(),
            rt_settings: Default::default(),
            // preview_register: {
            //     PlayingPreviewRegister::new(shared_source.clone(), track.as_ref())
            // },
            preview_register: None,
            column_source: shared_source,
            rt_command_sender: ColumnCommandSender::new(command_sender),
            slots: vec![],
            event_receiver,
            project: permanent_project,
        }
    }

    pub fn load(
        &mut self,
        api_column: api::Column,
        permanent_project: Option<Project>,
        recorder_equipment: &RecorderEquipment,
        matrix_settings: &MatrixSettings,
    ) -> ClipEngineResult<()> {
        self.clear_slots();
        // Track
        let track = if let Some(id) = api_column.clip_play_settings.track.as_ref() {
            let guid = Guid::from_string_without_braces(&id.0)?;
            Some(permanent_project.or_current_project().track_by_guid(&guid))
        } else {
            None
        };
        self.preview_register = Some(PlayingPreviewRegister::new(
            self.column_source.clone(),
            track,
        ));
        // Settings
        self.settings.audio_resample_mode =
            api_column.clip_play_settings.audio_settings.resample_mode;
        self.settings.audio_time_stretch_mode = api_column
            .clip_play_settings
            .audio_settings
            .time_stretch_mode;
        self.settings.audio_cache_behavior =
            api_column.clip_play_settings.audio_settings.cache_behavior;
        self.rt_settings.play_mode = api_column.clip_play_settings.mode.unwrap_or_default();
        self.rt_settings.clip_play_start_timing = api_column.clip_play_settings.start_timing;
        self.rt_settings.clip_play_stop_timing = api_column.clip_play_settings.stop_timing;
        self.rt_command_sender
            .update_settings(self.rt_settings.clone());
        // Slots
        for api_slot in api_column.slots.unwrap_or_default() {
            if let Some(api_clip) = api_slot.clip {
                let clip = Clip::load(api_clip);
                self.fill_slot_internal(
                    api_slot.row,
                    clip,
                    permanent_project,
                    recorder_equipment,
                    matrix_settings,
                )?;
            }
        }
        Ok(())
    }

    pub fn clear_slots(&mut self) {
        self.slots.clear();
        self.rt_command_sender.clear_slots();
    }

    pub fn slot(&self, index: usize) -> Option<&Slot> {
        self.slots.get(index)
    }

    pub fn save(&self) -> api::Column {
        let track_id = self.preview_register.as_ref().and_then(|reg| {
            reg.track
                .as_ref()
                .map(|t| t.guid().to_string_without_braces())
                .map(api::TrackId)
        });
        api::Column {
            clip_play_settings: ColumnClipPlaySettings {
                mode: Some(self.rt_settings.play_mode),
                track: track_id,
                start_timing: None,
                stop_timing: None,
                audio_settings: ColumnClipPlayAudioSettings {
                    resample_mode: self.settings.audio_resample_mode.clone(),
                    time_stretch_mode: self.settings.audio_time_stretch_mode.clone(),
                    cache_behavior: self.settings.audio_cache_behavior.clone(),
                },
            },
            clip_record_settings: ColumnClipRecordSettings {
                track: None,
                origin: TrackRecordOrigin::TrackInput,
            },
            slots: {
                let slots = self
                    .slots
                    .iter()
                    .enumerate()
                    .filter_map(|(i, s)| {
                        if let Some(clip) = &s.clip {
                            let api_slot = api::Slot {
                                row: i,
                                clip: Some(clip.save()),
                            };
                            Some(api_slot)
                        } else {
                            None
                        }
                    })
                    .collect();
                Some(slots)
            },
        }
    }

    pub fn source(&self) -> WeakColumn {
        self.column_source.downgrade()
    }

    fn fill_slot_internal(
        &mut self,
        row: usize,
        mut clip: Clip,
        permanent_project: Option<Project>,
        recorder_equipment: &RecorderEquipment,
        matrix_settings: &MatrixSettings,
    ) -> ClipEngineResult<()> {
        let rt_clip = clip.create_real_time_clip(
            permanent_project,
            recorder_equipment,
            matrix_settings,
            &self.settings,
        )?;
        clip.connect_to(&rt_clip);
        get_slot_mut(&mut self.slots, row).clip = Some(clip);
        let args = ColumnFillSlotArgs {
            slot_index: row,
            clip: rt_clip,
        };
        self.rt_command_sender.fill_slot(args);
        Ok(())
    }

    pub fn poll(&mut self, _timeline_tempo: Bpm) -> Vec<(usize, ClipChangedEvent)> {
        // Process source events and generate clip change events
        let mut change_events = vec![];
        while let Ok(evt) = self.event_receiver.try_recv() {
            use ColumnEvent::*;
            let change_event = match evt {
                ClipPlayStateChanged {
                    slot_index,
                    play_state,
                } => {
                    get_clip_mut(&mut self.slots, slot_index).update_play_state(play_state);
                    Some((slot_index, ClipChangedEvent::PlayState(play_state)))
                }
                ClipFrameCountUpdated {
                    slot_index,
                    frame_count,
                } => {
                    get_clip_mut(&mut self.slots, slot_index).update_frame_count(frame_count);
                    None
                }
            };
            if let Some(evt) = change_event {
                change_events.push(evt);
            }
        }
        // Add position updates
        let pos_change_events = self.slots.iter().enumerate().filter_map(|(row, slot)| {
            let clip = slot.clip.as_ref()?;
            if clip.play_state().is_advancing() {
                let proportional_pos = clip.proportional_pos().unwrap_or(UnitValue::MIN);
                let event = ClipChangedEvent::ClipPosition(proportional_pos);
                Some((row, event))
            } else {
                None
            }
        });
        change_events.extend(pos_change_events);
        change_events
    }

    pub fn play_clip(&self, args: ColumnPlayClipArgs) {
        self.rt_command_sender.play_clip(args);
    }

    pub fn stop_clip(&self, args: ColumnStopClipArgs) {
        self.rt_command_sender.stop_clip(args);
    }

    pub fn set_clip_looped(&self, args: ColumnSetClipRepeatedArgs) {
        self.rt_command_sender.set_clip_looped(args);
    }

    pub fn pause_clip(&self, slot_index: usize) {
        self.rt_command_sender.pause_clip(slot_index);
    }

    pub fn seek_clip(&self, slot_index: usize, desired_pos: UnitValue) {
        self.rt_command_sender.seek_clip(slot_index, desired_pos);
    }

    pub fn set_clip_volume(&self, slot_index: usize, volume: ReaperVolumeValue) {
        self.rt_command_sender.set_clip_volume(slot_index, volume);
    }

    pub fn toggle_clip_looped(&mut self, slot_index: usize) -> ClipEngineResult<ClipChangedEvent> {
        let clip = get_slot_mut(&mut self.slots, slot_index)
            .clip
            .as_mut()
            .ok_or("no clip")?;
        let looped = clip.toggle_looped();
        let args = ColumnSetClipRepeatedArgs { slot_index, looped };
        self.set_clip_looped(args);
        Ok(ClipChangedEvent::ClipLooped(looped))
    }

    pub fn clip_position_in_seconds(&self, slot_index: usize) -> Option<PositionInSeconds> {
        let clip = get_slot(&self.slots, slot_index).ok()?.clip.as_ref()?;
        let timeline = clip_timeline(self.project, false);
        Some(clip.position_in_seconds(&timeline))
    }

    pub fn clip_play_state(&self, slot_index: usize) -> Option<ClipPlayState> {
        let clip = get_slot(&self.slots, slot_index).ok()?.clip.as_ref()?;
        Some(clip.play_state())
    }

    pub fn clip_repeated(&self, slot_index: usize) -> Option<bool> {
        let clip = get_slot(&self.slots, slot_index).ok()?.clip.as_ref()?;
        Some(clip.data().looped)
    }

    pub fn clip_volume(&self, _slot_index: usize) -> Option<ReaperVolumeValue> {
        // TODO-high implement
        // let clip = get_slot(&self.slots, slot_index).ok()?.clip.as_ref()?;
        Some(Default::default())
    }

    pub fn proportional_clip_position(&self, row: usize) -> Option<UnitValue> {
        get_slot(&self.slots, row)
            .ok()?
            .clip
            .as_ref()?
            .proportional_pos()
    }

    pub fn record_clip(
        &mut self,
        slot_index: usize,
        behavior: RecordBehavior,
        equipment: RecorderEquipment,
    ) -> ClipEngineResult<ClipRecordTask> {
        self.with_source_mut(|s| s.record_clip(slot_index, behavior, equipment))?;
        let task = ClipRecordTask {
            column_source: self.column_source.clone(),
            slot_index,
        };
        Ok(task)
    }

    fn with_source_mut<R>(&mut self, f: impl FnOnce(&mut rt::Column) -> R) -> R {
        let mut guard = self.column_source.lock();
        f(&mut guard)
    }
}

impl Drop for PlayingPreviewRegister {
    fn drop(&mut self) {
        self.stop_playing_preview();
    }
}

impl PlayingPreviewRegister {
    pub fn new(source: impl CustomPcmSource + 'static, track: Option<Track>) -> Self {
        let mut register = OwnedPreviewRegister::default();
        register.set_volume(ReaperVolumeValue::ZERO_DB);
        let (out_chan, preview_track) = if let Some(t) = track.as_ref() {
            (-1, Some(t.raw()))
        } else {
            (0, None)
        };
        register.set_out_chan(out_chan);
        register.set_preview_track(preview_track);
        let source = create_custom_owned_pcm_source(source);
        register.set_src(Some(FlexibleOwnedPcmSource::Custom(source)));
        let preview_register = Arc::new(ReaperMutex::new(register));
        let play_handle = start_playing_preview(&preview_register, track.as_ref());
        Self {
            _preview_register: preview_register,
            play_handle,
            track,
        }
    }

    fn stop_playing_preview(&mut self) {
        if let Some(track) = &self.track {
            // Check prevents error message on project close.
            let project = track.project();
            // If not successful this probably means it was stopped already, so okay.
            let _ = Reaper::get()
                .medium_session()
                .stop_track_preview_2(project.context(), self.play_handle);
        } else {
            // If not successful this probably means it was stopped already, so okay.
            let _ = Reaper::get()
                .medium_session()
                .stop_preview(self.play_handle);
        };
    }
}

fn start_playing_preview(
    reg: &SharedRegister,
    track: Option<&Track>,
) -> NonNull<preview_register_t> {
    debug!("Starting preview on track {:?}", &track);
    let buffering_behavior = BitFlags::empty();
    let measure_alignment = MeasureAlignment::PlayImmediately;
    let result = if let Some(track) = track {
        Reaper::get().medium_session().play_track_preview_2_ex(
            track.project().context(),
            reg.clone(),
            buffering_behavior,
            measure_alignment,
        )
    } else {
        Reaper::get().medium_session().play_preview_ex(
            reg.clone(),
            buffering_behavior,
            measure_alignment,
        )
    };
    result.unwrap()
}

fn get_slot(slots: &[Slot], index: usize) -> ClipEngineResult<&Slot> {
    slots.get(index).ok_or("slot doesn't exist")
}

fn get_clip_mut(slots: &mut Vec<Slot>, index: usize) -> &mut Clip {
    get_slot_mut(slots, index)
        .clip
        .as_mut()
        .expect("slot not filled")
}

fn get_slot_mut(slots: &mut Vec<Slot>, index: usize) -> &mut Slot {
    if index >= slots.len() {
        slots.resize_with(index + 1, Default::default);
    }
    slots.get_mut(index).unwrap()
}
