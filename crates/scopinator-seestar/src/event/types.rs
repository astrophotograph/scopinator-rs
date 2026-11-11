use serde::{Deserialize, Serialize};

/// Common event state values used across many event types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventState {
    Start,
    Cancel,
    Working,
    Complete,
    Fail,
}

/// Charger status reported in PiStatus events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChargerStatus {
    Discharging,
    Charging,
    Full,
    #[serde(rename = "Not charging")]
    NotCharging,
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
        ];
        for (wire, expected) in cases {
            assert_eq!(serde_json::from_str::<EventState>(wire).unwrap(), expected);
        }
    }

    // Mirrors TestEventStates::test_invalid_state_raises: EventState is a
    // closed enum, so an unknown string is a deserialization error.
    #[test]
    fn event_state_rejects_unknown_strings() {
        let err = serde_json::from_str::<EventState>("\"invalid_state_xyz\"").unwrap_err();
        assert!(err.to_string().contains("invalid_state_xyz"));
    }

    // Mirrors TestPiStatusEvent::test_invalid_charger_status (the event-level
    // counterpart is in event/mod.rs): ChargerStatus is a closed enum whose
    // four wire strings all deserialize, including the spaced "Not charging"
    // rename; anything else errors.
    #[test]
    fn charger_status_deserializes_known_values_only() {
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
        let err = serde_json::from_str::<ChargerStatus>("\"InvalidStatus\"").unwrap_err();
        assert!(err.to_string().contains("InvalidStatus"));
    }
}
