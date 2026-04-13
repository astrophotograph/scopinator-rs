pub mod types;

pub use types::*;

use serde::Deserialize;

/// A Seestar event received from the control port.
///
/// Events are distinguished by the "Event" key in the JSON message.
/// This enum uses serde's internally-tagged representation.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "Event")]
pub enum SeestarEvent {
    Alert(AlertEventData),
    AutoFocus(AutoFocusEventData),
    AutoGoto(AutoGotoEventData),
    ContinuousExposure(serde_json::Value),
    DarkLibrary(serde_json::Value),
    DiskSpace(DiskSpaceEventData),
    Exposure(ExposureEventData),
    FocuserMove(FocuserMoveEventData),
    Initialise(InitialiseEventData),
    PiStatus(PiStatusEventData),
    RTSP(serde_json::Value),
    SaveImage(SaveImageEventData),
    ScopeGoto(ScopeGotoEventData),
    ScopeHome(ScopeHomeEventData),
    ScopeMoveToHorizon(serde_json::Value),
    ScopeTrack(ScopeTrackEventData),
    Stack(StackEventData),
    View(ViewEventData),
    WheelMove(WheelMoveEventData),
    // Less common events — capture as raw JSON for now
    Annotate(serde_json::Value),
    AutoGotoStep(serde_json::Value),
    BatchStack(serde_json::Value),
    Client(serde_json::Value),
    EqModePA(serde_json::Value),
    GoPixel(serde_json::Value),
    Internal(serde_json::Value),
    PlateSolve(serde_json::Value),
    ScanSun(serde_json::Value),
    SecondView(serde_json::Value),
    SelectCamera(serde_json::Value),
    Setting(serde_json::Value),
    #[serde(rename = "3PPA")]
    ThreePPA(serde_json::Value),
    ViewPlan(serde_json::Value),
    /// Forward-compatible: unknown event types are captured as raw JSON.
    #[serde(other)]
    Unknown,
}

impl SeestarEvent {
    /// Returns the event name string.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Alert(_) => "Alert",
            Self::AutoFocus(_) => "AutoFocus",
            Self::AutoGoto(_) => "AutoGoto",
            Self::ContinuousExposure(_) => "ContinuousExposure",
            Self::DarkLibrary(_) => "DarkLibrary",
            Self::DiskSpace(_) => "DiskSpace",
            Self::Exposure(_) => "Exposure",
            Self::FocuserMove(_) => "FocuserMove",
            Self::Initialise(_) => "Initialise",
            Self::PiStatus(_) => "PiStatus",
            Self::RTSP(_) => "RTSP",
            Self::SaveImage(_) => "SaveImage",
            Self::ScopeGoto(_) => "ScopeGoto",
            Self::ScopeHome(_) => "ScopeHome",
            Self::ScopeMoveToHorizon(_) => "ScopeMoveToHorizon",
            Self::ScopeTrack(_) => "ScopeTrack",
            Self::Stack(_) => "Stack",
            Self::View(_) => "View",
            Self::WheelMove(_) => "WheelMove",
            Self::Annotate(_) => "Annotate",
            Self::AutoGotoStep(_) => "AutoGotoStep",
            Self::BatchStack(_) => "BatchStack",
            Self::Client(_) => "Client",
            Self::EqModePA(_) => "EqModePA",
            Self::GoPixel(_) => "GoPixel",
            Self::Internal(_) => "Internal",
            Self::PlateSolve(_) => "PlateSolve",
            Self::ScanSun(_) => "ScanSun",
            Self::SecondView(_) => "SecondView",
            Self::SelectCamera(_) => "SelectCamera",
            Self::Setting(_) => "Setting",
            Self::ThreePPA(_) => "3PPA",
            Self::ViewPlan(_) => "ViewPlan",
            Self::Unknown => "Unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_pi_status_event() {
        let json = r#"{
            "Event": "PiStatus",
            "Timestamp": "1234567890.0",
            "temp": 42.5,
            "charger_status": "Charging",
            "charge_online": true,
            "battery_capacity": 85
        }"#;
        let event: SeestarEvent = serde_json::from_str(json).unwrap();
        match event {
            SeestarEvent::PiStatus(data) => {
                assert_eq!(data.temp, Some(42.5));
                assert_eq!(data.battery_capacity, Some(85));
            }
            _ => panic!("expected PiStatus"),
        }
    }

    #[test]
    fn deserialize_stack_event() {
        let json = r#"{
            "Event": "Stack",
            "Timestamp": "1234567890.0",
            "state": "frame_complete",
            "stacked_frame": 5,
            "dropped_frame": 1,
            "lapse_ms": 12000,
            "frame_errcode": 0,
            "can_annotate": true,
            "total_frame": 10,
            "error": "",
            "code": 0
        }"#;
        let event: SeestarEvent = serde_json::from_str(json).unwrap();
        match event {
            SeestarEvent::Stack(data) => {
                assert_eq!(data.stacked_frame, 5);
                assert_eq!(data.state.as_deref(), Some("frame_complete"));
            }
            _ => panic!("expected Stack"),
        }
    }

    #[test]
    fn deserialize_auto_goto_event() {
        let json = r#"{
            "Event": "AutoGoto",
            "Timestamp": "1234567890.0",
            "state": "complete",
            "lapse_ms": 30000,
            "count": 3,
            "hint": false
        }"#;
        let event: SeestarEvent = serde_json::from_str(json).unwrap();
        match event {
            SeestarEvent::AutoGoto(data) => {
                assert_eq!(data.state, Some(EventState::Complete));
                assert_eq!(data.count, 3);
            }
            _ => panic!("expected AutoGoto"),
        }
    }

    #[test]
    fn deserialize_unknown_event() {
        let json = r#"{
            "Event": "SomeFutureEvent",
            "Timestamp": "1234567890.0",
            "data": 42
        }"#;
        let event: SeestarEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, SeestarEvent::Unknown));
    }

    #[test]
    fn deserialize_scope_track_event() {
        let json = r#"{
            "Event": "ScopeTrack",
            "Timestamp": "1234567890.0",
            "state": "on",
            "tracking": true,
            "manual": false,
            "code": 0
        }"#;
        let event: SeestarEvent = serde_json::from_str(json).unwrap();
        match event {
            SeestarEvent::ScopeTrack(data) => {
                assert!(data.tracking);
                assert_eq!(data.state.as_deref(), Some("on"));
            }
            _ => panic!("expected ScopeTrack"),
        }
    }
}
