pub mod types;

pub use types::*;

use serde::Deserialize;
use serde::de::Error as _;

/// Defines [`SeestarEvent`] and everything derived from its list of modeled
/// events: the parser, [`SeestarEvent::name`] and [`known_event_names`].
macro_rules! seestar_events {
    ($( $variant:ident($payload:ty) = $tag:literal, )*) => {
        /// A Seestar event received from the control port.
        ///
        /// Events are distinguished by the `Event` member of the JSON
        /// message. Each modeled event is a variant holding the rest of the
        /// message; any other event is [`SeestarEvent::Unknown`], which keeps
        /// its name and payload. The firmware has many more events than are
        /// modeled here, and adds more between versions, so `Unknown` is an
        /// ordinary outcome, not an error.
        #[derive(Debug, Clone)]
        #[allow(clippy::upper_case_acronyms)]
        pub enum SeestarEvent {
            $( $variant($payload), )*
            /// An event this crate has no variant for, or a modeled event
            /// whose payload did not have the expected shape.
            Unknown {
                /// The `Event` member as sent by the device.
                name: String,
                /// The rest of the message (every member except `Event`).
                payload: serde_json::Value,
            },
        }

        /// The set of `Event` tag strings this crate models (everything that
        /// does not become [`SeestarEvent::Unknown`]).
        ///
        /// Used by tests to assert the captured corpus contains no unmodeled
        /// events, and available to callers that want to detect firmware drift.
        pub fn known_event_names() -> &'static [&'static str] {
            &[ $( $tag, )* ]
        }

        impl SeestarEvent {
            /// Returns the event name string: the `Event` member on the wire.
            /// For [`SeestarEvent::Unknown`] this is the device's own name
            /// for the event.
            pub fn name(&self) -> &str {
                match self {
                    $( Self::$variant(_) => $tag, )*
                    Self::Unknown { name, .. } => name,
                }
            }

            /// Parse the payload of a modeled event. `None` if `name` is not
            /// a modeled event.
            fn parse_modeled(
                name: &str,
                payload: serde_json::Value,
            ) -> Option<Result<Self, serde_json::Error>> {
                Some(match name {
                    $( $tag => serde_json::from_value(payload).map(Self::$variant), )*
                    _ => return None,
                })
            }
        }
    };
}

seestar_events! {
    Alert(AlertEventData) = "Alert",
    AutoFocus(AutoFocusEventData) = "AutoFocus",
    AutoGoto(AutoGotoEventData) = "AutoGoto",
    ContinuousExposure(serde_json::Value) = "ContinuousExposure",
    DarkLibrary(serde_json::Value) = "DarkLibrary",
    DiskSpace(DiskSpaceEventData) = "DiskSpace",
    Exposure(ExposureEventData) = "Exposure",
    FocuserMove(FocuserMoveEventData) = "FocuserMove",
    Initialise(InitialiseEventData) = "Initialise",
    PiStatus(PiStatusEventData) = "PiStatus",
    RTSP(serde_json::Value) = "RTSP",
    SaveImage(SaveImageEventData) = "SaveImage",
    ScopeGoto(ScopeGotoEventData) = "ScopeGoto",
    ScopeHome(ScopeHomeEventData) = "ScopeHome",
    ScopeMoveToHorizon(serde_json::Value) = "ScopeMoveToHorizon",
    ScopeTrack(ScopeTrackEventData) = "ScopeTrack",
    Stack(StackEventData) = "Stack",
    View(ViewEventData) = "View",
    WheelMove(WheelMoveEventData) = "WheelMove",
    Annotate(serde_json::Value) = "Annotate",
    AutoGotoStep(serde_json::Value) = "AutoGotoStep",
    BatchStack(serde_json::Value) = "BatchStack",
    Client(serde_json::Value) = "Client",
    EqModePA(serde_json::Value) = "EqModePA",
    GoPixel(serde_json::Value) = "GoPixel",
    Internal(serde_json::Value) = "Internal",
    PlateSolve(serde_json::Value) = "PlateSolve",
    ScanSun(serde_json::Value) = "ScanSun",
    SecondView(ViewEventData) = "SecondView",
    SelectCamera(serde_json::Value) = "SelectCamera",
    Setting(serde_json::Value) = "Setting",
    ThreePPA(serde_json::Value) = "3PPA",
    ViewPlan(serde_json::Value) = "ViewPlan",
    // Operational events typed from the spec cards.
    Version(VersionEventData) = "Version",
    Target(TargetEventData) = "Target",
    Sequence(SequenceEventData) = "Sequence",
    MeridianFlip(MeridianFlipEventData) = "MeridianFlip",
    Settle(StateEventData) = "Settle",
    MountSettle(StateEventData) = "MountSettle",
    Dither(StateEventData) = "Dither",
    FindStar(FindStarEventData) = "FindStar",
    BalanceSensor(BalanceSensorEventData) = "BalanceSensor",
    AutoExp(AutoExpEventData) = "AutoExp",
    Station(StationEventData) = "Station",
}

impl SeestarEvent {
    /// Build an event from a decoded JSON message.
    ///
    /// The only error is a message with no string `Event` member. An event
    /// name that is not modeled, and a modeled event whose payload does not
    /// fit its type, both become [`SeestarEvent::Unknown`] with the payload
    /// intact, so nothing the device sends is dropped.
    pub fn from_value(mut value: serde_json::Value) -> Result<Self, String> {
        let name = match value.as_object_mut().and_then(|m| m.remove("Event")) {
            Some(serde_json::Value::String(name)) => name,
            Some(other) => return Err(format!("`Event` is not a string: {other}")),
            None => return Err("missing field `Event`".to_string()),
        };
        match Self::parse_modeled(&name, value.clone()) {
            Some(Ok(event)) => Ok(event),
            Some(Err(error)) => {
                tracing::debug!(event = %name, %error, "event payload did not match its type");
                Ok(Self::Unknown {
                    name,
                    payload: value,
                })
            }
            None => Ok(Self::Unknown {
                name,
                payload: value,
            }),
        }
    }

    /// True for an event this crate has no typed variant for.
    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown { .. })
    }
}

impl<'de> Deserialize<'de> for SeestarEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        Self::from_value(value).map_err(D::Error::custom)
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
        assert!(event.is_unknown());
    }

    // An unmodeled event keeps its wire name and its payload (the gap filed
    // in docs/bugs/response-drop-timeout.md, now closed). The firmware has
    // dozens of events this crate does not model, so this is the common case
    // on newer firmware, not an oddity.
    #[test]
    fn unknown_event_keeps_tag_and_payload() {
        let json = r#"{
            "Event": "SkyReplay",
            "Timestamp": "1234567890.0",
            "state": "complete",
            "count": 3
        }"#;
        let event: SeestarEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.name(), "SkyReplay");
        match &event {
            SeestarEvent::Unknown { name, payload } => {
                assert_eq!(name, "SkyReplay");
                assert_eq!(
                    payload,
                    &json!({"Timestamp": "1234567890.0", "state": "complete", "count": 3})
                );
            }
            other => panic!("expected Unknown, got {}", other.name()),
        }

        // Different unmodeled events are distinguishable.
        let other_event: SeestarEvent =
            serde_json::from_str(r#"{"Event": "AnotherFutureEvent", "data": 99}"#).unwrap();
        assert_eq!(other_event.name(), "AnotherFutureEvent");
        assert_ne!(other_event.name(), event.name());
    }

    // A modeled event whose payload does not fit its type is not dropped: it
    // falls back to Unknown with the raw payload.
    #[test]
    fn modeled_event_with_unexpected_payload_falls_back_to_unknown() {
        let event: SeestarEvent = serde_json::from_value(json!({
            "Event": "PiStatus",
            "temp": "warm",
            "battery_capacity": 80,
        }))
        .unwrap();
        match event {
            SeestarEvent::Unknown { name, payload } => {
                assert_eq!(name, "PiStatus");
                assert_eq!(payload, json!({"temp": "warm", "battery_capacity": 80}));
            }
            other => panic!("expected Unknown, got {}", other.name()),
        }
    }

    // The spec lists `idle` as a state on many events and requires clients to
    // treat unknown state strings as "other" rather than failing.
    #[test]
    fn idle_and_unknown_states_parse() {
        for (wire, expected) in [
            ("idle", EventState::Idle),
            ("brand_new", EventState::Other("brand_new".into())),
        ] {
            let event: SeestarEvent =
                serde_json::from_value(json!({"Event": "View", "state": wire})).unwrap();
            match event {
                SeestarEvent::View(data) => assert_eq!(data.state, Some(expected)),
                other => panic!("expected View, got {}", other.name()),
            }
        }
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
    // tag; Rust maps it to SeestarEvent::Unknown, keeping the tag and payload
    // (see `unknown_event_keeps_tag_and_payload` above).
    #[test]
    fn unknown_event_tag_parses_to_unknown() {
        let event: SeestarEvent =
            serde_json::from_value(json!({"Event": "UnknownXYZ", "Timestamp": TIMESTAMP})).unwrap();
        assert!(event.is_unknown());
        assert_eq!(event.name(), "UnknownXYZ");
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
    fn pistatus_event_unknown_charger_status_is_kept() {
        // Difference from Python (pydantic raises): the spec notes the
        // charger mapping may differ by build, so an unrecognized status is
        // kept as `Other` and the rest of the event still arrives.
        let event: SeestarEvent = serde_json::from_value(json!({
            "Event": "PiStatus",
            "Timestamp": TIMESTAMP,
            "charger_status": "InvalidStatus",
            "battery_capacity": 55,
        }))
        .unwrap();
        match event {
            SeestarEvent::PiStatus(data) => {
                assert_eq!(
                    data.charger_status,
                    Some(ChargerStatus::Other("InvalidStatus".into()))
                );
                assert_eq!(data.battery_capacity, Some(55));
            }
            other => panic!("expected PiStatus, got {}", other.name()),
        }
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

    // Mirrors TestEventStates::test_invalid_state_raises, with the opposite
    // outcome. Difference from Python (pydantic raises): the firmware uses
    // states beyond the original five (`idle` at least), and the spec
    // requires unknown state strings to be tolerated, so they parse as
    // `EventState::Other` instead of dropping the whole event.
    #[test]
    fn autogoto_event_unknown_state_is_kept() {
        let event: SeestarEvent = serde_json::from_value(json!({
            "Event": "AutoGoto",
            "Timestamp": TIMESTAMP,
            "state": "invalid_state_xyz",
        }))
        .unwrap();
        match event {
            SeestarEvent::AutoGoto(data) => {
                assert_eq!(
                    data.state,
                    Some(EventState::Other("invalid_state_xyz".into()))
                );
            }
            other => panic!("expected AutoGoto, got {}", other.name()),
        }
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
    // two-element array. The Python wire shape does not fit the typed
    // payload, so the event arrives as Unknown with the raw payload rather
    // than as a typed ScopeGoto. Pinned as-is; the field type is not fixed.
    #[test]
    fn scopegoto_event_with_coordinates() {
        // The object form Python accepts does not fit the tuple field.
        let event: SeestarEvent = serde_json::from_value(json!({
            "Event": "ScopeGoto",
            "Timestamp": TIMESTAMP,
            "cur_ra_dec": {"ra": 10.5, "dec": 45.0},
            "dist_deg": 2.5,
        }))
        .unwrap();
        match &event {
            SeestarEvent::Unknown { name, payload } => {
                assert_eq!(name, "ScopeGoto");
                assert_eq!(payload["cur_ra_dec"]["ra"], 10.5);
            }
            other => panic!("expected Unknown, got {}", other.name()),
        }

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
            "Version",
            "Target",
            "Sequence",
            "MeridianFlip",
            "Settle",
            "MountSettle",
            "Dither",
            "FindStar",
            "BalanceSensor",
            "AutoExp",
            "Station",
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
            assert!(!event.is_unknown());
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

    // ── Operational events typed from the spec cards ─────────────────────────

    // Real payloads from the 2026-11-14 hardware sessions (S50 fw 6.70 and
    // S30 fw 9.75): `{"Event":"BalanceSensor","code":0}`.
    #[test]
    fn balance_sensor_event_as_seen_on_hardware() {
        let event: SeestarEvent = serde_json::from_value(json!({
            "Event": "BalanceSensor", "Timestamp": "5140.2", "code": 0,
        }))
        .unwrap();
        match event {
            SeestarEvent::BalanceSensor(data) => {
                assert_eq!(data.code, Some(0));
                assert!(data.data.is_none());
            }
            other => panic!("expected BalanceSensor, got {}", other.name()),
        }
    }

    #[test]
    fn version_greeting_carries_the_firmware() {
        let event: SeestarEvent = serde_json::from_value(json!({
            "Event": "Version", "Timestamp": "1.0", "name": "ASI AIR imager",
            "firmware_ver_int": 2975, "firmware_ver_string": "9.75", "is_verified": true,
            "main_version": "9.75", "modules": [{"name": "imager"}],
        }))
        .unwrap();
        match event {
            SeestarEvent::Version(data) => {
                assert_eq!(data.firmware_ver_int, Some(2975));
                assert_eq!(data.firmware_ver_string.as_deref(), Some("9.75"));
                assert_eq!(data.is_verified, Some(true));
                assert!(data.modules.is_some());
            }
            other => panic!("expected Version, got {}", other.name()),
        }
        // The guider's greeting has only a name.
        let event: SeestarEvent =
            serde_json::from_value(json!({"Event": "Version", "name": "guider"})).unwrap();
        assert!(matches!(event, SeestarEvent::Version(d) if d.firmware_ver_int.is_none()));
    }

    #[test]
    fn target_event_reports_both_cameras() {
        let event: SeestarEvent = serde_json::from_value(json!({
            "Event": "Target", "state": "working", "stage": "Stack", "lapse_ms": 1200,
            "route": ["ViewPlan"], "target_id": 3, "camera_mode": "dual",
            "wide_state": "fail", "wide_code": 207, "wide_error": "fail to operate",
            "stack_total_sec": 120.5, "progress": {"done": 2},
        }))
        .unwrap();
        match event {
            SeestarEvent::Target(data) => {
                assert_eq!(data.state, Some(EventState::Working));
                assert_eq!(data.target_id, Some(3));
                assert_eq!(data.wide_state, Some(EventState::Fail));
                assert_eq!(data.wide_code, Some(207));
                assert_eq!(data.stack_total_sec, Some(120.5));
            }
            other => panic!("expected Target, got {}", other.name()),
        }
    }

    #[test]
    fn sequence_event_frame_states_are_kept() {
        let event: SeestarEvent = serde_json::from_value(json!({
            "Event": "Sequence", "state": "frame_complete", "id": 1,
            "frame": 4, "total_frame": 10, "total_lapse_frame": 4,
        }))
        .unwrap();
        match event {
            SeestarEvent::Sequence(data) => {
                assert_eq!(data.state, Some(EventState::Other("frame_complete".into())));
                assert_eq!((data.frame, data.total_frame), (Some(4), Some(10)));
            }
            other => panic!("expected Sequence, got {}", other.name()),
        }
    }

    #[test]
    fn state_only_events_share_a_payload() {
        for name in ["Settle", "MountSettle", "Dither"] {
            let event: SeestarEvent = serde_json::from_value(json!({
                "Event": name, "state": "fail", "error": "aborted", "code": 253,
            }))
            .unwrap();
            assert_eq!(event.name(), name);
            let data = match event {
                SeestarEvent::Settle(d)
                | SeestarEvent::MountSettle(d)
                | SeestarEvent::Dither(d) => d,
                other => panic!("expected a state event, got {}", other.name()),
            };
            assert_eq!(data.state, Some(EventState::Fail));
            assert_eq!(data.code, Some(253));
            assert_eq!(data.error.as_deref(), Some("aborted"));
        }
    }

    #[test]
    fn meridian_flip_find_star_auto_exp_and_station_parse() {
        let parse = |v: serde_json::Value| serde_json::from_value::<SeestarEvent>(v).unwrap();
        match parse(json!({"Event": "MeridianFlip", "state": "start", "count": 2})) {
            SeestarEvent::MeridianFlip(d) => {
                assert_eq!((d.state, d.count), (Some(EventState::Start), Some(2)));
            }
            other => panic!("got {}", other.name()),
        }
        match parse(json!({"Event": "FindStar", "state": "complete", "result": {"stars": []}})) {
            SeestarEvent::FindStar(d) => assert!(d.result.is_some()),
            other => panic!("got {}", other.name()),
        }
        match parse(
            json!({"Event": "AutoExp", "state": "complete", "exp_us": 250000, "lapse_ms": 900}),
        ) {
            SeestarEvent::AutoExp(d) => assert_eq!((d.exp_us, d.lapse_ms), (Some(250_000), 900)),
            other => panic!("got {}", other.name()),
        }
        match parse(json!({"Event": "Station", "state": "fail", "code": 207, "error": "x"})) {
            SeestarEvent::Station(d) => assert_eq!(d.code, Some(207)),
            other => panic!("got {}", other.name()),
        }
        // A payload that does not fit the type is kept as Unknown, not lost.
        let odd = parse(json!({"Event": "AutoExp", "exp_us": "long"}));
        assert!(odd.is_unknown());
        assert_eq!(odd.name(), "AutoExp");
    }

    // Stable-toolchain counterpart of the `event_parse` fuzz target: whatever
    // JSON arrives under a modeled or unmodeled event name, parsing returns
    // an event (never panics, never errors), and it reports the name it was
    // given.
    mod never_panics {
        use super::*;
        use proptest::prelude::*;

        fn arbitrary_json() -> impl Strategy<Value = serde_json::Value> {
            let leaf = prop_oneof![
                Just(serde_json::Value::Null),
                any::<bool>().prop_map(serde_json::Value::from),
                any::<i64>().prop_map(serde_json::Value::from),
                any::<f64>().prop_map(|f| serde_json::json!(f)),
                ".{0,12}".prop_map(serde_json::Value::from),
            ];
            leaf.prop_recursive(3, 24, 4, |inner| {
                prop_oneof![
                    proptest::collection::vec(inner.clone(), 0..4)
                        .prop_map(serde_json::Value::from),
                    proptest::collection::btree_map("[a-z_]{1,10}", inner, 0..4)
                        .prop_map(|m| serde_json::Value::Object(m.into_iter().collect())),
                ]
            })
        }

        /// Member names the typed payloads read, so the generated objects hit
        /// the typed fields with wrong-typed values and not only extra keys.
        const MEMBERS: &[&str] = &[
            "state",
            "lapse_ms",
            "count",
            "hint",
            "error",
            "code",
            "cur_ra_dec",
            "dist_deg",
            "temp",
            "charger_status",
            "battery_capacity",
            "position",
            "mode",
            "tracking",
            "stacked_frame",
            "result",
            "Timestamp",
        ];

        proptest! {
            #[test]
            fn any_payload_under_any_name_parses(
                known in proptest::sample::select(known_event_names()),
                other in "[A-Za-z0-9_]{0,12}",
                use_known in any::<bool>(),
                members in proptest::collection::vec(
                    (proptest::sample::select(MEMBERS), arbitrary_json()), 0..8),
            ) {
                let name = if use_known { known.to_string() } else { other };
                let mut object = serde_json::Map::new();
                for (key, value) in members {
                    object.insert(key.to_string(), value);
                }
                object.insert("Event".into(), name.clone().into());
                let event = SeestarEvent::from_value(serde_json::Value::Object(object))
                    .expect("an object with a string Event always parses");
                prop_assert_eq!(event.name(), name.as_str());
            }

            #[test]
            fn arbitrary_json_never_panics(value in arbitrary_json()) {
                // Ok or Err, but never a panic.
                let _ = SeestarEvent::from_value(value.clone());
                let _ = serde_json::from_value::<SeestarEvent>(value);
            }
        }
    }
}
