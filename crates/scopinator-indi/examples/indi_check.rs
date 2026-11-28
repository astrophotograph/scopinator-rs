//! Drive an INDI server's devices with the client, to see that the client
//! and a real driver understand each other. Meant for the simulators
//! (`indiserver indi_simulator_telescope indi_simulator_ccd
//! indi_simulator_focus indi_simulator_wheel`): **it moves whatever mount,
//! focuser and filter wheel the server has.**
//!
//! ```text
//! cargo run -p scopinator-indi --example indi_check -- 127.0.0.1:7624
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use scopinator_indi::protocol::property::{PropertyState, SwitchState, interface};
use scopinator_indi::protocol::xml::BlobMode;
use scopinator_indi::standard::{ccd, filter_wheel, focuser, general, telescope};
use scopinator_indi::{IndiClient, IndiClientConfig, IndiDevice, IndiError, fits};

fn show<T: std::fmt::Debug>(result: Result<T, IndiError>) -> String {
    match result {
        Ok(value) => format!("{value:?}"),
        Err(e) => format!("ERR {e}"),
    }
}

const WAIT: Duration = Duration::from_secs(60);

/// Wait until `property` has been busy and is busy no longer, or was never
/// seen busy within two seconds and is not now.
async fn settle(device: &IndiDevice, property: &str) -> Result<PropertyState, IndiError> {
    let client = device.client();
    let started = Instant::now();
    let _ = client
        .wait_for(device.name(), property, Duration::from_secs(2), |p| {
            p.state == PropertyState::Busy
        })
        .await;
    let seen_busy = started.elapsed() < Duration::from_secs(2);
    let done = client
        .wait_for(device.name(), property, WAIT, |p| {
            p.state != PropertyState::Busy
        })
        .await?;
    println!(
        "    {property}: {} -> {:?} after {:.1}s{}",
        if seen_busy { "Busy" } else { "(never busy)" },
        done.state,
        started.elapsed().as_secs_f64(),
        done.message
            .as_deref()
            .map(|m| format!(" ({m})"))
            .unwrap_or_default()
    );
    Ok(done.state)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:7624".into())
        .parse()?;
    let client = Arc::new(IndiClient::connect(addr, IndiClientConfig::default()).await?);
    // Let the drivers define themselves.
    tokio::time::sleep(Duration::from_secs(2)).await;
    println!("devices: {:?}", client.devices());
    for (name, flag) in [
        ("telescope", interface::TELESCOPE),
        ("ccd", interface::CCD),
        ("focuser", interface::FOCUSER),
        ("filter", interface::FILTER),
        ("guider", interface::GUIDER),
    ] {
        println!("  {name}: {:?}", client.devices_with(flag));
    }

    for name in client.devices() {
        let device = IndiDevice::new(Arc::clone(&client), &name);
        println!(
            "{name}: driver {:?}, interfaces {:?}, {} properties; connect: {}",
            device.driver(),
            device.interfaces(),
            client.properties(&name).len(),
            show(device.connect(Duration::from_secs(10)).await)
        );
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    for name in client.devices() {
        println!(
            "  {name}: {} properties once connected",
            client.properties(&name).len()
        );
    }

    if let Some(name) = client.devices_with(interface::TELESCOPE).first() {
        let mount = IndiDevice::new(Arc::clone(&client), name);
        println!("mount {name}:");
        let coordinates = |mount: &IndiDevice| {
            (
                mount.number(telescope::EQUATORIAL_EOD_COORD, telescope::RA),
                mount.number(telescope::EQUATORIAL_EOD_COORD, telescope::DEC),
                mount.state(telescope::EQUATORIAL_EOD_COORD),
            )
        };
        println!("    at {:?}", coordinates(&mount));
        println!(
            "    parked={:?} tracking={:?} pier={:?} site=({:?}, {:?}) utc={:?}",
            mount.switch(telescope::TELESCOPE_PARK, telescope::PARK),
            mount.switch(telescope::TELESCOPE_TRACK_STATE, telescope::TRACK_ON),
            mount
                .property(telescope::TELESCOPE_PIER_SIDE)
                .and_then(|p| p.selected().map(str::to_string)),
            mount.number(telescope::GEOGRAPHIC_COORD, telescope::LAT),
            mount.number(telescope::GEOGRAPHIC_COORD, telescope::LONG),
            mount.text(telescope::TIME_UTC, telescope::UTC),
        );
        if mount.switch(telescope::TELESCOPE_PARK, telescope::PARK) == Some(SwitchState::On) {
            println!(
                "    unpark: {}",
                show(
                    client
                        .set_switches(
                            name,
                            telescope::TELESCOPE_PARK,
                            &[(telescope::UNPARK, SwitchState::On)]
                        )
                        .await
                )
            );
            settle(&mount, telescope::TELESCOPE_PARK).await?;
        }
        let (ra, dec, _) = coordinates(&mount);
        let target = (
            (ra.unwrap_or(0.0) + 1.0) % 24.0,
            dec.unwrap_or(0.0).clamp(-60.0, 60.0) - 10.0,
        );
        println!(
            "    ON_COORD_SET TRACK: {}",
            show(
                client
                    .set_switches(
                        name,
                        telescope::ON_COORD_SET,
                        &[(telescope::TRACK, SwitchState::On)]
                    )
                    .await
            )
        );
        println!(
            "    slew to {target:?}: {}",
            show(
                client
                    .set_numbers(
                        name,
                        telescope::EQUATORIAL_EOD_COORD,
                        &[(telescope::RA, target.0), (telescope::DEC, target.1)]
                    )
                    .await
            )
        );
        settle(&mount, telescope::EQUATORIAL_EOD_COORD).await?;
        println!(
            "    at {:?}, tracking={:?}",
            coordinates(&mount),
            mount.switch(telescope::TELESCOPE_TRACK_STATE, telescope::TRACK_ON)
        );

        // Which way do the motion switches go?
        for (property, member, what) in [
            (
                telescope::TELESCOPE_MOTION_WE,
                telescope::MOTION_EAST,
                "east",
            ),
            (
                telescope::TELESCOPE_MOTION_NS,
                telescope::MOTION_NORTH,
                "north",
            ),
        ] {
            let before = coordinates(&mount);
            println!(
                "    {what} on: {}",
                show(
                    client
                        .set_switches(name, property, &[(member, SwitchState::On)])
                        .await
                )
            );
            tokio::time::sleep(Duration::from_secs(2)).await;
            let other = if member == telescope::MOTION_EAST {
                telescope::MOTION_WEST
            } else {
                telescope::MOTION_SOUTH
            };
            println!(
                "    {what} off: {}",
                show(
                    client
                        .set_switches(
                            name,
                            property,
                            &[(member, SwitchState::Off), (other, SwitchState::Off)]
                        )
                        .await
                )
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
            let after = coordinates(&mount);
            println!(
                "    moving {what}: RA {:+.5} h, Dec {:+.5} deg",
                after.0.unwrap_or(0.0) - before.0.unwrap_or(0.0),
                after.1.unwrap_or(0.0) - before.1.unwrap_or(0.0)
            );
        }
    }

    if let Some(name) = client.devices_with(interface::CCD).first() {
        let camera = IndiDevice::new(Arc::clone(&client), name);
        println!("camera {name}:");
        println!(
            "    sensor {:?}x{:?}, {:?} bits, pixel {:?} um, gain {:?}, temperature {:?}",
            camera.number(ccd::CCD_INFO, ccd::CCD_MAX_X),
            camera.number(ccd::CCD_INFO, ccd::CCD_MAX_Y),
            camera.number(ccd::CCD_INFO, ccd::CCD_BITSPERPIXEL),
            camera.number(ccd::CCD_INFO, ccd::CCD_PIXEL_SIZE_X),
            camera.number(ccd::CCD_GAIN, ccd::GAIN),
            camera.number(ccd::CCD_TEMPERATURE, ccd::CCD_TEMPERATURE_VALUE),
        );
        println!(
            "    enable BLOBs: {}",
            show(client.enable_blobs(name, BlobMode::Also).await)
        );
        for (label, binning, frame) in [
            ("whole sensor", 1.0, None),
            (
                "binned 2x2 subframe",
                2.0,
                Some((100.0, 200.0, 400.0, 300.0)),
            ),
        ] {
            println!(
                "    binning {binning}: {}",
                show(
                    client
                        .set_numbers(
                            name,
                            ccd::CCD_BINNING,
                            &[(ccd::HOR_BIN, binning), (ccd::VER_BIN, binning)]
                        )
                        .await
                )
            );
            if let Some((x, y, w, h)) = frame {
                println!(
                    "    frame: {}",
                    show(
                        client
                            .set_numbers(
                                name,
                                ccd::CCD_FRAME,
                                &[(ccd::X, x), (ccd::Y, y), (ccd::WIDTH, w), (ccd::HEIGHT, h)]
                            )
                            .await
                    )
                );
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            let mut listener = client.blob_listener();
            let started = Instant::now();
            println!(
                "    expose 1 s ({label}): {}",
                show(
                    client
                        .set_numbers(name, ccd::CCD_EXPOSURE, &[(ccd::CCD_EXPOSURE_VALUE, 1.0)])
                        .await
                )
            );
            match listener.next(name, ccd::CCD1, WAIT).await {
                Ok(blob) => {
                    println!(
                        "    BLOB {} of {} bytes after {:.1}s; exposure state {:?}",
                        blob.format,
                        blob.data.len(),
                        started.elapsed().as_secs_f64(),
                        camera.state(ccd::CCD_EXPOSURE)
                    );
                    match fits::decode(&blob.data) {
                        Ok(image) => println!(
                            "    FITS {}x{} {} bits {} plane(s) bayer {:?} exposure {:?} gain {:?} temp {:?}; {} pixel bytes, max sample {}",
                            image.width,
                            image.height,
                            image.bits_per_pixel,
                            image.planes,
                            image.bayer,
                            image.exposure_s,
                            image.gain,
                            image.temperature_c,
                            image.pixels.len(),
                            image
                                .pixels
                                .as_chunks::<2>()
                                .0
                                .iter()
                                .map(|p| u16::from_le_bytes(*p))
                                .max()
                                .unwrap_or(0)
                        ),
                        Err(e) => println!("    FITS ERR {e}"),
                    }
                }
                Err(e) => println!("    BLOB ERR {e}"),
            }
        }
    }

    if let Some(name) = client.devices_with(interface::FOCUSER).first() {
        let device = IndiDevice::new(Arc::clone(&client), name);
        println!("focuser {name}:");
        let position = || {
            device.number(
                focuser::ABS_FOCUS_POSITION,
                focuser::FOCUS_ABSOLUTE_POSITION,
            )
        };
        println!(
            "    at {:?}, max {:?}, temperature {:?}, abort defined: {}",
            position(),
            device.number(focuser::FOCUS_MAX, focuser::FOCUS_MAX_VALUE),
            device.number(focuser::FOCUS_TEMPERATURE, focuser::TEMPERATURE),
            device.has(focuser::FOCUS_ABORT_MOTION)
        );
        let target = position().unwrap_or(0.0) + 500.0;
        println!(
            "    move to {target}: {}",
            show(
                client
                    .set_numbers(
                        name,
                        focuser::ABS_FOCUS_POSITION,
                        &[(focuser::FOCUS_ABSOLUTE_POSITION, target)]
                    )
                    .await
            )
        );
        settle(&device, focuser::ABS_FOCUS_POSITION).await?;
        println!("    at {:?}", position());
        println!(
            "    inward: {}, 200 steps: {}",
            show(
                client
                    .set_switches(
                        name,
                        focuser::FOCUS_MOTION,
                        &[(focuser::FOCUS_INWARD, SwitchState::On)]
                    )
                    .await
            ),
            show(
                client
                    .set_numbers(
                        name,
                        focuser::REL_FOCUS_POSITION,
                        &[(focuser::FOCUS_RELATIVE_POSITION, 200.0)]
                    )
                    .await
            )
        );
        settle(&device, focuser::REL_FOCUS_POSITION).await?;
        println!("    at {:?}", position());
    }

    if let Some(name) = client.devices_with(interface::FILTER).first() {
        let wheel = IndiDevice::new(Arc::clone(&client), name);
        println!("filter wheel {name}:");
        let slot = || wheel.number(filter_wheel::FILTER_SLOT, filter_wheel::FILTER_SLOT_VALUE);
        let names: Vec<String> = wheel
            .property(filter_wheel::FILTER_NAME)
            .map(|p| {
                p.members
                    .iter()
                    .filter_map(|m| p.text(&m.name).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let limits = wheel
            .property(filter_wheel::FILTER_SLOT)
            .and_then(|p| p.members.first().and_then(|m| m.limits.clone()));
        println!("    slot {:?}, limits {limits:?}, names {names:?}", slot());
        let target = if slot() == Some(3.0) { 2.0 } else { 3.0 };
        println!(
            "    to slot {target}: {}",
            show(
                client
                    .set_numbers(
                        name,
                        filter_wheel::FILTER_SLOT,
                        &[(filter_wheel::FILTER_SLOT_VALUE, target)]
                    )
                    .await
            )
        );
        settle(&wheel, filter_wheel::FILTER_SLOT).await?;
        println!("    slot {:?}", slot());
    }

    for name in client.devices() {
        let device = IndiDevice::new(Arc::clone(&client), &name);
        println!(
            "{name}: disconnect: {}; connected now: {:?}",
            show(device.disconnect(Duration::from_secs(10)).await),
            device.switch(general::CONNECTION, general::CONNECT)
        );
    }
    client.shutdown().await;
    Ok(())
}
