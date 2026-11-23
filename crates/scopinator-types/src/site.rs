use serde::{Deserialize, Serialize};

use crate::error::TypeError;

/// Where on Earth an instrument is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawSiteLocation")]
pub struct SiteLocation {
    latitude_deg: f64,
    longitude_deg: f64,
    elevation_m: Option<f64>,
}

/// The serialized form, validated on the way in.
#[derive(Deserialize)]
struct RawSiteLocation {
    latitude_deg: f64,
    longitude_deg: f64,
    #[serde(default)]
    elevation_m: Option<f64>,
}

impl TryFrom<RawSiteLocation> for SiteLocation {
    type Error = TypeError;

    fn try_from(raw: RawSiteLocation) -> Result<Self, TypeError> {
        let mut site = Self::new(raw.latitude_deg, raw.longitude_deg)?;
        site.elevation_m = raw.elevation_m;
        Ok(site)
    }
}

impl SiteLocation {
    /// A site at `latitude_deg` (north positive, -90 to 90) and
    /// `longitude_deg` (east positive, -180 to 180).
    pub fn new(latitude_deg: f64, longitude_deg: f64) -> Result<Self, TypeError> {
        if !(-90.0..=90.0).contains(&latitude_deg) {
            return Err(TypeError::InvalidLatitude(latitude_deg));
        }
        if !(-180.0..=180.0).contains(&longitude_deg) {
            return Err(TypeError::InvalidLongitude(longitude_deg));
        }
        Ok(Self {
            latitude_deg,
            longitude_deg,
            elevation_m: None,
        })
    }

    /// The same site with its height above sea level in metres.
    pub fn with_elevation(mut self, elevation_m: f64) -> Self {
        self.elevation_m = Some(elevation_m);
        self
    }

    /// Degrees north of the equator.
    pub fn latitude_deg(&self) -> f64 {
        self.latitude_deg
    }

    /// Degrees east of Greenwich; negative in the Americas.
    pub fn longitude_deg(&self) -> f64 {
        self.longitude_deg
    }

    /// Height above sea level in metres, if known.
    pub fn elevation_m(&self) -> Option<f64> {
        self.elevation_m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latitude_and_longitude_are_range_checked() {
        assert!(SiteLocation::new(90.0, 180.0).is_ok());
        assert!(SiteLocation::new(-90.0, -180.0).is_ok());
        assert!(SiteLocation::new(90.1, 0.0).is_err());
        assert!(SiteLocation::new(-90.1, 0.0).is_err());
        assert!(SiteLocation::new(0.0, 180.1).is_err());
        assert!(SiteLocation::new(0.0, -180.1).is_err());
        assert!(SiteLocation::new(f64::NAN, 0.0).is_err());
        assert!(SiteLocation::new(0.0, f64::NAN).is_err());
    }

    #[test]
    fn west_is_negative_and_elevation_is_optional() {
        let chicago = SiteLocation::new(41.88, -87.63).unwrap();
        assert_eq!(chicago.latitude_deg(), 41.88);
        assert_eq!(chicago.longitude_deg(), -87.63);
        assert_eq!(chicago.elevation_m(), None);
        assert_eq!(chicago.with_elevation(180.0).elevation_m(), Some(180.0));
    }

    #[test]
    fn site_roundtrip() {
        let site = SiteLocation::new(41.88, -87.63)
            .unwrap()
            .with_elevation(180.0);
        let json = serde_json::to_string(&site).unwrap();
        let back: SiteLocation = serde_json::from_str(&json).unwrap();
        assert_eq!(site, back);
    }

    #[test]
    fn an_out_of_range_site_does_not_deserialize() {
        let json = r#"{"latitude_deg": 123.0, "longitude_deg": 0.0}"#;
        assert!(serde_json::from_str::<SiteLocation>(json).is_err());
    }
}
