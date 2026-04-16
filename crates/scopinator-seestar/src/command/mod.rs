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
    IscopeStopView(StopViewParams),
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

    // -- Imaging --
    BeginStreaming,
    StopStreaming,
    GetStackedImage,

    // -- Plate solving --
    StartSolve,
    StartScanPlanet,

    // -- Plans --
    SetViewPlan(serde_json::Value),
    StopViewPlan,
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
            Self::BeginStreaming => "begin_streaming",
            Self::StopStreaming => "stop_streaming",
            Self::GetStackedImage => "get_stacked_img",
            Self::StartSolve => "start_solve",
            Self::StartScanPlanet => "start_scan_planet",
            Self::SetViewPlan(_) => "set_view_plan",
            Self::StopViewPlan => "stop_func",
        }
    }
}
