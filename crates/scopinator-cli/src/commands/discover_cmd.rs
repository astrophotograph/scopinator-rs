use std::time::Duration;

use anyhow::Result;

/// Look for every kind of telescope this tool can talk to.
pub async fn discover(timeout: Duration) -> Result<()> {
    println!("Searching for telescopes...");

    let found = scopinator::discovery::discover_all(timeout).await;

    if found.is_empty() {
        println!("No telescopes found.");
    } else {
        println!("Found {} telescope(s):\n", found.len());
        for device in &found {
            // The address in the form `--host` takes.
            match device.backend_kind {
                "seestar" => println!("  Address: seestar://{}", device.address.ip()),
                kind => println!("  Address: {kind}://{}", device.address),
            }
            if let Some(name) = &device.name {
                println!("  Name:    {name}");
            }
            if let Some(model) = &device.model {
                println!("  Model:   {model}");
            }
            if let Some(serial) = &device.serial {
                println!("  Serial:  {serial}");
            }
            println!();
        }
    }

    Ok(())
}
