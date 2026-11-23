//! An Alpaca camera through the neutral `Camera` trait.
#![cfg(feature = "alpaca")]

mod alpaca_common;

use std::net::Ipv4Addr;
use std::sync::Arc;

use alpaca_common::FakeAlpaca;
use scopinator::backend::alpaca::AlpacaCameraDevice;
use scopinator::{Camera, Device, ScopinatorError};
use scopinator_alpaca::{AlpacaCamera, AlpacaClient};
use scopinator_types::{
    BayerPattern, CameraState, DeviceId, ExposureSettings, FrameType, PixelFormat, Roi, SensorType,
};
use serde_json::{Value, json};

const CAMERA: &str = "camera/0";

fn key(property: &str) -> String {
    format!("{CAMERA}/{property}")
}

/// A cooled 4000x3000 colour camera, RGGB with the mosaic starting one
/// column in (so the first row reads G R).
fn full_camera() -> Value {
    json!({
        "connected": true, "name": "Fake Camera", "driverinfo": "fake camera driver",
        "cameraxsize": 4000, "cameraysize": 3000, "pixelsizex": 3.76, "pixelsizey": 3.76,
        "sensortype": 2, "bayeroffsetx": 1, "bayeroffsety": 0, "maxadu": 65535,
        "gainmin": 0, "gainmax": 500, "offsetmin": 0, "offsetmax": 100,
        "exposuremin": 0.001, "exposuremax": 3600.0, "maxbinx": 4, "maxbiny": 4,
        "cooleron": true, "cansetccdtemperature": true, "coolerpower": 35.0,
        "ccdtemperature": -9.8, "setccdtemperature": -10.0,
        "fullwellcapacity": 50000.0, "electronsperadu": 0.8, "canabortexposure": true,
        "imageready": false, "camerastate": 0, "gain": 100,
    })
}

async fn camera(fake: &FakeAlpaca) -> AlpacaCameraDevice {
    let client = Arc::new(AlpacaClient::new(Ipv4Addr::LOCALHOST, fake.addr.port()));
    let backend_id = DeviceId::new(format!("alpaca:127.0.0.1:{}", fake.addr.port()));
    AlpacaCameraDevice::new(AlpacaCamera::new(client, 0), &backend_id)
}

async fn connected(fake: &FakeAlpaca) -> AlpacaCameraDevice {
    fake.set_all(CAMERA, full_camera());
    let camera = camera(fake).await;
    camera.connect().await.expect("connect");
    camera
}

/// An ImageBytes reply: 16-bit unsigned samples in Alpaca's column order.
fn image_bytes(width: i32, height: i32, planes: i32, column_major: &[u16]) -> Vec<u8> {
    let rank = if planes > 1 { 3 } else { 2 };
    let dimension3 = if planes > 1 { planes } else { 0 };
    let mut out = Vec::new();
    for field in [1, 0, 0, 0, 44, 2, 8, rank, width, height, dimension3] {
        out.extend_from_slice(&i32::to_le_bytes(field));
    }
    for sample in column_major {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

fn sent(fake: &FakeAlpaca, property: &str, parameter: &str) -> Option<String> {
    fake.puts(&key(property))
        .last()
        .map(|params| params[parameter].clone())
}

#[tokio::test]
async fn connect_reads_the_sensor_description() {
    let fake = FakeAlpaca::start().await;
    let camera = connected(&fake).await;
    let caps = camera.capabilities();

    assert_eq!((caps.max_width, caps.max_height), (Some(4000), Some(3000)));
    assert_eq!(
        (caps.pixel_size_x_um, caps.pixel_size_y_um),
        (Some(3.76), Some(3.76))
    );
    // RGGB shifted one column: the first row reads G R.
    assert_eq!(
        caps.sensor_type,
        Some(SensorType::Bayer(BayerPattern::Grbg))
    );
    assert_eq!(caps.bit_depth, Some(16));
    assert_eq!(caps.gain_range, Some((0, 500)));
    assert_eq!(caps.offset_range, Some((0, 100)));
    assert_eq!(caps.exposure_range_s, Some((0.001, 3600.0)));
    assert_eq!(caps.max_bin, Some(4));
    assert!(caps.can_expose && caps.can_abort_exposure && caps.can_set_roi);
    assert!(caps.has_cooler && caps.can_set_temperature);
    assert_eq!(
        (caps.full_well_e, caps.e_per_adu),
        (Some(50000.0), Some(0.8))
    );

    let info = camera.info();
    assert_eq!(info.name, "Fake Camera");
    assert_eq!(
        info.id.as_str(),
        format!("alpaca:127.0.0.1:{}:camera:0", fake.addr.port())
    );
}

#[tokio::test]
async fn a_camera_without_optional_properties_reports_them_as_unknown() {
    let fake = FakeAlpaca::start().await;
    fake.set_all(
        CAMERA,
        json!({"connected": true, "name": "Bare", "cameraxsize": 640, "cameraysize": 480,
            "sensortype": 0, "maxadu": 4095}),
    );
    for property in [
        "driverinfo",
        "pixelsizex",
        "pixelsizey",
        "gainmin",
        "gainmax",
        "offsetmin",
        "offsetmax",
        "exposuremin",
        "exposuremax",
        "maxbinx",
        "maxbiny",
        "cooleron",
        "cansetccdtemperature",
        "fullwellcapacity",
        "electronsperadu",
        "canabortexposure",
    ] {
        fake.not_implemented(&key(property));
    }
    let camera = camera(&fake).await;
    camera.connect().await.unwrap();
    let caps = camera.capabilities();

    assert_eq!(caps.sensor_type, Some(SensorType::Mono));
    assert_eq!(caps.bit_depth, Some(12));
    assert_eq!(caps.gain_range, None);
    assert_eq!(caps.exposure_range_s, None);
    assert_eq!(caps.max_bin, None);
    assert!(!caps.has_cooler && !caps.can_set_temperature && !caps.can_abort_exposure);
    assert!(
        camera
            .set_cooler(true)
            .await
            .unwrap_err()
            .is_not_supported()
    );
}

#[tokio::test]
async fn an_exposure_applies_its_settings_before_it_starts() {
    let fake = FakeAlpaca::start().await;
    let camera = connected(&fake).await;

    let settings = ExposureSettings::new(30.0)
        .with_gain(120)
        .with_offset(10)
        .with_binning(2)
        .with_roi(Roi::new(1000, 500, 2000, 1000).unwrap());
    camera.start_exposure(&settings).await.unwrap();

    assert_eq!(sent(&fake, "binx", "BinX").as_deref(), Some("2"));
    assert_eq!(sent(&fake, "biny", "BinY").as_deref(), Some("2"));
    // The subframe is given in sensor pixels and sent in binned pixels.
    assert_eq!(sent(&fake, "startx", "StartX").as_deref(), Some("500"));
    assert_eq!(sent(&fake, "starty", "StartY").as_deref(), Some("250"));
    assert_eq!(sent(&fake, "numx", "NumX").as_deref(), Some("1000"));
    assert_eq!(sent(&fake, "numy", "NumY").as_deref(), Some("500"));
    assert_eq!(sent(&fake, "gain", "Gain").as_deref(), Some("120"));
    assert_eq!(sent(&fake, "offset", "Offset").as_deref(), Some("10"));
    assert_eq!(
        sent(&fake, "startexposure", "Duration").as_deref(),
        Some("30")
    );
    assert_eq!(
        sent(&fake, "startexposure", "Light").as_deref(),
        Some("True")
    );

    // The exposure is the last thing sent.
    assert_eq!(fake.put_keys().last().unwrap(), &key("startexposure"));
}

#[tokio::test]
async fn without_a_subframe_the_whole_sensor_is_read_and_gain_is_left_alone() {
    let fake = FakeAlpaca::start().await;
    let camera = connected(&fake).await;
    camera
        .start_exposure(&ExposureSettings::new(1.0).with_binning(2))
        .await
        .unwrap();
    assert_eq!(sent(&fake, "startx", "StartX").as_deref(), Some("0"));
    assert_eq!(sent(&fake, "numx", "NumX").as_deref(), Some("2000"));
    assert_eq!(sent(&fake, "numy", "NumY").as_deref(), Some("1500"));
    assert!(fake.puts(&key("gain")).is_empty());
    assert!(fake.puts(&key("offset")).is_empty());
}

#[tokio::test]
async fn dark_and_bias_frames_close_the_shutter_and_flats_do_not() {
    for (frame_type, light) in [
        (FrameType::Dark, "False"),
        (FrameType::Bias, "False"),
        (FrameType::Flat, "True"),
        (FrameType::Light, "True"),
    ] {
        let fake = FakeAlpaca::start().await;
        let camera = connected(&fake).await;
        camera
            .start_exposure(&ExposureSettings::new(1.0).with_frame_type(frame_type))
            .await
            .unwrap();
        assert_eq!(
            sent(&fake, "startexposure", "Light").as_deref(),
            Some(light),
            "{frame_type:?}"
        );
    }
}

#[tokio::test]
async fn settings_the_camera_cannot_take_are_refused_before_anything_is_sent() {
    let fake = FakeAlpaca::start().await;
    let camera = connected(&fake).await;
    let before = fake.put_keys().len();

    let cases = [
        ExposureSettings::new(1.0).with_gain(501),
        ExposureSettings::new(1.0).with_offset(-1),
        ExposureSettings::new(4000.0),
        ExposureSettings::new(0.0001),
        ExposureSettings::new(1.0).with_binning(8),
        // Hangs off the right edge of a 4000-pixel sensor.
        ExposureSettings::new(1.0).with_roi(Roi::new(3000, 0, 1001, 100).unwrap()),
    ];
    for settings in &cases {
        let err = camera.start_exposure(settings).await.unwrap_err();
        assert!(
            matches!(err, ScopinatorError::OutOfRange(_)),
            "{settings:?}: {err:?}"
        );
    }
    let zero_bin = ExposureSettings::new(1.0).with_binning(0);
    assert!(matches!(
        camera.start_exposure(&zero_bin).await,
        Err(ScopinatorError::InvalidArgument(_))
    ));
    let mut readout = ExposureSettings::new(1.0);
    readout.readout_mode = Some("fast".into());
    assert!(
        camera
            .start_exposure(&readout)
            .await
            .unwrap_err()
            .is_not_supported()
    );

    assert_eq!(fake.put_keys().len(), before, "nothing reached the camera");
}

#[tokio::test]
async fn an_image_arrives_in_rows_as_little_endian_samples_with_its_metadata() {
    let fake = FakeAlpaca::start().await;
    let camera = connected(&fake).await;
    // The image
    //     1 2 3
    //     4 5 6
    // in Alpaca's order: column by column.
    fake.set_image(image_bytes(3, 2, 1, &[1, 4, 2, 5, 3, 6]));

    // Not before there is one.
    let err = camera.get_image().await.unwrap_err();
    assert!(matches!(err, ScopinatorError::InvalidState(_)), "{err:?}");

    camera
        .start_exposure(&ExposureSettings::new(2.0).with_gain(100))
        .await
        .unwrap();
    assert!(!camera.image_ready().await.unwrap());
    fake.set(&key("imageready"), json!(true));
    assert!(camera.image_ready().await.unwrap());

    let image = camera.get_image().await.unwrap();
    assert_eq!((image.width, image.height), (3, 2));
    assert_eq!(image.format, PixelFormat::Bayer16(BayerPattern::Grbg));
    assert_eq!(
        &image.data[..],
        &[1, 0, 2, 0, 3, 0, 4, 0, 5, 0, 6, 0],
        "rows, low byte first"
    );
    assert!(image.has_expected_len());
    assert_eq!(image.metadata.exposure_seconds, Some(2.0));
    assert_eq!(image.metadata.gain, Some(100));
    assert_eq!((image.metadata.bin_x, image.metadata.bin_y), (1, 1));
    assert_eq!(image.metadata.sensor_temperature_c, Some(-9.8));
    assert!(image.metadata.timestamp.is_some());
}

#[tokio::test]
async fn a_three_plane_image_is_interleaved_rgb_and_a_mono_sensor_gives_mono() {
    let fake = FakeAlpaca::start().await;
    let camera = connected(&fake).await;
    fake.set(&key("imageready"), json!(true));
    // One pixel: planes 10, 20, 30.
    fake.set_image(image_bytes(1, 1, 3, &[10, 20, 30]));
    let image = camera.get_image().await.unwrap();
    assert_eq!(image.format, PixelFormat::Rgb16);
    assert_eq!(&image.data[..], &[10, 0, 20, 0, 30, 0]);

    let fake = FakeAlpaca::start().await;
    fake.set_all(CAMERA, full_camera());
    fake.set(&key("sensortype"), json!(0));
    fake.set(&key("imageready"), json!(true));
    fake.set_image(image_bytes(1, 1, 1, &[777]));
    let mono = self::camera(&fake).await;
    mono.connect().await.unwrap();
    assert_eq!(mono.get_image().await.unwrap().format, PixelFormat::Mono16);
}

#[tokio::test]
async fn state_cooling_and_abort_map_onto_the_camera() {
    let fake = FakeAlpaca::start().await;
    let camera = connected(&fake).await;

    assert!(!camera.is_exposing().await.unwrap());
    fake.set(&key("camerastate"), json!(2));
    assert!(camera.is_exposing().await.unwrap());
    fake.set(&key("camerastate"), json!(4));
    assert!(
        camera.is_exposing().await.unwrap(),
        "downloading is not idle"
    );

    let status = camera.get_camera_status().await.unwrap();
    assert_eq!(status.state, CameraState::Downloading);
    assert_eq!(status.temperature, Some(-9.8));
    assert_eq!(status.cooler_on, Some(true));
    assert_eq!(status.cooler_power, Some(35.0));
    assert_eq!(status.target_temperature, Some(-10.0));
    assert_eq!(status.gain, Some(100));

    camera.set_cooler(false).await.unwrap();
    assert_eq!(
        sent(&fake, "cooleron", "CoolerOn").as_deref(),
        Some("False")
    );
    camera.set_target_temperature(-15.0).await.unwrap();
    assert_eq!(
        sent(&fake, "setccdtemperature", "SetCCDTemperature").as_deref(),
        Some("-15")
    );
    assert_eq!(camera.cooler_power().await.unwrap(), 35.0);

    camera.abort_exposure().await.unwrap();
    assert!(fake.put_keys().contains(&key("abortexposure")));
}

#[tokio::test]
async fn a_camera_with_no_cooler_says_not_supported() {
    let fake = FakeAlpaca::start().await;
    let camera = connected(&fake).await;
    fake.not_implemented(&key("cooleron"));
    fake.not_implemented(&key("coolerpower"));
    assert!(
        camera
            .set_cooler(true)
            .await
            .unwrap_err()
            .is_not_supported()
    );
    assert!(camera.cooler_power().await.unwrap_err().is_not_supported());
    // And the status simply has no cooler in it.
    let status = camera.get_camera_status().await.unwrap();
    assert_eq!((status.cooler_on, status.cooler_power), (None, None));
}
