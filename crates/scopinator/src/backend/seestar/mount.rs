use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use scopinator_seestar::SeestarClient;
use scopinator_seestar::command::Command;
use scopinator_seestar::command::params::Direction;
use scopinator_types::{
    AlignmentMode, AltAzCoordinates, Coordinates, DecDegrees, DeviceId, DeviceInfo, DeviceKind,
    Epoch, MountAxis, RaDegrees, SiteLocation,
};

use crate::device::capabilities::MountCapabilities;
use crate::device::status::{DeviceStatus, MountStatus};
use crate::device::traits::{Device, Mount};
use crate::error::ScopinatorError;

use super::GotoTracker;

/// How long after a goto is accepted the mount is taken to be slewing even
/// though the telescope does not say so yet. An S30 took 6 s to start
/// moving. A goto the telescope accepts and then silently ignores (seen with
/// an unset clock) stops counting as a slew when this runs out.
pub(super) const GOTO_START_GRACE: Duration = Duration::from_secs(15);

/// The equatorial system a Seestar takes goto targets in and reports its
/// position in: the equinox of date, according to the protocol
/// specification. This has not been checked against a telescope; plate-solve
/// centring hides the difference from J2000 (about 0.37 degrees in 2026) on
/// the device itself.
pub(super) const SEESTAR_EPOCH: Epoch = Epoch::JNow;

/// The speed gear manual moves use. Gear 2 at 60 percent moved an S50
/// gently; what the other gears do has not been measured.
const JOG_LEVEL: i32 = 2;

/// How long the telescope keeps moving after one `move_axis` before it stops
/// by itself, in seconds.
const JOG_SECONDS: i32 = 10;

/// Turn a refused command into an error.
pub(super) fn command_ok(
    response: scopinator_seestar::response::CommandResponse,
) -> Result<(), ScopinatorError> {
    response
        .into_result()
        .map_err(|e| scopinator_seestar::SeestarError::CommandFailed {
            code: e.code,
            message: e.message,
        })?;
    Ok(())
}

/// What `pi_get_time` returns: wall-clock fields and the name of the zone
/// they are in.
#[derive(Debug, PartialEq)]
pub(super) struct DeviceClock {
    pub(super) local: chrono::NaiveDateTime,
    /// `None` if the telescope names no zone or one that is not in the IANA
    /// database.
    pub(super) zone: Option<chrono_tz::Tz>,
    zone_name: String,
}

impl DeviceClock {
    pub(super) fn parse(reply: &serde_json::Value) -> Result<Self, ScopinatorError> {
        let bad = || ScopinatorError::Backend(format!("pi_get_time reply is not a time: {reply}"));
        let field = |name: &str| {
            reply
                .get(name)
                .and_then(|v| v.as_i64())
                .and_then(|v| u32::try_from(v).ok())
                .ok_or_else(bad)
        };
        let year = i32::try_from(field("year")?).map_err(|_| bad())?;
        let local = chrono::NaiveDate::from_ymd_opt(year, field("mon")?, field("day")?)
            .and_then(|date| {
                date.and_hms_opt(field("hour").ok()?, field("min").ok()?, field("sec").ok()?)
            })
            .ok_or_else(bad)?;
        let zone_name = reply
            .get("time_zone")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        Ok(Self {
            local,
            zone: zone_name.parse().ok(),
            zone_name,
        })
    }

    /// The instant the clock shows. Fails if the zone is unknown, or if the
    /// local time does not exist or is ambiguous in it (the hour around a
    /// daylight-saving change).
    pub(super) fn utc(&self) -> Result<chrono::DateTime<chrono::Utc>, ScopinatorError> {
        use chrono::TimeZone;
        let zone = self.zone.ok_or_else(|| {
            ScopinatorError::Backend(format!(
                "the telescope's time zone `{}` is not a known zone",
                self.zone_name
            ))
        })?;
        zone.from_local_datetime(&self.local)
            .single()
            .map(|time| time.with_timezone(&chrono::Utc))
            .ok_or_else(|| {
                ScopinatorError::Backend(format!(
                    "{} is not a single instant in {}",
                    self.local, self.zone_name
                ))
            })
    }
}

/// `time` as the wall-clock fields `pi_set_time` takes, in `zone` (UTC if
/// there is none).
pub(super) fn time_in_zone(
    time: chrono::DateTime<chrono::Utc>,
    zone: Option<chrono_tz::Tz>,
) -> scopinator_seestar::command::params::SetTimeParams {
    use chrono::{Datelike, Timelike};
    let zone = zone.unwrap_or(chrono_tz::UTC);
    let local = time.with_timezone(&zone);
    scopinator_seestar::command::params::SetTimeParams {
        year: local.year(),
        mon: local.month() as i32,
        day: local.day() as i32,
        hour: local.hour() as i32,
        min: local.minute() as i32,
        sec: local.second() as i32,
        time_zone: zone.name().to_string(),
    }
}

/// Seestar mount adapter.
pub struct SeestarMount {
    client: Arc<SeestarClient>,
    device_id: DeviceId,
    goto: Arc<GotoTracker>,
}

/// The `mount` section of the device state.
struct MountState {
    /// `move_type`: what is moving the mount (`ScopeGoto`, ...), or `none`.
    moving: bool,
    tracking: bool,
    /// `close` with nothing moving the mount: the arm is folded and at rest,
    /// which is the park position.
    parked: bool,
}

impl SeestarMount {
    pub(crate) fn new(
        client: Arc<SeestarClient>,
        device_id: DeviceId,
        goto: Arc<GotoTracker>,
    ) -> Self {
        Self {
            client,
            device_id,
            goto,
        }
    }

    /// Ask the telescope for the `mount` section of its state. A reply
    /// without one of the members is an error, not "false": a mount that is
    /// not known to be parked must not be reported as unparked.
    async fn mount_state(&self) -> Result<MountState, ScopinatorError> {
        let state = self
            .client
            .send_raw_and_validate(
                "get_device_state",
                Some(serde_json::json!({"keys": ["mount"]})),
            )
            .await?;
        let member = |name: &str| {
            state.pointer(&format!("/mount/{name}")).ok_or_else(|| {
                ScopinatorError::Backend(format!(
                    "get_device_state reply has no `mount.{name}`: {state}"
                ))
            })
        };
        let flag = |name: &str| {
            member(name)?.as_bool().ok_or_else(|| {
                ScopinatorError::Backend(format!("`mount.{name}` is not a boolean: {state}"))
            })
        };
        let moving = member("move_type")?.as_str() != Some("none");
        Ok(MountState {
            moving,
            tracking: flag("tracking")?,
            // The arm reads as closed from the start of a park, while the
            // mount is still on its way home (`move_type: ScopeHome`), and
            // for the first seconds of a goto from the park position.
            parked: flag("close")? && !moving,
        })
    }

    /// Where the goto stands according to the view state, which nests its
    /// stages by name: `View` > `AutoGoto` > `ScopeGoto`.
    async fn goto_stage(&self) -> Result<GotoStage, ScopinatorError> {
        let view = self.client.send_and_validate(Command::GetViewState).await?;
        let Some(auto_goto) = view.pointer("/View/AutoGoto") else {
            return Ok(GotoStage::None);
        };
        if let Some(failure) = goto_failure(auto_goto) {
            return Ok(GotoStage::Failed(failure));
        }
        Ok(match auto_goto.get("state").and_then(|s| s.as_str()) {
            Some("start" | "working") => GotoStage::Running,
            _ => GotoStage::None,
        })
    }
}

enum GotoStage {
    /// No goto in the view state, or one that has ended without failing.
    None,
    /// Slewing, plate solving or re-centring.
    Running,
    Failed(String),
}

/// Why the goto under `View` > `AutoGoto` has failed, if it has.
///
/// The goto has failed when `AutoGoto` itself says so, or when the mount
/// could not be moved (`ScopeGoto` failed under an `AutoGoto` still marked
/// `working`, seen on firmware 9.75 with code 501). A failed plate solve or
/// autofocus under a working `AutoGoto` is not a failure of the goto: the
/// telescope takes a longer exposure and tries again, several times.
pub(super) fn goto_failure(auto_goto: &serde_json::Value) -> Option<String> {
    let failed =
        |value: &serde_json::Value| value.get("state").and_then(|s| s.as_str()) == Some("fail");
    if failed(auto_goto) {
        return find_failure(auto_goto, "AutoGoto");
    }
    let scope_goto = auto_goto.get("ScopeGoto")?;
    if failed(scope_goto) {
        return find_failure(scope_goto, "ScopeGoto");
    }
    None
}

/// The first stage at or below `value` that reports `state: fail`, as
/// "stage: error (code n)".
pub(super) fn find_failure(value: &serde_json::Value, name: &str) -> Option<String> {
    let object = value.as_object()?;
    if object.get("state").and_then(|s| s.as_str()) == Some("fail") {
        let error = object
            .get("error")
            .and_then(|e| e.as_str())
            .unwrap_or("no reason given");
        return Some(match object.get("code").and_then(|c| c.as_i64()) {
            Some(code) => format!("{name}: {error} (code {code})"),
            None => format!("{name}: {error}"),
        });
    }
    object
        .iter()
        .find_map(|(key, child)| find_failure(child, key))
}

#[async_trait]
impl Device for SeestarMount {
    fn id(&self) -> DeviceId {
        self.device_id.clone()
    }

    fn info(&self) -> DeviceInfo {
        let summary = self.client.device_info();
        let mut info = DeviceInfo::new(self.device_id.clone(), DeviceKind::Mount, "Seestar mount");
        info.model = summary.model;
        info.firmware = summary.firmware_string;
        info
    }

    async fn connect(&self) -> Result<(), ScopinatorError> {
        // Connection is managed by the backend
        Ok(())
    }

    async fn disconnect(&self) -> Result<(), ScopinatorError> {
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.client.is_control_connected()
    }

    async fn get_status(&self) -> Result<DeviceStatus, ScopinatorError> {
        Ok(DeviceStatus {
            connected: self.is_connected(),
            name: Some(self.device_id.to_string()),
            description: Some("Seestar mount".into()),
        })
    }
}

#[async_trait]
impl Mount for SeestarMount {
    async fn get_coordinates(&self) -> Result<Coordinates, ScopinatorError> {
        let response = self
            .client
            .send_and_validate(Command::ScopeGetEquCoord)
            .await?;

        // Response result contains ra (hours) and dec (degrees). A reply
        // without them is an error: (0, 0) is a real position, not "unknown".
        let coord = |name: &str| {
            response.get(name).and_then(|v| v.as_f64()).ok_or_else(|| {
                ScopinatorError::Backend(format!(
                    "scope_get_equ_coord reply has no numeric `{name}`: {response}"
                ))
            })
        };
        let ra_hours = coord("ra")?;
        let dec_deg = coord("dec")?;

        let ra =
            RaDegrees::from_hours(ra_hours).map_err(|e| ScopinatorError::Backend(e.to_string()))?;
        let dec = DecDegrees::new(dec_deg).map_err(|e| ScopinatorError::Backend(e.to_string()))?;

        // The telescope reports the equinox of date, as it takes it.
        Ok(Coordinates::with_epoch(ra, dec, SEESTAR_EPOCH))
    }

    async fn slew_to_coordinates(&self, coords: &Coordinates) -> Result<(), ScopinatorError> {
        // Name the target after what was asked for; send what the
        // telescope understands.
        let name = format!(
            "RA {:.4} Dec {:.4}",
            coords.ra.as_hours(),
            coords.dec.as_degrees()
        );
        let of_date = coords.to_epoch(SEESTAR_EPOCH, chrono::Utc::now());
        let (ra_hours, dec_deg) = (of_date.ra.as_hours(), of_date.dec.as_degrees());
        self.client
            .goto(&name, ra_hours, dec_deg)
            .await
            .map_err(ScopinatorError::from)
            .and_then(command_ok)?;
        self.goto.accepted();
        Ok(())
    }

    async fn abort_slew(&self) -> Result<(), ScopinatorError> {
        use scopinator_seestar::command::params::{StopStage, StopViewParams};
        // Cancel the goto. On its own this stops the slew but leaves the
        // scope in star view taking exposures (firmware 6.70 and 9.75).
        let cancel_goto = Command::IscopeStopView(Some(StopViewParams {
            stage: StopStage::AutoGoto,
        }));
        self.client.send_and_validate(cancel_goto).await?;
        // Then leave the view, so that abort returns the scope to idle.
        self.client
            .send_and_validate(Command::IscopeStopView(None))
            .await?;
        // The cancelled goto shows as failed in the view state; it was
        // asked for, so it is not reported as a failed slew.
        self.goto.clear();
        Ok(())
    }

    async fn is_slewing(&self) -> Result<bool, ScopinatorError> {
        if let Some(failure) = self.goto.take_failure() {
            return Err(ScopinatorError::Backend(format!(
                "goto failed at {failure}"
            )));
        }
        match self.goto_stage().await? {
            GotoStage::Running => {
                self.goto.running();
                return Ok(true);
            }
            GotoStage::Failed(failure) => {
                // Report the failure of a goto sent through this backend
                // once; the stage stays in the view state after that.
                if self.goto.is_pending() {
                    self.goto.clear();
                    return Err(ScopinatorError::Backend(format!(
                        "goto failed at {failure}"
                    )));
                }
            }
            GotoStage::None => {}
        }
        if self.mount_state().await?.moving {
            self.goto.running();
            return Ok(true);
        }
        // Accepted, and the telescope has not started on it yet. A goto
        // stage that is already over belongs to an earlier goto.
        if self.goto.awaiting_start(GOTO_START_GRACE) {
            return Ok(true);
        }
        self.goto.finished();
        Ok(false)
    }

    async fn is_parked(&self) -> Result<bool, ScopinatorError> {
        Ok(self.mount_state().await?.parked)
    }

    async fn park(&self) -> Result<(), ScopinatorError> {
        self.client.send_and_validate(Command::ScopePark).await?;
        Ok(())
    }

    /// `scope_sync` with the position in the telescope's own system. That
    /// it takes right ascension in hours and declination in degrees, like a
    /// goto, has not been checked against a telescope.
    async fn sync_to_coordinates(&self, coords: &Coordinates) -> Result<(), ScopinatorError> {
        let of_date = coords.to_epoch(SEESTAR_EPOCH, chrono::Utc::now());
        self.client
            .send_and_validate(Command::ScopeSync(
                of_date.ra.as_hours(),
                of_date.dec.as_degrees(),
            ))
            .await?;
        Ok(())
    }

    /// A manual move with `scope_speed_move`.
    ///
    /// The telescope moves in one direction at a time, so moving a second
    /// axis replaces the motion of the first, and it stops by itself after
    /// ten seconds. The directions are those found on an S50 in
    /// equatorial mode (east increases right ascension, north increases
    /// declination); in alt-az mode they have not been checked.
    async fn move_axis(&self, axis: MountAxis, speed: f64) -> Result<(), ScopinatorError> {
        if !(-1.0..=1.0).contains(&speed) {
            return Err(ScopinatorError::InvalidArgument(format!(
                "axis speed {speed} is not between -1 and 1"
            )));
        }
        let percent = (speed.abs() * 100.0).round() as i32;
        if percent == 0 {
            return self.stop_motion().await;
        }
        let direction = match (axis, speed > 0.0) {
            (MountAxis::Primary, true) => Direction::East,
            (MountAxis::Primary, false) => Direction::West,
            (MountAxis::Secondary, true) => Direction::North,
            (MountAxis::Secondary, false) => Direction::South,
            (other, _) => {
                return Err(ScopinatorError::NotSupported(format!("axis {other:?}")));
            }
        };
        command_ok(
            self.client
                .jog(direction, JOG_LEVEL, percent, JOG_SECONDS)
                .await?,
        )
    }

    async fn stop_motion(&self) -> Result<(), ScopinatorError> {
        command_ok(self.client.stop_jog().await?)
    }

    /// `scope_get_horiz_coord`, which answers with two numbers. They are
    /// taken as altitude then azimuth: the one reply on record is
    /// `[-29.708611, 0.0]`, and an azimuth is not negative.
    async fn get_alt_az(&self) -> Result<AltAzCoordinates, ScopinatorError> {
        let reply = self
            .client
            .send_and_validate(Command::ScopeGetHorizCoord)
            .await?;
        let degrees = |index: usize| {
            reply.get(index).and_then(|v| v.as_f64()).ok_or_else(|| {
                ScopinatorError::Backend(format!(
                    "scope_get_horiz_coord reply is not two numbers: {reply}"
                ))
            })
        };
        AltAzCoordinates::new(degrees(0)?, degrees(1)?.rem_euclid(360.0))
            .map_err(|e| ScopinatorError::Backend(format!("the telescope reports an {e}")))
    }

    /// `mount.equ_mode`: a Seestar is an alt-az mount unless it has been put
    /// in equatorial mode on a wedge. It never flips at the meridian.
    async fn alignment_mode(&self) -> Result<AlignmentMode, ScopinatorError> {
        let state = self
            .client
            .send_raw_and_validate(
                "get_device_state",
                Some(serde_json::json!({"keys": ["mount"]})),
            )
            .await?;
        match state.pointer("/mount/equ_mode").and_then(|v| v.as_bool()) {
            Some(true) => Ok(AlignmentMode::Polar),
            Some(false) => Ok(AlignmentMode::AltAz),
            None => Err(ScopinatorError::Backend(format!(
                "get_device_state reply has no `mount.equ_mode`: {state}"
            ))),
        }
    }

    /// `location_lon_lat` from the device state. The telescope does not
    /// keep an elevation.
    async fn get_site(&self) -> Result<SiteLocation, ScopinatorError> {
        let state = self
            .client
            .send_raw_and_validate(
                "get_device_state",
                Some(serde_json::json!({"keys": ["location_lon_lat"]})),
            )
            .await?;
        let degrees = |index: usize| {
            state
                .pointer(&format!("/location_lon_lat/{index}"))
                .and_then(|v| v.as_f64())
                .ok_or_else(|| {
                    ScopinatorError::Backend(format!(
                        "get_device_state reply has no `location_lon_lat`: {state}"
                    ))
                })
        };
        // Longitude first.
        SiteLocation::new(degrees(1)?, degrees(0)?)
            .map_err(|e| ScopinatorError::Backend(format!("the telescope reports an {e}")))
    }

    /// `set_user_location`. That the telescope counts longitude east
    /// positive has not been checked against one.
    async fn set_site(&self, site: &SiteLocation) -> Result<(), ScopinatorError> {
        use scopinator_seestar::command::params::SetUserLocationParams;
        self.client
            .send_and_validate(Command::SetUserLocation(SetUserLocationParams {
                lat: site.latitude_deg(),
                lon: site.longitude_deg(),
                force: true,
            }))
            .await?;
        Ok(())
    }

    async fn get_utc(&self) -> Result<chrono::DateTime<chrono::Utc>, ScopinatorError> {
        let reply = self.client.send_and_validate(Command::PiGetTime).await?;
        DeviceClock::parse(&reply)?.utc()
    }

    /// Set the clock, keeping the telescope's time zone: the time is sent as
    /// local time in the zone the telescope already has, because sending a
    /// zone name replaces it. A telescope with no zone, or one this library
    /// does not know, is set to UTC.
    async fn set_utc(&self, time: chrono::DateTime<chrono::Utc>) -> Result<(), ScopinatorError> {
        let zone = match self.client.send_and_validate(Command::PiGetTime).await {
            Ok(reply) => DeviceClock::parse(&reply).ok().and_then(|clock| clock.zone),
            Err(_) => None,
        };
        self.client
            .send_and_validate(Command::PiSetTime(time_in_zone(time, zone)))
            .await?;
        Ok(())
    }

    async fn set_tracking(&self, enabled: bool) -> Result<(), ScopinatorError> {
        self.client
            .send_and_validate(Command::ScopeSetTrackState(enabled))
            .await?;
        Ok(())
    }

    async fn is_tracking(&self) -> Result<bool, ScopinatorError> {
        Ok(self.mount_state().await?.tracking)
    }

    fn capabilities(&self) -> MountCapabilities {
        MountCapabilities {
            can_slew: true,
            can_sync: true,
            can_park: true,
            // A Seestar opens its arm when it is sent a goto; it has no
            // separate unpark request.
            can_unpark: false,
            can_track: true,
            can_move_axis: true,
            can_set_site: true,
            can_set_time: true,
            equatorial_system: SEESTAR_EPOCH,
            ..MountCapabilities::default()
        }
    }

    async fn get_mount_status(&self) -> Result<MountStatus, ScopinatorError> {
        use scopinator_types::SlewState;

        let coords = self.get_coordinates().await.ok();
        let state = self.mount_state().await?;
        // A failed goto is reported by `is_slewing`; here it only means the
        // mount is not slewing.
        let slewing = state.moving || matches!(self.goto_stage().await, Ok(GotoStage::Running));
        Ok(MountStatus {
            slew_state: if slewing {
                SlewState::Slewing
            } else if state.parked {
                SlewState::Parked
            } else if state.tracking {
                SlewState::Tracking
            } else {
                SlewState::Idle
            },
            tracking_rate: None,
            coordinates: coords,
            is_tracking: state.tracking,
        })
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;

    use super::*;

    #[test]
    fn a_clock_in_a_named_zone_reads_as_the_right_instant() {
        // 11:18:54 in Chicago on 15 November is CST, six hours behind UTC.
        let reply = json!({"year": 2026, "mon": 11, "day": 15, "hour": 11, "min": 18, "sec": 54,
            "time_zone": "America/Chicago"});
        let clock = DeviceClock::parse(&reply).unwrap();
        assert_eq!(
            clock.utc().unwrap(),
            chrono::Utc
                .with_ymd_and_hms(2026, 11, 15, 17, 18, 54)
                .unwrap()
        );
        // In July it is CDT, five hours behind.
        let reply = json!({"year": 2026, "mon": 7, "day": 1, "hour": 22, "min": 0, "sec": 0,
            "time_zone": "America/Chicago"});
        assert_eq!(
            DeviceClock::parse(&reply).unwrap().utc().unwrap(),
            chrono::Utc.with_ymd_and_hms(2026, 7, 2, 3, 0, 0).unwrap()
        );
    }

    #[test]
    fn a_clock_in_utc_reads_as_itself() {
        // As captured from an S30.
        let reply = json!({"year": 2026, "mon": 11, "day": 15, "hour": 11, "min": 18, "sec": 54,
            "time_zone": "UTC"});
        assert_eq!(
            DeviceClock::parse(&reply).unwrap().utc().unwrap(),
            chrono::Utc
                .with_ymd_and_hms(2026, 11, 15, 11, 18, 54)
                .unwrap()
        );
    }

    #[test]
    fn an_unknown_or_missing_zone_parses_but_has_no_instant() {
        for zone in [json!("Mars/Olympus"), json!(""), serde_json::Value::Null] {
            let reply = json!({"year": 2026, "mon": 11, "day": 15, "hour": 11, "min": 18,
                "sec": 54, "time_zone": zone});
            let clock = DeviceClock::parse(&reply).unwrap();
            assert_eq!(clock.zone, None);
            assert!(clock.utc().is_err());
        }
    }

    #[test]
    fn a_reply_that_is_not_a_date_is_an_error() {
        assert!(DeviceClock::parse(&json!({})).is_err());
        let reply = json!({"year": 2026, "mon": 13, "day": 1, "hour": 0, "min": 0, "sec": 0,
            "time_zone": "UTC"});
        assert!(DeviceClock::parse(&reply).is_err());
    }

    #[test]
    fn a_time_is_sent_as_local_time_in_the_telescope_zone() {
        let time = chrono::Utc
            .with_ymd_and_hms(2026, 11, 15, 17, 18, 54)
            .unwrap();
        let sent = time_in_zone(time, Some(chrono_tz::America::Chicago));
        assert_eq!(
            (sent.year, sent.mon, sent.day, sent.hour, sent.min, sent.sec),
            (2026, 11, 15, 11, 18, 54)
        );
        assert_eq!(sent.time_zone, "America/Chicago");

        // Without a zone, UTC.
        let sent = time_in_zone(time, None);
        assert_eq!((sent.day, sent.hour), (15, 17));
        assert_eq!(sent.time_zone, "UTC");
    }

    #[test]
    fn the_local_date_can_differ_from_the_utc_date() {
        // 03:00 UTC on the 16th is still the 15th in Chicago.
        let time = chrono::Utc.with_ymd_and_hms(2026, 11, 16, 3, 0, 0).unwrap();
        let sent = time_in_zone(time, Some(chrono_tz::America::Chicago));
        assert_eq!((sent.day, sent.hour), (15, 21));
    }
}
