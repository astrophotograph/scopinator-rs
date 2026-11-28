//! Print what an Alpaca server has, through the neutral traits. Nothing is
//! moved or set: the only thing written is `Connected` on each device.
//!
//! ```text
//! cargo run -p scopinator --example alpaca_info -- 192.168.x.x [PORT] [--exercise]
//! ```
//!
//! With `--exercise` it also switches each writable switch over and back,
//! turns the filter wheel one position on and back, moves the focuser 20
//! steps and back, and takes one one-second exposure. The mount is not
//! moved.
//!
//! The port defaults to 11111. A Seestar's own Alpaca server, once switched
//! on in the app, is on 32323.

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use scopinator::backend::alpaca::{AlpacaBackend, AlpacaConfig};
use scopinator::types::ExposureSettings;
use scopinator::{
    Backend, UnifiedEventBus, WaitOptions, wait_for_filter_wheel, wait_for_focuser, wait_for_image,
};

fn show<T: std::fmt::Debug, E: std::fmt::Display>(result: Result<T, E>) -> String {
    match result {
        Ok(value) => format!("{value:?}"),
        Err(e) => format!("ERR {e}"),
    }
}

fn patient() -> WaitOptions {
    WaitOptions::with_timeout(Duration::from_secs(60))
}

/// Use everything but the mount, and put it back.
async fn exercise_devices(backend: &AlpacaBackend) {
    use scopinator::Switch;

    for switch in backend.alpaca_switches() {
        for info in switch.switches().await.unwrap_or_default() {
            if !info.writable {
                continue;
            }
            let before = switch.get(info.index).await;
            let Ok(before) = before else {
                println!("exercise switch {}: {}", info.index, show(before));
                continue;
            };
            let other = if info.is_on(before) {
                info.min
            } else {
                info.max
            };
            println!(
                "exercise switch {} ({}): set {other}: {}, reads {}; set {before}: {}, reads {}",
                info.index,
                info.name,
                show(switch.set(info.index, other).await),
                show(switch.get(info.index).await),
                show(switch.set(info.index, before).await),
                show(switch.get(info.index).await),
            );
            println!(
                "  beyond the range: {}",
                show(switch.set(info.index, info.max + 1.0).await)
            );
        }
    }

    if let Some(wheel) = backend.filter_wheel() {
        let slots = wheel.capabilities().slot_count;
        if let (Ok(Some(start)), true) = (wheel.filter_position().await, slots > 1) {
            let next = (start.position + 1) % slots;
            for target in [next, start.position] {
                let began = Instant::now();
                println!(
                    "exercise wheel: to {target}: {}, wait: {}, at {} after {:.1}s",
                    show(wheel.set_filter_position(target).await),
                    show(wait_for_filter_wheel(wheel.as_ref(), patient()).await),
                    show(wheel.filter_position().await),
                    began.elapsed().as_secs_f64(),
                );
            }
            println!(
                "  beyond the last: {}",
                show(wheel.set_filter_position(slots).await)
            );
        }
    }

    if let Some(focuser) = backend.focuser()
        && let Ok(start) = focuser.focuser_position().await
    {
        for target in [start.position + 20, start.position] {
            let began = Instant::now();
            println!(
                "exercise focuser: to {target}: {}, wait: {}, at {} after {:.1}s",
                show(focuser.move_to(target).await),
                show(wait_for_focuser(focuser.as_ref(), patient()).await),
                show(focuser.focuser_position().await.map(|p| p.position)),
                began.elapsed().as_secs_f64(),
            );
        }
    }

    for camera in backend.cameras().await.unwrap_or_default() {
        let began = Instant::now();
        let image = async {
            camera.start_exposure(&ExposureSettings::default()).await?;
            wait_for_image(camera.as_ref(), patient()).await?;
            camera.get_image().await
        }
        .await;
        println!(
            "exercise camera: one second: {} after {:.1}s",
            show(image.map(|image| format!(
                "{}x{} {:?} {:?}, {} bytes",
                image.width,
                image.height,
                image.format,
                image.frame_kind,
                image.data.len()
            ))),
            began.elapsed().as_secs_f64(),
        );
        println!("  status after: {}", show(camera.get_camera_status().await));
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let host: Ipv4Addr = args
        .next()
        .ok_or("usage: alpaca_info <host> [port]")?
        .parse()?;
    let rest: Vec<String> = args.collect();
    let exercise = rest.iter().any(|arg| arg == "--exercise");
    let port: u16 = rest
        .iter()
        .find(|arg| !arg.starts_with("--"))
        .map_or(Ok(11111), |port| port.parse())?;

    let bus = Arc::new(UnifiedEventBus::new(64));
    let backend = AlpacaBackend::connect(host, port, AlpacaConfig::default(), bus).await?;
    println!("connected: {}", backend.id());

    if let Some(mount) = backend.mount() {
        println!("mount: {:?}", mount.info());
        println!("  capabilities: {:?}", mount.capabilities());
        println!("  status: {}", show(mount.get_mount_status().await));
        println!("  coordinates: {}", show(mount.get_coordinates().await));
        println!("  alt/az: {}", show(mount.get_alt_az().await));
        println!("  parked: {}", show(mount.is_parked().await));
        println!("  slewing: {}", show(mount.is_slewing().await));
        println!("  tracking: {}", show(mount.is_tracking().await));
        println!("  at home: {}", show(mount.is_at_home().await));
        println!("  alignment: {}", show(mount.alignment_mode().await));
        println!("  side of pier: {}", show(mount.side_of_pier().await));
        println!("  site: {}", show(mount.get_site().await));
        println!("  utc: {}", show(mount.get_utc().await));
    } else {
        println!("mount: none");
    }

    for camera in backend.cameras().await? {
        println!("camera: {:?}", camera.info());
        println!("  capabilities: {:?}", camera.capabilities());
        println!("  status: {}", show(camera.get_camera_status().await));
        println!("  exposing: {}", show(camera.is_exposing().await));
        println!("  image ready: {}", show(camera.image_ready().await));
    }

    if let Some(focuser) = backend.focuser() {
        println!("focuser: {:?}", focuser.info());
        println!("  capabilities: {:?}", focuser.capabilities());
        println!("  position: {}", show(focuser.focuser_position().await));
    } else {
        println!("focuser: none");
    }

    if let Some(wheel) = backend.filter_wheel() {
        println!("filter wheel: {:?}", wheel.info());
        println!("  capabilities: {:?}", wheel.capabilities());
        println!("  names: {}", show(wheel.get_filter_names().await));
        println!("  position: {}", show(wheel.filter_position().await));
    } else {
        println!("filter wheel: none");
    }

    for switch in backend.alpaca_switches() {
        use scopinator::{Device, Switch};
        println!("switch: {:?}", switch.info());
        for info in switch.switches().await? {
            println!(
                "  {info:?}\n    value: {}",
                show(switch.get(info.index).await)
            );
        }
    }

    if exercise {
        exercise_devices(&backend).await;
    }

    backend.disconnect().await?;
    println!("disconnected");
    Ok(())
}
