use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use scopinator_alpaca::AlpacaTelescope;
use scopinator_alpaca::telescope::{
    AlignmentMode as AlpacaAlignment, DriveRate, EquatorialSystem, GuideDirection as AlpacaGuide,
    PierSide as AlpacaPierSide, TelescopeAxis,
};
use scopinator_types::{
    AlignmentMode, AltAzCoordinates, Coordinates, DecDegrees, DeviceId, DeviceInfo, DeviceKind,
    Epoch, GuideDirection, MountAxis, PierSide, RaDegrees, SiteLocation, SlewState, TrackingRate,
};

use super::{neutral, optional};
use crate::device::capabilities::MountCapabilities;
use crate::device::status::{DeviceStatus, MountStatus};
use crate::device::traits::{Device, Mount};
use crate::error::ScopinatorError;

/// An Alpaca `telescope` device as a neutral mount.
///
/// What the mount can do is read from its `Can...` properties when it is
/// connected and kept; [`capabilities`](Mount::capabilities) answers from
/// that.
pub struct AlpacaMount {
    scope: AlpacaTelescope,
    id: DeviceId,
    connected: AtomicBool,
    /// Read at connect: what the mount can do and how it describes itself.
    profile: RwLock<Profile>,
}

#[derive(Debug, Clone, Default)]
struct Profile {
    capabilities: MountCapabilities,
    /// The system the mount works in, when it is one this library converts
    /// to. `None` for B1950, J2050 and "other".
    system: Option<Epoch>,
    system_name: String,
    /// The fastest each axis can be moved by hand, in degrees per second.
    max_axis_rate: [Option<f64>; 2],
    name: Option<String>,
    driver: Option<String>,
}

impl AlpacaMount {
    /// The mount `scope` on the server whose backend id is `backend_id`
    /// (`alpaca:<host>:<port>`). Nothing is sent until
    /// [`connect`](Device::connect).
    pub fn new(scope: AlpacaTelescope, backend_id: &DeviceId) -> Self {
        let id = DeviceId::new(format!("{backend_id}:telescope:{}", scope.device_number()));
        Self {
            scope,
            id,
            connected: AtomicBool::new(false),
            profile: RwLock::new(Profile::default()),
        }
    }

    /// The typed Alpaca client, for what the neutral trait does not cover.
    pub fn telescope(&self) -> &AlpacaTelescope {
        &self.scope
    }

    fn profile(&self) -> Profile {
        self.profile
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Ask the mount what it can do.
    async fn read_profile(&self) -> Result<Profile, ScopinatorError> {
        let scope = &self.scope;
        // A property the driver does not implement counts as "cannot".
        let can = |result| optional(result).map(|value: Option<bool>| value.unwrap_or(false));

        let system = optional(scope.equatorial_system().await)?;
        let rates = optional(scope.tracking_rates().await)?.unwrap_or_default();
        let mut max_axis_rate = [None, None];
        for (slot, axis) in [TelescopeAxis::Primary, TelescopeAxis::Secondary]
            .into_iter()
            .enumerate()
        {
            if can(scope.can_move_axis(axis).await)? {
                max_axis_rate[slot] = optional(scope.axis_rates(axis).await)?
                    .unwrap_or_default()
                    .iter()
                    .map(|rate| rate.maximum)
                    .fold(None, |best: Option<f64>, rate| {
                        Some(best.map_or(rate, |b| b.max(rate)))
                    })
                    .filter(|rate| *rate > 0.0);
            }
        }
        let can_slew = can(scope.can_slew_async().await)?;
        let can_set_tracking = can(scope.can_set_tracking().await)?;

        let mut capabilities = MountCapabilities::default();
        capabilities.can_slew = can_slew;
        capabilities.can_sync = can(scope.can_sync().await)?;
        capabilities.can_park = can(scope.can_park().await)?;
        capabilities.can_unpark = can(scope.can_unpark().await)?;
        capabilities.can_track = can_set_tracking;
        capabilities.can_move_axis = max_axis_rate.iter().any(Option::is_some);
        capabilities.can_slew_alt_az = can(scope.can_slew_alt_az_async().await)?;
        capabilities.can_find_home = can(scope.can_find_home().await)?;
        capabilities.can_pulse_guide = can(scope.can_pulse_guide().await)?;
        capabilities.tracking_rates = rates.iter().map(|rate| tracking_rate(*rate)).collect();
        capabilities.can_set_tracking_rate = capabilities.tracking_rates.len() > 1;
        // Alpaca has no "can set site" or "can set clock"; a driver that
        // cannot answers the write with "not implemented".
        capabilities.can_set_site = true;
        capabilities.can_set_time = true;
        let epoch = match system {
            Some(EquatorialSystem::Topocentric) => Some(Epoch::JNow),
            Some(EquatorialSystem::J2000) => Some(Epoch::J2000),
            _ => None,
        };
        capabilities.equatorial_system = epoch.unwrap_or_default();

        Ok(Profile {
            capabilities,
            system: epoch,
            system_name: system.map_or_else(|| "unknown".to_string(), |s| format!("{s:?}")),
            max_axis_rate,
            name: optional(scope.name().await)?,
            driver: optional(scope.driver_info().await)?,
        })
    }

    /// The system the mount works in, or `NotSupported` if it is one this
    /// library cannot convert to.
    fn native_system(&self) -> Result<Epoch, ScopinatorError> {
        let profile = self.profile();
        profile.system.ok_or_else(|| {
            ScopinatorError::NotSupported(format!(
                "a mount that works in the {} equatorial system",
                profile.system_name
            ))
        })
    }

    /// `coords` as the hours and degrees the mount takes.
    fn to_native(&self, coords: &Coordinates) -> Result<(f64, f64), ScopinatorError> {
        let native = coords.to_epoch(self.native_system()?, chrono::Utc::now());
        Ok((native.ra.as_hours(), native.dec.as_degrees()))
    }
}

fn tracking_rate(rate: DriveRate) -> TrackingRate {
    match rate {
        DriveRate::Sidereal => TrackingRate::Sidereal,
        DriveRate::Lunar => TrackingRate::Lunar,
        DriveRate::Solar => TrackingRate::Solar,
        DriveRate::King => TrackingRate::King,
    }
}

fn axis(axis: MountAxis) -> Result<(usize, TelescopeAxis), ScopinatorError> {
    match axis {
        MountAxis::Primary => Ok((0, TelescopeAxis::Primary)),
        MountAxis::Secondary => Ok((1, TelescopeAxis::Secondary)),
        other => Err(ScopinatorError::NotSupported(format!("axis {other:?}"))),
    }
}

#[async_trait]
impl Device for AlpacaMount {
    fn id(&self) -> DeviceId {
        self.id.clone()
    }

    fn info(&self) -> DeviceInfo {
        let profile = self.profile();
        let mut info = DeviceInfo::new(
            self.id.clone(),
            DeviceKind::Mount,
            profile
                .name
                .unwrap_or_else(|| "Alpaca telescope".to_string()),
        );
        info.driver = profile.driver;
        info
    }

    /// Set the device's `Connected` property and read what it can do.
    async fn connect(&self) -> Result<(), ScopinatorError> {
        self.scope.set_connected(true).await.map_err(neutral)?;
        let profile = self.read_profile().await?;
        *self
            .profile
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = profile;
        self.connected.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn disconnect(&self) -> Result<(), ScopinatorError> {
        self.connected.store(false, Ordering::SeqCst);
        self.scope.set_connected(false).await.map_err(neutral)
    }

    /// Whether [`connect`](Device::connect) has succeeded since the last
    /// disconnect. It is not a fresh question to the device.
    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    async fn get_status(&self) -> Result<DeviceStatus, ScopinatorError> {
        let profile = self.profile();
        Ok(DeviceStatus {
            connected: self.scope.connected().await.map_err(neutral)?,
            name: profile.name,
            description: profile.driver,
        })
    }
}

#[async_trait]
impl Mount for AlpacaMount {
    async fn get_coordinates(&self) -> Result<Coordinates, ScopinatorError> {
        let system = self.native_system()?;
        let ra_hours = self.scope.right_ascension().await.map_err(neutral)?;
        let dec_degrees = self.scope.declination().await.map_err(neutral)?;
        // Some drivers report exactly 24 h; RaDegrees folds that to 0.
        let ra = RaDegrees::from_hours(ra_hours.rem_euclid(24.0))
            .map_err(|e| ScopinatorError::Backend(e.to_string()))?;
        let dec =
            DecDegrees::new(dec_degrees).map_err(|e| ScopinatorError::Backend(e.to_string()))?;
        Ok(Coordinates::with_epoch(ra, dec, system))
    }

    async fn slew_to_coordinates(&self, coords: &Coordinates) -> Result<(), ScopinatorError> {
        let (ra_hours, dec_degrees) = self.to_native(coords)?;
        // Ask first: not every driver refuses a slew while parked with the
        // error that says so.
        if optional(self.scope.at_park().await)? == Some(true) {
            return Err(ScopinatorError::Parked);
        }
        self.scope
            .slew_to_coordinates_async(ra_hours, dec_degrees)
            .await
            .map_err(neutral)
    }

    async fn abort_slew(&self) -> Result<(), ScopinatorError> {
        self.scope.abort_slew().await.map_err(neutral)
    }

    async fn is_slewing(&self) -> Result<bool, ScopinatorError> {
        self.scope.slewing().await.map_err(neutral)
    }

    async fn is_parked(&self) -> Result<bool, ScopinatorError> {
        self.scope.at_park().await.map_err(neutral)
    }

    async fn park(&self) -> Result<(), ScopinatorError> {
        self.scope.park().await.map_err(neutral)
    }

    async fn unpark(&self) -> Result<(), ScopinatorError> {
        self.scope.unpark().await.map_err(neutral)
    }

    async fn sync_to_coordinates(&self, coords: &Coordinates) -> Result<(), ScopinatorError> {
        let (ra_hours, dec_degrees) = self.to_native(coords)?;
        self.scope
            .sync_to_coordinates(ra_hours, dec_degrees)
            .await
            .map_err(neutral)
    }

    /// `speed` is a fraction of the fastest rate the axis offers
    /// (`AxisRates`).
    async fn move_axis(&self, mount_axis: MountAxis, speed: f64) -> Result<(), ScopinatorError> {
        if !(-1.0..=1.0).contains(&speed) {
            return Err(ScopinatorError::InvalidArgument(format!(
                "axis speed {speed} is not between -1 and 1"
            )));
        }
        let (slot, alpaca_axis) = axis(mount_axis)?;
        let max = self.profile().max_axis_rate[slot].ok_or_else(|| {
            ScopinatorError::NotSupported(format!("moving the {mount_axis:?} axis"))
        })?;
        self.scope
            .move_axis(alpaca_axis, speed * max)
            .await
            .map_err(neutral)
    }

    async fn stop_motion(&self) -> Result<(), ScopinatorError> {
        let profile = self.profile();
        if profile.max_axis_rate.iter().all(Option::is_none) {
            return Err(ScopinatorError::NotSupported("move_axis".into()));
        }
        for (slot, alpaca_axis) in [TelescopeAxis::Primary, TelescopeAxis::Secondary]
            .into_iter()
            .enumerate()
        {
            if profile.max_axis_rate[slot].is_some() {
                self.scope
                    .move_axis(alpaca_axis, 0.0)
                    .await
                    .map_err(neutral)?;
            }
        }
        Ok(())
    }

    async fn get_alt_az(&self) -> Result<AltAzCoordinates, ScopinatorError> {
        let altitude = self.scope.altitude().await.map_err(neutral)?;
        let azimuth = self.scope.azimuth().await.map_err(neutral)?;
        AltAzCoordinates::new(altitude, azimuth.rem_euclid(360.0))
            .map_err(|e| ScopinatorError::Backend(e.to_string()))
    }

    async fn slew_to_alt_az(&self, target: &AltAzCoordinates) -> Result<(), ScopinatorError> {
        if optional(self.scope.at_park().await)? == Some(true) {
            return Err(ScopinatorError::Parked);
        }
        self.scope
            .slew_to_alt_az_async(target.azimuth, target.altitude)
            .await
            .map_err(neutral)
    }

    async fn set_tracking(&self, enabled: bool) -> Result<(), ScopinatorError> {
        self.scope.set_tracking(enabled).await.map_err(neutral)
    }

    async fn is_tracking(&self) -> Result<bool, ScopinatorError> {
        self.scope.tracking().await.map_err(neutral)
    }

    async fn set_tracking_rate(&self, rate: TrackingRate) -> Result<(), ScopinatorError> {
        let drive_rate = match rate {
            TrackingRate::Sidereal => DriveRate::Sidereal,
            TrackingRate::Lunar => DriveRate::Lunar,
            TrackingRate::Solar => DriveRate::Solar,
            TrackingRate::King => DriveRate::King,
            other => {
                return Err(ScopinatorError::NotSupported(format!(
                    "the {other:?} tracking rate"
                )));
            }
        };
        self.scope
            .set_tracking_rate(drive_rate)
            .await
            .map_err(neutral)
    }

    async fn side_of_pier(&self) -> Result<PierSide, ScopinatorError> {
        Ok(match self.scope.side_of_pier().await.map_err(neutral)? {
            AlpacaPierSide::East => PierSide::East,
            AlpacaPierSide::West => PierSide::West,
            AlpacaPierSide::Unknown => PierSide::Unknown,
        })
    }

    async fn find_home(&self) -> Result<(), ScopinatorError> {
        self.scope.find_home().await.map_err(neutral)
    }

    async fn is_at_home(&self) -> Result<bool, ScopinatorError> {
        self.scope.at_home().await.map_err(neutral)
    }

    async fn pulse_guide(
        &self,
        direction: GuideDirection,
        duration: Duration,
    ) -> Result<(), ScopinatorError> {
        let direction = match direction {
            GuideDirection::North => AlpacaGuide::North,
            GuideDirection::South => AlpacaGuide::South,
            GuideDirection::East => AlpacaGuide::East,
            GuideDirection::West => AlpacaGuide::West,
            other => {
                return Err(ScopinatorError::NotSupported(format!("guiding {other:?}")));
            }
        };
        let milliseconds = u32::try_from(duration.as_millis()).map_err(|_| {
            ScopinatorError::InvalidArgument(format!("a guide pulse of {duration:?}"))
        })?;
        self.scope
            .pulse_guide(direction, milliseconds)
            .await
            .map_err(neutral)
    }

    async fn alignment_mode(&self) -> Result<AlignmentMode, ScopinatorError> {
        Ok(match self.scope.alignment_mode().await.map_err(neutral)? {
            AlpacaAlignment::AltAz => AlignmentMode::AltAz,
            AlpacaAlignment::Polar => AlignmentMode::Polar,
            AlpacaAlignment::GermanPolar => AlignmentMode::GermanPolar,
        })
    }

    async fn get_site(&self) -> Result<SiteLocation, ScopinatorError> {
        let latitude = self.scope.site_latitude().await.map_err(neutral)?;
        let longitude = self.scope.site_longitude().await.map_err(neutral)?;
        let site = SiteLocation::new(latitude, longitude)
            .map_err(|e| ScopinatorError::Backend(format!("the mount reports an {e}")))?;
        Ok(match optional(self.scope.site_elevation().await)? {
            Some(elevation) => site.with_elevation(elevation),
            None => site,
        })
    }

    async fn set_site(&self, site: &SiteLocation) -> Result<(), ScopinatorError> {
        self.scope
            .set_site_latitude(site.latitude_deg())
            .await
            .map_err(neutral)?;
        self.scope
            .set_site_longitude(site.longitude_deg())
            .await
            .map_err(neutral)?;
        if let Some(elevation) = site.elevation_m() {
            self.scope
                .set_site_elevation(elevation)
                .await
                .map_err(neutral)?;
        }
        Ok(())
    }

    async fn get_utc(&self) -> Result<chrono::DateTime<chrono::Utc>, ScopinatorError> {
        self.scope.utc_date().await.map_err(neutral)
    }

    async fn set_utc(&self, time: chrono::DateTime<chrono::Utc>) -> Result<(), ScopinatorError> {
        self.scope.set_utc_date(time).await.map_err(neutral)
    }

    fn capabilities(&self) -> MountCapabilities {
        self.profile().capabilities
    }

    async fn get_mount_status(&self) -> Result<MountStatus, ScopinatorError> {
        let parked = optional(self.scope.at_park().await)?.unwrap_or(false);
        let slewing = self.scope.slewing().await.map_err(neutral)?;
        let tracking = self.scope.tracking().await.map_err(neutral)?;
        Ok(MountStatus {
            slew_state: if parked {
                SlewState::Parked
            } else if slewing {
                SlewState::Slewing
            } else if tracking {
                SlewState::Tracking
            } else {
                SlewState::Idle
            },
            is_tracking: tracking,
            tracking_rate: optional(self.scope.tracking_rate().await)?.map(tracking_rate),
            coordinates: self.get_coordinates().await.ok(),
        })
    }
}
