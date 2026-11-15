//! Connect to a Seestar through the high-level backend and print what it
//! reports about itself and the capabilities derived from that. Read-only.
//!
//! ```text
//! cargo run -p scopinator --example seestar_info -- 192.168.x.x
//! SEESTAR_INTEROP_PEM=client.pem cargo run -p scopinator --example seestar_info -- 192.168.x.x
//! ```
//!
//! With `SEESTAR_SYNC_CLOCK=1` it is no longer read-only: the telescope's
//! clock is set from this computer at connect.

use std::sync::Arc;
use std::time::Duration;

use scopinator::backend::seestar::SeestarBackend;
use scopinator::{Camera, Mount, UnifiedEventBus};
use scopinator_seestar::{ClockSync, InteropKey, SeestarConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let host = std::env::args()
        .nth(1)
        .ok_or("usage: seestar_info <host>")?;
    let interop_key = match std::env::var("SEESTAR_INTEROP_PEM") {
        Ok(path) => Some(InteropKey::from_pem(&std::fs::read_to_string(path)?)?),
        Err(_) => None,
    };
    // SEESTAR_SYNC_CLOCK=1 sets the telescope's clock (UTC, zone "UTC") at connect.
    let clock_sync = if std::env::var_os("SEESTAR_SYNC_CLOCK").is_some() {
        ClockSync::Utc
    } else {
        ClockSync::Off
    };
    let config = SeestarConfig {
        interop_key,
        clock_sync,
        ..Default::default()
    };
    let bus = Arc::new(UnifiedEventBus::new(64));
    let backend = SeestarBackend::connect_with_config(host.parse()?, config, bus).await?;
    let client = backend.client();
    client.wait_for_connection(Duration::from_secs(10)).await?;
    println!("verification: {:?}", client.verification());

    // The device state is requested at connect; give the reply a moment.
    for _ in 0..50 {
        if backend.device_info().chip_size.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    println!("device: {:#?}", backend.device_info());
    println!("camera capabilities: {:?}", backend.camera().capabilities());
    println!(
        "mount coordinates: {}",
        backend.mount().get_coordinates().await?
    );

    backend.disconnect().await;
    Ok(())
}
