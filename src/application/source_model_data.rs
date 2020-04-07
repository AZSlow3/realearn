use crate::domain::{MidiSourceKind, MidiSourceModel};
use helgoboss_learn::{MidiClockTransportMessageKind, SourceCharacter};
use helgoboss_midi::{Channel, U14, U7};
use serde::{Deserialize, Serialize};
use std::convert::TryInto;
use validator::{Validate, ValidationError, ValidationErrors};
use validator_derive::*;

/// This is the structure in which source settings are loaded and saved. It's optimized for being
/// represented as JSON. The JSON representation must be 100% backward-compatible.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Validate)]
#[serde(rename_all = "camelCase")]
#[validate(schema(function = "validate_schema"))]
pub struct SourceModelData {
    pub r#type: MidiSourceKind,
    #[validate(range(min = -1, max = 15))]
    pub channel: Option<i16>,
    #[validate(range(min = -1, max = 16383))]
    pub number: Option<i32>,
    #[validate(range(min = 0, max = 4))]
    pub character: Option<u8>,
    pub is_registered: Option<bool>,
    pub is_14_bit: Option<bool>,
    #[validate(range(min = 0, max = 2))]
    pub message: Option<u8>,
}

fn validate_schema(data: &SourceModelData) -> Result<(), ValidationError> {
    if data.r#type != MidiSourceKind::ParameterNumberValue
        && data.number.map(|n| U7::is_valid(n)) == Some(false)
    {
        let mut error = ValidationError::new("number_too_large");
        error.add_param("number".into(), &data.number);
        return Err(error);
    }
    Ok(())
}

impl SourceModelData {
    /// Applies this data to the given source model. Doesn't proceed if data is invalid.
    pub fn apply_to_source_model(
        &self,
        model: &mut MidiSourceModel,
    ) -> Result<(), ValidationErrors> {
        self.validate()?;
        model.kind.set(self.r#type);
        model.channel.set(
            self.channel
                .none_if_negative()
                .map(|v| v.try_into().unwrap()),
        );
        if self.r#type == MidiSourceKind::ParameterNumberValue {
            model.parameter_number_message_number.set(
                self.number
                    .none_if_negative()
                    .map(|v| v.try_into().unwrap()),
            )
        } else {
            model.midi_message_number.set(
                self.number
                    .none_if_negative()
                    .map(|v| v.try_into().unwrap()),
            )
        }
        {
            use SourceCharacter::*;
            let character = match self.character.unwrap_or(0) {
                0 => Range,
                1 => Switch,
                2 => Encoder1,
                3 => Encoder2,
                4 => Encoder3,
                _ => unreachable!(),
            };
            model.custom_character.set(character);
        }
        model.is_registered.set(self.is_registered);
        model.is_14_bit.set(self.is_14_bit);
        {
            use MidiClockTransportMessageKind::*;
            let transport_msg_kind = match self.message.unwrap_or(0) {
                0 => Start,
                1 => Continue,
                2 => Stop,
                _ => unreachable!(),
            };
            model
                .midi_clock_transport_message_kind
                .set(transport_msg_kind);
        }
        Ok(())
    }
}

trait NoneIfNegative {
    fn none_if_negative(&self) -> &Self;
}

impl<T: PartialOrd + From<i8>> NoneIfNegative for Option<T> {
    fn none_if_negative(&self) -> &Self {
        match self {
            Some(v) if *v >= 0.into() => self,
            _ => &None,
        }
    }
}

impl From<&MidiSourceModel<'_>> for SourceModelData {
    fn from(model: &MidiSourceModel<'_>) -> Self {
        SourceModelData {
            r#type: *model.kind.get(),
            channel: model.channel.get().map(|ch| ch.into()),
            number: if *model.kind.get() == MidiSourceKind::ParameterNumberValue {
                model
                    .parameter_number_message_number
                    .get()
                    .map(|n| n.into())
            } else {
                model.midi_message_number.get().map(|n| n.into())
            },
            character: {
                use SourceCharacter::*;
                match model.custom_character.get() {
                    Range => 0,
                    Switch => 1,
                    Encoder1 => 2,
                    Encoder2 => 3,
                    Encoder3 => 4,
                }
                .into()
            },
            is_registered: *model.is_registered.get(),
            is_14_bit: *model.is_14_bit.get(),
            message: {
                use MidiClockTransportMessageKind::*;
                match model.midi_clock_transport_message_kind.get() {
                    Start => 0,
                    Continue => 1,
                    Stop => 2,
                }
                .into()
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helgoboss_midi::test_util::*;
    use serde_json::json;
    use validator::ValidationErrors;

    #[test]
    fn deserialize_1() {
        // Given
        let json = json!(
            {
                "channel": 0,
                "character": 0,
                "is14Bit": false,
                "number": 0,
                "type": 0
            }
        );
        // When
        let data: SourceModelData = serde_json::from_value(json).unwrap();
        // Then
        assert_eq!(
            data,
            SourceModelData {
                r#type: MidiSourceKind::ControlChangeValue,
                channel: Some(0),
                number: Some(0),
                character: Some(0),
                is_registered: None,
                is_14_bit: Some(false),
                message: None
            }
        );
        assert!(data.validate().is_ok());
    }

    #[test]
    fn deserialize_2() {
        // Given
        let json = json!(
            {
                "channel": -1,
                "is14Bit": true,
                "isRegistered": true,
                "number": 12542,
                "type": 6
            }
        );
        // When
        let data: SourceModelData = serde_json::from_value(json).unwrap();
        // Then
        assert_eq!(
            data,
            SourceModelData {
                r#type: MidiSourceKind::ParameterNumberValue,
                channel: Some(-1),
                number: Some(12542),
                character: None,
                is_registered: Some(true),
                is_14_bit: Some(true),
                message: None
            }
        );
        assert!(data.validate().is_ok());
    }

    #[test]
    fn validate_1() {
        // Given
        let data = SourceModelData {
            r#type: MidiSourceKind::ParameterNumberValue,
            channel: Some(-4),
            number: Some(21000),
            character: Some(90),
            is_registered: Some(true),
            is_14_bit: Some(true),
            message: Some(80),
        };
        // When
        let result: Result<(), ValidationErrors> = data.validate();
        // Then
        assert!(result.is_err());
        let err = result.unwrap_err();
        let errors = err.errors();
        assert_eq!(errors.len(), 4);
        assert!(errors.contains_key("channel"));
        assert!(errors.contains_key("number"));
        assert!(errors.contains_key("message"));
        assert!(errors.contains_key("character"));
    }

    #[test]
    fn validate_2() {
        // Given
        let data = SourceModelData {
            r#type: MidiSourceKind::ControlChangeValue,
            channel: Some(-1),
            number: Some(500),
            character: Some(1),
            is_registered: Some(false),
            is_14_bit: None,
            message: None,
        };
        // When
        let result: Result<(), ValidationErrors> = data.validate();
        // Then
        assert!(result.is_err());
        let err = result.unwrap_err();
        let errors = err.errors();
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn apply_1() {
        // Given
        let data = SourceModelData {
            r#type: MidiSourceKind::ParameterNumberValue,
            channel: Some(8),
            number: Some(-1),
            character: None,
            is_registered: Some(true),
            is_14_bit: Some(true),
            message: None,
        };
        let mut model = MidiSourceModel::default();
        // When
        let result = data.apply_to_source_model(&mut model);
        // Then
        assert!(result.is_ok());
        assert_eq!(*model.kind.get(), MidiSourceKind::ParameterNumberValue);
        assert_eq!(*model.channel.get(), Some(channel(8)));
        assert_eq!(*model.midi_message_number.get(), None);
        assert_eq!(*model.parameter_number_message_number.get(), None);
        assert_eq!(*model.custom_character.get(), SourceCharacter::Range);
        assert_eq!(
            *model.midi_clock_transport_message_kind.get(),
            MidiClockTransportMessageKind::Start
        );
        assert_eq!(*model.is_registered.get(), Some(true));
        assert_eq!(*model.is_14_bit.get(), Some(true));
    }

    #[test]
    fn apply_2() {
        // Given
        let data = SourceModelData {
            r#type: MidiSourceKind::ClockTransport,
            channel: None,
            number: Some(112),
            character: None,
            is_registered: None,
            is_14_bit: Some(false),
            message: Some(2),
        };
        let mut model = MidiSourceModel::default();
        // When
        let result = data.apply_to_source_model(&mut model);
        assert!(result.is_ok());
        // Then
        assert_eq!(*model.kind.get(), MidiSourceKind::ClockTransport);
        assert_eq!(*model.channel.get(), None);
        assert_eq!(*model.midi_message_number.get(), Some(u7(112)));
        assert_eq!(*model.parameter_number_message_number.get(), None);
        assert_eq!(*model.custom_character.get(), SourceCharacter::Range);
        assert_eq!(
            *model.midi_clock_transport_message_kind.get(),
            MidiClockTransportMessageKind::Stop
        );
        assert_eq!(*model.is_registered.get(), None);
        assert_eq!(*model.is_14_bit.get(), Some(false));
    }

    #[test]
    fn from_1() {
        // Given
        let mut model = MidiSourceModel::default();
        model.kind.set(MidiSourceKind::ControlChangeValue);
        model.channel.set(Some(channel(15)));
        model.midi_message_number.set(Some(u7(12)));
        model.custom_character.set(SourceCharacter::Encoder2);
        model.is_14_bit.set(Some(true));
        // When
        let data: SourceModelData = (&model).into();
        // Then
        assert_eq!(
            data,
            SourceModelData {
                r#type: MidiSourceKind::ControlChangeValue,
                channel: Some(15),
                number: Some(12),
                character: Some(3),
                is_registered: None,
                is_14_bit: Some(true),
                message: Some(0),
            }
        );
    }

    #[test]
    fn from_2() {
        // Given
        let mut model = MidiSourceModel::default();
        model.kind.set(MidiSourceKind::ParameterNumberValue);
        model.channel.set(None);
        model.midi_message_number.set(Some(u7(77)));
        model.parameter_number_message_number.set(Some(u14(78)));
        model.custom_character.set(SourceCharacter::Encoder1);
        model.is_14_bit.set(Some(true));
        model.is_registered.set(Some(true));
        model
            .midi_clock_transport_message_kind
            .set(MidiClockTransportMessageKind::Continue);
        // When
        let data: SourceModelData = (&model).into();
        // Then
        assert_eq!(
            data,
            SourceModelData {
                r#type: MidiSourceKind::ParameterNumberValue,
                channel: None,
                number: Some(78),
                character: Some(2),
                is_registered: Some(true),
                is_14_bit: Some(true),
                message: Some(1),
            }
        );
    }
}
