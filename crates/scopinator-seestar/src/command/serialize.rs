use serde_json::{Value, json};

use super::Command;
use crate::auth::Verification;
use scopinator_types::FirmwareVersion;

/// How commands are put on the wire for one control connection.
///
/// It is decided per connection, from what the device showed at connect
/// rather than from a table of firmware versions: the verification mode comes
/// from the handshake negotiation, and the firmware version is read from the
/// device once the connection is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireProfile {
    /// How this connection passes the device's verification gate.
    pub verification: Verification,
    /// The device's `firmware_ver_int`, once known.
    pub firmware: Option<FirmwareVersion>,
}

impl WireProfile {
    /// The profile for a connection that negotiated `verification`, before
    /// the firmware version is known.
    pub fn new(verification: Verification) -> Self {
        Self {
            verification,
            firmware: None,
        }
    }

    /// The profile for a device without the verification handshake. This is
    /// also what a pyscopinator-style client assumes of every device.
    pub fn legacy(firmware: Option<FirmwareVersion>) -> Self {
        Self {
            verification: Verification::Legacy,
            firmware,
        }
    }

    /// The same profile with the firmware version filled in.
    pub fn with_firmware(self, firmware: Option<FirmwareVersion>) -> Self {
        Self { firmware, ..self }
    }

    /// Whether requests carry the `verify` marker.
    ///
    /// A socket verified by the handshake never needs it. Marker mode always
    /// does. For a legacy device the firmware-version threshold inherited
    /// from pyscopinator is the fallback: mark unless the firmware is known
    /// to predate the marker.
    pub fn marker(&self) -> bool {
        match self.verification {
            Verification::Handshake => false,
            Verification::Marker => true,
            Verification::Legacy => self.firmware.is_none_or(|fw| fw.requires_verify()),
        }
    }

    /// The JSON-RPC message for `cmd`: `id`, `method` and, where there are
    /// any, `params`.
    pub fn message(&self, cmd: &Command, id: u64) -> Value {
        let mut msg = json!({
            "id": id,
            "method": cmd.method(),
        });
        let params = command_params(cmd);
        let params = if self.marker() {
            Some(inject_verify(cmd, params))
        } else {
            params
        };
        if let Some(params) = params {
            msg["params"] = params;
        }
        msg
    }

    /// The `\r\n`-terminated line for `cmd`, ready to write to the socket.
    pub fn line(&self, cmd: &Command, id: u64) -> String {
        wire_line(&self.message(cmd, id))
    }
}

/// Serialize a command to a JSON-RPC message for a connection with the given
/// profile. See [`WireProfile::message`]; use [`WireProfile::line`] for the
/// bytes to send, which also fixes the marker's position on the wire.
pub fn serialize_command(cmd: &Command, id: u64, profile: &WireProfile) -> Value {
    profile.message(cmd, id)
}

/// Encode a message as one `\r\n`-terminated wire line.
///
/// Identical to `serde_json`'s compact form except that, when `params` is an
/// object carrying the `verify` marker, the marker is written as its **last**
/// member. The firmware handles `set_setting` members in wire order and stops
/// at the marker (it is an unknown member there), so a marker written before a
/// setting would silently prevent that setting from being applied.
pub fn wire_line(msg: &Value) -> String {
    let marked_params = msg
        .get("params")
        .and_then(Value::as_object)
        .filter(|p| p.contains_key("verify"));
    let (Some(top), Some(params)) = (msg.as_object(), marked_params) else {
        return format!("{msg}\r\n");
    };

    let member = |key: &str, value: &Value| format!("{}:{value}", Value::from(key));
    let params_members: Vec<String> = params
        .iter()
        .filter(|(key, _)| key.as_str() != "verify")
        .chain(params.get_key_value("verify"))
        .map(|(key, value)| member(key, value))
        .collect();
    let members: Vec<String> = top
        .iter()
        .map(|(key, value)| {
            if key == "params" {
                format!("\"params\":{{{}}}", params_members.join(","))
            } else {
                member(key, value)
            }
        })
        .collect();
    format!("{{{}}}\r\n", members.join(","))
}

/// Extract the params value for a command.
fn command_params(cmd: &Command) -> Option<Value> {
    match cmd {
        // No-param commands
        Command::TestConnection
        | Command::PiIsVerified
        | Command::PiReboot
        | Command::PiGetTime
        | Command::GetDeviceState
        | Command::GetViewState
        | Command::GetCameraInfo
        | Command::GetCameraState
        | Command::GetSetting
        | Command::GetStackSetting
        | Command::GetStackInfo
        | Command::GetDiskVolume
        | Command::GetUserLocation
        | Command::GetWheelPosition
        | Command::GetWheelSetting
        | Command::GetWheelState
        | Command::GetLastSolveResult
        | Command::GetSolveResult
        | Command::GetAnnotatedResult
        | Command::ScopeGetEquCoord
        | Command::ScopeGetRaDec
        | Command::ScopeGetHorizCoord
        | Command::ScopePark
        | Command::ScopeMoveToHorizon
        | Command::GetFocuserPosition
        | Command::StartAutoFocus
        | Command::StopAutoFocus
        | Command::StartSolve
        | Command::StartScanPlanet
        | Command::PiStationState => None,

        // Mount mode switch: scope_park with equ_mode param
        Command::ScopeParkMode(eq) => Some(json!({ "equ_mode": eq })),

        // Tuple params
        Command::ScopeSync(ra, dec) => Some(json!([ra, dec])),
        Command::ScopeSetTrackState(enabled) => Some(json!(enabled)),
        Command::SetControlValue(name, value) => Some(json!([name, value])),

        // Struct params
        Command::IscopeStartView(p) => serde_json::to_value(p).ok(),
        // `stage` serializes via serde (StopStage -> "DarkLibrary"/"Stack"/"AutoGoto").
        // `None` sends no params, matching firmware 6.70's parameterless stop.
        Command::IscopeStopView(Some(p)) => serde_json::to_value(p).ok(),
        Command::IscopeStopView(None) => None,
        Command::IscopeStartStack(Some(p)) => serde_json::to_value(p).ok(),
        Command::IscopeStartStack(None) => None,
        Command::ScopeSpeedMove(p) => serde_json::to_value(p).ok(),
        Command::StartPolarAlign(p) => serde_json::to_value(p).ok(),
        Command::MoveFocuser(p) => serde_json::to_value(p).ok(),
        Command::PiSetTime(p) => Some(json!([p])),
        Command::SetUserLocation(p) => serde_json::to_value(p).ok(),
        Command::SetSetting(p) => serde_json::to_value(p).ok(),
        Command::SetStackSetting(p) => serde_json::to_value(p).ok(),
        Command::PiOutputSet2(v) => Some(v.clone()),
        Command::SetViewPlan(v) => Some(v.clone()),
        Command::StopViewPlan => Some(json!({"name": "ViewPlan"})),
        Command::PlaySound(p) => serde_json::to_value(p).ok(),
        // Nested form: a list wrapping the list of group entries. Firmware then
        // appends "verify" -> [[{group_name: ...}], "verify"].
        Command::SetSequenceSetting(groups) => Some(json!([groups])),
    }
}

/// Add the `verify` marker to a command's params.
///
/// The shapes match pyscopinator's `_transform_message_for_verify`, which is
/// also what the firmware accepts on an unverified socket:
/// - object params: `"verify": true` is added ([`wire_line`] writes it last)
/// - array params: `"verify"` is appended, after the real elements
/// - scalar params: wrapped as `[value, "verify"]`
/// - no params: `["verify"]`
///
/// One method cannot take the array form. `get_disk_volume` reads its first
/// positional element as a path, so `["verify"]` fails with 255 "file not
/// exist" (S50, firmware 6.70); it gets `{"verify": true}`, which works.
fn inject_verify(cmd: &Command, params: Option<Value>) -> Value {
    match params {
        None if matches!(cmd, Command::GetDiskVolume) => json!({"verify": true}),
        None => json!(["verify"]),
        Some(Value::Object(mut map)) => {
            map.insert("verify".to_string(), Value::Bool(true));
            Value::Object(map)
        }
        Some(Value::Array(mut arr)) => {
            arr.push(Value::String("verify".to_string()));
            Value::Array(arr)
        }
        Some(other) => json!([other, "verify"]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::params::StartViewParams;

    #[test]
    fn serialize_no_param_command_with_verify() {
        let msg = serialize_command(&Command::TestConnection, 100, &WireProfile::legacy(None));
        assert_eq!(msg["method"], "test_connection");
        assert_eq!(msg["id"], 100);
        assert_eq!(msg["params"], json!(["verify"]));
    }

    #[test]
    fn serialize_no_param_command_without_verify() {
        let fw = Some(FirmwareVersion(2500));
        let msg = serialize_command(&Command::TestConnection, 100, &WireProfile::legacy(fw));
        assert_eq!(msg["method"], "test_connection");
        assert!(msg.get("params").is_none());
    }

    // A goto is `iscope_start_view` in star mode with a target. The expected
    // message is the captured M44 goto from conformance/sessions/s30_fw706_small.
    #[test]
    fn serialize_goto_matches_corpus_line() {
        let cmd = Command::IscopeStartView(StartViewParams::goto(
            "M44",
            8.691286104794766,
            19.514886908101506,
        ));
        let captured: Value = serde_json::from_str(
            r#"{"method":"iscope_start_view","params":{"mode":"star","target_ra_dec":[8.691286104794766,19.514886908101506],"target_name":"M44","lp_filter":false,"verify":true},"id":10020}"#,
        )
        .unwrap();
        assert_eq!(
            serialize_command(&cmd, 10020, &WireProfile::legacy(None)),
            captured
        );

        // On a verified socket the same goto carries no marker.
        let unmarked = serialize_command(&cmd, 10020, &WireProfile::new(Verification::Handshake));
        assert!(unmarked["params"].get("verify").is_none());
        assert_eq!(
            unmarked["params"]["target_ra_dec"],
            captured["params"]["target_ra_dec"]
        );
    }

    // Every command under each verification mode: the marker is present
    // exactly when the profile says so, and removing it gives back the same
    // message a verified socket would be sent.
    #[test]
    fn marker_follows_the_profile_for_every_command() {
        let old = Some(FirmwareVersion(FirmwareVersion::VERIFY_THRESHOLD));
        let new = Some(FirmwareVersion(2670));
        let cases = [
            (WireProfile::new(Verification::Handshake), false),
            (
                WireProfile::new(Verification::Handshake).with_firmware(new),
                false,
            ),
            (WireProfile::new(Verification::Marker), true),
            (
                WireProfile::new(Verification::Marker).with_firmware(old),
                true,
            ),
            (WireProfile::legacy(None), true),
            (WireProfile::legacy(new), true),
            (WireProfile::legacy(old), false),
        ];
        let has_marker = |params: Option<&Value>| match params {
            Some(Value::Object(m)) => m.get("verify") == Some(&Value::Bool(true)),
            Some(Value::Array(a)) => a.last() == Some(&json!("verify")),
            _ => false,
        };
        for cmd in crate::command::tests::one_of_every_variant() {
            let plain = WireProfile::new(Verification::Handshake).message(&cmd, 5);
            assert!(
                !has_marker(plain.get("params")),
                "{}: unmarked form carries a marker",
                cmd.method()
            );
            for (profile, marked) in cases {
                assert_eq!(profile.marker(), marked, "{profile:?}");
                let msg = profile.message(&cmd, 5);
                assert_eq!(msg["method"], cmd.method());
                assert_eq!(msg["id"], 5);
                assert_eq!(
                    has_marker(msg.get("params")),
                    marked,
                    "{} under {profile:?}: {msg}",
                    cmd.method()
                );
                if !marked {
                    assert_eq!(msg, plain, "{} under {profile:?}", cmd.method());
                }
                // The line is the same JSON, with the marker (if any) last.
                let line = profile.line(&cmd, 5);
                assert!(line.ends_with("\r\n"));
                assert_eq!(serde_json::from_str::<Value>(line.trim()).unwrap(), msg);
                if marked && msg["params"].is_object() {
                    assert!(
                        line.ends_with(",\"verify\":true}}\r\n")
                            || line.ends_with("{\"verify\":true}}\r\n"),
                        "{line}"
                    );
                }
            }
        }
    }

    // The expected bytes for one command of each params shape in each mode.
    #[test]
    fn wire_bytes_per_params_shape_and_mode() {
        use crate::command::params::StartViewParams;
        let marker = WireProfile::new(Verification::Marker);
        let handshake = WireProfile::new(Verification::Handshake);
        let goto = Command::IscopeStartView(StartViewParams {
            mode: None,
            target_name: Some("x".into()),
            target_ra_dec: None,
            target_type: None,
            lp_filter: None,
        });
        for (cmd, marked, plain) in [
            (
                Command::GetDeviceState,
                r#"{"id":9,"method":"get_device_state","params":["verify"]}"#,
                r#"{"id":9,"method":"get_device_state"}"#,
            ),
            (
                Command::GetDiskVolume,
                r#"{"id":9,"method":"get_disk_volume","params":{"verify":true}}"#,
                r#"{"id":9,"method":"get_disk_volume"}"#,
            ),
            (
                goto,
                r#"{"id":9,"method":"iscope_start_view","params":{"target_name":"x","verify":true}}"#,
                r#"{"id":9,"method":"iscope_start_view","params":{"target_name":"x"}}"#,
            ),
            (
                Command::ScopeSync(1.5, 2.0),
                r#"{"id":9,"method":"scope_sync","params":[1.5,2.0,"verify"]}"#,
                r#"{"id":9,"method":"scope_sync","params":[1.5,2.0]}"#,
            ),
            (
                Command::ScopeSetTrackState(true),
                r#"{"id":9,"method":"scope_set_track_state","params":[true,"verify"]}"#,
                r#"{"id":9,"method":"scope_set_track_state","params":true}"#,
            ),
        ] {
            assert_eq!(marker.line(&cmd, 9), format!("{marked}\r\n"));
            assert_eq!(handshake.line(&cmd, 9), format!("{plain}\r\n"));
            // A legacy device of unknown firmware is sent the marked form.
            assert_eq!(
                WireProfile::legacy(None).line(&cmd, 9),
                format!("{marked}\r\n")
            );
        }
    }

    #[test]
    fn wire_line_writes_the_marker_last() {
        // `viewplan_gohome` and `wide_cam` sort after `verify`; the marker
        // must still come last or the firmware never reaches them.
        let msg = json!({
            "id": 7,
            "method": "set_setting",
            "params": {"auto_af": true, "verify": true, "viewplan_gohome": false, "wide_cam": true},
        });
        let line = wire_line(&msg);
        assert_eq!(
            line,
            "{\"id\":7,\"method\":\"set_setting\",\"params\":\
             {\"auto_af\":true,\"viewplan_gohome\":false,\"wide_cam\":true,\"verify\":true}}\r\n"
        );
        // Still the same JSON value.
        assert_eq!(serde_json::from_str::<Value>(line.trim()).unwrap(), msg);
    }

    #[test]
    fn wire_line_is_plain_compact_json_otherwise() {
        for msg in [
            json!({"id": 1, "method": "pi_get_time"}),
            json!({"id": 1, "method": "pi_get_time", "params": ["verify"]}),
            json!({"id": 2, "method": "set_setting", "params": {"a \"q\"": [1, {"verify": 1}]}}),
        ] {
            assert_eq!(wire_line(&msg), format!("{msg}\r\n"));
        }
    }

    #[test]
    fn serialize_scope_sync() {
        let cmd = Command::ScopeSync(10.5, 45.0);
        let msg = serialize_command(&cmd, 300, &WireProfile::legacy(None));
        assert_eq!(msg["method"], "scope_sync");
        // Array params get "verify" appended
        let params = msg["params"].as_array().unwrap();
        assert_eq!(params.len(), 3);
        assert_eq!(params[2], "verify");
    }

    #[test]
    fn serialize_track_state() {
        let cmd = Command::ScopeSetTrackState(true);
        let msg = serialize_command(&cmd, 400, &WireProfile::legacy(None));
        assert_eq!(msg["method"], "scope_set_track_state");
        // Bool scalar gets wrapped: [true, "verify"]
        let params = msg["params"].as_array().unwrap();
        assert_eq!(params[0], true);
        assert_eq!(params[1], "verify");
    }

    #[test]
    fn serialize_play_sound_matches_capture() {
        // Captured (fw 7.06): {"method":"play_sound","params":{"num":80,"verify":true},"id":...}
        use crate::command::params::PlaySoundParams;
        let cmd = Command::PlaySound(PlaySoundParams { num: 80 });
        let msg = serialize_command(&cmd, 10016, &WireProfile::legacy(None));
        assert_eq!(msg["method"], "play_sound");
        assert_eq!(msg["params"]["num"], 80);
        assert_eq!(msg["params"]["verify"], true);
    }

    #[test]
    fn serialize_pi_station_state_matches_capture() {
        // Captured: {"method":"pi_station_state","params":["verify"],"id":...}
        let msg = serialize_command(&Command::PiStationState, 10001, &WireProfile::legacy(None));
        assert_eq!(msg["method"], "pi_station_state");
        assert_eq!(msg["params"], json!(["verify"]));
    }

    #[test]
    fn serialize_set_sequence_setting_matches_capture() {
        // Captured (fw 7.06):
        // {"method":"set_sequence_setting","params":[[{"group_name":"M44"}],"verify"],"id":...}
        use crate::command::params::SequenceSettingParams;
        let cmd = Command::SetSequenceSetting(vec![SequenceSettingParams {
            group_name: Some("M44".into()),
        }]);
        let msg = serialize_command(&cmd, 10027, &WireProfile::legacy(None));
        assert_eq!(msg["method"], "set_sequence_setting");
        assert_eq!(msg["params"], json!([[{ "group_name": "M44" }], "verify"]));
    }

    #[test]
    fn serialize_stop_view_without_stage_matches_fw670() {
        // Captured (fw 6.70): {"method":"iscope_stop_view","params":["verify"],"id":...}
        let msg = serialize_command(
            &Command::IscopeStopView(None),
            10094,
            &WireProfile::legacy(None),
        );
        assert_eq!(msg["method"], "iscope_stop_view");
        assert_eq!(msg["params"], json!(["verify"]));
    }

    #[test]
    fn serialize_stop_view_with_stage_uses_serde_names() {
        use crate::command::params::{StopStage, StopViewParams};
        let cmd = Command::IscopeStopView(Some(StopViewParams {
            stage: StopStage::DarkLibrary,
        }));
        // Without verify (old firmware) the stage is serialized via serde,
        // not Debug formatting.
        let msg = serialize_command(&cmd, 1, &WireProfile::legacy(Some(FirmwareVersion(2500))));
        assert_eq!(msg["params"], json!({ "stage": "DarkLibrary" }));
    }

    #[test]
    fn serialize_stop_view_plan() {
        let cmd = Command::StopViewPlan;
        let msg = serialize_command(&cmd, 500, &WireProfile::legacy(None));
        assert_eq!(msg["method"], "stop_func");
        assert_eq!(msg["params"]["name"], "ViewPlan");
        assert_eq!(msg["params"]["verify"], true);
    }

    mod prop {
        use super::*;
        use proptest::prelude::*;

        fn arbitrary_scalar() -> impl Strategy<Value = Value> {
            prop_oneof![
                Just(Value::Null),
                any::<bool>().prop_map(Value::Bool),
                any::<i64>().prop_map(|n| Value::Number(n.into())),
                "[A-Za-z0-9_]{0,16}".prop_map(Value::String),
            ]
        }

        // Key for arbitrary objects — must NOT include "verify" so we can
        // observe injection deterministically.
        /// Params for an ordinary command, with or without the marker: the
        /// shape rules these properties pin.
        fn inject_verify(params: Option<Value>, needs_verify: bool) -> Option<Value> {
            if needs_verify {
                Some(crate::command::serialize::inject_verify(
                    &Command::TestConnection,
                    params,
                ))
            } else {
                params
            }
        }

        fn non_verify_key() -> impl Strategy<Value = String> {
            "[a-z]{1,8}".prop_filter("not verify", |s| s != "verify")
        }

        fn arbitrary_object() -> impl Strategy<Value = serde_json::Map<String, Value>> {
            proptest::collection::vec((non_verify_key(), arbitrary_scalar()), 0..5)
                .prop_map(|entries| entries.into_iter().collect())
        }

        #[test]
        fn no_verify_with_none_stays_none() {
            let result = inject_verify(None, false);
            assert_eq!(result, None);
        }

        #[test]
        fn verify_with_none_yields_singleton_array() {
            let result = inject_verify(None, true).unwrap();
            assert_eq!(result, json!(["verify"]));
        }

        proptest! {
            #[test]
            fn no_verify_means_params_unchanged(scalar in arbitrary_scalar()) {
                let result = inject_verify(Some(scalar.clone()), false);
                prop_assert_eq!(result, Some(scalar));
            }

            #[test]
            fn verify_object_adds_verify_true(map in arbitrary_object()) {
                let original = Value::Object(map.clone());
                let result = inject_verify(Some(original), true).unwrap();
                let obj = result.as_object().unwrap();
                prop_assert_eq!(obj.get("verify"), Some(&Value::Bool(true)));
                // All original keys preserved with original values.
                for (k, v) in &map {
                    prop_assert_eq!(obj.get(k), Some(v));
                }
            }

            #[test]
            fn verify_array_appends_verify_string(items in proptest::collection::vec(arbitrary_scalar(), 0..5)) {
                let original = Value::Array(items.clone());
                let result = inject_verify(Some(original), true).unwrap();
                let arr = result.as_array().unwrap();
                prop_assert_eq!(arr.len(), items.len() + 1);
                prop_assert_eq!(&arr[..items.len()], items.as_slice());
                prop_assert_eq!(arr.last(), Some(&Value::String("verify".to_string())));
            }

            #[test]
            fn verify_scalar_wraps_in_array_with_verify(scalar in prop_oneof![
                Just(Value::Null),
                any::<bool>().prop_map(Value::Bool),
                any::<i64>().prop_map(|n| Value::Number(n.into())),
                "[A-Za-z0-9_]{0,16}".prop_map(Value::String),
            ]) {
                let result = inject_verify(Some(scalar.clone()), true).unwrap();
                let arr = result.as_array().unwrap();
                prop_assert_eq!(arr.len(), 2);
                prop_assert_eq!(&arr[0], &scalar);
                prop_assert_eq!(&arr[1], &Value::String("verify".to_string()));
            }

            #[test]
            fn serialize_command_id_and_method_preserved(
                id in any::<u64>(),
                fw in proptest::option::of(any::<u32>().prop_map(FirmwareVersion)),
            ) {
                let cmd = Command::TestConnection;
                let msg = serialize_command(&cmd, id, &WireProfile::legacy(fw));
                prop_assert_eq!(msg["id"].as_u64(), Some(id));
                prop_assert_eq!(msg["method"].as_str(), Some(cmd.method()));
            }

            // needs_verify behavior: None firmware OR firmware > 2582 → verify injected.
            #[test]
            fn verify_injection_follows_firmware_threshold(
                id in any::<u64>(),
                fw_version in any::<u32>(),
            ) {
                let cmd = Command::TestConnection;
                let msg = serialize_command(&cmd, id, &WireProfile::legacy(Some(FirmwareVersion(fw_version))));
                let has_params = msg.get("params").is_some();
                prop_assert_eq!(has_params, fw_version > FirmwareVersion::VERIFY_THRESHOLD);
            }
        }
    }
}
