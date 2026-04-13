/// Describes what a mount can do.
#[derive(Debug, Clone)]
pub struct MountCapabilities {
    pub can_slew: bool,
    pub can_sync: bool,
    pub can_park: bool,
    pub can_track: bool,
    pub can_move_axis: bool,
}

/// Describes what a camera can do.
#[derive(Debug, Clone)]
pub struct CameraCapabilities {
    pub can_expose: bool,
    pub can_abort_exposure: bool,
    pub can_stream: bool,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub pixel_size_um: Option<f64>,
    pub bit_depth: Option<u32>,
}
