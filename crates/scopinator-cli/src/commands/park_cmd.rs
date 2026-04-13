use std::net::Ipv4Addr;
use std::time::Duration;

use anyhow::{Context, Result};
use scopinator_seestar::command::Command;
use scopinator_seestar::SeestarClient;

pub async fn park(host: Ipv4Addr) -> Result<()> {
    println!("Connecting to {host}...");
    let client = SeestarClient::connect(host).await?;
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
