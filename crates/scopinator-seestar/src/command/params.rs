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
    /// Heading of travel in degrees. See [`Direction`] for the cardinal mapping.
    pub angle: i32,
    /// Speed gear.
    pub level: i32,
    /// Duration in seconds; the scope auto-stops after this.
    pub dur_sec: i32,
    /// Speed percent. `0` stops; positive moves.
    pub percent: i32,
}

/// A cardinal slew direction for manual jogging.
///
/// Each variant maps to a `scope_speed_move` [`angle`](Direction::angle). The
/// mapping was verified empirically against a Seestar S50 on firmware 6.70 in
/// **EQ mode** (`equ_mode = true`): each cardinal is a pure axis — N/S change
/// only Declination (North increases Dec), E/W change only Right Ascension
/// (East increases RA). Alt-Az mode is **not** verified and may differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Direction {
    North,
    South,
    East,
    West,
}

impl Direction {
    /// The `scope_speed_move` `angle` (degrees) for this direction in EQ mode.
    pub const fn angle(self) -> i32 {
        match self {
            Direction::West => 0,
            Direction::North => 90,
            Direction::East => 180,
            Direction::South => 270,
        }
    }
}

impl SpeedMoveParams {
    /// Build a jog toward `direction`. `level` is the speed gear, `percent` the
    /// speed (`0` stops), `dur_sec` the run time (the scope auto-stops after it).
    pub fn toward(direction: Direction, level: i32, percent: i32, dur_sec: i32) -> Self {
        Self {
            angle: direction.angle(),
            level,
            dur_sec,
            percent,
        }
    }

    /// A stop command — `scope_speed_move` with `percent = 0`.
    pub fn stop() -> Self {
        Self {
            angle: 0,
            level: 0,
            dur_sec: 1,
            percent: 0,
        }
    }
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

/// A single sequence-setting group entry for `set_sequence_setting`.
///
/// On the wire the command sends `[[{group_name: ...}], "verify"]` — a list
/// containing the list of group entries, with `"verify"` appended by firmware
/// injection. See [`crate::command::serialize`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SequenceSettingParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_name: Option<String>,
}

/// Parameters for `play_sound`.
///
/// Observed in firmware 7.06 captures as `{"num": 80, "verify": true}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaySoundParams {
    /// Sound index to play.
    pub num: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_angles_match_verified_eq_mode_mapping() {
        // Verified live against fw 6.70 (EQ mode): 0=W, 90=N, 180=E, 270=S.
        assert_eq!(Direction::West.angle(), 0);
        assert_eq!(Direction::North.angle(), 90);
        assert_eq!(Direction::East.angle(), 180);
        assert_eq!(Direction::South.angle(), 270);
    }

    #[test]
    fn speed_move_toward_uses_direction_angle_and_stop_is_zero_percent() {
        let p = SpeedMoveParams::toward(Direction::North, 2, 60, 1);
        assert_eq!(p.angle, 90);
        assert_eq!((p.level, p.percent, p.dur_sec), (2, 60, 1));
        assert_eq!(SpeedMoveParams::stop().percent, 0);
    }

    #[test]
    fn direction_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&Direction::North).unwrap(),
            "\"north\""
        );
    }
}
