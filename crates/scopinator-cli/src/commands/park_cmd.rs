use std::net::Ipv4Addr;
use std::time::Duration;

use anyhow::{Context, Result};
use scopinator_seestar::command::Command;
use scopinator_seestar::{InteropKey, SeestarClient, SeestarConfig};

pub async fn park(host: Ipv4Addr, interop_key: Option<InteropKey>) -> Result<()> {
    println!("Connecting to {host}...");
    let client = SeestarClient::connect_with_config(host, SeestarConfig { interop_key }).await?;
    client
        .wait_for_connection(Duration::from_secs(10))
        .await
        .context("timed out waiting for connection")?;

    println!("Parking telescope...");

    let response = client.send_command(Command::ScopePark).await?;
    if response.is_success() {
        println!("Park command accepted.");
    } else {
        println!(
            "Park failed: {} (code {})",
            response.error.unwrap_or_default(),
            response.code
        );
    }

    client.shutdown().await;
    Ok(())
}
