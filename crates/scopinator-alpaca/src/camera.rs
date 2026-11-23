//! The Alpaca `camera` device. Fetching the image is in
//! [`crate::image`]; this is everything else.

use std::sync::Arc;

use crate::client::AlpacaClient;
use crate::device::{Endpoint, actions, bool_param, getters, int_enum, setters};
use crate::error::AlpacaError;

int_enum! {
    /// What the camera is doing (`CameraState`).
    CameraState {
        Idle = 0,
        /// Waiting for a filter wheel, shutter or the like.
        Waiting = 1,
        Exposing = 2,
        /// Reading the sensor out.
        Reading = 3,
        /// Sending the image to the driver.
        Download = 4,
        Error = 5,
    }
}

int_enum! {
    /// What kind of sensor the camera has (`SensorType`).
    SensorType {
        Monochrome = 0,
        /// Colour, delivered as finished colour pixels.
        Color = 1,
        /// A red, green, green, blue mosaic; `BayerOffsetX/Y` say which
        /// colour the first pixel is.
        Rggb = 2,
        Cmyg = 3,
        Cmyg2 = 4,
        Lrgb = 5,
    }
}

/// One `camera` device on an Alpaca server.
#[derive(Clone)]
pub struct AlpacaCamera {
    pub(crate) endpoint: Endpoint,
    pub(crate) client: Arc<AlpacaClient>,
}

impl AlpacaCamera {
    pub fn new(client: Arc<AlpacaClient>, device_number: u32) -> Self {
        Self {
            endpoint: Endpoint::new(Arc::clone(&client), "camera", device_number),
            client,
        }
    }

    pub fn device_number(&self) -> u32 {
        self.endpoint.device_number()
    }

    getters! {
        connected: bool = "connected";
        name: String = "name";
        driver_info: String = "driverinfo";
        /// True when the last exposure has produced an image to fetch.
        image_ready: bool = "imageready";
        camera_state: CameraState = "camerastate";
        /// Progress of the current operation, 0 to 100.
        percent_completed: i32 = "percentcompleted";
        /// Sensor width in unbinned pixels.
        camera_x_size: u32 = "cameraxsize";
        /// Sensor height in unbinned pixels.
        camera_y_size: u32 = "cameraysize";
        bin_x: u32 = "binx";
        bin_y: u32 = "biny";
        max_bin_x: u32 = "maxbinx";
        max_bin_y: u32 = "maxbiny";
        /// Left edge of the subframe, in binned pixels.
        start_x: u32 = "startx";
        /// Top edge of the subframe, in binned pixels.
        start_y: u32 = "starty";
        /// Subframe width in binned pixels.
        num_x: u32 = "numx";
        /// Subframe height in binned pixels.
        num_y: u32 = "numy";
        /// Pixel width in micrometres.
        pixel_size_x: f64 = "pixelsizex";
        /// Pixel height in micrometres.
        pixel_size_y: f64 = "pixelsizey";
        sensor_type: SensorType = "sensortype";
        bayer_offset_x: u32 = "bayeroffsetx";
        bayer_offset_y: u32 = "bayeroffsety";
        /// The largest value a pixel can have.
        max_adu: i64 = "maxadu";
        electrons_per_adu: f64 = "electronsperadu";
        /// Full well capacity in electrons.
        full_well_capacity: f64 = "fullwellcapacity";
        /// Shortest exposure in seconds.
        exposure_min: f64 = "exposuremin";
        /// Longest exposure in seconds.
        exposure_max: f64 = "exposuremax";
        gain: i32 = "gain";
        gain_min: i32 = "gainmin";
        gain_max: i32 = "gainmax";
        offset: i32 = "offset";
        offset_min: i32 = "offsetmin";
        offset_max: i32 = "offsetmax";
        /// Sensor temperature in degrees Celsius.
        ccd_temperature: f64 = "ccdtemperature";
        cooler_on: bool = "cooleron";
        /// Cooler power as a percentage.
        cooler_power: f64 = "coolerpower";
        /// The temperature the cooler regulates to, in degrees Celsius.
        set_ccd_temperature_target: f64 = "setccdtemperature";
        can_set_ccd_temperature: bool = "cansetccdtemperature";
        can_abort_exposure: bool = "canabortexposure";
        can_stop_exposure: bool = "canstopexposure";
    }

    setters! {
        set_connected(bool) = "connected", "Connected";
        set_bin_x(u32) = "binx", "BinX";
        set_bin_y(u32) = "biny", "BinY";
        set_start_x(u32) = "startx", "StartX";
        set_start_y(u32) = "starty", "StartY";
        set_num_x(u32) = "numx", "NumX";
        set_num_y(u32) = "numy", "NumY";
        set_gain(i32) = "gain", "Gain";
        set_offset(i32) = "offset", "Offset";
        set_cooler_on(bool) = "cooleron", "CoolerOn";
        /// Set the temperature the cooler regulates to.
        set_ccd_temperature(f64) = "setccdtemperature", "SetCCDTemperature";
    }

    actions! {
        /// Stop the exposure and throw the image away.
        abort_exposure = "abortexposure";
        /// Stop the exposure early and keep what was collected.
        stop_exposure = "stopexposure";
    }

    /// Start an exposure of `duration_seconds`. `light` is false for a dark
    /// frame (shutter closed). Returns at once; `image_ready` says when the
    /// image can be fetched.
    pub async fn start_exposure(
        &self,
        duration_seconds: f64,
        light: bool,
    ) -> Result<(), AlpacaError> {
        self.endpoint
            .put(
                "startexposure",
                &[
                    ("Duration", duration_seconds.to_string()),
                    ("Light", bool_param(light)),
                ],
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;
    use crate::client::tests::{request_line_parts, serve};

    async fn camera(body: &'static str) -> (AlpacaCamera, tokio::task::JoinHandle<Vec<String>>) {
        let (addr, handle) = serve("200 OK", "application/json", body, 1).await;
        let client = Arc::new(AlpacaClient::new(Ipv4Addr::LOCALHOST, addr.port()));
        (AlpacaCamera::new(client, 0), handle)
    }

    async fn sent(handle: tokio::task::JoinHandle<Vec<String>>) -> (String, String) {
        let request = handle.await.unwrap().pop().expect("one request");
        let (_, path, _) = request_line_parts(&request);
        let body = request.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        (path.to_string(), body)
    }

    #[tokio::test]
    async fn start_exposure_sends_duration_and_light() {
        let (cam, handle) = camera(r#"{"ErrorNumber": 0}"#).await;
        cam.start_exposure(2.5, false).await.unwrap();
        let (path, body) = sent(handle).await;
        assert_eq!(path, "/api/v1/camera/0/startexposure");
        assert!(body.contains("Duration=2.5"), "{body}");
        assert!(body.contains("Light=False"), "{body}");
    }

    #[tokio::test]
    async fn state_and_sensor_type_are_read_from_their_numbers() {
        let (cam, _h) = camera(r#"{"Value": 2, "ErrorNumber": 0}"#).await;
        assert_eq!(cam.camera_state().await.unwrap(), CameraState::Exposing);
        let (cam, _h) = camera(r#"{"Value": 2, "ErrorNumber": 0}"#).await;
        assert_eq!(cam.sensor_type().await.unwrap(), SensorType::Rggb);
        let (cam, _h) = camera(r#"{"Value": 6, "ErrorNumber": 0}"#).await;
        assert!(cam.camera_state().await.is_err());
        let (cam, _h) = camera(r#"{"Value": 42, "ErrorNumber": 0}"#).await;
        assert!(cam.sensor_type().await.is_err());
    }

    #[tokio::test]
    async fn the_subframe_and_binning_are_set_by_name() {
        for (expected_path, expected_body) in [
            ("/api/v1/camera/0/binx", "BinX=2"),
            ("/api/v1/camera/0/startx", "StartX=100"),
            ("/api/v1/camera/0/numy", "NumY=480"),
        ] {
            let (cam, handle) = camera(r#"{"ErrorNumber": 0}"#).await;
            match expected_path {
                p if p.ends_with("binx") => cam.set_bin_x(2).await.unwrap(),
                p if p.ends_with("startx") => cam.set_start_x(100).await.unwrap(),
                _ => cam.set_num_y(480).await.unwrap(),
            }
            let (path, body) = sent(handle).await;
            assert_eq!(path, expected_path);
            assert!(body.contains(expected_body), "{body}");
        }
    }

    #[tokio::test]
    async fn the_cooler_setpoint_is_read_and_written_on_setccdtemperature() {
        let (cam, handle) = camera(r#"{"Value": -10.0, "ErrorNumber": 0}"#).await;
        assert_eq!(cam.set_ccd_temperature_target().await.unwrap(), -10.0);
        assert_eq!(sent(handle).await.0, "/api/v1/camera/0/setccdtemperature");

        let (cam, handle) = camera(r#"{"ErrorNumber": 0}"#).await;
        cam.set_ccd_temperature(-15.5).await.unwrap();
        let (path, body) = sent(handle).await;
        assert_eq!(path, "/api/v1/camera/0/setccdtemperature");
        assert!(body.contains("SetCCDTemperature=-15.5"), "{body}");
    }

    #[tokio::test]
    async fn a_camera_without_a_cooler_says_not_implemented() {
        let (cam, _h) =
            camera(r#"{"ErrorNumber": 1024, "ErrorMessage": "CoolerOn not implemented"}"#).await;
        assert!(cam.cooler_on().await.unwrap_err().is_not_implemented());
    }
}
