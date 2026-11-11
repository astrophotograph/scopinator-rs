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
    /// Forward-compatible catch-all: any event whose `Event` tag is not one of
    /// the variants above deserializes here. The payload is not retained on the
    /// variant (serde's `other` requires a unit variant), but the connection
    /// reader logs the raw event name so firmware drift (new event types) is
    /// observable at runtime. See [`known_event_names`] for the modeled set.
    #[serde(other)]
    Unknown,
}

/// The set of `Event` tag strings this crate models (everything except the
/// forward-compatible [`SeestarEvent::Unknown`] catch-all).
///
/// Used by tests to assert the captured corpus contains no unmodeled events,
/// and available to callers that want to detect firmware drift.
pub fn known_event_names() -> &'static [&'static str] {
    &[
        "Alert",
        "AutoFocus",
        "AutoGoto",
        "ContinuousExposure",
        "DarkLibrary",
        "DiskSpace",
        "Exposure",
        "FocuserMove",
        "Initialise",
        "PiStatus",
        "RTSP",
        "SaveImage",
        "ScopeGoto",
        "ScopeHome",
        "ScopeMoveToHorizon",
        "ScopeTrack",
        "Stack",
        "View",
        "WheelMove",
        "Annotate",
        "AutoGotoStep",
        "BatchStack",
        "Client",
        "EqModePA",
        "GoPixel",
        "Internal",
        "PlateSolve",
        "ScanSun",
        "SecondView",
        "SelectCamera",
        "Setting",
        "3PPA",
        "ViewPlan",
    ]
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
    use serde_json::json;

    // ------------------------------------------------------------------
    // Ported from pyscopinator's tests/unit/test_events.py (55 test
    // functions; 60 cases counting TestEventStates::test_valid_states's
    // 6-way parametrize). Every test below names the Python case(s) it
    // mirrors. These tests assert what the Rust code does today; where the
    // port differs from the Python oracle the difference is pinned with a
    // comment, not fixed.
    //
    // Skipped Python cases (no Rust counterpart):
    // - TestBaseEvent::test_requires_event_and_timestamp: asserts pydantic
    //   direct-construction validation of the shared BaseEvent model. Rust
    //   has no BaseEvent — the shared "Event"/"Timestamp" fields are serde
    //   tag machinery and are not modeled on any payload struct. The
    //   parse-level analog is ported as `missing_event_field_errors`
    //   (mirrors test_missing_event_field_raises_validation_error).
    // - TestAnnotateResult::test_empty: Rust has no AnnotateResult struct;
    //   the Annotate payload is captured as raw serde_json::Value
    //   (see `annotate_event_with_result`).
    // - TestEventSerialization::test_model_dump_roundtrip and
    //   TestEventSerialization::test_json_roundtrip: SeestarEvent and every
    //   event payload struct derive only Deserialize, so there is nothing to
    //   serialize back out and no dump to round-trip.
    //
    // Rust/Python differences pinned below (production code unchanged):
    // - "Timestamp" is not modeled in Rust: extra or missing "Timestamp"
    //   keys are ignored by every payload, where pydantic requires one
    //   (`missing_timestamp_is_ignored`).
    // - Unknown tags map to SeestarEvent::Unknown instead of raising
    //   (`unknown_event_tag_parses_to_unknown`).
    // - "Internal" is a parseable variant in Rust but deliberately excluded
    //   from Python's EventTypes union (`internal_event_parses_to_variant`,
    //   docs/bugs/event-tag-set-drift.md).
    // - cur_ra_dec is Option<(f64, f64)> in Rust (a 2-element array on the
    //   wire) where Python takes a {"ra": .., "dec": ..} object
    //   (`scopegoto_event_with_coordinates`).
    // - ViewEvent defaults differ: Rust leaves lp_filter/gain/mode None,
    //   where Python defaults to False/0/"star" (`view_event_defaults`).
    // - AutoGotoEventData does not model Python's `route` field.
    // ------------------------------------------------------------------

    const TIMESTAMP: &str = "2024-01-01T12:00:00";

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

    // Characterizes the observability gap filed in
    // docs/bugs/response-drop-timeout.md (Forge task "Characterize silent
    // drop paths and write bug report"). Serde's `#[serde(other)]` requires a
    // unit variant, so an unmodeled event loses both its wire tag and its
    // payload at deserialization: consumers see only `Unknown` and cannot
    // tell which event fired, let alone what it carried. The only signal is
    // the connection reader's `warn!` in `handle_event` (control.rs), which
    // logs the raw event name at runtime. Pinned as-is, not fixed here.
    #[test]
    fn unknown_event_drops_tag_and_payload() {
        let json = r#"{
            "Event": "SomeFutureEvent",
            "Timestamp": "1234567890.0",
            "state": "complete",
            "count": 3
        }"#;
        let event: SeestarEvent = serde_json::from_str(json).unwrap();
        // The match binds nothing: `Unknown` is a unit variant, so there is
        // no payload field to read.
        match &event {
            SeestarEvent::Unknown => {}
            other => panic!("expected Unknown, got {}", other.name()),
        }
        // The wire tag ("SomeFutureEvent") is not recoverable either:
        // name() reports the catch-all.
        assert_eq!(event.name(), "Unknown");

        // A different unmodeled tag is indistinguishable from the first —
        // firmware drift is invisible to broadcast subscribers.
        let other_event: SeestarEvent =
            serde_json::from_str(r#"{"Event": "AnotherFutureEvent", "data": 99}"#).unwrap();
        assert!(matches!(&other_event, SeestarEvent::Unknown));
        assert_eq!(other_event.name(), event.name());
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

    // ==================================================================
    // Ported cases (see the header comment above for the skip list)
    // ==================================================================

    // Mirrors TestParsedEventDiscriminator's isinstance cases:
    // test_pistatus, test_autogoto, test_stack, test_focusermove,
    // test_wheelmove, test_view, test_rtsp, test_autofocus, test_scopegoto,
    // test_initialise, test_darklibrary, test_annotate, test_client_event,
    // test_continuousexposure, test_alert, test_diskspace, test_plateSolve,
    // test_selectcamera and test_batchstack. Python parses each message and
    // asserts the pydantic class; Rust asserts the SeestarEvent variant via
    // name(), which is 1:1 with the variants (an unrecognized tag would
    // parse to Unknown and report "Unknown"). Note that "Timestamp" is an
    // unknown extra key to every typed payload struct and is simply
    // ignored — pydantic also ignores extra keys by default.
    #[test]
    fn discriminator_tags_parse_to_their_variants() {
        for tag in [
            "PiStatus",           // test_pistatus
            "AutoGoto",           // test_autogoto
            "Stack",              // test_stack
            "FocuserMove",        // test_focusermove
            "WheelMove",          // test_wheelmove
            "View",               // test_view
            "RTSP",               // test_rtsp
            "AutoFocus",          // test_autofocus
            "ScopeGoto",          // test_scopegoto
            "Initialise",         // test_initialise
            "DarkLibrary",        // test_darklibrary
            "Annotate",           // test_annotate
            "Client",             // test_client_event
            "ContinuousExposure", // test_continuousexposure
            "Alert",              // test_alert
            "DiskSpace",          // test_diskspace
            "PlateSolve",         // test_plateSolve
            "SelectCamera",       // test_selectcamera
            "BatchStack",         // test_batchstack
        ] {
            let event: SeestarEvent =
                serde_json::from_value(json!({"Event": tag, "Timestamp": TIMESTAMP})).unwrap();
            assert_eq!(event.name(), tag, "tag {tag} parsed to the wrong variant");
        }
    }

    // Mirrors TestParsedEventDiscriminator::test_unknown_event_raises_validation_error.
    // Difference from Python: pydantic raises ValidationError for an unknown
    // tag; Rust's forward-compatible #[serde(other)] catch-all maps it to
    // SeestarEvent::Unknown instead (tag and payload are dropped — see
    // `unknown_event_drops_tag_and_payload` above).
    #[test]
    fn unknown_event_tag_parses_to_unknown() {
        let event: SeestarEvent =
            serde_json::from_value(json!({"Event": "UnknownXYZ", "Timestamp": TIMESTAMP})).unwrap();
        assert!(matches!(event, SeestarEvent::Unknown));
    }

    // Mirrors TestParsedEventDiscriminator::test_missing_timestamp_raises_validation_error.
    // Difference from Python: pydantic requires Timestamp and raises when it
    // is absent; Rust's payload structs do not model the field at all, so a
    // message without "Timestamp" parses and the key is ignored.
    #[test]
    fn missing_timestamp_is_ignored() {
        let event: SeestarEvent = serde_json::from_value(json!({"Event": "PiStatus"})).unwrap();
        assert!(matches!(event, SeestarEvent::PiStatus(_)));
    }

    // Mirrors TestParsedEventDiscriminator::test_missing_event_field_raises_validation_error:
    // a message without the discriminating "Event" tag is rejected in both
    // languages.
    #[test]
    fn missing_event_field_errors() {
        let err =
            serde_json::from_value::<SeestarEvent>(json!({"Timestamp": TIMESTAMP})).unwrap_err();
        assert!(err.to_string().contains("missing field `Event`"));
    }

    // Mirrors TestBaseEvent::test_minimal_construction and
    // TestPiStatusEvent::test_minimal: a bare PiStatus message yields the
    // PiStatus variant with every optional field None, and the parsed event
    // reports its own tag ("PiStatus") via name(), like Python's e.Event.
    #[test]
    fn pistatus_event_minimal() {
        let event: SeestarEvent =
            serde_json::from_value(json!({"Event": "PiStatus", "Timestamp": TIMESTAMP})).unwrap();
        assert_eq!(event.name(), "PiStatus");
        match event {
            SeestarEvent::PiStatus(data) => {
                assert_eq!(data.temp, None);
                assert_eq!(data.charger_status, None);
                assert_eq!(data.charge_online, None);
                assert_eq!(data.battery_capacity, None);
            }
            _ => panic!("expected PiStatus"),
        }
    }

    // Mirrors TestPiStatusEvent::test_full.
    #[test]
    fn pistatus_event_full() {
        let event: SeestarEvent = serde_json::from_value(json!({
            "Event": "PiStatus",
            "Timestamp": TIMESTAMP,
            "temp": 35.2,
            "charger_status": "Charging",
            "charge_online": true,
            "battery_capacity": 80,
        }))
        .unwrap();
        match event {
            SeestarEvent::PiStatus(data) => {
                assert_eq!(data.temp, Some(35.2));
                assert_eq!(data.charger_status, Some(ChargerStatus::Charging));
                assert_eq!(data.charge_online, Some(true));
                assert_eq!(data.battery_capacity, Some(80));
            }
            _ => panic!("expected PiStatus"),
        }
    }

    // Mirrors TestPiStatusEvent::test_via_discriminator.
    #[test]
    fn pistatus_event_via_discriminator() {
        let event: SeestarEvent = serde_json::from_value(json!({
            "Event": "PiStatus",
            "Timestamp": TIMESTAMP,
            "temp": 25.0,
            "charger_status": "Full",
        }))
        .unwrap();
        match event {
            SeestarEvent::PiStatus(data) => {
                assert_eq!(data.temp, Some(25.0));
                assert_eq!(data.charger_status, Some(ChargerStatus::Full));
            }
            _ => panic!("expected PiStatus"),
        }
    }

    // Mirrors TestPiStatusEvent::test_invalid_charger_status: an unmodelled
    // charger_status string is rejected in both languages. Difference: in
    // Rust the rejection is a deserialization error for the whole event
    // (there is no half-parsed event to inspect); pydantic reports a
    // ValidationError from the model constructor. The ChargerStatus wire
    // strings themselves are pinned in types.rs.
    #[test]
    fn pistatus_event_invalid_charger_status_errors() {
        let err = serde_json::from_value::<SeestarEvent>(json!({
            "Event": "PiStatus",
            "Timestamp": TIMESTAMP,
            "charger_status": "InvalidStatus",
        }))
        .unwrap_err();
        assert!(err.to_string().contains("InvalidStatus"));
    }

    // Mirrors TestStackEvent::test_defaults.
    #[test]
    fn stack_event_defaults() {
        let event: SeestarEvent =
            serde_json::from_value(json!({"Event": "Stack", "Timestamp": TIMESTAMP})).unwrap();
        match event {
            SeestarEvent::Stack(data) => {
                assert_eq!(data.stacked_frame, 0);
                assert_eq!(data.dropped_frame, 0);
                assert!(!data.can_annotate);
            }
            _ => panic!("expected Stack"),
        }
    }

    // Mirrors TestStackEvent::test_with_frames.
    #[test]
    fn stack_event_with_frames() {
        let event: SeestarEvent = serde_json::from_value(json!({
            "Event": "Stack",
            "Timestamp": TIMESTAMP,
            "stacked_frame": 50,
            "dropped_frame": 2,
            "can_annotate": true,
        }))
        .unwrap();
        match event {
            SeestarEvent::Stack(data) => {
                assert_eq!(data.stacked_frame, 50);
                assert_eq!(data.dropped_frame, 2);
            }
            _ => panic!("expected Stack"),
        }
    }

    // Mirrors TestStackEvent::test_frame_complete_state. Rust models
    // StackEventData.state as Option<String>, matching Python's plain str
    // ("frame_complete" is not an EventState value in either language).
    #[test]
    fn stack_event_frame_complete_state() {
        let event: SeestarEvent = serde_json::from_value(json!({
            "Event": "Stack",
            "Timestamp": TIMESTAMP,
            "state": "frame_complete",
        }))
        .unwrap();
        match event {
            SeestarEvent::Stack(data) => {
                assert_eq!(data.state.as_deref(), Some("frame_complete"));
            }
            _ => panic!("expected Stack"),
        }
    }

    // Mirrors TestAutoGotoEvent::test_defaults. Difference from Python:
    // AutoGotoEventData does not model Python's `route` field, so the
    // `route == []` assertion has no Rust counterpart (an incoming "route"
    // key is ignored).
    #[test]
    fn autogoto_event_defaults() {
        let event: SeestarEvent =
            serde_json::from_value(json!({"Event": "AutoGoto", "Timestamp": TIMESTAMP})).unwrap();
        match event {
            SeestarEvent::AutoGoto(data) => {
                assert_eq!(data.state, None);
                assert_eq!(data.error, None);
            }
            _ => panic!("expected AutoGoto"),
        }
    }

    // Mirrors TestAutoGotoEvent::test_with_error.
    #[test]
    fn autogoto_event_with_error() {
        let event: SeestarEvent = serde_json::from_value(json!({
            "Event": "AutoGoto",
            "Timestamp": TIMESTAMP,
            "state": "fail",
            "error": "below horizon",
        }))
        .unwrap();
        match event {
            SeestarEvent::AutoGoto(data) => {
                assert_eq!(data.state, Some(EventState::Fail));
                assert_eq!(data.error.as_deref(), Some("below horizon"));
            }
            _ => panic!("expected AutoGoto"),
        }
    }

    // Mirrors TestAutoGotoEvent::test_complete_state.
    #[test]
    fn autogoto_event_complete_state() {
        let event: SeestarEvent = serde_json::from_value(json!({
            "Event": "AutoGoto",
            "Timestamp": TIMESTAMP,
            "state": "complete",
        }))
        .unwrap();
        match event {
            SeestarEvent::AutoGoto(data) => {
                assert_eq!(data.state, Some(EventState::Complete));
            }
            _ => panic!("expected AutoGoto"),
        }
    }

    // Mirrors TestEventStates::test_valid_states (the parametrized "start",
    // "cancel", "working", "complete", "fail" values plus None): every
    // EventState value round-trips through the AutoGoto payload, and an
    // absent state key is None (the None case of the parametrize).
    #[test]
    fn autogoto_event_states() {
        for (wire, expected) in [
            ("start", EventState::Start),
            ("cancel", EventState::Cancel),
            ("working", EventState::Working),
            ("complete", EventState::Complete),
            ("fail", EventState::Fail),
        ] {
            let event: SeestarEvent = serde_json::from_value(json!({
                "Event": "AutoGoto",
                "Timestamp": TIMESTAMP,
                "state": wire,
            }))
            .unwrap();
            match event {
                SeestarEvent::AutoGoto(data) => {
                    assert_eq!(data.state, Some(expected), "state {wire}");
                }
                _ => panic!("expected AutoGoto"),
            }
        }

        // The None parametrize case: no state key on the wire.
        let event: SeestarEvent =
            serde_json::from_value(json!({"Event": "AutoGoto", "Timestamp": TIMESTAMP})).unwrap();
        match event {
            SeestarEvent::AutoGoto(data) => assert_eq!(data.state, None),
            _ => panic!("expected AutoGoto"),
        }
    }

    // Mirrors TestEventStates::test_invalid_state_raises: an unknown state
    // string is rejected in both languages (EventState is a closed enum in
    // Rust, same idea as pydantic's literal union).
    #[test]
    fn autogoto_event_invalid_state_errors() {
        let err = serde_json::from_value::<SeestarEvent>(json!({
            "Event": "AutoGoto",
            "Timestamp": TIMESTAMP,
            "state": "invalid_state_xyz",
        }))
        .unwrap_err();
        assert!(err.to_string().contains("invalid_state_xyz"));
    }

    // Mirrors TestFocuserMoveEvent::test_defaults.
    #[test]
    fn focusermove_event_defaults() {
        let event: SeestarEvent =
            serde_json::from_value(json!({"Event": "FocuserMove", "Timestamp": TIMESTAMP}))
                .unwrap();
        match event {
            SeestarEvent::FocuserMove(data) => {
                assert_eq!(data.position, 0);
                assert_eq!(data.state, None);
            }
            _ => panic!("expected FocuserMove"),
        }
    }

    // Mirrors TestFocuserMoveEvent::test_with_position.
    #[test]
    fn focusermove_event_with_position() {
        let event: SeestarEvent = serde_json::from_value(json!({
            "Event": "FocuserMove",
            "Timestamp": TIMESTAMP,
            "state": "complete",
            "position": 4500,
        }))
        .unwrap();
        match event {
            SeestarEvent::FocuserMove(data) => {
                assert_eq!(data.position, 4500);
                assert_eq!(data.state, Some(EventState::Complete));
            }
            _ => panic!("expected FocuserMove"),
        }
    }

    // Mirrors TestWheelMoveEvent::test_defaults.
    #[test]
    fn wheelmove_event_defaults() {
        let event: SeestarEvent =
            serde_json::from_value(json!({"Event": "WheelMove", "Timestamp": TIMESTAMP})).unwrap();
        match event {
            SeestarEvent::WheelMove(data) => {
                assert_eq!(data.position, 0);
                assert_eq!(data.state, None);
            }
            _ => panic!("expected WheelMove"),
        }
    }

    // Mirrors TestWheelMoveEvent::test_filter_position_2. Rust's
    // WheelMoveEventData models only state and position (no filter field).
    #[test]
    fn wheelmove_event_filter_position_2() {
        let event: SeestarEvent = serde_json::from_value(json!({
            "Event": "WheelMove",
            "Timestamp": TIMESTAMP,
            "state": "complete",
            "position": 2,
        }))
        .unwrap();
        match event {
            SeestarEvent::WheelMove(data) => {
                assert_eq!(data.position, 2);
                assert_eq!(data.state, Some(EventState::Complete));
            }
            _ => panic!("expected WheelMove"),
        }
    }

    // Mirrors TestAnnotateEvent::test_defaults. Difference from Python: the
    // Rust Annotate payload is a raw serde_json::Value with no typed
    // `result` field, so Python's `result is None` has no counterpart; what
    // the variant carries is exactly the wire object minus the tag.
    #[test]
    fn annotate_event_defaults() {
        let event: SeestarEvent =
            serde_json::from_value(json!({"Event": "Annotate", "Timestamp": TIMESTAMP})).unwrap();
        match event {
            SeestarEvent::Annotate(v) => {
                assert_eq!(v, json!({"Timestamp": TIMESTAMP}));
            }
            _ => panic!("expected Annotate"),
        }
    }

    // Mirrors TestAnnotateEvent::test_with_result. Python asserts typed
    // AnnotateResult fields (result.image_id, result.image_size); Rust has
    // no AnnotateResult struct, so the result object is pinned as raw JSON.
    // TestAnnotateResult::test_empty is skipped for the same reason (see
    // the header comment).
    #[test]
    fn annotate_event_with_result() {
        let event: SeestarEvent = serde_json::from_value(json!({
            "Event": "Annotate",
            "Timestamp": TIMESTAMP,
            "result": {"image_size": [1920, 1080], "image_id": 1},
        }))
        .unwrap();
        match event {
            SeestarEvent::Annotate(v) => {
                assert_eq!(v["result"]["image_id"], json!(1));
                assert_eq!(v["result"]["image_size"], json!([1920, 1080]));
            }
            _ => panic!("expected Annotate"),
        }
    }

    // Mirrors TestScopeGotoEvent::test_defaults.
    #[test]
    fn scopegoto_event_defaults() {
        let event: SeestarEvent =
            serde_json::from_value(json!({"Event": "ScopeGoto", "Timestamp": TIMESTAMP})).unwrap();
        match event {
            SeestarEvent::ScopeGoto(data) => {
                assert_eq!(data.cur_ra_dec, None);
                assert_eq!(data.dist_deg, 0.0);
            }
            _ => panic!("expected ScopeGoto"),
        }
    }

    // Mirrors TestScopeGotoEvent::test_with_coordinates. Difference from
    // Python: Python models cur_ra_dec as an object ({"ra": .., "dec": ..});
    // Rust models it as Option<(f64, f64)>, which serde accepts only as a
    // two-element array, so the Python wire shape is a parse error here.
    // Pinned as-is; not fixed.
    #[test]
    fn scopegoto_event_with_coordinates() {
        // The object form Python accepts is rejected by the tuple field.
        let err = serde_json::from_value::<SeestarEvent>(json!({
            "Event": "ScopeGoto",
            "Timestamp": TIMESTAMP,
            "cur_ra_dec": {"ra": 10.5, "dec": 45.0},
            "dist_deg": 2.5,
        }))
        .unwrap_err();
        assert!(err.to_string().contains("expected a tuple of size 2"));

        // The array form the tuple field actually wants.
        let event: SeestarEvent = serde_json::from_value(json!({
            "Event": "ScopeGoto",
            "Timestamp": TIMESTAMP,
            "cur_ra_dec": [10.5, 45.0],
            "dist_deg": 2.5,
        }))
        .unwrap();
        match event {
            SeestarEvent::ScopeGoto(data) => {
                assert_eq!(data.cur_ra_dec, Some((10.5, 45.0)));
                assert_eq!(data.dist_deg, 2.5);
            }
            _ => panic!("expected ScopeGoto"),
        }
    }

    // Mirrors TestViewEvent::test_defaults. Difference from Python: Python
    // defaults lp_filter=False, gain=0, mode="star"; Rust models all three
    // as Options and leaves them None when absent.
    #[test]
    fn view_event_defaults() {
        let event: SeestarEvent =
            serde_json::from_value(json!({"Event": "View", "Timestamp": TIMESTAMP})).unwrap();
        match event {
            SeestarEvent::View(data) => {
                assert_eq!(data.lp_filter, None);
                assert_eq!(data.gain, None);
                assert_eq!(data.mode, None);
            }
            _ => panic!("expected View"),
        }
    }

    // Mirrors TestViewEvent::test_lp_filter_set.
    #[test]
    fn view_event_lp_filter_set() {
        let event: SeestarEvent = serde_json::from_value(json!({
            "Event": "View",
            "Timestamp": TIMESTAMP,
            "lp_filter": true,
            "gain": 80,
        }))
        .unwrap();
        match event {
            SeestarEvent::View(data) => {
                assert_eq!(data.lp_filter, Some(true));
                assert_eq!(data.gain, Some(80));
            }
            _ => panic!("expected View"),
        }
    }

    // Mirrors TestInternalEvent::test_construction: an Internal event carries
    // its params on the variant payload (Python's InternalEvent.params).
    #[test]
    fn internal_event_construction() {
        let event = SeestarEvent::Internal(json!({"params": {"key": "value"}}));
        assert_eq!(event.name(), "Internal");
        match event {
            SeestarEvent::Internal(v) => {
                assert_eq!(v["params"]["key"], json!("value"));
            }
            _ => panic!("expected Internal"),
        }
    }

    // Mirrors TestInternalEvent::test_internal_not_in_discriminated_union.
    // Difference from Python: Python's EventTypes union deliberately keeps
    // InternalEvent out — parse({"Event": "Internal"}) raises
    // ValidationError there (it is a client-side model, not a wire event);
    // the Rust enum has an Internal variant, so the same message parses and
    // reaches subscribers. Filed in docs/bugs/event-tag-set-drift.md.
    #[test]
    fn internal_event_parses_to_variant() {
        let event: SeestarEvent = serde_json::from_value(json!({
            "Event": "Internal",
            "Timestamp": TIMESTAMP,
            "params": {"key": "value"},
        }))
        .unwrap();
        match event {
            SeestarEvent::Internal(v) => {
                assert_eq!(v["params"]["key"], json!("value"));
            }
            _ => panic!("expected Internal"),
        }
    }

    // Mirrors TestDiskSpaceEvent::test_defaults.
    #[test]
    fn diskspace_event_defaults() {
        let event: SeestarEvent =
            serde_json::from_value(json!({"Event": "DiskSpace", "Timestamp": TIMESTAMP})).unwrap();
        match event {
            SeestarEvent::DiskSpace(data) => assert_eq!(data.used_percent, 0),
            _ => panic!("expected DiskSpace"),
        }
    }

    // Mirrors TestDiskSpaceEvent::test_with_value.
    #[test]
    fn diskspace_event_with_value() {
        let event: SeestarEvent = serde_json::from_value(json!({
            "Event": "DiskSpace",
            "Timestamp": TIMESTAMP,
            "used_percent": 75,
        }))
        .unwrap();
        match event {
            SeestarEvent::DiskSpace(data) => assert_eq!(data.used_percent, 75),
            _ => panic!("expected DiskSpace"),
        }
    }

    // Mirrors TestClientEvent::test_defaults. Difference from Python: Rust
    // keeps the Client payload as raw serde_json::Value instead of modeling
    // connected/master_index/is_master, so Python's defaults (connected ==
    // [], master_index None, is_master None) have no Rust counterpart; the
    // variant carries exactly the wire object minus the tag.
    #[test]
    fn client_event_defaults() {
        let event: SeestarEvent =
            serde_json::from_value(json!({"Event": "Client", "Timestamp": TIMESTAMP})).unwrap();
        match event {
            SeestarEvent::Client(v) => {
                assert_eq!(v, json!({"Timestamp": TIMESTAMP}));
            }
            _ => panic!("expected Client"),
        }
    }

    // Mirrors TestClientEvent::test_with_clients. Python asserts typed
    // fields; Rust reads them back out of the raw Value.
    #[test]
    fn client_event_with_clients() {
        let event: SeestarEvent = serde_json::from_value(json!({
            "Event": "Client",
            "Timestamp": TIMESTAMP,
            "connected": ["client1", "client2"],
            "is_master": true,
        }))
        .unwrap();
        match event {
            SeestarEvent::Client(v) => {
                assert_eq!(v["connected"], json!(["client1", "client2"]));
                assert_eq!(v["is_master"], json!(true));
            }
            _ => panic!("expected Client"),
        }
    }

    // Task-level cross-check, no single Python mirror: the exact set of
    // event tags this crate models, so membership or spelling drift in
    // known_event_names() fails loudly. Python's EventTypes union
    // (pyscopinator, src/scopinator/seestar/events/__init__.py:385) has 32
    // arms; the Rust set is identical except it additionally models
    // "Internal", which Python deliberately keeps out of the union (see
    // `internal_event_parses_to_variant` and
    // docs/bugs/event-tag-set-drift.md).
    #[test]
    fn known_event_names_lists_the_full_tag_set() {
        let expected = [
            "Alert",
            "AutoFocus",
            "AutoGoto",
            "ContinuousExposure",
            "DarkLibrary",
            "DiskSpace",
            "Exposure",
            "FocuserMove",
            "Initialise",
            "PiStatus",
            "RTSP",
            "SaveImage",
            "ScopeGoto",
            "ScopeHome",
            "ScopeMoveToHorizon",
            "ScopeTrack",
            "Stack",
            "View",
            "WheelMove",
            "Annotate",
            "AutoGotoStep",
            "BatchStack",
            "Client",
            "EqModePA",
            "GoPixel",
            "Internal",
            "PlateSolve",
            "ScanSun",
            "SecondView",
            "SelectCamera",
            "Setting",
            "3PPA",
            "ViewPlan",
        ];
        assert_eq!(known_event_names(), expected.as_slice());
    }

    // Task-level cross-check: every name known_event_names() reports must
    // parse to its own (non-Unknown) variant. This also covers the 14 tags
    // with no isinstance case in the Python suite (Exposure, SaveImage,
    // ScopeHome, ScopeMoveToHorizon, ScopeTrack, AutoGotoStep, EqModePA,
    // GoPixel, Internal, ScanSun, SecondView, Setting, 3PPA, ViewPlan) and
    // exercises every variant arm of name().
    #[test]
    fn every_known_event_name_parses_to_its_variant() {
        for &tag in known_event_names() {
            let event: SeestarEvent =
                serde_json::from_value(json!({"Event": tag, "Timestamp": TIMESTAMP})).unwrap();
            assert_eq!(event.name(), tag, "tag {tag} did not parse to its variant");
            assert!(!matches!(event, SeestarEvent::Unknown));
        }
    }

    // Task-level cross-check of field tolerance under the current serde
    // config (no single Python mirror; pydantic also ignores extra keys by
    // default): unknown keys are ignored, absent keys fall back to None for
    // Option fields and to the declared defaults for the rest.
    #[test]
    fn typed_payloads_tolerate_extra_and_missing_fields() {
        let event: SeestarEvent = serde_json::from_value(json!({
            "Event": "Stack",
            "Timestamp": TIMESTAMP,         // extra key: ignored
            "some_future_field": {"a": 1}, // extra key: ignored
        }))
        .unwrap();
        match event {
            SeestarEvent::Stack(data) => {
                assert_eq!(data.stacked_frame, 0);
                assert_eq!(data.lapse_ms, 0);
                assert!(!data.can_annotate);
                assert_eq!(data.frame_type, None);
                assert_eq!(data.error, "");
            }
            _ => panic!("expected Stack"),
        }
    }

    // Task-level characterization, no Python mirror (Python keeps fully
    // typed models for these events): serde's internally-tagged handling
    // strips the "Event" key from the buffered map before deserializing the
    // newtype payload, so a raw serde_json::Value variant sees every wire
    // field except the tag itself.
    #[test]
    fn raw_value_variants_retain_all_fields_except_the_tag() {
        let event: SeestarEvent = serde_json::from_value(json!({
            "Event": "RTSP",
            "Timestamp": TIMESTAMP,
            "url": "rtsp://example/1",
        }))
        .unwrap();
        match event {
            SeestarEvent::RTSP(v) => {
                assert_eq!(
                    v,
                    json!({"Timestamp": TIMESTAMP, "url": "rtsp://example/1"})
                );
            }
            other => panic!("expected RTSP, got {}", other.name()),
        }

        // A raw variant with nothing but the tag yields an empty object.
        let event: SeestarEvent = serde_json::from_value(json!({"Event": "PlateSolve"})).unwrap();
        match event {
            SeestarEvent::PlateSolve(v) => assert_eq!(v, json!({})),
            other => panic!("expected PlateSolve, got {}", other.name()),
        }
    }
}
