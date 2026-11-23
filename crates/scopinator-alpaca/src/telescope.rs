//! The Alpaca `telescope` device: a mount.

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::client::AlpacaClient;
use crate::device::{Endpoint, actions, getters, int_enum, setters};
use crate::error::AlpacaError;

int_enum! {
    /// The equatorial system a mount works in (`EquatorialSystem`).
    EquatorialSystem {
        /// Custom or unknown.
        Other = 0,
        /// The equinox of date (JNow).
        Topocentric = 1,
        J2000 = 2,
        J2050 = 3,
        B1950 = 4,
    }
}

int_enum! {
    /// How the mount's axes are arranged (`AlignmentMode`).
    AlignmentMode {
        AltAz = 0,
        /// A polar axis without a meridian flip.
        Polar = 1,
        GermanPolar = 2,
    }
}

int_enum! {
    /// Which side of the pier the mount is on (`SideOfPier`).
    PierSide {
        Unknown = -1,
        East = 0,
        West = 1,
    }
}

int_enum! {
    /// A tracking rate (`TrackingRate`, the `DriveRates` enumeration).
    DriveRate {
        Sidereal = 0,
        Lunar = 1,
        Solar = 2,
        King = 3,
    }
}

int_enum! {
    /// A direction for a guiding pulse (`GuideDirections`).
    GuideDirection {
        North = 0,
        South = 1,
        East = 2,
        West = 3,
    }
}

int_enum! {
    /// A mount axis (`TelescopeAxes`).
    TelescopeAxis {
        /// Right ascension or azimuth.
        Primary = 0,
        /// Declination or altitude.
        Secondary = 1,
        /// An image rotator or de-rotator.
        Tertiary = 2,
    }
}

/// A range of speeds an axis can be moved at, in degrees per second
/// (`AxisRates`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AxisRate {
    pub minimum: f64,
    pub maximum: f64,
}

/// One `telescope` device on an Alpaca server.
#[derive(Clone)]
pub struct AlpacaTelescope {
    endpoint: Endpoint,
}

impl AlpacaTelescope {
    pub fn new(client: Arc<AlpacaClient>, device_number: u32) -> Self {
        Self {
            endpoint: Endpoint::new(client, "telescope", device_number),
        }
    }

    pub fn device_number(&self) -> u32 {
        self.endpoint.device_number()
    }

    getters! {
        connected: bool = "connected";
        /// The driver's name.
        name: String = "name";
        /// The driver's description of itself and its version.
        driver_info: String = "driverinfo";
        /// Right ascension in hours, in the mount's equatorial system.
        right_ascension: f64 = "rightascension";
        /// Declination in degrees, in the mount's equatorial system.
        declination: f64 = "declination";
        /// Altitude above the horizon in degrees.
        altitude: f64 = "altitude";
        /// Azimuth in degrees, from north through east.
        azimuth: f64 = "azimuth";
        slewing: bool = "slewing";
        at_park: bool = "atpark";
        at_home: bool = "athome";
        tracking: bool = "tracking";
        side_of_pier: PierSide = "sideofpier";
        tracking_rate: DriveRate = "trackingrate";
        /// The tracking rates the mount offers.
        tracking_rates: Vec<DriveRate> = "trackingrates";
        equatorial_system: EquatorialSystem = "equatorialsystem";
        alignment_mode: AlignmentMode = "alignmentmode";
        can_slew: bool = "canslew";
        can_slew_async: bool = "canslewasync";
        can_slew_alt_az_async: bool = "canslewaltazasync";
        can_sync: bool = "cansync";
        can_park: bool = "canpark";
        can_unpark: bool = "canunpark";
        can_set_tracking: bool = "cansettracking";
        can_find_home: bool = "canfindhome";
        can_pulse_guide: bool = "canpulseguide";
        /// Latitude in degrees, north positive.
        site_latitude: f64 = "sitelatitude";
        /// Longitude in degrees, east positive.
        site_longitude: f64 = "sitelongitude";
        /// Height above sea level in metres.
        site_elevation: f64 = "siteelevation";
    }

    setters! {
        set_connected(bool) = "connected", "Connected";
        set_tracking(bool) = "tracking", "Tracking";
        set_tracking_rate(DriveRate) = "trackingrate", "TrackingRate";
        set_site_latitude(f64) = "sitelatitude", "SiteLatitude";
        set_site_longitude(f64) = "sitelongitude", "SiteLongitude";
        set_site_elevation(f64) = "siteelevation", "SiteElevation";
    }

    actions! {
        abort_slew = "abortslew";
        /// Start parking; `at_park` says when the mount is there.
        park = "park";
        unpark = "unpark";
        /// Start a search for home; `at_home` says when the mount is there.
        find_home = "findhome";
    }

    /// Whether `axis` can be moved with [`move_axis`](Self::move_axis).
    pub async fn can_move_axis(&self, axis: TelescopeAxis) -> Result<bool, AlpacaError> {
        // The one GET that takes a parameter; it goes in the query string.
        self.endpoint
            .get(&format!("canmoveaxis?Axis={}", axis.number()))
            .await
    }

    /// The speeds `axis` can be moved at.
    pub async fn axis_rates(&self, axis: TelescopeAxis) -> Result<Vec<AxisRate>, AlpacaError> {
        self.endpoint
            .get(&format!("axisrates?Axis={}", axis.number()))
            .await
    }

    /// Start a slew to a right ascension in hours and a declination in
    /// degrees, in the mount's equatorial system. Returns at once;
    /// `slewing` says when the mount has arrived.
    pub async fn slew_to_coordinates_async(
        &self,
        ra_hours: f64,
        dec_degrees: f64,
    ) -> Result<(), AlpacaError> {
        self.endpoint
            .put("slewtocoordinatesasync", &radec(ra_hours, dec_degrees))
            .await
    }

    /// Start a slew to an azimuth and altitude in degrees.
    pub async fn slew_to_alt_az_async(
        &self,
        azimuth: f64,
        altitude: f64,
    ) -> Result<(), AlpacaError> {
        self.endpoint
            .put(
                "slewtoaltazasync",
                &[
                    ("Azimuth", azimuth.to_string()),
                    ("Altitude", altitude.to_string()),
                ],
            )
            .await
    }

    /// Tell the mount it is pointing at these coordinates.
    pub async fn sync_to_coordinates(
        &self,
        ra_hours: f64,
        dec_degrees: f64,
    ) -> Result<(), AlpacaError> {
        self.endpoint
            .put("synctocoordinates", &radec(ra_hours, dec_degrees))
            .await
    }

    /// Move `axis` at `rate` degrees per second until told otherwise; a rate
    /// of zero stops it.
    pub async fn move_axis(&self, axis: TelescopeAxis, rate: f64) -> Result<(), AlpacaError> {
        self.endpoint
            .put(
                "moveaxis",
                &[
                    ("Axis", axis.number().to_string()),
                    ("Rate", rate.to_string()),
                ],
            )
            .await
    }

    /// Nudge the mount in `direction` for `duration_ms` milliseconds.
    pub async fn pulse_guide(
        &self,
        direction: GuideDirection,
        duration_ms: u32,
    ) -> Result<(), AlpacaError> {
        self.endpoint
            .put(
                "pulseguide",
                &[
                    ("Direction", direction.number().to_string()),
                    ("Duration", duration_ms.to_string()),
                ],
            )
            .await
    }

    /// The mount's clock.
    pub async fn utc_date(&self) -> Result<DateTime<Utc>, AlpacaError> {
        let text: String = self.endpoint.get("utcdate").await?;
        DateTime::parse_from_rfc3339(&text)
            .map(|time| time.with_timezone(&Utc))
            .map_err(|e| AlpacaError::Value(format!("UTCDate `{text}` is not a date: {e}")))
    }

    /// Set the mount's clock.
    pub async fn set_utc_date(&self, time: DateTime<Utc>) -> Result<(), AlpacaError> {
        let text = time.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        self.endpoint.put("utcdate", &[("UTCDate", text)]).await
    }
}

fn radec(ra_hours: f64, dec_degrees: f64) -> [(&'static str, String); 2] {
    [
        ("RightAscension", ra_hours.to_string()),
        ("Declination", dec_degrees.to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;
    use crate::client::tests::{request_line_parts, serve};

    async fn telescope(
        body: &'static str,
    ) -> (AlpacaTelescope, tokio::task::JoinHandle<Vec<String>>) {
        let (addr, handle) = serve("200 OK", "application/json", body, 1).await;
        let client = Arc::new(AlpacaClient::new(Ipv4Addr::LOCALHOST, addr.port()));
        (AlpacaTelescope::new(client, 2), handle)
    }

    /// The path of the one request sent, and its form body.
    async fn sent(handle: tokio::task::JoinHandle<Vec<String>>) -> (String, String, String) {
        let request = handle.await.unwrap().pop().expect("one request");
        let (method, path, _) = request_line_parts(&request);
        let body = request.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        (method.to_string(), path.to_string(), body)
    }

    #[tokio::test]
    async fn numbers_and_booleans_are_read_from_value() {
        let (scope, handle) = telescope(r#"{"Value": 20.6917, "ErrorNumber": 0}"#).await;
        assert_eq!(scope.right_ascension().await.unwrap(), 20.6917);
        let (method, path, _) = sent(handle).await;
        assert_eq!(
            (method.as_str(), path.as_str()),
            ("GET", "/api/v1/telescope/2/rightascension")
        );

        let (scope, handle) = telescope(r#"{"Value": true, "ErrorNumber": 0}"#).await;
        assert!(scope.slewing().await.unwrap());
        assert_eq!(sent(handle).await.1, "/api/v1/telescope/2/slewing");
    }

    #[tokio::test]
    async fn enumerations_are_read_from_their_numbers() {
        let (scope, _h) = telescope(r#"{"Value": 1, "ErrorNumber": 0}"#).await;
        assert_eq!(
            scope.equatorial_system().await.unwrap(),
            EquatorialSystem::Topocentric
        );
        let (scope, _h) = telescope(r#"{"Value": 2, "ErrorNumber": 0}"#).await;
        assert_eq!(
            scope.alignment_mode().await.unwrap(),
            AlignmentMode::GermanPolar
        );
        let (scope, _h) = telescope(r#"{"Value": -1, "ErrorNumber": 0}"#).await;
        assert_eq!(scope.side_of_pier().await.unwrap(), PierSide::Unknown);
        let (scope, _h) = telescope(r#"{"Value": [0, 1, 2], "ErrorNumber": 0}"#).await;
        assert_eq!(
            scope.tracking_rates().await.unwrap(),
            [DriveRate::Sidereal, DriveRate::Lunar, DriveRate::Solar]
        );
    }

    #[tokio::test]
    async fn an_unknown_enumeration_number_is_an_error() {
        let (scope, _h) = telescope(r#"{"Value": 9, "ErrorNumber": 0}"#).await;
        assert!(matches!(
            scope.equatorial_system().await,
            Err(AlpacaError::Json(_))
        ));
    }

    #[tokio::test]
    async fn a_slew_sends_right_ascension_and_declination_by_name() {
        let (scope, handle) = telescope(r#"{"ErrorNumber": 0, "ErrorMessage": ""}"#).await;
        scope.slew_to_coordinates_async(20.69, 45.28).await.unwrap();
        let (method, path, body) = sent(handle).await;
        assert_eq!(method, "PUT");
        assert_eq!(path, "/api/v1/telescope/2/slewtocoordinatesasync");
        assert!(body.contains("RightAscension=20.69"), "{body}");
        assert!(body.contains("Declination=45.28"), "{body}");
    }

    #[tokio::test]
    async fn move_axis_sends_the_axis_number_and_rate() {
        let (scope, handle) = telescope(r#"{"ErrorNumber": 0}"#).await;
        scope
            .move_axis(TelescopeAxis::Secondary, -0.5)
            .await
            .unwrap();
        let (_, path, body) = sent(handle).await;
        assert_eq!(path, "/api/v1/telescope/2/moveaxis");
        assert!(body.contains("Axis=1"), "{body}");
        assert!(body.contains("Rate=-0.5"), "{body}");
    }

    #[tokio::test]
    async fn can_move_axis_puts_the_axis_in_the_query() {
        let (scope, handle) = telescope(r#"{"Value": true, "ErrorNumber": 0}"#).await;
        assert!(scope.can_move_axis(TelescopeAxis::Primary).await.unwrap());
        let request = handle.await.unwrap().pop().unwrap();
        let (method, path, params) = request_line_parts(&request);
        assert_eq!((method, path), ("GET", "/api/v1/telescope/2/canmoveaxis"));
        assert!(params.iter().any(|p| p == "Axis=0"), "{params:?}");
        assert!(
            params.iter().any(|p| p.starts_with("ClientTransactionID=")),
            "{params:?}"
        );
    }

    #[tokio::test]
    async fn axis_rates_are_ranges_in_degrees_per_second() {
        let (scope, _h) =
            telescope(r#"{"Value": [{"Minimum": 0.0, "Maximum": 4.0}], "ErrorNumber": 0}"#).await;
        assert_eq!(
            scope.axis_rates(TelescopeAxis::Primary).await.unwrap(),
            [AxisRate {
                minimum: 0.0,
                maximum: 4.0
            }]
        );
    }

    #[tokio::test]
    async fn booleans_are_sent_capitalised() {
        let (scope, handle) = telescope(r#"{"ErrorNumber": 0}"#).await;
        scope.set_tracking(true).await.unwrap();
        let (_, path, body) = sent(handle).await;
        assert_eq!(path, "/api/v1/telescope/2/tracking");
        assert!(body.contains("Tracking=True"), "{body}");
    }

    #[tokio::test]
    async fn the_clock_is_read_and_written_as_iso_8601() {
        use chrono::TimeZone;

        // ASCOM writes seven fractional digits.
        let (scope, _h) =
            telescope(r#"{"Value": "2026-11-15T17:18:54.1234567Z", "ErrorNumber": 0}"#).await;
        let time = scope.utc_date().await.unwrap();
        assert_eq!(
            time.timestamp(),
            Utc.with_ymd_and_hms(2026, 11, 15, 17, 18, 54)
                .unwrap()
                .timestamp()
        );

        let (scope, handle) = telescope(r#"{"ErrorNumber": 0}"#).await;
        scope
            .set_utc_date(Utc.with_ymd_and_hms(2026, 11, 15, 17, 18, 54).unwrap())
            .await
            .unwrap();
        let (_, _, body) = sent(handle).await;
        // Form-encoded: the colons are escaped.
        assert!(
            body.contains("UTCDate=2026-11-15T17%3A18%3A54.000Z"),
            "{body}"
        );

        let (scope, _h) = telescope(r#"{"Value": "yesterday", "ErrorNumber": 0}"#).await;
        assert!(matches!(scope.utc_date().await, Err(AlpacaError::Value(_))));
    }

    #[tokio::test]
    async fn not_implemented_and_other_device_errors_are_told_apart() {
        let (scope, _h) =
            telescope(r#"{"ErrorNumber": 1024, "ErrorMessage": "FindHome is not implemented"}"#)
                .await;
        let err = scope.find_home().await.unwrap_err();
        assert!(err.is_not_implemented(), "{err:?}");

        // 0x408: the mount is parked.
        let (scope, _h) = telescope(r#"{"ErrorNumber": 1032, "ErrorMessage": "parked"}"#).await;
        let err = scope.slew_to_coordinates_async(1.0, 2.0).await.unwrap_err();
        assert_eq!(
            err.device_code(),
            Some(crate::error::codes::INVALID_WHILE_PARKED)
        );
        assert!(!err.is_not_implemented());
    }
}
