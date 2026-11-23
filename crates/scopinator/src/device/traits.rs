//! The device traits every backend implements.
//!
//! # Operations that take time
//!
//! A slew, a park, an exposure, a focuser or filter wheel move all outlast
//! the request that starts them. Every backend follows the same rule (see
//! `docs/device-contract.md`):
//!
//! 1. The method that starts one returns `Ok(())` when the device has
//!    **accepted** the request, not when the operation has finished.
//! 2. A state query says whether it is still running:
//!    [`Mount::is_slewing`], [`Mount::is_parked`], [`Camera::image_ready`],
//!    [`Focuser::is_moving`], [`FilterWheel::is_moving`].
//! 3. An operation that fails after it was accepted makes the next state
//!    query return an error, once.
//! 4. Waiting is done on top of the queries, the same way for every
//!    backend, not by the backends themselves.

use async_trait::async_trait;
use scopinator_types::{Coordinates, DeviceId, DeviceInfo, ExposureSettings, ImageData};

use crate::device::capabilities::{CameraCapabilities, MountCapabilities};
use crate::device::status::{CameraStatus, DeviceStatus, MountStatus};
use crate::error::ScopinatorError;

/// Base trait for all telescope devices.
#[async_trait]
pub trait Device: Send + Sync {
    /// This device's id: unique among the devices a program has open. The
    /// devices of one instrument share its id as a prefix, e.g.
    /// `seestar:192.168.1.100:mount` and `seestar:192.168.1.100:camera`.
    fn id(&self) -> DeviceId;

    /// Who this device is: id, kind, name and, where the device reports
    /// them, model and firmware. Does no I/O; fields the device has not
    /// reported yet are `None`.
    fn info(&self) -> DeviceInfo;

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

    /// Start a slew to equatorial coordinates. Returns when the mount has
    /// accepted the request; [`is_slewing`](Self::is_slewing) says when it
    /// has arrived.
    async fn slew_to_coordinates(&self, coords: &Coordinates) -> Result<(), ScopinatorError>;

    /// Abort any in-progress slew.
    async fn abort_slew(&self) -> Result<(), ScopinatorError>;

    /// True from the moment a slew is accepted until the mount has arrived
    /// and is ready to be used, which on a telescope that centres its target
    /// by plate solving includes the centring.
    ///
    /// If the slew failed after it was accepted, the first call after the
    /// failure returns the error and later calls return `false`.
    async fn is_slewing(&self) -> Result<bool, ScopinatorError>;

    /// True when the mount is parked. False while it is on its way to the
    /// park position.
    async fn is_parked(&self) -> Result<bool, ScopinatorError>;

    /// Start parking the mount. Returns when the mount has accepted the
    /// request; [`is_parked`](Self::is_parked) says when it is there.
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
    /// Start an exposure with the given settings. Returns when the camera
    /// has accepted the request; [`image_ready`](Self::image_ready) says
    /// when there is an image to fetch.
    async fn start_exposure(&self, settings: &ExposureSettings) -> Result<(), ScopinatorError>;

    /// Abort the current exposure.
    async fn abort_exposure(&self) -> Result<(), ScopinatorError>;

    /// True when the exposure started by the last
    /// [`start_exposure`](Self::start_exposure) has produced an image that
    /// [`get_image`](Self::get_image) will return. On a camera that streams
    /// or stacks, true as soon as there is a frame.
    async fn image_ready(&self) -> Result<bool, ScopinatorError>;

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

    /// Start a move to an absolute position. Returns when the focuser has
    /// accepted the request.
    async fn move_to(&self, position: i32) -> Result<(), ScopinatorError>;

    /// Start a move by a relative amount. Returns when the focuser has
    /// accepted the request.
    async fn move_relative(&self, offset: i32) -> Result<(), ScopinatorError>;

    /// True while the focuser is moving.
    async fn is_moving(&self) -> Result<bool, ScopinatorError>;

    /// Halt any in-progress movement.
    async fn halt(&self) -> Result<(), ScopinatorError>;
}

/// A filter wheel.
#[async_trait]
pub trait FilterWheel: Device {
    /// Get the current filter position.
    async fn get_position(&self) -> Result<u32, ScopinatorError>;

    /// Start a move to a filter position. Returns when the wheel has
    /// accepted the request.
    async fn set_position(&self, position: u32) -> Result<(), ScopinatorError>;

    /// True while the wheel is turning.
    async fn is_moving(&self) -> Result<bool, ScopinatorError>;

    /// Get the names of available filters.
    async fn get_filter_names(&self) -> Result<Vec<String>, ScopinatorError>;
}
