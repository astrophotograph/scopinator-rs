use std::net::Ipv4Addr;
use std::time::Duration;

use anyhow::{Context, Result};
use scopinator_seestar::command::Command;
use scopinator_seestar::command::params::GotoTargetParams;
use scopinator_seestar::{InteropKey, SeestarClient, SeestarConfig};
use scopinator_types::Coordinates;

pub async fn goto(host: Ipv4Addr, ra_hours: f64, dec_deg: f64, name: &str, interop_key: Option<InteropKey>) -> Result<()> {
    let coords = Coordinates::from_hours(ra_hours, dec_deg)
        .context("invalid coordinates")?;

    println!("Connecting to {host}...");
    let client = SeestarClient::connect_with_config(host, SeestarConfig { interop_key }).await?;
    client
        .wait_for_connection(Duration::from_secs(10))
        .await
        .context("timed out waiting for connection")?;

    println!("Slewing to {name} ({coords})...");

    let cmd = Command::GotoTarget(GotoTargetParams {
        target_name: name.to_string(),
        is_j2000: true,
        ra: coords.ra.as_degrees(),
        dec: coords.dec.as_degrees(),
    });

    let response = client.send_command(cmd).await?;
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
