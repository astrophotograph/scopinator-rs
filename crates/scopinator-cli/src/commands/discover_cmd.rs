use std::time::Duration;

use anyhow::Result;
use scopinator_seestar::protocol::discovery;

pub async fn discover(timeout: Duration) -> Result<()> {
    println!("Searching for Seestar telescopes...");

    let devices = discovery::discover(timeout).await?;

    if devices.is_empty() {
        println!("No telescopes found.");
    } else {
        println!("Found {} telescope(s):\n", devices.len());
        for device in &devices {
            println!("  Address: {}", device.address);
            if let Some(model) = &device.product_model {
                println!("  Model:   {model}");
            }
            if let Some(sn) = &device.serial_number {
                println!("  Serial:  {sn}");
            }
            println!();
        }
    }

    Ok(())
}
