use scopinator_types::Epoch;

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
    pub pixel_size_um: Option<f64>,
    pub bit_depth: Option<u32>,
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
        assert_eq!(caps.pixel_size_um, None);
        assert_eq!(caps.bit_depth, None);
    }
}
