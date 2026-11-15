use serde::{Deserialize, Serialize};

/// Defines a string-valued enum that is open to values this crate does not
/// know: they deserialize into `Other(String)` instead of failing, and
/// serialize back to the same string.
macro_rules! open_string_enum {
    ($(#[$meta:meta])* $name:ident { $( $(#[$vmeta:meta])* $variant:ident = $wire:literal, )* }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub enum $name {
            $( $(#[$vmeta])* $variant, )*
            /// A value this crate does not know, exactly as the device sent it.
            Other(String),
        }

        impl $name {
            /// The value as it appears on the wire.
            pub fn as_str(&self) -> &str {
                match self {
                    $( Self::$variant => $wire, )*
                    Self::Other(other) => other,
                }
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                match value {
                    $( $wire => Self::$variant, )*
                    other => Self::Other(other.to_string()),
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = std::borrow::Cow::<str>::deserialize(deserializer)?;
                Ok(Self::from(value.as_ref()))
            }
        }
    };
}

open_string_enum! {
    /// Common event state values used across many event types.
    ///
    /// The firmware uses more states than the original five (`idle` is
    /// common), and may add others; an unrecognized one is kept in `Other`.
    EventState {
        Start = "start",
        Cancel = "cancel",
        Working = "working",
        Complete = "complete",
        Fail = "fail",
        /// Nothing is running. Common in `get_view_state`-style snapshots.
        Idle = "idle",
    }
}

open_string_enum! {
    /// Charger status reported in PiStatus events. The mapping may differ by
    /// firmware build; an unrecognized status is kept in `Other`.
    ChargerStatus {
        Discharging = "Discharging",
        Charging = "Charging",
        Full = "Full",
        NotCharging = "Not charging",
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AutoGotoEventData {
    pub state: Option<EventState>,
    #[serde(default)]
    pub lapse_ms: i64,
    #[serde(default)]
    pub count: i32,
    #[serde(default)]
    pub hint: bool,
    pub error: Option<String>,
    pub code: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StackEventData {
    pub state: Option<String>,
    #[serde(default)]
    pub lapse_ms: i64,
    #[serde(default)]
    pub frame_errcode: i32,
    #[serde(default)]
    pub stacked_frame: i32,
    #[serde(default)]
    pub dropped_frame: i32,
    #[serde(default)]
    pub can_annotate: bool,
    pub frame_type: Option<String>,
    #[serde(default)]
    pub total_frame: i32,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub code: i32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PiStatusEventData {
    pub temp: Option<f64>,
    pub charger_status: Option<ChargerStatus>,
    pub charge_online: Option<bool>,
    pub battery_capacity: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ViewEventData {
    pub state: Option<EventState>,
    #[serde(default)]
    pub lapse_ms: i64,
    pub mode: Option<String>,
    pub cam_id: Option<i32>,
    pub lp_filter: Option<bool>,
    pub gain: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScopeGotoEventData {
    pub state: Option<EventState>,
    #[serde(default)]
    pub lapse_ms: i64,
    pub cur_ra_dec: Option<(f64, f64)>,
    #[serde(default)]
    pub dist_deg: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScopeHomeEventData {
    pub state: Option<EventState>,
    #[serde(default)]
    pub lapse_ms: i64,
    #[serde(default)]
    pub close: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScopeTrackEventData {
    pub state: Option<String>,
    #[serde(default)]
    pub tracking: bool,
    #[serde(default)]
    pub manual: bool,
    pub error: Option<String>,
    #[serde(default)]
    pub code: i32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AutoFocusEventData {
    pub state: Option<EventState>,
    #[serde(default)]
    pub lapse_ms: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExposureEventData {
    pub state: Option<String>,
    #[serde(default)]
    pub lapse_ms: i64,
    #[serde(default)]
    pub exp_ms: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FocuserMoveEventData {
    pub state: Option<EventState>,
    #[serde(default)]
    pub lapse_ms: i64,
    #[serde(default)]
    pub position: i32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InitialiseEventData {
    pub state: Option<EventState>,
    #[serde(default)]
    pub lapse_ms: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AlertEventData {
    pub state: Option<EventState>,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub code: i32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WheelMoveEventData {
    pub state: Option<EventState>,
    #[serde(default)]
    pub position: i32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DiskSpaceEventData {
    #[serde(default)]
    pub used_percent: i32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SaveImageEventData {
    pub state: Option<EventState>,
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub fullname: String,
}

// Ported from pyscopinator's tests/unit/test_events.py. These pin the
// wire-string behavior of the closed enums that live in this module at the
// enum level; the event-level counterparts (parsing a full message through
// SeestarEvent) are in event/mod.rs.
#[cfg(test)]
mod tests {
    use super::*;

    // Mirrors TestEventStates::test_valid_states (the string half; the None
    // case lives at the event level in event/mod.rs): every EventState value
    // deserializes from its snake_case wire string.
    #[test]
    fn event_state_deserializes_snake_case_strings() {
        let cases = [
            ("\"start\"", EventState::Start),
            ("\"cancel\"", EventState::Cancel),
            ("\"working\"", EventState::Working),
            ("\"complete\"", EventState::Complete),
            ("\"fail\"", EventState::Fail),
            ("\"idle\"", EventState::Idle),
        ];
        for (wire, expected) in cases {
            let state = serde_json::from_str::<EventState>(wire).unwrap();
            assert_eq!(state, expected);
            // And back to the same wire string.
            assert_eq!(serde_json::to_string(&state).unwrap(), wire);
        }
    }

    // Mirrors TestEventStates::test_invalid_state_raises, with the opposite
    // outcome: EventState is open, so an unknown string is kept, not an
    // error (pydantic raises). A non-string is still an error.
    #[test]
    fn event_state_keeps_unknown_strings() {
        let state = serde_json::from_str::<EventState>("\"invalid_state_xyz\"").unwrap();
        assert_eq!(state, EventState::Other("invalid_state_xyz".into()));
        assert_eq!(state.as_str(), "invalid_state_xyz");
        assert_eq!(
            serde_json::to_string(&state).unwrap(),
            "\"invalid_state_xyz\""
        );
        assert!(serde_json::from_str::<EventState>("7").is_err());
    }

    // Mirrors TestPiStatusEvent::test_invalid_charger_status (the event-level
    // counterpart is in event/mod.rs): the four known wire strings all
    // deserialize, including the spaced "Not charging"; anything else is
    // kept as `Other` (pydantic raises).
    #[test]
    fn charger_status_deserializes_known_values_and_keeps_others() {
        let cases = [
            ("\"Discharging\"", ChargerStatus::Discharging),
            ("\"Charging\"", ChargerStatus::Charging),
            ("\"Full\"", ChargerStatus::Full),
            ("\"Not charging\"", ChargerStatus::NotCharging),
        ];
        for (wire, expected) in cases {
            assert_eq!(
                serde_json::from_str::<ChargerStatus>(wire).unwrap(),
                expected
            );
        }
        assert_eq!(
            serde_json::from_str::<ChargerStatus>("\"InvalidStatus\"").unwrap(),
            ChargerStatus::Other("InvalidStatus".into())
        );
    }
}
