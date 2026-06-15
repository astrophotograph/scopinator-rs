pub mod params;
pub mod serialize;

use params::*;

/// A command to send to the Seestar telescope on the control port (4700).
///
/// Each variant maps to a specific JSON-RPC method string. The `id` field
/// is assigned by the client at send time.
#[derive(Debug, Clone)]
pub enum Command {
    // -- Connection / system --
    TestConnection,
    PiIsVerified,
    PiReboot,
    PiGetTime,
    PiSetTime(SetTimeParams),

    // -- Device state / info --
    GetDeviceState,
    GetViewState,
    GetCameraInfo,
    GetCameraState,
    GetSetting,
    GetStackSetting,
    GetStackInfo,
    GetDiskVolume,
    GetUserLocation,
    GetWheelPosition,
    GetWheelSetting,
    GetWheelState,
    GetLastSolveResult,
    GetSolveResult,
    GetAnnotatedResult,

    // -- Mount --
    ScopeGetEquCoord,
    ScopeGetRaDec,
    ScopeGetHorizCoord,
    ScopeSync(f64, f64),
    ScopePark,
    /// Park and switch mount mode. `true` = Equatorial, `false` = Alt-Az.
    ScopeParkMode(bool),
    ScopeMoveToHorizon,
    ScopeSpeedMove(SpeedMoveParams),
    ScopeSetTrackState(bool),

    // -- Observation --
    GotoTarget(GotoTargetParams),
    IscopeStartView(StartViewParams),
    /// Stop the live view. The `stage` parameter is optional: firmware 6.70
    /// is observed to accept a parameterless `iscope_stop_view` (just the
    /// injected `verify`), so `None` sends no `stage`.
    IscopeStopView(Option<StopViewParams>),
    IscopeStartStack(Option<StartStackParams>),

    // -- Focus --
    GetFocuserPosition,
    MoveFocuser(MoveFocuserParams),
    /// Note: typo in method name is intentional (matches firmware).
    StartAutoFocus,
    /// Note: typo in method name is intentional (matches firmware).
    StopAutoFocus,

    // -- Settings --
    SetUserLocation(SetUserLocationParams),
    SetSetting(SettingParams),
    SetStackSetting(SetStackSettingParams),
    SetControlValue(String, i32),
    PiOutputSet2(serde_json::Value),
    /// Configure an imaging sequence (firmware 7.06+). Wire form is a list of
    /// group entries, sent nested as `[[{group_name: ...}]]`.
    SetSequenceSetting(Vec<SequenceSettingParams>),

    // -- Misc / system --
    /// Play a built-in sound (firmware 7.06+).
    PlaySound(PlaySoundParams),
    /// Query the Wi-Fi station state. No parameters.
    PiStationState,

    // -- Imaging --
    // NOTE: `begin_streaming` / `stop_streaming` are intentionally NOT modeled
    // here. They are imaging-port (4800) commands, not control-port (4700)
    // JSON-RPC commands — the scope rejects `begin_streaming` on 4700 with code
    // 103. They live in [`ImagingCommand`]; use
    // [`crate::SeestarClient::send_imaging`] / `begin_streaming` to drive them.
    GetStackedImage,

    // -- Plate solving --
    StartSolve,
    StartScanPlanet,

    // -- Plans --
    SetViewPlan(serde_json::Value),
    StopViewPlan,
}

/// Every JSON-RPC method string the [`Command`] enum can produce.
///
/// Kept in lockstep with [`Command::method`] (asserted by a unit test). Used by
/// the conformance/parity tooling to report which methods this crate models.
pub fn command_method_names() -> &'static [&'static str] {
    &[
        "test_connection",
        "pi_is_verified",
        "pi_reboot",
        "pi_get_time",
        "pi_set_time",
        "get_device_state",
        "get_view_state",
        "get_camera_info",
        "get_camera_state",
        "get_setting",
        "get_stack_setting",
        "get_stack_info",
        "get_disk_volume",
        "get_user_location",
        "get_wheel_position",
        "get_wheel_setting",
        "get_wheel_state",
        "get_last_solve_result",
        "get_solve_result",
        "get_annotated_result",
        "scope_get_equ_coord",
        "scope_get_ra_dec",
        "scope_get_horiz_coord",
        "scope_sync",
        "scope_park",
        "scope_move_to_horizon",
        "scope_speed_move",
        "scope_set_track_state",
        "goto_target",
        "iscope_start_view",
        "iscope_stop_view",
        "iscope_start_stack",
        "get_focuser_position",
        "move_focuser",
        "start_auto_focuse",
        "stop_auto_focuse",
        "set_user_location",
        "set_setting",
        "set_stack_setting",
        "set_control_value",
        "pi_output_set2",
        "set_sequence_setting",
        "play_sound",
        "pi_station_state",
        "get_stacked_img",
        "start_solve",
        "start_scan_planet",
        "set_view_plan",
        "stop_func",
    ]
}

impl Command {
    /// Returns the JSON-RPC method string for this command.
    pub fn method(&self) -> &'static str {
        match self {
            Self::TestConnection => "test_connection",
            Self::PiIsVerified => "pi_is_verified",
            Self::PiReboot => "pi_reboot",
            Self::PiGetTime => "pi_get_time",
            Self::PiSetTime(_) => "pi_set_time",
            Self::GetDeviceState => "get_device_state",
            Self::GetViewState => "get_view_state",
            Self::GetCameraInfo => "get_camera_info",
            Self::GetCameraState => "get_camera_state",
            Self::GetSetting => "get_setting",
            Self::GetStackSetting => "get_stack_setting",
            Self::GetStackInfo => "get_stack_info",
            Self::GetDiskVolume => "get_disk_volume",
            Self::GetUserLocation => "get_user_location",
            Self::GetWheelPosition => "get_wheel_position",
            Self::GetWheelSetting => "get_wheel_setting",
            Self::GetWheelState => "get_wheel_state",
            Self::GetLastSolveResult => "get_last_solve_result",
            Self::GetSolveResult => "get_solve_result",
            Self::GetAnnotatedResult => "get_annotated_result",
            Self::ScopeGetEquCoord => "scope_get_equ_coord",
            Self::ScopeGetRaDec => "scope_get_ra_dec",
            Self::ScopeGetHorizCoord => "scope_get_horiz_coord",
            Self::ScopeSync(_, _) => "scope_sync",
            Self::ScopePark => "scope_park",
            Self::ScopeParkMode(_) => "scope_park",
            Self::ScopeMoveToHorizon => "scope_move_to_horizon",
            Self::ScopeSpeedMove(_) => "scope_speed_move",
            Self::ScopeSetTrackState(_) => "scope_set_track_state",
            Self::GotoTarget(_) => "goto_target",
            Self::IscopeStartView(_) => "iscope_start_view",
            Self::IscopeStopView(_) => "iscope_stop_view",
            Self::IscopeStartStack(_) => "iscope_start_stack",
            Self::GetFocuserPosition => "get_focuser_position",
            Self::MoveFocuser(_) => "move_focuser",
            Self::StartAutoFocus => "start_auto_focuse",
            Self::StopAutoFocus => "stop_auto_focuse",
            Self::SetUserLocation(_) => "set_user_location",
            Self::SetSetting(_) => "set_setting",
            Self::SetStackSetting(_) => "set_stack_setting",
            Self::SetControlValue(_, _) => "set_control_value",
            Self::PiOutputSet2(_) => "pi_output_set2",
            Self::SetSequenceSetting(_) => "set_sequence_setting",
            Self::PlaySound(_) => "play_sound",
            Self::PiStationState => "pi_station_state",
            Self::GetStackedImage => "get_stacked_img",
            Self::StartSolve => "start_solve",
            Self::StartScanPlanet => "start_scan_planet",
            Self::SetViewPlan(_) => "set_view_plan",
            Self::StopViewPlan => "stop_func",
        }
    }
}

/// A command sent to the telescope's **imaging port (4800)**.
///
/// These are distinct from control-port [`Command`]s in two ways: they are
/// fire-and-forget (the scope answers with binary image frames, not a
/// correlated JSON response), and they never get `verify` injected. Send one
/// with [`SeestarClient::send_imaging`](crate::SeestarClient::send_imaging).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ImagingCommand {
    /// Start the live frame stream. In star mode the scope then pushes
    /// full-resolution raw frames over 4800 (verified against firmware 6.70);
    /// solar/moon/planet/scenery modes stream via RTSP instead.
    BeginStreaming,
    /// Stop the live frame stream. Port inferred by symmetry with
    /// [`BeginStreaming`](Self::BeginStreaming); not yet hardware-verified.
    StopStreaming,
    /// Liveness check. Used internally as the imaging-port heartbeat.
    TestConnection,
}

impl ImagingCommand {
    /// The JSON-RPC method string for this command.
    pub fn method(&self) -> &'static str {
        match self {
            Self::BeginStreaming => "begin_streaming",
            Self::StopStreaming => "stop_streaming",
            Self::TestConnection => "test_connection",
        }
    }

    /// Serialize to the wire form — a single JSON line **without** a trailing
    /// newline (the imaging connection appends `\r\n`). `id` is included for
    /// protocol shape but the imaging port does not correlate responses to it.
    pub fn serialize(&self, id: u64) -> Vec<u8> {
        let method = self.method();
        // `test_connection` carries `params:"verify"`; the others take none.
        let line = match self {
            Self::TestConnection => {
                format!(r#"{{"id":{id},"method":"{method}","params":"verify"}}"#)
            }
            _ => format!(r#"{{"id":{id},"method":"{method}"}}"#),
        };
        line.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// One `Command` per variant — used to prove [`command_method_names`] stays
    /// exhaustive and in sync with [`Command::method`]. Adding a variant without
    /// updating either list fails this test.
    fn one_of_every_variant() -> Vec<Command> {
        vec![
            Command::TestConnection,
            Command::PiIsVerified,
            Command::PiReboot,
            Command::PiGetTime,
            Command::PiSetTime(SetTimeParams {
                year: 2026,
                mon: 1,
                day: 1,
                hour: 0,
                min: 0,
                sec: 0,
                time_zone: "UTC".into(),
            }),
            Command::GetDeviceState,
            Command::GetViewState,
            Command::GetCameraInfo,
            Command::GetCameraState,
            Command::GetSetting,
            Command::GetStackSetting,
            Command::GetStackInfo,
            Command::GetDiskVolume,
            Command::GetUserLocation,
            Command::GetWheelPosition,
            Command::GetWheelSetting,
            Command::GetWheelState,
            Command::GetLastSolveResult,
            Command::GetSolveResult,
            Command::GetAnnotatedResult,
            Command::ScopeGetEquCoord,
            Command::ScopeGetRaDec,
            Command::ScopeGetHorizCoord,
            Command::ScopeSync(0.0, 0.0),
            Command::ScopePark,
            Command::ScopeParkMode(true),
            Command::ScopeMoveToHorizon,
            Command::ScopeSpeedMove(SpeedMoveParams {
                angle: 0,
                level: 0,
                dur_sec: 0,
                percent: 0,
            }),
            Command::ScopeSetTrackState(true),
            Command::GotoTarget(GotoTargetParams {
                target_name: "M31".into(),
                is_j2000: true,
                ra: 0.0,
                dec: 0.0,
            }),
            Command::IscopeStartView(StartViewParams {
                mode: None,
                target_name: None,
                target_ra_dec: None,
                target_type: None,
                lp_filter: None,
            }),
            Command::IscopeStopView(None),
            Command::IscopeStartStack(None),
            Command::GetFocuserPosition,
            Command::MoveFocuser(MoveFocuserParams {
                step: 0,
                ret_step: true,
            }),
            Command::StartAutoFocus,
            Command::StopAutoFocus,
            Command::SetUserLocation(SetUserLocationParams {
                lat: 0.0,
                lon: 0.0,
                force: true,
            }),
            Command::SetSetting(SettingParams::default()),
            Command::SetStackSetting(SetStackSettingParams::default()),
            Command::SetControlValue("gain".into(), 0),
            Command::PiOutputSet2(json!({})),
            Command::SetSequenceSetting(vec![]),
            Command::PlaySound(PlaySoundParams { num: 0 }),
            Command::PiStationState,
            Command::GetStackedImage,
            Command::StartSolve,
            Command::StartScanPlanet,
            Command::SetViewPlan(json!({})),
            Command::StopViewPlan,
        ]
    }

    #[test]
    fn method_names_list_is_exhaustive_and_in_sync() {
        use std::collections::BTreeSet;
        let from_variants: BTreeSet<&str> =
            one_of_every_variant().iter().map(|c| c.method()).collect();
        let from_list: BTreeSet<&str> = command_method_names().iter().copied().collect();

        // Every modeled command's method is in the published list, and vice versa.
        assert_eq!(
            from_variants, from_list,
            "command_method_names() is out of sync with Command::method()"
        );
        // The published list has no duplicates.
        assert_eq!(
            command_method_names().len(),
            from_list.len(),
            "duplicate method names"
        );
    }

    #[test]
    fn imaging_command_serializes_to_valid_json_with_method() {
        for (cmd, method, has_verify) in [
            (ImagingCommand::BeginStreaming, "begin_streaming", false),
            (ImagingCommand::StopStreaming, "stop_streaming", false),
            (ImagingCommand::TestConnection, "test_connection", true),
        ] {
            let bytes = cmd.serialize(7);
            // No trailing newline — the imaging connection appends it.
            assert!(
                !bytes.ends_with(b"\n"),
                "{method} should not end with newline"
            );
            let v: serde_json::Value = serde_json::from_slice(&bytes)
                .unwrap_or_else(|e| panic!("{method} not valid JSON: {e}"));
            assert_eq!(v["method"], method);
            assert_eq!(v["id"], 7);
            assert_eq!(v.get("params").is_some(), has_verify, "{method} params");
            assert_eq!(cmd.method(), method);
        }
    }
}
