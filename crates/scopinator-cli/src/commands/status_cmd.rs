use std::net::Ipv4Addr;
use std::time::Duration;

use anyhow::{Context, Result};
use scopinator_seestar::command::Command;
use scopinator_seestar::response::DeviceStateResult;
use scopinator_seestar::{InteropKey, SeestarClient, SeestarConfig};

pub async fn status(host: Ipv4Addr, interop_key: Option<InteropKey>) -> Result<()> {
    println!("Connecting to {host}...");

    let client = SeestarClient::connect_with_config(host, SeestarConfig { interop_key }).await?;
    client
        .wait_for_connection(Duration::from_secs(10))
        .await
        .context("timed out waiting for connection")?;

    println!("Connected. Fetching device state...\n");

    let response = client.send_command(Command::GetDeviceState).await?;
    let result = response.result.unwrap_or_default();

    let state: DeviceStateResult = serde_json::from_value(result)
        .context("failed to parse device state")?;

    // Device info
    if let Some(device) = &state.device {
        if let Some(name) = &device.name {
            println!("Device:    {name}");
        }
        if let Some(model) = &device.product_model {
            println!("Model:     {model}");
        }
        if let Some(sn) = &device.sn {
            println!("Serial:    {sn}");
        }
        if let Some(fw) = &device.firmware_ver_string {
            println!("Firmware:  {fw}");
        }
    }

    // Pi status
    if let Some(pi) = &state.pi_status {
        if let Some(temp) = pi.temp {
            println!("Temp:      {temp:.1}C");
        }
        if let Some(batt) = pi.battery_capacity {
            println!("Battery:   {batt}%");
        }
        if let Some(charger) = &pi.charger_status {
            println!("Charger:   {charger}");
        }
    }

    // Mount
    if let Some(mount) = &state.mount
        && let Some(tracking) = mount.tracking
    {
        println!("Tracking:  {}", if tracking { "on" } else { "off" });
    }

    // Focuser
    if let Some(focuser) = &state.focuser
        && let Some(step) = focuser.step
    {
        let max = focuser.max_step.unwrap_or(0);
        println!("Focuser:   {step}/{max}");
    }

    client.shutdown().await;
    Ok(())
}
