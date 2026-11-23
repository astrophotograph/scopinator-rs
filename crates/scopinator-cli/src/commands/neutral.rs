//! Commands written against the vendor-neutral traits: they work on any
//! backend `scopinator` has.

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use scopinator::backend::alpaca::{AlpacaBackend, AlpacaConfig};
use scopinator::{Backend, Mount, UnifiedEventBus, WaitOptions, wait_for_park, wait_for_slew};
use scopinator_types::{Coordinates, Epoch, SlewState};

/// Connect to the Alpaca server at `host:port`.
pub async fn connect_alpaca(host: Ipv4Addr, port: u16) -> Result<Arc<dyn Backend>> {
    println!("Connecting to the Alpaca server at {host}:{port}...");
    let backend = AlpacaBackend::connect(
        host,
        port,
        AlpacaConfig::default(),
        Arc::new(UnifiedEventBus::new(64)),
    )
    .await
    .with_context(|| format!("could not connect to the Alpaca server at {host}:{port}"))?;
    Ok(Arc::new(backend))
}

/// Run `command` on the backend, then disconnect whatever it returned.
async fn with_backend<F>(backend: Arc<dyn Backend>, command: F) -> Result<()>
where
    F: AsyncFnOnce(&dyn Backend) -> Result<()>,
{
    let result = command(backend.as_ref()).await;
    // Leave devices connected on the server: other clients may be using
    // them, and an Alpaca `Connected = false` disconnects them for everyone.
    drop(backend);
    result
}

fn mount_of(backend: &dyn Backend) -> Result<Arc<dyn Mount>> {
    backend
        .mount()
        .with_context(|| format!("{} has no mount", backend.id()))
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

/// Print what the instrument reports about itself and its devices.
pub async fn status(backend: Arc<dyn Backend>) -> Result<()> {
    with_backend(backend, async |backend| {
        println!("Instrument: {} ({})", backend.id(), backend.kind());

        if let Some(mount) = backend.mount() {
            let info = mount.info();
            println!("\nMount:      {}", info.name);
            if let Some(driver) = info.driver {
                println!("  Driver:   {driver}");
            }
            let status = mount.get_mount_status().await?;
            let state = match status.slew_state {
                SlewState::Parked => "parked",
                SlewState::Slewing => "slewing",
                SlewState::Tracking => "tracking",
                SlewState::Idle => "idle",
                SlewState::Homing => "homing",
                _ => "unknown",
            };
            println!("  State:    {state}");
            if let Some(position) = status.coordinates {
                println!("  Position: {position}");
                if position.epoch != Epoch::J2000 {
                    println!(
                        "            {}",
                        position.to_epoch(Epoch::J2000, chrono::Utc::now())
                    );
                }
            }
            if let Ok(horizontal) = mount.get_alt_az().await {
                println!(
                    "  Alt/Az:   {:.2} / {:.2} degrees",
                    horizontal.altitude, horizontal.azimuth
                );
            }
            println!("  Tracking: {}", yes_no(status.is_tracking));
            if let Ok(site) = mount.get_site().await {
                println!(
                    "  Site:     {:.4}, {:.4}",
                    site.latitude_deg(),
                    site.longitude_deg()
                );
            }
            if let Ok(clock) = mount.get_utc().await {
                println!("  Clock:    {}", clock.format("%Y-%m-%d %H:%M:%S UTC"));
            }
        } else {
            println!("\nMount:      none");
        }

        for camera in backend.cameras().await? {
            let info = camera.info();
            let caps = camera.capabilities();
            println!("\nCamera:     {}", info.name);
            if let (Some(width), Some(height)) = (caps.max_width, caps.max_height) {
                println!("  Sensor:   {width} x {height}");
            }
            if let Ok(status) = camera.get_camera_status().await {
                println!("  State:    {:?}", status.state);
                if let Some(temperature) = status.temperature {
                    println!("  Temp:     {temperature:.1} C");
                }
            }
        }

        if let Some(focuser) = backend.focuser() {
            println!("\nFocuser:    {}", focuser.info().name);
            if let Ok(position) = focuser.focuser_position().await {
                match position.max_position {
                    Some(max) => println!("  Position: {} / {max}", position.position),
                    None => println!("  Position: {}", position.position),
                }
            }
        }

        if let Some(wheel) = backend.filter_wheel() {
            println!("\nFilters:    {}", wheel.info().name);
            match wheel.filter_position().await {
                Ok(Some(position)) => println!(
                    "  Current:  {} ({})",
                    position.position,
                    position.name.as_deref().unwrap_or("unnamed")
                ),
                Ok(None) => println!("  Current:  turning"),
                Err(_) => {}
            }
        }
        Ok(())
    })
    .await
}

/// Slew to `target` and wait for the mount to arrive.
pub async fn goto(backend: Arc<dyn Backend>, target: Coordinates, timeout: Duration) -> Result<()> {
    with_backend(backend, async |backend| {
        let mount = mount_of(backend)?;
        if mount.is_parked().await.unwrap_or(false) {
            bail!("the mount is parked; run `scopinator unpark` first");
        }
        // Most mounts refuse a slew to equatorial coordinates unless they
        // are tracking.
        if mount.capabilities().can_track && !mount.is_tracking().await? {
            mount
                .set_tracking(true)
                .await
                .context("could not turn tracking on")?;
            println!("Tracking turned on.");
        }

        println!("Slewing to {target}...");
        mount
            .slew_to_coordinates(&target)
            .await
            .context("the mount refused the slew")?;
        wait_for_slew(mount.as_ref(), WaitOptions::with_timeout(timeout))
            .await
            .context("the slew did not finish")?;

        // Report the arrival in the system the target was given in, so the
        // two can be compared by eye.
        let arrived = mount
            .get_coordinates()
            .await?
            .to_epoch(target.epoch, chrono::Utc::now());
        println!("Arrived at {arrived}.");
        Ok(())
    })
    .await
}

/// Park the mount and wait for it to get there.
pub async fn park(backend: Arc<dyn Backend>, timeout: Duration) -> Result<()> {
    with_backend(backend, async |backend| {
        let mount = mount_of(backend)?;
        if mount.is_parked().await.unwrap_or(false) {
            println!("The mount is already parked.");
            return Ok(());
        }
        println!("Parking...");
        mount.park().await.context("the mount refused to park")?;
        wait_for_park(mount.as_ref(), WaitOptions::with_timeout(timeout))
            .await
            .context("the mount did not reach its park position")?;
        println!("Parked.");
        Ok(())
    })
    .await
}

/// Take the mount out of park.
pub async fn unpark(backend: Arc<dyn Backend>) -> Result<()> {
    with_backend(backend, async |backend| {
        let mount = mount_of(backend)?;
        mount.unpark().await.context("could not unpark the mount")?;
        println!("Unparked.");
        Ok(())
    })
    .await
}
