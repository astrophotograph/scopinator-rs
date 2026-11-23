use scopinator_types::{CameraState, Coordinates, SlewState, TrackingRate};

/// General device status.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct DeviceStatus {
    pub connected: bool,
    pub name: Option<String>,
    pub description: Option<String>,
}

/// Mount-specific status.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct MountStatus {
    pub slew_state: SlewState,
    pub tracking_rate: Option<TrackingRate>,
    pub coordinates: Option<Coordinates>,
    pub is_tracking: bool,
}

/// Camera-specific status.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct CameraStatus {
    pub state: CameraState,
    /// Sensor temperature in degrees Celsius.
    pub temperature: Option<f64>,
    pub cooler_on: Option<bool>,
    /// Cooler power as a percentage of its maximum.
    pub cooler_power: Option<f64>,
    /// The temperature the cooler regulates to, in degrees Celsius.
    pub target_temperature: Option<f64>,
    pub gain: Option<i32>,
    pub stacked_frames: Option<i32>,
    pub dropped_frames: Option<i32>,
}
