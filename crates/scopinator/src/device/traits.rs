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
use scopinator_types::{
    AlignmentMode, AltAzCoordinates, Coordinates, DeviceId, DeviceInfo, ExposureSettings,
    FilterPosition, FocuserPosition, GuideDirection, ImageData, MountAxis, PierSide, SiteLocation,
    SwitchInfo, TrackingRate,
};

use crate::device::capabilities::{
    CameraCapabilities, FilterWheelCapabilities, FocuserCapabilities, MountCapabilities,
};
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
    /// Get the current equatorial coordinates, in the mount's own system
    /// ([`MountCapabilities::equatorial_system`]) and labelled with it. Use
    /// [`Coordinates::to_epoch`] for another system.
    async fn get_coordinates(&self) -> Result<Coordinates, ScopinatorError>;

    /// Start a slew to equatorial coordinates. Returns when the mount has
    /// accepted the request; [`is_slewing`](Self::is_slewing) says when it
    /// has arrived.
    ///
    /// The coordinates can be in any system: the backend converts them to
    /// the one the mount works in. A catalogue position is J2000.
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

    /// Leave the park position, on a mount that has a request for it
    /// ([`MountCapabilities::can_unpark`]). Returns when the mount has
    /// accepted the request; [`is_parked`](Self::is_parked) says when it is
    /// out.
    async fn unpark(&self) -> Result<(), ScopinatorError> {
        Err(ScopinatorError::NotSupported("unpark".into()))
    }

    /// Tell the mount that it is pointing at `coords`, correcting its
    /// pointing model, without moving it. The coordinates can be in any
    /// system, as for a slew.
    async fn sync_to_coordinates(&self, _coords: &Coordinates) -> Result<(), ScopinatorError> {
        Err(ScopinatorError::NotSupported("sync".into()))
    }

    /// Start moving one axis by hand, as from a direction pad, and keep
    /// moving until [`stop_motion`](Self::stop_motion).
    ///
    /// `speed` is a fraction of the mount's manual slewing speed, from -1 to
    /// 1; a value outside that is an `InvalidArgument`. Positive is towards
    /// increasing right ascension or azimuth on the primary axis and
    /// increasing declination or altitude on the secondary axis. Zero stops
    /// that axis.
    ///
    /// A backend may stop the motion by itself after some seconds as a
    /// safeguard against a lost connection; to keep moving, send the request
    /// again. Whether both axes can move at once depends on the mount.
    async fn move_axis(&self, _axis: MountAxis, _speed: f64) -> Result<(), ScopinatorError> {
        Err(ScopinatorError::NotSupported("move_axis".into()))
    }

    /// Stop all motion started with [`move_axis`](Self::move_axis). Does not
    /// abort a slew or stop tracking.
    async fn stop_motion(&self) -> Result<(), ScopinatorError> {
        Err(ScopinatorError::NotSupported("move_axis".into()))
    }

    /// Where the mount points above the horizon.
    async fn get_alt_az(&self) -> Result<AltAzCoordinates, ScopinatorError> {
        Err(ScopinatorError::NotSupported("alt-az position".into()))
    }

    /// Start a slew to an altitude and azimuth. The mount does not track
    /// afterwards. Returns on acceptance, like
    /// [`slew_to_coordinates`](Self::slew_to_coordinates).
    async fn slew_to_alt_az(&self, _target: &AltAzCoordinates) -> Result<(), ScopinatorError> {
        Err(ScopinatorError::NotSupported("alt-az slew".into()))
    }

    /// Choose the rate the mount tracks at, from
    /// [`MountCapabilities::tracking_rates`]. This does not turn tracking
    /// on or off.
    async fn set_tracking_rate(&self, _rate: TrackingRate) -> Result<(), ScopinatorError> {
        Err(ScopinatorError::NotSupported("tracking rate".into()))
    }

    /// Which side of the pier a German equatorial mount is on.
    async fn side_of_pier(&self) -> Result<PierSide, ScopinatorError> {
        Err(ScopinatorError::NotSupported("side of pier".into()))
    }

    /// Start a search for the home position. Returns on acceptance;
    /// [`is_at_home`](Self::is_at_home) says when the mount is there.
    async fn find_home(&self) -> Result<(), ScopinatorError> {
        Err(ScopinatorError::NotSupported("find home".into()))
    }

    /// True when the mount is at its home position.
    async fn is_at_home(&self) -> Result<bool, ScopinatorError> {
        Err(ScopinatorError::NotSupported("home position".into()))
    }

    /// Nudge the mount in one direction for `duration`, at its guide rate.
    /// Returns when the mount has accepted the pulse.
    async fn pulse_guide(
        &self,
        _direction: GuideDirection,
        _duration: std::time::Duration,
    ) -> Result<(), ScopinatorError> {
        Err(ScopinatorError::NotSupported("pulse guiding".into()))
    }

    /// How the mount's axes are arranged right now. Some mounts can be used
    /// either way.
    async fn alignment_mode(&self) -> Result<AlignmentMode, ScopinatorError> {
        Err(ScopinatorError::NotSupported("alignment mode".into()))
    }

    /// Where the mount believes it is.
    ///
    /// A goto mount works out where a target is in the sky from its site and
    /// its clock. With either wrong it points wrong, and some refuse to move
    /// without saying why.
    async fn get_site(&self) -> Result<SiteLocation, ScopinatorError> {
        Err(ScopinatorError::NotSupported("site location".into()))
    }

    /// Tell the mount where it is.
    async fn set_site(&self, _site: &SiteLocation) -> Result<(), ScopinatorError> {
        Err(ScopinatorError::NotSupported("site location".into()))
    }

    /// The time on the mount's clock.
    async fn get_utc(&self) -> Result<chrono::DateTime<chrono::Utc>, ScopinatorError> {
        Err(ScopinatorError::NotSupported("clock".into()))
    }

    /// Set the mount's clock. A mount that keeps a time zone keeps it.
    async fn set_utc(&self, _time: chrono::DateTime<chrono::Utc>) -> Result<(), ScopinatorError> {
        Err(ScopinatorError::NotSupported("clock".into()))
    }

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

    /// Turn the sensor cooler on or off, on a camera that has one
    /// ([`CameraCapabilities::has_cooler`]).
    async fn set_cooler(&self, _on: bool) -> Result<(), ScopinatorError> {
        Err(ScopinatorError::NotSupported("cooler".into()))
    }

    /// Set the temperature the cooler regulates to, in degrees Celsius.
    async fn set_target_temperature(&self, _celsius: f64) -> Result<(), ScopinatorError> {
        Err(ScopinatorError::NotSupported("cooler".into()))
    }

    /// How hard the cooler is working, as a percentage of its maximum.
    async fn cooler_power(&self) -> Result<f64, ScopinatorError> {
        Err(ScopinatorError::NotSupported("cooler".into()))
    }

    /// Get camera capabilities.
    fn capabilities(&self) -> CameraCapabilities;

    /// Get camera-specific status.
    async fn get_camera_status(&self) -> Result<CameraStatus, ScopinatorError>;
}

/// A telescope focuser.
#[async_trait]
pub trait Focuser: Device {
    /// Get focuser capabilities.
    fn capabilities(&self) -> FocuserCapabilities;

    /// Where the focuser is, with what else it reports: the end of its
    /// travel, its temperature, whether it is moving. On a focuser that is
    /// not [`absolute`](FocuserCapabilities::absolute) the position means
    /// nothing.
    async fn focuser_position(&self) -> Result<FocuserPosition, ScopinatorError>;

    /// Start a move to an absolute position. Returns when the focuser has
    /// accepted the request. `NotSupported` on a focuser that is not
    /// absolute.
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
///
/// Filter positions count from zero, whatever the device's own protocol
/// does; a backend translates.
#[async_trait]
pub trait FilterWheel: Device {
    /// Get filter wheel capabilities.
    fn capabilities(&self) -> FilterWheelCapabilities;

    /// The filter in the light path, or `None` while the wheel is turning.
    async fn filter_position(&self) -> Result<Option<FilterPosition>, ScopinatorError>;

    /// Start a move to a filter position, counted from zero. Returns when
    /// the wheel has accepted the request.
    async fn set_filter_position(&self, position: u32) -> Result<(), ScopinatorError>;

    /// True while the wheel is turning.
    async fn is_moving(&self) -> Result<bool, ScopinatorError>;

    /// Get the names of available filters, in position order.
    async fn get_filter_names(&self) -> Result<Vec<String>, ScopinatorError>;

    /// The focus offset of each filter, in focuser steps and position
    /// order, on a wheel that knows them.
    async fn get_focus_offsets(&self) -> Result<Vec<i32>, ScopinatorError> {
        Err(ScopinatorError::NotSupported("focus offsets".into()))
    }
}

/// A device with one or more outputs that can be switched or set: dew
/// heaters, power ports, a flat panel's brightness.
///
/// Each output has a number from zero and a value in the range its
/// [`SwitchInfo`] gives. An on/off output has the values 0 and 1.
#[async_trait]
pub trait Switch: Device {
    /// The outputs this device has, in index order.
    async fn switches(&self) -> Result<Vec<SwitchInfo>, ScopinatorError>;

    /// The value of output `index`. `OutOfRange` if there is no such output.
    async fn get(&self, index: u32) -> Result<f64, ScopinatorError>;

    /// Set output `index` to `value`. `OutOfRange` if there is no such
    /// output or the value is outside its range; `InvalidArgument` if the
    /// output cannot be written.
    async fn set(&self, index: u32, value: f64) -> Result<(), ScopinatorError>;

    /// What is known about output `index`. `OutOfRange` if there is none.
    async fn switch_info(&self, index: u32) -> Result<SwitchInfo, ScopinatorError> {
        self.switches()
            .await?
            .into_iter()
            .find(|info| info.index == index)
            .ok_or_else(|| ScopinatorError::OutOfRange(format!("there is no switch {index}")))
    }

    /// True if output `index` is on: at or above the middle of its range.
    async fn get_bool(&self, index: u32) -> Result<bool, ScopinatorError> {
        let info = self.switch_info(index).await?;
        Ok(info.is_on(self.get(index).await?))
    }

    /// Turn output `index` fully on (the top of its range) or off (the
    /// bottom).
    async fn set_bool(&self, index: u32, on: bool) -> Result<(), ScopinatorError> {
        let info = self.switch_info(index).await?;
        self.set(index, if on { info.max } else { info.min }).await
    }
}

/// Check a value about to be written to a switch, the same way for every
/// backend: the switch must be writable and the value within its range.
pub fn check_switch_value(info: &SwitchInfo, value: f64) -> Result<(), ScopinatorError> {
    if !info.writable {
        return Err(ScopinatorError::InvalidArgument(format!(
            "switch {} ({}) can only be read",
            info.index, info.name
        )));
    }
    if !info.accepts(value) {
        return Err(ScopinatorError::OutOfRange(format!(
            "switch {} ({}) takes {} to {}, not {value}",
            info.index, info.name, info.min, info.max
        )));
    }
    Ok(())
}
