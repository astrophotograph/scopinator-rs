//! Stream a few frames from the wide camera of a two-camera Seestar.
//!
//! Starts the wide camera's own view (the mount is not commanded), prints
//! the frames it delivers, and stops the view again.
//!
//! ```text
//! SEESTAR_INTEROP_PEM=client.pem cargo run -p scopinator --example seestar_wide -- 192.168.x.x
//! ```

use std::sync::Arc;
use std::time::Duration;

use scopinator::backend::seestar::SeestarBackend;
use scopinator::{Camera, Device, Mount, UnifiedEventBus};
use scopinator_seestar::{InteropKey, SeestarConfig};
use scopinator_types::ExposureSettings;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let host = std::env::args()
        .nth(1)
        .ok_or("usage: seestar_wide <host>")?;
    let interop_key = match std::env::var("SEESTAR_INTEROP_PEM") {
        Ok(path) => Some(InteropKey::from_pem(&std::fs::read_to_string(path)?)?),
        Err(_) => None,
    };
    let config = SeestarConfig {
        interop_key,
        ..Default::default()
    };
    let bus = Arc::new(UnifiedEventBus::new(64));
    let mut events = bus.subscribe();
    let backend = SeestarBackend::connect_with_config(host.parse()?, config, bus).await?;
    backend
        .client()
        .wait_for_connection(Duration::from_secs(10))
        .await?;

    let Some(wide) = backend.second_camera().await? else {
        println!(
            "{} has no second camera",
            backend.device_info().model.unwrap_or_default()
        );
        backend.disconnect().await;
        return Ok(());
    };
    wide.connect().await?;
    println!("wide camera: {:?}", wide.capabilities());
    println!("verification: {:?}", wide.client().verification());
    let before = backend.mount().get_coordinates().await?;

    wide.start_exposure(&ExposureSettings::default()).await?;
    for n in 1..=3 {
        match wide.get_image().await {
            Ok(image) => println!(
                "frame {n}: {}x{} {}-bit bayer={:?} kind={:?} {} bytes",
                image.width,
                image.height,
                image.bit_depth,
                image.bayer_pattern,
                image.frame_kind,
                image.data.len()
            ),
            Err(e) => println!("frame {n}: {e}"),
        }
    }
    println!("exposing: {}", wide.is_exposing().await?);
    wide.abort_exposure().await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    println!("exposing after stop: {}", wide.is_exposing().await?);

    let after = backend.mount().get_coordinates().await?;
    println!("mount before: {before}");
    println!("mount after:  {after}");
    let mut seen = 0;
    while let Ok(event) = events.try_recv() {
        seen += 1;
        if seen <= 6 {
            println!("bus event: {:?}", event.event_type);
        }
    }
    println!("{seen} events on the bus");
    backend.disconnect().await;
    Ok(())
}
