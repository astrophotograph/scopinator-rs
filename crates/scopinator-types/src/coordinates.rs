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

    mod prop {
        use super::*;
        use proptest::prelude::*;

        proptest! {
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
