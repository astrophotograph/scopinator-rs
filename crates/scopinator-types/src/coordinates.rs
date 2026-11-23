use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

use crate::error::TypeError;

/// Right ascension in degrees (0.0..360.0).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct RaDegrees(f64);

impl RaDegrees {
    pub fn new(degrees: f64) -> Result<Self, TypeError> {
        if !(0.0..360.0).contains(&degrees) && (degrees - 360.0).abs() > f64::EPSILON {
            return Err(TypeError::InvalidRa(degrees));
        }
        // Normalize 360.0 to 0.0
        let normalized = if (degrees - 360.0).abs() < f64::EPSILON {
            0.0
        } else {
            degrees
        };
        Ok(Self(normalized))
    }

    /// Create from hours (0.0..24.0), as used by the Seestar protocol.
    pub fn from_hours(hours: f64) -> Result<Self, TypeError> {
        if !(0.0..24.0).contains(&hours) && (hours - 24.0).abs() > f64::EPSILON {
            return Err(TypeError::InvalidRaHours(hours));
        }
        Self::new(hours * 15.0)
    }

    pub fn as_degrees(&self) -> f64 {
        self.0
    }

    pub fn as_hours(&self) -> f64 {
        self.0 / 15.0
    }
}

impl fmt::Display for RaDegrees {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hours = self.as_hours();
        let h = hours.floor() as u32;
        let remainder = (hours - h as f64) * 60.0;
        let m = remainder.floor() as u32;
        let s = (remainder - m as f64) * 60.0;
        write!(f, "{h}h {m}m {s:.1}s")
    }
}

/// Declination in degrees (-90.0..=90.0).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct DecDegrees(f64);

impl DecDegrees {
    pub fn new(degrees: f64) -> Result<Self, TypeError> {
        if !(-90.0..=90.0).contains(&degrees) {
            return Err(TypeError::InvalidDec(degrees));
        }
        Ok(Self(degrees))
    }

    pub fn as_degrees(&self) -> f64 {
        self.0
    }
}

impl fmt::Display for DecDegrees {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0.0 { "-" } else { "+" };
        let abs = self.0.abs();
        let d = abs.floor() as u32;
        let remainder = (abs - d as f64) * 60.0;
        let m = remainder.floor() as u32;
        let s = (remainder - m as f64) * 60.0;
        write!(f, "{sign}{d}d {m}' {s:.1}\"")
    }
}

/// Coordinate epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[non_exhaustive]
pub enum Epoch {
    #[default]
    J2000,
    JNow,
}

/// Equatorial coordinates (RA/Dec).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Coordinates {
    pub ra: RaDegrees,
    pub dec: DecDegrees,
    pub epoch: Epoch,
}

impl Coordinates {
    pub fn new(ra: RaDegrees, dec: DecDegrees) -> Self {
        Self {
            ra,
            dec,
            epoch: Epoch::J2000,
        }
    }

    pub fn with_epoch(ra: RaDegrees, dec: DecDegrees, epoch: Epoch) -> Self {
        Self { ra, dec, epoch }
    }

    /// Create from RA in hours and Dec in degrees (Seestar's native format).
    pub fn from_hours(ra_hours: f64, dec_degrees: f64) -> Result<Self, TypeError> {
        Ok(Self {
            ra: RaDegrees::from_hours(ra_hours)?,
            dec: DecDegrees::new(dec_degrees)?,
            epoch: Epoch::J2000,
        })
    }

    /// The same direction on the sky expressed in another equatorial system.
    ///
    /// `at` is the date that [`Epoch::JNow`] refers to (the equinox of
    /// date); it is used whenever either system is `JNow`. Converting to the
    /// system the coordinates are already in returns them unchanged.
    ///
    /// Mounts differ in which system they take and report: a catalogue
    /// position is J2000, while many mounts work in the equinox of date.
    /// The two are about 0.37 degrees apart in 2026 and drift further every
    /// year.
    ///
    /// This applies precession only (the IAU 1976 rotation, Meeus,
    /// *Astronomical Algorithms*, chapter 21). Nutation and aberration are
    /// left out, so the result can differ from an apparent place by up to
    /// about 40 arcseconds: fine for pointing a telescope, not for
    /// astrometry.
    pub fn to_epoch(self, target: Epoch, at: DateTime<Utc>) -> Self {
        let to_date = match (self.epoch, target) {
            (Epoch::J2000, Epoch::JNow) => true,
            (Epoch::JNow, Epoch::J2000) => false,
            _ => return self,
        };
        let (ra, dec) = precess(
            self.ra.as_degrees().to_radians(),
            self.dec.as_degrees().to_radians(),
            julian_centuries_since_j2000(at),
            to_date,
        );
        // rem_euclid can round up to exactly 360 for a tiny negative angle.
        let ra = ra.to_degrees().rem_euclid(360.0);
        Self {
            ra: RaDegrees(if ra >= 360.0 { 0.0 } else { ra }),
            dec: DecDegrees(dec.to_degrees().clamp(-90.0, 90.0)),
            epoch: target,
        }
    }
}

/// Julian centuries from J2000.0 (JD 2451545.0) to `at`. UTC is used where
/// the theory wants Terrestrial Time; the 69 seconds between them move the
/// result by less than a milliarcsecond.
fn julian_centuries_since_j2000(at: DateTime<Utc>) -> f64 {
    const UNIX_EPOCH_JD: f64 = 2_440_587.5;
    const J2000_JD: f64 = 2_451_545.0;
    let seconds = at.timestamp() as f64 + f64::from(at.timestamp_subsec_nanos()) * 1e-9;
    (seconds / 86_400.0 + UNIX_EPOCH_JD - J2000_JD) / 36_525.0
}

/// Precess (ra, dec), in radians, between the mean equinox of J2000.0 and
/// the mean equinox `t` Julian centuries after it. `to_date` goes from J2000
/// to the date; otherwise from the date back to J2000.
///
/// The precession is the rotation `Rz(-z) * Ry(theta) * Rz(-zeta)` (Meeus
/// equations 21.2 to 21.4 with J2000.0 as the starting epoch). It is applied
/// to the unit vector, and the declination is taken with `atan2`, so that
/// positions near a pole keep their accuracy. The reverse direction applies
/// the inverse rotation rather than the same formulae with `-t`, which makes
/// a round trip exact to rounding.
fn precess(ra: f64, dec: f64, t: f64, to_date: bool) -> (f64, f64) {
    let arcsec = |a: f64| (a / 3600.0).to_radians();
    let zeta = arcsec(2306.2181 * t + 0.30188 * t * t + 0.017998 * t * t * t);
    let z = arcsec(2306.2181 * t + 1.09468 * t * t + 0.018203 * t * t * t);
    let theta = arcsec(2004.3109 * t - 0.42665 * t * t - 0.041833 * t * t * t);

    // Rotate about the pole by `first`, tilt the pole by `tilt` (in the
    // x-z plane), then rotate about the new pole by `last`.
    let (first, tilt, last) = if to_date {
        (zeta, theta, z)
    } else {
        (-z, -theta, -zeta)
    };
    let a = ra + first;
    let (x, y, zc) = (dec.cos() * a.cos(), dec.cos() * a.sin(), dec.sin());
    let x2 = tilt.cos() * x - tilt.sin() * zc;
    let z2 = tilt.sin() * x + tilt.cos() * zc;
    (y.atan2(x2) + last, z2.atan2(x2.hypot(y)))
}

impl fmt::Display for Coordinates {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RA {} Dec {} ({:?})", self.ra, self.dec, self.epoch)
    }
}

/// Altitude/Azimuth coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AltAzCoordinates {
    /// Altitude in degrees (-90.0..=90.0).
    pub altitude: f64,
    /// Azimuth in degrees (0.0..360.0).
    pub azimuth: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ra_from_hours() {
        let ra = RaDegrees::from_hours(6.0).unwrap();
        assert!((ra.as_degrees() - 90.0).abs() < f64::EPSILON);
        assert!((ra.as_hours() - 6.0).abs() < f64::EPSILON);
    }

    #[test]
    fn ra_bounds() {
        assert!(RaDegrees::new(-1.0).is_err());
        assert!(RaDegrees::new(361.0).is_err());
        assert!(RaDegrees::new(0.0).is_ok());
        assert!(RaDegrees::new(359.9).is_ok());
        // 360.0 normalizes to 0.0
        let ra = RaDegrees::new(360.0).unwrap();
        assert!((ra.as_degrees()).abs() < f64::EPSILON);
    }

    #[test]
    fn dec_bounds() {
        assert!(DecDegrees::new(-91.0).is_err());
        assert!(DecDegrees::new(91.0).is_err());
        assert!(DecDegrees::new(-90.0).is_ok());
        assert!(DecDegrees::new(90.0).is_ok());
    }

    #[test]
    fn coordinates_from_hours() {
        let coords = Coordinates::from_hours(12.0, 45.0).unwrap();
        assert!((coords.ra.as_degrees() - 180.0).abs() < f64::EPSILON);
        assert!((coords.dec.as_degrees() - 45.0).abs() < f64::EPSILON);
    }

    #[test]
    fn ra_display() {
        let ra = RaDegrees::from_hours(5.5).unwrap();
        let s = format!("{ra}");
        assert!(s.starts_with("5h 30m"));
    }

    #[test]
    fn dec_display() {
        let dec = DecDegrees::new(-22.5).unwrap();
        let s = format!("{dec}");
        assert!(s.starts_with("-22d 30'"));
    }

    // ---- Differential tests mirroring pyscopinator
    // tests/v2/test_core_types.py (Forge task: "Add differential coordinate
    // tests mirroring core types oracle"). Python takes RA in degrees directly
    // (`Coordinates(ra=..., dec=...)`); Rust splits it into RaDegrees/DecDegrees.

    // Python: TestCoordinates::test_create_coordinates
    #[test]
    fn py_test_create_coordinates() {
        let c = Coordinates::new(
            RaDegrees::new(10.5).unwrap(),
            DecDegrees::new(45.0).unwrap(),
        );
        assert_eq!(c.ra.as_degrees(), 10.5);
        assert_eq!(c.dec.as_degrees(), 45.0);
        assert_eq!(c.epoch, Epoch::J2000);
    }

    // Python: TestCoordinates::test_ra_validation
    #[test]
    fn py_test_ra_validation() {
        assert_eq!(RaDegrees::new(0.0).unwrap().as_degrees(), 0.0);
        assert_eq!(RaDegrees::new(359.99).unwrap().as_degrees(), 359.99);
        assert!(RaDegrees::new(-1.0).is_err());
        // Python rejects ra=360.0 here (pytest.raises(ValueError)); Rust
        // accepts it, see py_divergence_ra_360_normalizes_to_zero.
    }

    // Python: TestCoordinates::test_ra_validation (the `ra=360.0` branch)
    // KNOWN DIVERGENCE (pinned, not fixed): Python `Field(ge=0, lt=360)`
    // rejects 360.0; Rust normalizes 360.0 -> 0.0 and 24h -> 0.0.
    // See docs/bugs/ra-360-divergence.md.
    #[test]
    fn py_divergence_ra_360_normalizes_to_zero() {
        let ra = RaDegrees::new(360.0).expect("Rust accepts 360.0 (Python rejects)");
        assert_eq!(ra.as_degrees(), 0.0);

        let ra = RaDegrees::from_hours(24.0)
            .expect("Rust accepts 24h (Python has no 24h; 360deg rejected)");
        assert_eq!(ra.as_degrees(), 0.0);

        let c = Coordinates::from_hours(24.0, 0.0).expect("Rust accepts 24h");
        assert_eq!(c.ra.as_degrees(), 0.0);

        // Still rejected beyond the tolerance band.
        assert!(RaDegrees::new(360.1).is_err());
        assert!(RaDegrees::from_hours(24.1).is_err());
        assert!(RaDegrees::from_hours(-0.1).is_err());
    }

    // Python: TestCoordinates::test_dec_validation
    #[test]
    fn py_test_dec_validation() {
        assert_eq!(DecDegrees::new(-90.0).unwrap().as_degrees(), -90.0);
        assert_eq!(DecDegrees::new(90.0).unwrap().as_degrees(), 90.0);
        assert!(DecDegrees::new(-91.0).is_err());
        assert!(DecDegrees::new(91.0).is_err());
        // Coordinates::from_hours validates Dec as well.
        assert!(Coordinates::from_hours(0.0, 91.0).is_err());
        assert!(Coordinates::from_hours(0.0, -91.0).is_err());
    }

    // Python: TestCoordinates::test_from_hours
    #[test]
    fn py_test_from_hours() {
        let c = Coordinates::from_hours(6.0, 45.0).unwrap();
        assert_eq!(c.ra.as_degrees(), 90.0); // 6 h * 15
        assert_eq!(c.dec.as_degrees(), 45.0);
        assert_eq!(c.epoch, Epoch::J2000);
    }

    // Python: TestCoordinates::test_ra_hours_property
    #[test]
    fn py_test_ra_hours_property() {
        let c = Coordinates::new(
            RaDegrees::new(90.0).unwrap(),
            DecDegrees::new(45.0).unwrap(),
        );
        assert_eq!(c.ra.as_hours(), 6.0);
    }

    // Python: TestCoordinates::test_str_representation
    // (Rust formats "RA 6h 0m 0.0s Dec +45d 0' 0.0\" (J2000)"; Python only
    // checks for the substrings "RA" and "Dec".)
    #[test]
    fn py_test_str_representation() {
        let c = Coordinates::new(
            RaDegrees::new(90.0).unwrap(),
            DecDegrees::new(45.0).unwrap(),
        );
        let s = c.to_string();
        assert!(s.contains("RA"));
        assert!(s.contains("Dec"));
    }

    // Python: TestAltAzCoordinates::test_create_altaz
    #[test]
    fn py_test_create_altaz() {
        let c = AltAzCoordinates {
            altitude: 45.0,
            azimuth: 180.0,
        };
        assert_eq!(c.altitude, 45.0);
        assert_eq!(c.azimuth, 180.0);
    }

    // Python: TestAltAzCoordinates::test_altitude_validation and
    // ::test_azimuth_validation.
    // KNOWN DIVERGENCE (pinned, not fixed): Python rejects altitude outside
    // -90..=90 and azimuth outside 0..360 (ValueError). Rust's
    // AltAzCoordinates has public fields and no validating constructor, so
    // every value is accepted. Documented in docs/bugs/ra-360-divergence.md
    // (secondary section).
    #[test]
    fn py_divergence_altaz_has_no_validation() {
        for (alt, az) in [(-91.0, 0.0), (91.0, 0.0), (0.0, -1.0), (0.0, 360.0)] {
            let c = AltAzCoordinates {
                altitude: alt,
                azimuth: az,
            };
            assert_eq!(c.altitude, alt);
            assert_eq!(c.azimuth, az);
        }
        // In-range values from the Python test.
        let c = AltAzCoordinates {
            altitude: 0.0,
            azimuth: 359.99,
        };
        assert_eq!(c.azimuth, 359.99);
    }

    // --- Precession ---

    use chrono::TimeZone;

    /// Angle between two positions, in arcseconds.
    fn separation_arcsec(a: Coordinates, b: Coordinates) -> f64 {
        let (ra1, dec1) = (
            a.ra.as_degrees().to_radians(),
            a.dec.as_degrees().to_radians(),
        );
        let (ra2, dec2) = (
            b.ra.as_degrees().to_radians(),
            b.dec.as_degrees().to_radians(),
        );
        // Haversine form: accurate for small separations.
        let h = ((dec1 - dec2) / 2.0).sin().powi(2)
            + dec1.cos() * dec2.cos() * ((ra1 - ra2) / 2.0).sin().powi(2);
        (2.0 * h.sqrt().asin()).to_degrees() * 3600.0
    }

    fn at_jd(jd: f64) -> DateTime<Utc> {
        let seconds = (jd - 2_440_587.5) * 86_400.0;
        Utc.timestamp_opt(seconds.floor() as i64, 0).unwrap()
    }

    /// Meeus, *Astronomical Algorithms* (2nd ed.), example 21.b: theta
    /// Persei. After proper motion the J2000.0 mean place is
    /// 41.054063, +49.227750; precessed to 2028 November 13.19 TT
    /// (JDE 2462088.69) it is 41.547214, +49.348483.
    #[test]
    fn precession_matches_meeus_example_21b() {
        let j2000 = Coordinates::new(
            RaDegrees::new(41.054063).unwrap(),
            DecDegrees::new(49.227750).unwrap(),
        );
        let of_date = j2000.to_epoch(Epoch::JNow, at_jd(2_462_088.69));
        assert_eq!(of_date.epoch, Epoch::JNow);
        assert!(
            (of_date.ra.as_degrees() - 41.547214).abs() < 1.0 / 3600.0,
            "ra {}",
            of_date.ra.as_degrees()
        );
        assert!(
            (of_date.dec.as_degrees() - 49.348483).abs() < 1.0 / 3600.0,
            "dec {}",
            of_date.dec.as_degrees()
        );
    }

    /// At the vernal equinox point precession is +m in right ascension and
    /// +n in declination: about 3.075 s and 20.04 arcsec per year. Over the
    /// 26.5 years to mid-2026 that is +0.34 and +0.148 degrees. A sign error
    /// in either angle fails this.
    #[test]
    fn precession_direction_at_the_equinox_point() {
        let origin = Coordinates::new(RaDegrees::new(0.0).unwrap(), DecDegrees::new(0.0).unwrap());
        let at = Utc.with_ymd_and_hms(2026, 7, 2, 0, 0, 0).unwrap();
        let of_date = origin.to_epoch(Epoch::JNow, at);
        let ra = of_date.ra.as_degrees();
        let dec = of_date.dec.as_degrees();
        assert!((0.33..0.35).contains(&ra), "ra {ra}");
        assert!((0.14..0.155).contains(&dec), "dec {dec}");
    }

    #[test]
    fn precession_is_the_identity_at_j2000() {
        let at = Utc.with_ymd_and_hms(2000, 1, 1, 12, 0, 0).unwrap();
        let c = Coordinates::from_hours(5.5, -5.4).unwrap();
        let of_date = c.to_epoch(Epoch::JNow, at);
        assert!(separation_arcsec(c, of_date) < 0.01);
    }

    #[test]
    fn same_epoch_is_returned_unchanged() {
        let at = Utc.with_ymd_and_hms(2026, 11, 22, 0, 0, 0).unwrap();
        let j2000 = Coordinates::from_hours(5.5, -5.4).unwrap();
        assert_eq!(j2000.to_epoch(Epoch::J2000, at), j2000);
        let jnow = Coordinates::with_epoch(j2000.ra, j2000.dec, Epoch::JNow);
        assert_eq!(jnow.to_epoch(Epoch::JNow, at), jnow);
    }

    #[test]
    fn precession_at_the_poles_is_finite_and_small() {
        let at = Utc.with_ymd_and_hms(2026, 11, 22, 0, 0, 0).unwrap();
        for dec in [90.0, -90.0] {
            let pole = Coordinates::new(
                RaDegrees::new(123.0).unwrap(),
                DecDegrees::new(dec).unwrap(),
            );
            let of_date = pole.to_epoch(Epoch::JNow, at);
            assert!(of_date.ra.as_degrees().is_finite());
            assert!((0.0..360.0).contains(&of_date.ra.as_degrees()));
            // The pole moves by theta: about 0.15 degrees in 26.9 years.
            let moved = separation_arcsec(pole, Coordinates::new(of_date.ra, of_date.dec));
            assert!((500.0..580.0).contains(&moved), "moved {moved} arcsec");
        }
    }

    mod prop {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn precession_round_trips(
                ra in 0.0f64..360.0,
                dec in -89.0f64..89.0,
                days in 0i64..36_525,
            ) {
                let at = Utc.with_ymd_and_hms(2000, 1, 1, 12, 0, 0).unwrap()
                    + chrono::Duration::days(days);
                let start = Coordinates::new(
                    RaDegrees::new(ra).unwrap(),
                    DecDegrees::new(dec).unwrap(),
                );
                let there = start.to_epoch(Epoch::JNow, at);
                prop_assert!((0.0..360.0).contains(&there.ra.as_degrees()));
                let back = there.to_epoch(Epoch::J2000, at);
                prop_assert_eq!(back.epoch, Epoch::J2000);
                let off = separation_arcsec(start, back);
                prop_assert!(off < 0.01, "round trip off by {} arcsec", off);
            }

            #[test]
            fn ra_from_hours_within_bounds(hours in 0.0f64..24.0) {
                let ra = RaDegrees::from_hours(hours).unwrap();
                let deg = ra.as_degrees();
                prop_assert!((0.0..360.0).contains(&deg), "deg out of range: {deg}");
            }

            #[test]
            fn dec_roundtrip(deg in -90.0f64..=90.0) {
                let dec = DecDegrees::new(deg).unwrap();
                prop_assert!((dec.as_degrees() - deg).abs() < f64::EPSILON);
            }

            #[test]
            fn ra_below_zero_rejected(deg in -1e9f64..0.0) {
                prop_assert!(RaDegrees::new(deg).is_err());
            }

            #[test]
            fn ra_above_360_rejected(deg in 360.0001f64..1e9) {
                prop_assert!(RaDegrees::new(deg).is_err());
            }
        }
    }
}
