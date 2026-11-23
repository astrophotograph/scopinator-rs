//! The wide camera of a two-camera Seestar, through `SeestarBackend`.
//!
//! Two fake telescopes stand in for the device's two imager processes: one
//! for the main ports (4700 / 4800) and one for the wide camera's
//! (4702 / 4804). What they answer follows an S30 on firmware 9.75
//! (`docs/protocol/hardware-observations.md`, "Wide camera").
#![cfg(feature = "seestar")]

#[path = "../../scopinator-seestar/tests/common/mod.rs"]
mod common;

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use common::{FakeFrame, FakeSeestar};
use scopinator::backend::seestar::{SeestarBackend, SeestarEndpoints};
use scopinator::{Camera, Device, UnifiedEventBus};
use scopinator_seestar::SeestarConfig;
use scopinator_types::{BayerPattern, ExposureSettings, PixelFormat};
use serde_json::{Value, json};

fn device_state(second_camera: bool) -> Value {
    let mut result = json!({
        "device": {
            "name": "fake", "firmware_ver_int": 2975, "firmware_ver_string": "9.75",
            "product_model": "Seestar S30", "focal_len": 150.0, "fnumber": 5.0,
            "can_dual_capture": false,
        },
        "camera": {
            "chip_size": [1080, 1920], "pixel_size_um": 2.9, "focal_len": 150.0,
            "debayer_pattern": "GR",
        },
    });
    if second_camera {
        result["second_camera"] = json!({
            "chip_size": [1920, 1080], "pixel_size_um": 2.7, "focal_len": 14.0,
            "debayer_pattern": "RG",
        });
    }
    json!({"jsonrpc": "2.0", "method": "get_device_state", "code": 0, "result": result, "id": 0})
}

async fn backend(main: &FakeSeestar, wide: &FakeSeestar) -> SeestarBackend {
    let endpoints = SeestarEndpoints {
        control: main.control_addr,
        imaging: main.imaging_addr,
        wide_control: wide.control_addr,
        wide_imaging: wide.imaging_addr,
    };
    let backend = SeestarBackend::connect_with_endpoints(
        Ipv4Addr::LOCALHOST,
        endpoints,
        SeestarConfig::default(),
        Arc::new(UnifiedEventBus::new(16)),
    )
    .await
    .expect("backend");
    backend
        .client()
        .wait_for_connection(Duration::from_secs(2))
        .await
        .expect("main connection");
    backend
}

#[tokio::test]
async fn one_camera_device_has_no_second_camera() {
    let main = FakeSeestar::start().await;
    let wide = FakeSeestar::start().await;
    main.respond("get_device_state", device_state(false));
    let backend = backend(&main, &wide).await;

    assert!(backend.second_camera().await.expect("lookup").is_none());
    // No connection was opened to the wide ports.
    assert_eq!(wide.connection_count(), 0);
    backend.disconnect().await;
}

#[tokio::test]
async fn wide_camera_streams_from_its_own_ports() {
    let main = FakeSeestar::start().await;
    let wide = FakeSeestar::start().await;
    main.respond("get_device_state", device_state(true));
    // The wide process: a view that reports as SecondView, and a stream of
    // 16-bit 4x2 frames after the "done" reply to begin_streaming.
    wide.respond_ok(
        "get_view_state",
        json!({"SecondView": {"state": "working", "mode": "star", "cam_id": 1}}),
    );
    let pixels: Vec<u8> = (0..16).collect();
    wide.respond_image(
        "begin_streaming",
        vec![
            FakeFrame {
                data_type: 0,
                img_type: 0,
                width: 0,
                height: 0,
                can_debayer: 0,
                payload: b"done".to_vec(),
                request_id: None,
            },
            FakeFrame::image(2, 1, 4, 2, pixels.clone()),
            FakeFrame::image(2, 1, 4, 2, pixels.clone()),
        ],
    );
    let backend = backend(&main, &wide).await;

    let camera = backend
        .second_camera()
        .await
        .expect("lookup")
        .expect("a two-camera device has a second camera");
    camera.connect().await.expect("wide connection");
    assert!(camera.is_connected());

    // Capabilities are the wide sensor's, not the main camera's.
    let caps = camera.capabilities();
    assert_eq!((caps.max_width, caps.max_height), (Some(1920), Some(1080)));
    assert_eq!(caps.pixel_size_um, Some(2.7));
    let main_caps = backend.camera().capabilities();
    assert_eq!(
        (main_caps.max_width, main_caps.max_height),
        (Some(1080), Some(1920))
    );

    // get_image takes the next stream frame. Subscribe (inside get_image)
    // before the stream starts so the frame cannot be missed.
    let image = {
        let wait = tokio::spawn({
            let wide_client = Arc::clone(camera.client());
            async move {
                let mut frames = wide_client.subscribe_frames();
                loop {
                    let frame = frames.recv().await.expect("frame");
                    if frame.header.is_image() {
                        return frame;
                    }
                }
            }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let get = tokio::spawn({
            let camera = backend.second_camera().await.unwrap().unwrap();
            async move { camera.get_image().await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        camera
            .start_exposure(&ExposureSettings::default())
            .await
            .expect("start wide view");
        let _ = tokio::time::timeout(Duration::from_secs(5), wait)
            .await
            .expect("a stream frame arrived");
        tokio::time::timeout(Duration::from_secs(5), get)
            .await
            .expect("get_image returned")
            .unwrap()
            .expect("image")
    };
    assert_eq!((image.width, image.height), (4, 2));
    assert_eq!(image.format, PixelFormat::Bayer16(BayerPattern::Rggb));
    assert_eq!(image.expected_len(), Some(16));
    assert_eq!(&image.data[..], &pixels[..]);
    assert!(camera.is_exposing().await.expect("view state"));

    // The view was started on the wide process, never on the main one.
    let methods = |fake: &FakeSeestar| -> Vec<String> {
        fake.received()
            .iter()
            .map(|m| m["method"].as_str().unwrap_or("").to_string())
            .collect()
    };
    assert!(methods(&wide).contains(&"iscope_start_view".to_string()));
    assert!(!methods(&main).contains(&"iscope_start_view".to_string()));

    // A second lookup reuses the same connection.
    let _again = backend.second_camera().await.unwrap().unwrap();
    assert_eq!(wide.connection_count(), 1);

    camera.abort_exposure().await.expect("stop wide view");
    assert!(methods(&wide).contains(&"iscope_stop_view".to_string()));
    backend.disconnect().await;
}

#[tokio::test]
async fn devices_of_one_telescope_have_distinct_ids_under_the_backend_id() {
    use scopinator_types::DeviceKind;

    let main = FakeSeestar::start().await;
    let wide = FakeSeestar::start().await;
    main.respond("get_device_state", device_state(true));
    let backend = backend(&main, &wide).await;

    let mount = backend.mount();
    let camera = backend.camera();
    let wide_camera = backend
        .second_camera()
        .await
        .expect("lookup")
        .expect("second camera");

    assert_eq!(backend.device_id().as_str(), "seestar:127.0.0.1");
    assert_eq!(mount.id().as_str(), "seestar:127.0.0.1:mount");
    assert_eq!(camera.id().as_str(), "seestar:127.0.0.1:camera");
    assert_eq!(wide_camera.id().as_str(), "seestar:127.0.0.1:wide");

    assert_eq!(mount.info().kind, DeviceKind::Mount);
    assert_eq!(camera.info().kind, DeviceKind::Camera);
    assert_eq!(wide_camera.info().kind, DeviceKind::Camera);
    for info in [mount.info(), camera.info(), wide_camera.info()] {
        assert_eq!(info.model.as_deref(), Some("Seestar S30"));
        assert_eq!(info.firmware.as_deref(), Some("9.75"));
    }
    assert_eq!(mount.info().id, mount.id());
    backend.disconnect().await;
}

#[tokio::test]
async fn a_seestar_is_usable_as_a_neutral_backend() {
    use scopinator::Backend;

    let main = FakeSeestar::start().await;
    let wide = FakeSeestar::start().await;
    main.respond("get_device_state", device_state(true));
    let seestar: Arc<dyn Backend> = Arc::new(backend(&main, &wide).await);

    assert_eq!(seestar.kind(), "seestar");
    assert_eq!(seestar.id().as_str(), "seestar:127.0.0.1");
    assert_eq!(
        seestar.mount().expect("mount").id().as_str(),
        "seestar:127.0.0.1:mount"
    );
    // Main camera first, then the wide camera of a two-camera model.
    let cameras = seestar.cameras().await.expect("cameras");
    let ids: Vec<String> = cameras.iter().map(|c| c.id().to_string()).collect();
    assert_eq!(ids, ["seestar:127.0.0.1:camera", "seestar:127.0.0.1:wide"]);
    assert!(seestar.focuser().is_none());
    assert!(seestar.filter_wheel().is_none());

    // What only a Seestar has is one downcast away.
    let concrete = seestar
        .as_any()
        .downcast_ref::<SeestarBackend>()
        .expect("a SeestarBackend");
    assert_eq!(concrete.device_info().model.as_deref(), Some("Seestar S30"));

    seestar.disconnect().await.expect("disconnect");
}

#[tokio::test]
async fn a_one_camera_seestar_has_one_camera_as_a_backend() {
    use scopinator::Backend;

    let main = FakeSeestar::start().await;
    let wide = FakeSeestar::start().await;
    main.respond("get_device_state", device_state(false));
    let seestar: Arc<dyn Backend> = Arc::new(backend(&main, &wide).await);

    assert_eq!(seestar.cameras().await.expect("cameras").len(), 1);
    assert_eq!(wide.connection_count(), 0);
    seestar.disconnect().await.expect("disconnect");
}
