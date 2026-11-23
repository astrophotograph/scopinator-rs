//! What `SeestarCamera::get_image` makes of the frames a telescope sends:
//! the pixel format comes from the frame header and the device state.
#![cfg(feature = "seestar")]

#[path = "../../scopinator-seestar/tests/common/mod.rs"]
mod common;

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use common::{FakeFrame, FakeSeestar};
use scopinator::backend::seestar::{SeestarBackend, SeestarEndpoints};
use scopinator::{Camera, ScopinatorError, UnifiedEventBus};
use scopinator_seestar::SeestarConfig;
use scopinator_types::{BayerPattern, FrameMetadata, PixelFormat};
use serde_json::json;

/// A backend whose main camera answers `get_stacked_img` with `frame`.
async fn backend_serving(fake: &FakeSeestar, frame: FakeFrame) -> SeestarBackend {
    fake.respond_ok(
        "get_device_state",
        json!({
            "device": {"name": "fake", "firmware_ver_int": 2670, "product_model": "Seestar S50"},
            "camera": {"chip_size": [1080, 1920], "debayer_pattern": "GR"},
        }),
    );
    fake.respond_image("get_stacked_img", vec![frame]);
    let endpoints = SeestarEndpoints {
        control: fake.control_addr,
        imaging: fake.imaging_addr,
        wide_control: fake.control_addr,
        wide_imaging: fake.imaging_addr,
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
        .expect("connection");
    // The Bayer pattern comes from the device state; wait for it.
    for _ in 0..100 {
        if backend.device_info().debayer_pattern.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    backend
}

/// A frame with the debayer flag as given (`FakeFrame::image` sets it).
fn frame(img_type: u8, width: u16, height: u16, can_debayer: u16, payload: Vec<u8>) -> FakeFrame {
    FakeFrame {
        can_debayer,
        ..FakeFrame::image(2, img_type, width, height, payload)
    }
}

#[tokio::test]
async fn a_16_bit_frame_with_the_debayer_flag_is_a_bayer_mosaic() {
    let fake = FakeSeestar::start().await;
    let pixels: Vec<u8> = (0..16).collect();
    let backend = backend_serving(&fake, frame(1, 4, 2, 1, pixels.clone())).await;

    let image = backend.camera().get_image().await.unwrap();
    // Not "colour": one sample per pixel, to be debayered, pattern from the
    // device state ("GR").
    assert_eq!(image.format, PixelFormat::Bayer16(BayerPattern::Grbg));
    assert_eq!((image.width, image.height), (4, 2));
    assert_eq!(&image.data[..], &pixels[..]);
    assert_eq!(image.metadata, FrameMetadata::default());
    backend.disconnect().await;
}

#[tokio::test]
async fn frames_without_the_debayer_flag_are_mono_and_three_samples_are_rgb() {
    for (img_type, bytes_per_pixel, expected) in [
        (0u8, 1usize, PixelFormat::Mono8),
        (1, 2, PixelFormat::Mono16),
        (2, 3, PixelFormat::Rgb8),
        (5, 6, PixelFormat::Rgb16),
    ] {
        let fake = FakeSeestar::start().await;
        let payload = vec![7u8; 4 * 2 * bytes_per_pixel];
        let backend = backend_serving(&fake, frame(img_type, 4, 2, 0, payload)).await;
        let image = backend.camera().get_image().await.unwrap();
        assert_eq!(image.format, expected, "imgType {img_type}");
        assert!(image.has_expected_len());
        backend.disconnect().await;
    }
}

#[tokio::test]
async fn an_8_bit_frame_with_the_debayer_flag_is_an_8_bit_mosaic() {
    let fake = FakeSeestar::start().await;
    let backend = backend_serving(&fake, frame(0, 4, 2, 1, vec![1; 8])).await;
    let image = backend.camera().get_image().await.unwrap();
    assert_eq!(image.format, PixelFormat::Bayer8(BayerPattern::Grbg));
    backend.disconnect().await;
}

#[tokio::test]
async fn a_jpeg_frame_is_returned_as_a_jpeg() {
    let fake = FakeSeestar::start().await;
    // imgType 4 is JPEG whatever the data type; the payload is the file.
    let jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 1, 2, 3, 0xff, 0xd9];
    let backend = backend_serving(&fake, frame(4, 1080, 1920, 0, jpeg.clone())).await;

    let image = backend.camera().get_image().await.unwrap();
    assert_eq!(image.format, PixelFormat::Jpeg);
    assert_eq!(&image.data[..], &jpeg[..]);
    assert_eq!(image.expected_len(), None);
    backend.disconnect().await;
}

#[tokio::test]
async fn a_frame_whose_size_does_not_match_its_header_is_an_error() {
    let fake = FakeSeestar::start().await;
    // 4x2 at two bytes per pixel is 16 bytes; this has 15.
    let backend = backend_serving(&fake, frame(1, 4, 2, 1, vec![0; 15])).await;
    let err = backend.camera().get_image().await.unwrap_err();
    assert!(matches!(err, ScopinatorError::Backend(_)), "{err:?}");
    assert!(err.to_string().contains("15 bytes"), "{err}");
    backend.disconnect().await;
}
