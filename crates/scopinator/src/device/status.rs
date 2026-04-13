use scopinator_types::{CameraState, Coordinates, SlewState, TrackingRate};

/// General device status.
#[derive(Debug, Clone)]
pub struct DeviceStatus {
    pub connected: bool,
    pub name: Option<String>,
    pub description: Option<String>,
}

/// Mount-specific status.
#[derive(Debug, Clone)]
pub struct MountStatus {
    pub slew_state: SlewState,
    pub tracking_rate: Option<TrackingRate>,
    pub coordinates: Option<Coordinates>,
    pub is_tracking: bool,
}

/// Camera-specific status.
#[derive(Debug, Clone)]
pub struct CameraStatus {
    pub state: CameraState,
    pub temperature: Option<f64>,
    pub gain: Option<i32>,
    pub stacked_frames: Option<i32>,
    pub dropped_frames: Option<i32>,
}
