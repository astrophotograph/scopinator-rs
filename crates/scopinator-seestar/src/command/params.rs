use serde::{Deserialize, Serialize};

/// Parameters for `goto_target`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GotoTargetParams {
    pub target_name: String,
    pub is_j2000: bool,
    /// RA in degrees (0.0..360.0).
    pub ra: f64,
    /// Dec in degrees (-90.0..90.0).
    pub dec: f64,
}

/// Parameters for `iscope_start_view`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartViewParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<ViewMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_ra_dec: Option<(f64, f64)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_type: Option<SolarTarget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lp_filter: Option<bool>,
}

/// View modes for `iscope_start_view`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewMode {
    Star,
    Scenery,
    #[serde(rename = "solar_sys")]
    SolarSys,
}

/// Solar system target types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SolarTarget {
    Sun,
    Moon,
    Planet,
}

/// Parameters for `iscope_start_stack`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartStackParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restart: Option<bool>,
}

/// Parameters for `iscope_stop_view`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopViewParams {
    pub stage: StopStage,
}

/// Stage to stop in `iscope_stop_view`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopStage {
    DarkLibrary,
    Stack,
    AutoGoto,
}

/// Parameters for `scope_speed_move`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeedMoveParams {
    pub angle: i32,
    pub level: i32,
    pub dur_sec: i32,
    pub percent: i32,
}

/// Parameters for `move_focuser`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveFocuserParams {
    pub step: i32,
    #[serde(default = "default_true")]
    pub ret_step: bool,
}

fn default_true() -> bool {
    true
}

/// Parameters for `set_user_location`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetUserLocationParams {
    pub lat: f64,
    pub lon: f64,
    #[serde(default = "default_true")]
    pub force: bool,
}

/// Parameters for `pi_set_time`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetTimeParams {
    pub year: i32,
    pub mon: i32,
    pub day: i32,
    pub hour: i32,
    pub min: i32,
    pub sec: i32,
    pub time_zone: String,
}

/// Parameters for `set_setting`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SettingParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp_ms: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack_dither: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_discrete_frame: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_discrete_ok_frame: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_3ppa_calib: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack_lenhance: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_af: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack_after_goto: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_calib: Option<bool>,
    // Extended fields
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_power_off: Option<bool>,
    /// dark_mode: sent as 0/1 integer per ZWO quirk.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dark_mode: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focal_pos: Option<i64>,
    /// Nested stack fields: cont_capt, drizzle2x, brightness, contrast, saturation, dbe_enable, etc.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expert_mode: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_target_af: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewplan_gohome: Option<bool>,
}

/// Parameters for `set_stack_setting`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetStackSettingParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_discrete_ok_frame: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_discrete_frame: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub light_duration_min: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capt_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capt_num: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brightness: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contrast: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saturation: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dbe_enable: Option<bool>,
}

/// Parameters for `set_control_value`.
pub type SetControlValueParams = (String, i32);
