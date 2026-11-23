use scopinator_types::{Epoch, SensorType, TrackingRate};

/// Describes what a mount can do.
///
/// More fields will be added as backends need them, so the struct cannot be
/// built with a literal outside this crate: start from
/// [`Default`] (everything unsupported) and set the fields that apply.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct MountCapabilities {
    pub can_slew: bool,
    pub can_sync: bool,
    pub can_park: bool,
    /// The mount has an unpark request. A mount without one usually leaves
    /// park by itself when it is sent a slew.
    pub can_unpark: bool,
    pub can_track: bool,
    pub can_move_axis: bool,
    /// The mount can slew to an altitude and azimuth.
    pub can_slew_alt_az: bool,
    /// The tracking rate can be chosen; `tracking_rates` lists the choices.
    pub can_set_tracking_rate: bool,
    pub tracking_rates: Vec<TrackingRate>,
    /// The mount can find its home position.
    pub can_find_home: bool,
    /// The mount takes guiding pulses.
    pub can_pulse_guide: bool,
    /// The observing site can be set ([`Mount::set_site`](super::traits::Mount::set_site)).
    pub can_set_site: bool,
    /// The clock can be set ([`Mount::set_utc`](super::traits::Mount::set_utc)).
    pub can_set_time: bool,
    /// The equatorial system the mount itself works in: what
    /// [`Mount::get_coordinates`](super::traits::Mount::get_coordinates)
    /// returns, and what a slew target is converted to before it is sent.
    pub equatorial_system: Epoch,
}

/// Describes what a camera can do.
///
/// Start from [`Default`] (everything unsupported or unknown) and set the
/// fields that apply; see [`MountCapabilities`].
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct CameraCapabilities {
    pub can_expose: bool,
    pub can_abort_exposure: bool,
    pub can_stream: bool,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    /// Pixel width and height in micrometres.
    pub pixel_size_x_um: Option<f64>,
    pub pixel_size_y_um: Option<f64>,
    /// Bits of real signal per sample (12 for a 12-bit sensor, even when it
    /// delivers 16-bit samples).
    pub bit_depth: Option<u32>,
    pub sensor_type: Option<SensorType>,
    /// The lowest and highest gain the camera accepts.
    pub gain_range: Option<(i32, i32)>,
    /// The lowest and highest offset the camera accepts.
    pub offset_range: Option<(i32, i32)>,
    /// The shortest and longest exposure in seconds.
    pub exposure_range_s: Option<(f64, f64)>,
    /// The highest binning factor.
    pub max_bin: Option<u32>,
    /// The camera can read out part of its sensor.
    pub can_set_roi: bool,
    pub has_cooler: bool,
    /// The cooler regulates to a temperature that can be set.
    pub can_set_temperature: bool,
    /// Full well capacity in electrons.
    pub full_well_e: Option<f64>,
    /// Electrons per ADU at the current gain.
    pub e_per_adu: Option<f64>,
}

/// Describes what a focuser can do. Start from [`Default`].
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct FocuserCapabilities {
    /// The focuser knows its position and can be sent to one. A focuser
    /// without this only moves by relative amounts.
    pub absolute: bool,
    /// The far end of the travel, in steps.
    pub max_position: Option<i32>,
    /// The most steps one move can cover.
    pub max_increment: Option<i32>,
    /// The size of one step in micrometres.
    pub step_size_um: Option<f64>,
    /// The focuser reports a temperature.
    pub has_temperature: bool,
    /// The focuser can compensate for temperature by itself.
    pub can_temp_compensate: bool,
}

/// Describes a filter wheel. Start from [`Default`].
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct FilterWheelCapabilities {
    /// How many filters the wheel holds.
    pub slot_count: u32,
    /// The wheel knows a focus offset for each filter.
    pub has_focus_offsets: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mount_capabilities_support_nothing() {
        let caps = MountCapabilities::default();
        assert!(!caps.can_slew);
        assert!(!caps.can_sync);
        assert!(!caps.can_park);
        assert!(!caps.can_unpark);
        assert!(!caps.can_track);
        assert!(!caps.can_move_axis);
        assert!(!caps.can_set_site);
        assert!(!caps.can_slew_alt_az);
        assert!(!caps.can_set_tracking_rate);
        assert!(caps.tracking_rates.is_empty());
        assert!(!caps.can_find_home);
        assert!(!caps.can_pulse_guide);
        assert!(!caps.can_set_time);
        assert_eq!(caps.equatorial_system, Epoch::J2000);
    }

    #[test]
    fn default_camera_capabilities_support_nothing() {
        let caps = CameraCapabilities::default();
        assert!(!caps.can_expose);
        assert!(!caps.can_abort_exposure);
        assert!(!caps.can_stream);
        assert_eq!(caps.max_width, None);
        assert_eq!(caps.max_height, None);
        assert_eq!(caps.pixel_size_x_um, None);
        assert_eq!(caps.pixel_size_y_um, None);
        assert_eq!(caps.sensor_type, None);
        assert_eq!(caps.gain_range, None);
        assert!(!caps.can_set_roi);
        assert!(!caps.has_cooler);
        assert!(!caps.can_set_temperature);
        assert_eq!(caps.bit_depth, None);
    }
}
