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
    pub can_track: bool,
    pub can_move_axis: bool,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mount_capabilities_support_nothing() {
        let caps = MountCapabilities::default();
        assert!(!caps.can_slew);
        assert!(!caps.can_sync);
        assert!(!caps.can_park);
        assert!(!caps.can_track);
        assert!(!caps.can_move_axis);
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
