use async_trait::async_trait;
use scopinator_types::{Coordinates, ExposureSettings, ImageData};

use crate::device::capabilities::{CameraCapabilities, MountCapabilities};
use crate::device::status::{CameraStatus, DeviceStatus, MountStatus};
use crate::error::ScopinatorError;

/// Base trait for all telescope devices.
#[async_trait]
pub trait Device: Send + Sync {
    /// Connect to the device.
    async fn connect(&self) -> Result<(), ScopinatorError>;

    /// Disconnect from the device.
    async fn disconnect(&self) -> Result<(), ScopinatorError>;

    /// Returns true if currently connected.
    fn is_connected(&self) -> bool;

    /// Get general device status.
    async fn get_status(&self) -> Result<DeviceStatus, ScopinatorError>;
}

/// A telescope mount.
#[async_trait]
pub trait Mount: Device {
    /// Get the current equatorial coordinates.
    async fn get_coordinates(&self) -> Result<Coordinates, ScopinatorError>;

    /// Slew to equatorial coordinates.
    async fn slew_to_coordinates(&self, coords: &Coordinates) -> Result<(), ScopinatorError>;

    /// Abort any in-progress slew.
    async fn abort_slew(&self) -> Result<(), ScopinatorError>;

    /// Park the mount.
    async fn park(&self) -> Result<(), ScopinatorError>;

    /// Enable or disable sidereal tracking.
    async fn set_tracking(&self, enabled: bool) -> Result<(), ScopinatorError>;

    /// Returns true if the mount is currently tracking.
    async fn is_tracking(&self) -> Result<bool, ScopinatorError>;

    /// Get mount capabilities.
    fn capabilities(&self) -> MountCapabilities;

    /// Get mount-specific status.
    async fn get_mount_status(&self) -> Result<MountStatus, ScopinatorError>;
}

/// A telescope camera.
#[async_trait]
pub trait Camera: Device {
    /// Start an exposure with the given settings.
    async fn start_exposure(&self, settings: &ExposureSettings) -> Result<(), ScopinatorError>;

    /// Abort the current exposure.
    async fn abort_exposure(&self) -> Result<(), ScopinatorError>;

    /// Get the latest image.
    async fn get_image(&self) -> Result<ImageData, ScopinatorError>;

    /// Returns true if currently exposing.
    async fn is_exposing(&self) -> Result<bool, ScopinatorError>;

    /// Get camera capabilities.
    fn capabilities(&self) -> CameraCapabilities;

    /// Get camera-specific status.
    async fn get_camera_status(&self) -> Result<CameraStatus, ScopinatorError>;
}

/// A telescope focuser.
#[async_trait]
pub trait Focuser: Device {
    /// Get the current focuser position.
    async fn get_position(&self) -> Result<i32, ScopinatorError>;

    /// Move to an absolute position.
    async fn move_to(&self, position: i32) -> Result<(), ScopinatorError>;

    /// Move by a relative amount.
    async fn move_relative(&self, offset: i32) -> Result<(), ScopinatorError>;

    /// Halt any in-progress movement.
    async fn halt(&self) -> Result<(), ScopinatorError>;
}

/// A filter wheel.
#[async_trait]
pub trait FilterWheel: Device {
    /// Get the current filter position.
    async fn get_position(&self) -> Result<u32, ScopinatorError>;

    /// Set the filter position.
    async fn set_position(&self, position: u32) -> Result<(), ScopinatorError>;

    /// Get the names of available filters.
    async fn get_filter_names(&self) -> Result<Vec<String>, ScopinatorError>;
}
