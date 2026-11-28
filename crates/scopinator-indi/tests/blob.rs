//! BLOBs from a scripted server, and a FITS file through the whole path.

mod common;

use std::time::Duration;

use common::{FakeIndi, describe};
use scopinator_indi::protocol::message::{IndiMessage, MemberValue};
use scopinator_indi::protocol::xml::BlobMode;
use scopinator_indi::{IndiClient, IndiClientConfig, IndiError, fits};

const CCD1: &str = r#"<defBLOBVector device="Camera" name="CCD1" state="Idle" perm="ro"><defBLOB name="CCD1" label="Image"/></defBLOBVector>"#;

/// Standard base64, wrapped at 72 columns as `indiserver` wraps it.
fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (index, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if index <= chunk.len() {
                out.push(ALPHABET[((n >> shift) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out.as_bytes()
        .chunks(72)
        .map(|line| std::str::from_utf8(line).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

fn blob_message(device: &str, format: &str, data: &[u8]) -> String {
    format!(
        "<setBLOBVector device=\"{device}\" name=\"CCD1\" state=\"Ok\">\n<oneBLOB name=\"CCD1\" size=\"{}\" format=\"{format}\">\n{}\n</oneBLOB>\n</setBLOBVector>\n",
        data.len(),
        base64(data)
    )
}

/// A 4x3 sixteen-bit Bayer FITS file whose pixel values are `values`.
fn fits_file(values: &[u16; 12]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (keyword, value) in [
        ("SIMPLE", "T"),
        ("BITPIX", "16"),
        ("NAXIS", "2"),
        ("NAXIS1", "4"),
        ("NAXIS2", "3"),
        ("BZERO", "32768"),
        ("BSCALE", "1"),
        ("BAYERPAT", "'RGGB    '"),
        ("EXPTIME", "1.5"),
    ] {
        let card = format!("{keyword:<8}= {value:>20}");
        bytes.extend_from_slice(format!("{card:<80}").as_bytes());
    }
    bytes.extend_from_slice(format!("{:<80}", "END").as_bytes());
    bytes.resize(2880, b' ');
    for value in values {
        bytes.extend_from_slice(&((i32::from(*value) - 32768) as i16).to_be_bytes());
    }
    bytes.resize(5760, 0);
    bytes
}

fn quick() -> IndiClientConfig {
    IndiClientConfig::default()
        .with_reconnect_delay(Duration::from_millis(30), Duration::from_millis(100))
}

async fn camera() -> (FakeIndi, IndiClient) {
    let fake = FakeIndi::start().await;
    fake.define(CCD1);
    let client = IndiClient::connect(fake.addr, quick())
        .await
        .expect("connect");
    client
        .wait_for("Camera", "CCD1", Duration::from_secs(2), |_| true)
        .await
        .expect("definitions");
    (fake, client)
}

async fn eventually(check: impl Fn() -> bool) {
    for _ in 0..150 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("it did not happen within three seconds");
}

#[tokio::test]
async fn blobs_are_asked_for_and_a_frame_comes_through_as_an_image() {
    let (fake, client) = camera().await;
    client.enable_blobs("Camera", BlobMode::Also).await.unwrap();
    eventually(|| fake.count("enableBLOB") == 1).await;
    let asked = fake
        .received()
        .into_iter()
        .find(|element| element.starts_with("<enableBLOB"))
        .unwrap();
    assert_eq!(
        describe(&asked).1.get("device").map(String::as_str),
        Some("Camera")
    );
    assert!(asked.contains(">Also</enableBLOB>"), "{asked}");

    let values = [
        0u16, 1, 255, 256, 32767, 32768, 40000, 65535, 10, 20, 30, 40,
    ];
    let file = fits_file(&values);
    let mut messages = client.subscribe();
    let mut listener = client.blob_listener();
    // In pieces, as a frame arrives.
    fake.send_in_chunks(&blob_message("Camera", ".fits", &file), 1000);

    let blob = listener
        .next("Camera", "CCD1", Duration::from_secs(3))
        .await
        .expect("the frame");
    assert_eq!(blob.format, ".fits");
    assert_eq!(blob.data, file);

    let image = fits::decode(&blob.data).unwrap();
    assert_eq!(
        (image.width, image.height, image.bits_per_pixel),
        (4, 3, 16)
    );
    assert_eq!(image.bayer.as_deref(), Some("RGGB"));
    assert_eq!(image.exposure_s, Some(1.5));
    let pixels: Vec<u16> = image
        .pixels
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    assert_eq!(pixels, values);

    // The bytes went to the listener and nowhere else: the message and the
    // stored property have the format and an empty body.
    loop {
        if let IndiMessage::Update(update) = messages.recv().await.unwrap() {
            match &update.members[0].1 {
                MemberValue::Blob(shell) => {
                    assert_eq!(shell.format, ".fits");
                    assert!(shell.data.is_empty());
                }
                other => panic!("{other:?}"),
            }
            break;
        }
    }
    match client.property("Camera", "CCD1").unwrap().member("CCD1") {
        Some(MemberValue::Blob(shell)) => assert!(shell.data.is_empty()),
        other => panic!("{other:?}"),
    }
    client.shutdown().await;
}

#[tokio::test]
async fn a_listener_only_takes_the_blob_it_was_asked_for() {
    let (fake, client) = camera().await;
    let mut listener = client.blob_listener();
    fake.send(&blob_message("Guider", ".fits", b"not ours"));
    fake.send(&blob_message("Camera", ".fits", b"ours"));
    let blob = listener
        .next("Camera", "CCD1", Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(blob.data, b"ours");
    client.shutdown().await;
}

#[tokio::test]
async fn a_blob_that_arrived_before_the_listener_was_made_is_not_seen() {
    let (fake, client) = camera().await;
    let mut early = client.blob_listener();
    fake.send(&blob_message("Camera", ".fits", b"early"));
    assert_eq!(
        early
            .next("Camera", "CCD1", Duration::from_secs(2))
            .await
            .unwrap()
            .data,
        b"early"
    );
    // Asked for afterwards: it is gone, and waiting times out.
    let outcome = client
        .next_blob("Camera", "CCD1", Duration::from_millis(150))
        .await;
    assert!(matches!(outcome, Err(IndiError::Timeout)), "{outcome:?}");
    client.shutdown().await;
}

#[tokio::test]
async fn blobs_are_asked_for_again_after_a_reconnect() {
    let (fake, client) = camera().await;
    client.enable_blobs("Camera", BlobMode::Also).await.unwrap();
    client.enable_blobs("Guider", BlobMode::Only).await.unwrap();
    eventually(|| fake.count("enableBLOB") == 2).await;

    fake.drop_connections();
    eventually(|| fake.connection_count() == 2 && fake.count("enableBLOB") == 4).await;
    let again: Vec<String> = fake
        .received()
        .into_iter()
        .filter(|element| element.starts_with("<enableBLOB"))
        .skip(2)
        .collect();
    assert!(again[0].contains("device=\"Camera\"") && again[0].contains(">Also<"));
    assert!(again[1].contains("device=\"Guider\"") && again[1].contains(">Only<"));

    // And frames flow on the new connection.
    eventually(|| client.is_connected() && client.property("Camera", "CCD1").is_some()).await;
    let mut listener = client.blob_listener();
    fake.send(&blob_message("Camera", ".fits", b"after"));
    assert_eq!(
        listener
            .next("Camera", "CCD1", Duration::from_secs(2))
            .await
            .unwrap()
            .data,
        b"after"
    );
    client.shutdown().await;
}
