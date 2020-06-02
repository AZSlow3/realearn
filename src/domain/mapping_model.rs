use crate::core::{prop, Prop};
use crate::domain::{
    MainProcessorMapping, MidiSourceModel, ModeModel, ProcessorMapping, RealTimeProcessorMapping,
    ReaperTarget, SessionContext, SharedMapping, TargetCharacter, TargetModel,
    TargetModelWithContext,
};
use helgoboss_learn::{Interval, Target, UnitValue};
use reaper_high::Fx;
use rx_util::{BoxedUnitEvent, UnitEvent};
use rxrust::prelude::ops::box_it::{BoxObservable, LocalBoxOp};
use rxrust::prelude::*;

/// A model for creating mappings (a combination of source, mode and target).
#[derive(Clone, Debug)]
pub struct MappingModel {
    pub name: Prop<String>,
    pub control_is_enabled: Prop<bool>,
    pub feedback_is_enabled: Prop<bool>,
    pub source_model: MidiSourceModel,
    pub mode_model: ModeModel,
    pub target_model: TargetModel,
}

impl Default for MappingModel {
    fn default() -> Self {
        Self {
            name: Default::default(),
            control_is_enabled: prop(true),
            feedback_is_enabled: prop(true),
            source_model: Default::default(),
            mode_model: Default::default(),
            target_model: Default::default(),
        }
    }
}

// We design mapping models as entity (in the DDD sense), so we compare them by ID, not by value.
// Because we store everything in memory instead of working with a database, the memory
// address serves us as ID. That means we just compare pointers.
//
// In all functions which don't need access to the mapping's internal state (comparisons, hashing
// etc.) we use `*const MappingModel` as parameter type because this saves the consumer from
// having to borrow the mapping (when kept in a RefCell). Whenever we can we should compare pointers
// directly, in order to prevent borrowing just to make the following comparison (the RefCell
// comparison internally calls `borrow()`!).
impl PartialEq for MappingModel {
    fn eq(&self, other: &Self) -> bool {
        self as *const _ == other as *const _
    }
}

impl MappingModel {
    pub fn with_context<'a>(&'a self, context: &'a SessionContext) -> MappingModelWithContext<'a> {
        MappingModelWithContext {
            mapping: self,
            context,
        }
    }

    pub fn reset_mode(&mut self, context: &SessionContext) {
        self.mode_model.reset_within_type();
        self.set_preferred_mode_values(context);
    }

    // Changes mode settings if there are some preferred ones for a certain source or target.
    pub fn set_preferred_mode_values(&mut self, context: &SessionContext) {
        self.mode_model
            .step_size_interval
            .set(self.with_context(context).preferred_step_size_interval())
    }

    pub fn control_relevant_prop_changed(&self) -> impl UnitEvent {
        self.source_model
            .changed()
            .merge(self.mode_model.changed())
            .merge(self.target_model.changed())
            .merge(self.control_is_enabled.changed())
    }

    pub fn target_value_changed(
        mapping: SharedMapping,
        context: SessionContext,
    ) -> impl LocalObservable<'static, Item = (), Err = ()> {
        Self::target_value_changed_observables(mapping, context).switch_on_next()
    }

    fn target_value_changed_observables(
        mapping: SharedMapping,
        context: SessionContext,
    ) -> impl LocalObservable<'static, Item = BoxedUnitEvent, Err = ()> {
        let target_changed = mapping.borrow().target_model.changed();
        target_changed.map(move |_| {
            let mapping = mapping.borrow();
            match mapping.target_model.with_context(&context).create_target() {
                Ok(t) => t.value_changed(),
                Err(_) => observable::empty().box_it(),
            }
        })
    }
}

pub struct MappingModelWithContext<'a> {
    mapping: &'a MappingModel,
    context: &'a SessionContext,
}

impl<'a> MappingModelWithContext<'a> {
    /// Creates a mapping for usage in real-time and main processors.
    ///
    /// Returns `None` if a target cannot be built because there's insufficient data available.
    /// Also returns `None` if a target condition (e.g. "track selected" or "FX focused") is not
    /// satisfied).
    pub fn create_processor_mapping(&self) -> Option<ProcessorMapping> {
        let target = self.target_with_context().create_target().ok()?;
        if !self.mapping.target_model.conditions_are_met(&target) {
            return None;
        }
        let source = self.mapping.source_model.create_source();
        let mode = self.mapping.mode_model.create_mode(&target);
        Some(ProcessorMapping::new(source, mode, target))
    }

    pub fn target_should_be_hit_with_increments(&self) -> bool {
        let target = self.target_with_context();
        target.is_known_to_want_increments()
            || (self.mapping.source_model.emits_increments() && target.is_known_to_be_discrete())
    }

    fn preferred_step_size_interval(&self) -> Interval<UnitValue> {
        match self.target_step_size() {
            Some(step_size) => Interval::new(step_size, step_size),
            None => ModeModel::default_step_size_interval(),
        }
    }

    fn target_step_size(&self) -> Option<UnitValue> {
        let target = self.target_with_context().create_target().ok()?;
        target.step_size()
    }

    fn target_with_context(&self) -> TargetModelWithContext<'_> {
        self.mapping.target_model.with_context(self.context)
    }
}
