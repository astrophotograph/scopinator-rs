use std::net::Ipv4Addr;
use std::time::Duration;

use anyhow::{Context, Result};
use scopinator_seestar::{InteropKey, SeestarClient, SeestarConfig};
use scopinator_types::Coordinates;

pub async fn goto(
    host: Ipv4Addr,
    ra_hours: f64,
    dec_deg: f64,
    name: &str,
    interop_key: Option<InteropKey>,
) -> Result<()> {
    let coords = Coordinates::from_hours(ra_hours, dec_deg).context("invalid coordinates")?;

    println!("Connecting to {host}...");
    let client = SeestarClient::connect_with_config(
        host,
        SeestarConfig {
            interop_key,
            ..Default::default()
        },
    )
    .await?;
    client
        .wait_for_connection(Duration::from_secs(10))
        .await
        .context("could not connect to the telescope")?;

    println!("Slewing to {name} ({coords})...");

    let response = client
        .goto(name, coords.ra.as_hours(), coords.dec.as_degrees())
        .await?;
    if response.is_success() {
        println!("Goto command accepted. Telescope is slewing.");
    } else {
        println!(
            "Goto failed: {} (code {})",
            response.error.unwrap_or_default(),
            response.code
        );
    }

    client.shutdown().await;
    Ok(())
}
