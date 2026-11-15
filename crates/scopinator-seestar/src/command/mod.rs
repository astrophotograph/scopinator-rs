pub mod params;
pub mod serialize;

use params::*;

/// A command to send to the Seestar telescope on the control port (4700).
///
/// Each variant maps to a specific JSON-RPC method string. The `id` field
/// is assigned by the client at send time.
/// How the `verify` marker is attached to a [`Command::Raw`] request on a
/// connection that needs it. It has no effect on a verified connection,
/// where no marker is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RawMarker {
    /// By the shape of the params, as for modeled commands: none becomes
    /// `["verify"]`, an object gains `"verify": true` (written last), an
    /// array has `"verify"` appended, a scalar becomes `[value, "verify"]`.
    #[default]
    Auto,
    /// For a method called with no params that reads its first positional
    /// element: send `{"verify": true}` instead of `["verify"]`.
    /// `get_disk_volume` and `get_annotate_result` need this on firmware
    /// 6.70. With params present it behaves like `Auto`.
    Object,
}

#[derive(Debug, Clone)]
pub enum Command {
    /// Any control method by name, for the many the firmware has that this
    /// enum does not model. Prefer
    /// [`SeestarClient::send_raw`](crate::SeestarClient::send_raw).
    ///
    /// The request is framed by the connection's wire profile like a modeled
    /// command: `params` are sent as given, and the `verify` marker is added
    /// only on a connection that needs it (see [`RawMarker`]).
    Raw {
        /// The method name as the firmware spells it.
        method: String,
        /// The params exactly as they should appear; `None` sends none.
        params: Option<serde_json::Value>,
        /// How the marker is attached where one is needed.
        marker: RawMarker,
    },

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
    GetStackInfo,
    GetDiskVolume,
    GetUserLocation,
    GetWheelPosition,
    GetWheelSetting,
    GetWheelState,
    GetLastSolveResult,
    GetSolveResult,
    /// `get_annotate_result`: the cached annotation of the latest image.
    /// The firmware answers 207 when there is none.
    GetAnnotateResult,

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
    SetControlValue(String, i32),
    PiOutputSet2(serde_json::Value),
    /// `set_sequence_setting`: patch the autosave and sequence settings. The
    /// params are the settings object (present on firmware 6.70 too).
    /// `get_sequence_setting`: the autosave and sequence settings.
    GetSequenceSetting,
    SetSequenceSetting(SequenceSettingParams),

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

    // -- Plate solving --
    StartSolve,
    StartScanPlanet,

    // -- Alignment --
    /// EQ-mode 3-point polar alignment ("3PPA"). Requires a clear view of the
    /// sky; EQ mode only.
    StartPolarAlign(PolarAlignParams),

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
        "get_stack_info",
        "get_disk_volume",
        "get_user_location",
        "get_wheel_position",
        "get_wheel_setting",
        "get_wheel_state",
        "get_last_solve_result",
        "get_solve_result",
        "get_annotate_result",
        "scope_get_equ_coord",
        "scope_get_ra_dec",
        "scope_get_horiz_coord",
        "scope_sync",
        "scope_park",
        "scope_move_to_horizon",
        "scope_speed_move",
        "scope_set_track_state",
        "iscope_start_view",
        "iscope_stop_view",
        "iscope_start_stack",
        "get_focuser_position",
        "move_focuser",
        "start_auto_focuse",
        "stop_auto_focuse",
        "set_user_location",
        "set_setting",
        "set_control_value",
        "pi_output_set2",
        "get_sequence_setting",
        "set_sequence_setting",
        "play_sound",
        "pi_station_state",
        "start_solve",
        "start_scan_planet",
        "start_polar_align",
        "set_view_plan",
        "stop_func",
    ]
}

impl Command {
    /// Returns the JSON-RPC method string for this command.
    pub fn method(&self) -> &str {
        match self {
            Self::Raw { method, .. } => method,
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
            Self::GetStackInfo => "get_stack_info",
            Self::GetDiskVolume => "get_disk_volume",
            Self::GetUserLocation => "get_user_location",
            Self::GetWheelPosition => "get_wheel_position",
            Self::GetWheelSetting => "get_wheel_setting",
            Self::GetWheelState => "get_wheel_state",
            Self::GetLastSolveResult => "get_last_solve_result",
            Self::GetSolveResult => "get_solve_result",
            Self::GetAnnotateResult => "get_annotate_result",
            Self::ScopeGetEquCoord => "scope_get_equ_coord",
            Self::ScopeGetRaDec => "scope_get_ra_dec",
            Self::ScopeGetHorizCoord => "scope_get_horiz_coord",
            Self::ScopeSync(_, _) => "scope_sync",
            Self::ScopePark => "scope_park",
            Self::ScopeParkMode(_) => "scope_park",
            Self::ScopeMoveToHorizon => "scope_move_to_horizon",
            Self::ScopeSpeedMove(_) => "scope_speed_move",
            Self::ScopeSetTrackState(_) => "scope_set_track_state",
            Self::IscopeStartView(_) => "iscope_start_view",
            Self::IscopeStopView(_) => "iscope_stop_view",
            Self::IscopeStartStack(_) => "iscope_start_stack",
            Self::GetFocuserPosition => "get_focuser_position",
            Self::MoveFocuser(_) => "move_focuser",
            Self::StartAutoFocus => "start_auto_focuse",
            Self::StopAutoFocus => "stop_auto_focuse",
            Self::SetUserLocation(_) => "set_user_location",
            Self::SetSetting(_) => "set_setting",
            Self::SetControlValue(_, _) => "set_control_value",
            Self::PiOutputSet2(_) => "pi_output_set2",
            Self::GetSequenceSetting => "get_sequence_setting",
            Self::SetSequenceSetting(_) => "set_sequence_setting",
            Self::PlaySound(_) => "play_sound",
            Self::PiStationState => "pi_station_state",
            Self::StartSolve => "start_solve",
            Self::StartScanPlanet => "start_scan_planet",
            Self::StartPolarAlign(_) => "start_polar_align",
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
    /// Fetch the stacked image: a JPEG when the device's
    /// `image_transfer_mode` is Jpg, otherwise zip-compressed raw. The device
    /// answers `there is no image now` when nothing has been stacked.
    GetStackedImage,
    /// Fetch the current preview frame. `compress` (device default true)
    /// selects zip-compressed raw; `raw` forces raw output even when the
    /// transfer mode is Jpg.
    GetCurrentImage { compress: Option<bool>, raw: bool },
    /// Fetch the photometry image; encoded like the stacked image.
    GetPhotometryImage,
}

impl ImagingCommand {
    /// The JSON-RPC method string for this command.
    pub fn method(&self) -> &'static str {
        match self {
            Self::BeginStreaming => "begin_streaming",
            Self::StopStreaming => "stop_streaming",
            Self::TestConnection => "test_connection",
            Self::GetStackedImage => "get_stacked_img",
            Self::GetCurrentImage { .. } => "get_current_img",
            Self::GetPhotometryImage => "get_photometry_img",
        }
    }

    /// Serialize to the wire form — a single JSON line **without** a trailing
    /// newline (the imaging connection appends `\r\n`). The device echoes the
    /// low byte of `id` in the header of every reply, which is how a reply is
    /// matched to its request; `id` must be an integer.
    pub fn serialize(&self, id: u64) -> Vec<u8> {
        let method = self.method();
        // `test_connection` carries `params:"verify"`; `get_current_img` is
        // the only method that reads params; the others take none.
        let line = match self {
            Self::TestConnection => {
                format!(r#"{{"id":{id},"method":"{method}","params":"verify"}}"#)
            }
            Self::GetCurrentImage { compress, raw } if compress.is_some() || *raw => {
                let mut params = serde_json::Map::new();
                if let Some(compress) = compress {
                    params.insert("compress".into(), (*compress).into());
                }
                if *raw {
                    params.insert("img_type".into(), "Raw".into());
                }
                let params = serde_json::Value::Object(params);
                format!(r#"{{"id":{id},"method":"{method}","params":{params}}}"#)
            }
            _ => format!(r#"{{"id":{id},"method":"{method}"}}"#),
        };
        line.into_bytes()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    /// One `Command` per variant — used to prove [`command_method_names`] stays
    /// exhaustive and in sync with [`Command::method`]. Adding a variant without
    /// updating either list fails this test.
    pub(crate) fn one_of_every_variant() -> Vec<Command> {
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
            Command::GetStackInfo,
            Command::GetDiskVolume,
            Command::GetUserLocation,
            Command::GetWheelPosition,
            Command::GetWheelSetting,
            Command::GetWheelState,
            Command::GetLastSolveResult,
            Command::GetSolveResult,
            Command::GetAnnotateResult,
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
            Command::SetControlValue("gain".into(), 0),
            Command::PiOutputSet2(json!({})),
            Command::GetSequenceSetting,
            Command::SetSequenceSetting(SequenceSettingParams::default()),
            Command::PlaySound(PlaySoundParams { num: 0 }),
            Command::PiStationState,
            Command::StartSolve,
            Command::StartScanPlanet,
            Command::StartPolarAlign(PolarAlignParams {
                restart: true,
                dec_pos_index: 0,
            }),
            Command::SetViewPlan(json!({})),
            Command::StopViewPlan,
        ]
    }

    #[test]
    fn method_names_list_is_exhaustive_and_in_sync() {
        use std::collections::BTreeSet;
        let variants = one_of_every_variant();
        let from_variants: BTreeSet<&str> = variants.iter().map(|c| c.method()).collect();
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
            (ImagingCommand::GetStackedImage, "get_stacked_img", false),
            (
                ImagingCommand::GetPhotometryImage,
                "get_photometry_img",
                false,
            ),
            (
                ImagingCommand::GetCurrentImage {
                    compress: None,
                    raw: false,
                },
                "get_current_img",
                false,
            ),
            (
                ImagingCommand::GetCurrentImage {
                    compress: Some(false),
                    raw: true,
                },
                "get_current_img",
                true,
            ),
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
