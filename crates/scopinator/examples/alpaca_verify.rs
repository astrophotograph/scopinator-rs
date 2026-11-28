//! Check an Alpaca server's mount and camera through the neutral traits,
//! one stage at a time. **This moves the telescope.**
//!
//! ```text
//! cargo run -p scopinator --example alpaca_verify -- HOST PORT <stage>... \
//!     [--ra HOURS --dec DEGREES] [--sync-ra HOURS --sync-dec DEGREES] [--out DIR] [--stay]
//! ```
//!
//! Stages run in the order given:
//!
//! - `where`: coordinates, alt/az, tracking, parked, and how far the mount
//!   says it is from the target.
//! - `unpark`, `park`: and wait.
//! - `slew`: slew to the target (`--ra`/`--dec`, J2000) and wait.
//! - `sync`: tell the mount it is pointing at `--sync-ra`/`--sync-dec`
//!   (equinox of date, as a plate solve reports it).
//! - `tracking`: switch tracking off and on, reading it back.
//! - `axis`: move each axis for two seconds at a fifth of full speed.
//! - `abort`: start a slew ten degrees away and abort it after three seconds.
//! - `guide`: a one-second guide pulse north.
//! - `frame`: a one-second exposure, written to `--out`.
//! - `camera`: an exposure with the gain set, a subframe, and an aborted
//!   exposure.
//!
//! `--stay` leaves the devices connected at the end (`Connected` is not
//! cleared), to see what the mount does when a client just goes away.
//! Events from the backend's poller are printed as they come.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use scopinator::backend::alpaca::{AlpacaBackend, AlpacaConfig};
use scopinator::types::{
    Coordinates, DecDegrees, Epoch, ExposureSettings, GuideDirection, MountAxis, RaDegrees, Roi,
};
use scopinator::{
    Backend, Camera, Mount, UnifiedEventBus, WaitOptions, wait_for_image, wait_for_park,
    wait_for_slew,
};

type AnyResult<T> = Result<T, Box<dyn std::error::Error>>;

fn show<T: std::fmt::Debug, E: std::fmt::Display>(result: Result<T, E>) -> String {
    match result {
        Ok(value) => format!("{value:?}"),
        Err(e) => format!("ERR {e}"),
    }
}

fn say(start: Instant, text: impl AsRef<str>) {
    println!("[{:7.1}s] {}", start.elapsed().as_secs_f64(), text.as_ref());
}

fn separation_deg(a: &Coordinates, b: &Coordinates) -> f64 {
    let (ra1, dec1) = (
        a.ra.as_degrees().to_radians(),
        a.dec.as_degrees().to_radians(),
    );
    let (ra2, dec2) = (
        b.ra.as_degrees().to_radians(),
        b.dec.as_degrees().to_radians(),
    );
    let cos = dec1.sin() * dec2.sin() + dec1.cos() * dec2.cos() * (ra1 - ra2).cos();
    cos.clamp(-1.0, 1.0).acos().to_degrees()
}

fn patient(seconds: u64) -> WaitOptions {
    WaitOptions::with_timeout(Duration::from_secs(seconds))
}

async fn state(mount: &dyn Mount) -> String {
    format!(
        "slewing={} parked={} tracking={} state={}",
        show(mount.is_slewing().await),
        show(mount.is_parked().await),
        show(mount.is_tracking().await),
        show(mount.get_mount_status().await.map(|s| s.slew_state)),
    )
}

/// Print the mount's state whenever it changes, for `seconds` or until
/// `done`.
async fn watch(mount: &dyn Mount, start: Instant, seconds: u64, done: impl Fn(&str) -> bool) {
    let began = Instant::now();
    let mut last = String::new();
    while began.elapsed() < Duration::from_secs(seconds) {
        let line = state(mount).await;
        if line != last {
            say(start, format!("  {line}"));
            last.clone_from(&line);
        }
        if done(&line) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    say(start, format!("  watch: {seconds}s passed"));
}

async fn position(mount: &dyn Mount, start: Instant, target: &Coordinates) {
    let jnow = target.to_epoch(Epoch::JNow, chrono::Utc::now());
    match mount.get_coordinates().await {
        Ok(at) => say(
            start,
            format!(
                "mount reports {:.6} h {:+.6} ({:?}): {:.4} deg from the target of date, {:.4} from its J2000 numbers",
                at.ra.as_hours(),
                at.dec.as_degrees(),
                at.epoch,
                separation_deg(&at, &jnow),
                separation_deg(&at, target),
            ),
        ),
        Err(e) => say(start, format!("get_coordinates: ERR {e}")),
    }
    say(start, format!("alt/az: {}", show(mount.get_alt_az().await)));
    say(start, state(mount).await);
}

async fn expose(camera: &dyn Camera, settings: &ExposureSettings) -> String {
    let began = Instant::now();
    let image = async {
        camera.start_exposure(settings).await?;
        wait_for_image(camera, patient(90)).await?;
        camera.get_image().await
    }
    .await;
    format!(
        "{} after {:.1}s",
        show(image.map(|image| format!(
            "{}x{} {:?}, {} bytes",
            image.width,
            image.height,
            image.format,
            image.data.len()
        ))),
        began.elapsed().as_secs_f64()
    )
}

#[tokio::main]
async fn main() -> AnyResult<()> {
    let mut args = std::env::args().skip(1);
    let host: Ipv4Addr = args
        .next()
        .ok_or("usage: alpaca_verify <host> <port> <stage>...")?
        .parse()?;
    let port: u16 = args.next().ok_or("the port")?.parse()?;
    let (mut ra, mut dec) = (0.13979, 29.0904);
    let (mut sync_ra, mut sync_dec) = (None, None);
    let mut out = PathBuf::from(".");
    let mut stay = false;
    let mut stages = Vec::new();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--ra" => ra = value()?.parse()?,
            "--dec" => dec = value()?.parse()?,
            "--sync-ra" => sync_ra = Some(value()?.parse::<f64>()?),
            "--sync-dec" => sync_dec = Some(value()?.parse::<f64>()?),
            "--out" => out = PathBuf::from(value()?),
            "--stay" => stay = true,
            _ => stages.push(arg),
        }
    }
    let target = Coordinates::from_hours(ra, dec)?;

    let bus = Arc::new(UnifiedEventBus::new(256));
    let mut events = bus.subscribe();
    let start = Instant::now();
    tokio::spawn(async move {
        while let Ok(event) = events.recv().await {
            say(
                start,
                format!(
                    "  event: {:?} {:?} from {:?}",
                    event.event_type,
                    event.payload,
                    event.source.map(|id| id.to_string())
                ),
            );
        }
    });
    let backend = AlpacaBackend::connect(host, port, AlpacaConfig::default(), bus).await?;
    say(start, format!("connected: {}", backend.id()));
    let mount = backend.mount().ok_or("the server has no telescope")?;
    let cameras = backend.cameras().await?;
    let mut frames = 0;

    for stage in &stages {
        say(start, format!("===== {stage}"));
        match stage.as_str() {
            "where" => position(mount.as_ref(), start, &target).await,
            "unpark" => {
                say(start, format!("unpark: {}", show(mount.unpark().await)));
                watch(mount.as_ref(), start, 20, |line| {
                    line.contains("parked=false") && line.contains("slewing=false")
                })
                .await;
            }
            "slew" => {
                say(
                    start,
                    format!(
                        "slew_to_coordinates: {}",
                        show(mount.slew_to_coordinates(&target).await)
                    ),
                );
                tokio::time::sleep(Duration::from_secs(1)).await;
                watch(mount.as_ref(), start, 240, |line| {
                    line.contains("slewing=false")
                })
                .await;
                say(
                    start,
                    format!(
                        "wait_for_slew: {}",
                        show(wait_for_slew(mount.as_ref(), patient(60)).await)
                    ),
                );
                position(mount.as_ref(), start, &target).await;
            }
            "sync" => {
                let (Some(ra), Some(dec)) = (sync_ra, sync_dec) else {
                    say(start, "sync needs --sync-ra and --sync-dec");
                    continue;
                };
                let at = Coordinates::with_epoch(
                    RaDegrees::from_hours(ra)?,
                    DecDegrees::new(dec)?,
                    Epoch::JNow,
                );
                say(
                    start,
                    format!(
                        "sync_to_coordinates({ra} h, {dec}, of date): {}",
                        show(mount.sync_to_coordinates(&at).await)
                    ),
                );
                position(mount.as_ref(), start, &target).await;
            }
            "tracking" => {
                for on in [false, true] {
                    say(
                        start,
                        format!(
                            "set_tracking({on}): {}, reads {}",
                            show(mount.set_tracking(on).await),
                            show(mount.is_tracking().await)
                        ),
                    );
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
            "axis" => {
                for axis in [MountAxis::Primary, MountAxis::Secondary] {
                    let before = mount.get_coordinates().await;
                    say(
                        start,
                        format!(
                            "move_axis({axis:?}, 0.2): {}",
                            show(mount.move_axis(axis, 0.2).await)
                        ),
                    );
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    say(start, format!("  during: {}", state(mount.as_ref()).await));
                    say(
                        start,
                        format!("stop_motion: {}", show(mount.stop_motion().await)),
                    );
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    let after = mount.get_coordinates().await;
                    if let (Ok(before), Ok(after)) = (&before, &after) {
                        say(
                            start,
                            format!(
                                "  moved {:.4} deg: RA {:+.5} h, Dec {:+.5} deg; {}",
                                separation_deg(before, after),
                                after.ra.as_hours() - before.ra.as_hours(),
                                after.dec.as_degrees() - before.dec.as_degrees(),
                                state(mount.as_ref()).await
                            ),
                        );
                    } else {
                        say(start, format!("  {} / {}", show(before), show(after)));
                    }
                }
            }
            "abort" => {
                let away = Coordinates::from_hours(ra, (dec - 10.0).max(-80.0))?;
                say(
                    start,
                    format!(
                        "slew ten degrees away: {}",
                        show(mount.slew_to_coordinates(&away).await)
                    ),
                );
                tokio::time::sleep(Duration::from_secs(3)).await;
                say(start, format!("  during: {}", state(mount.as_ref()).await));
                say(
                    start,
                    format!("abort_slew: {}", show(mount.abort_slew().await)),
                );
                watch(mount.as_ref(), start, 15, |line| {
                    line.contains("slewing=false")
                })
                .await;
                position(mount.as_ref(), start, &target).await;
            }
            "guide" => {
                let before = mount.get_coordinates().await;
                say(
                    start,
                    format!(
                        "pulse_guide(North, 1s): {}",
                        show(
                            mount
                                .pulse_guide(GuideDirection::North, Duration::from_secs(1))
                                .await
                        )
                    ),
                );
                tokio::time::sleep(Duration::from_secs(2)).await;
                let after = mount.get_coordinates().await;
                if let (Ok(before), Ok(after)) = (before, after) {
                    say(
                        start,
                        format!(
                            "  Dec changed by {:+.2} arcseconds",
                            (after.dec.as_degrees() - before.dec.as_degrees()) * 3600.0
                        ),
                    );
                }
            }
            "frame" => {
                for camera in &cameras {
                    let image = async {
                        camera.start_exposure(&ExposureSettings::default()).await?;
                        wait_for_image(camera.as_ref(), patient(60)).await?;
                        camera.get_image().await
                    }
                    .await;
                    match image {
                        Ok(image) => {
                            frames += 1;
                            let path = out.join(format!(
                                "frame{frames}-{}x{}.bin",
                                image.width, image.height
                            ));
                            std::fs::write(&path, &image.data)?;
                            say(
                                start,
                                format!("frame: {:?} written to {}", image.format, path.display()),
                            );
                        }
                        Err(e) => say(start, format!("frame: ERR {e}")),
                    }
                }
            }
            "camera" => {
                for camera in &cameras {
                    let camera = camera.as_ref();
                    say(
                        start,
                        format!("status: {}", show(camera.get_camera_status().await)),
                    );
                    let mut gain = ExposureSettings::default();
                    gain.duration_seconds = 2.0;
                    gain.gain = Some(80);
                    say(
                        start,
                        format!("two seconds at gain 80: {}", expose(camera, &gain).await),
                    );
                    say(
                        start,
                        format!("  status: {}", show(camera.get_camera_status().await)),
                    );
                    let mut subframe = ExposureSettings::default();
                    subframe.roi = Some(Roi::new(340, 760, 400, 400)?);
                    subframe.gain = Some(200);
                    say(
                        start,
                        format!("a 400x400 subframe: {}", expose(camera, &subframe).await),
                    );
                    let mut long = ExposureSettings::default();
                    long.duration_seconds = 20.0;
                    say(
                        start,
                        format!(
                            "start twenty seconds: {}",
                            show(camera.start_exposure(&long).await)
                        ),
                    );
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    say(
                        start,
                        format!(
                            "  exposing={} status={}",
                            show(camera.is_exposing().await),
                            show(camera.get_camera_status().await.map(|s| s.state))
                        ),
                    );
                    say(
                        start,
                        format!("abort_exposure: {}", show(camera.abort_exposure().await)),
                    );
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    say(
                        start,
                        format!(
                            "  exposing={} image_ready={} status={}",
                            show(camera.is_exposing().await),
                            show(camera.image_ready().await),
                            show(camera.get_camera_status().await.map(|s| s.state))
                        ),
                    );
                    // Back to the whole sensor for whoever comes next.
                    say(
                        start,
                        format!(
                            "whole sensor again: {}",
                            expose(camera, &ExposureSettings::default()).await
                        ),
                    );
                }
            }
            "park" => {
                say(start, format!("park: {}", show(mount.park().await)));
                watch(mount.as_ref(), start, 150, |line| {
                    line.contains("slewing=false") && line.contains("parked=true")
                })
                .await;
                say(
                    start,
                    format!(
                        "wait_for_park: {}",
                        show(wait_for_park(mount.as_ref(), patient(30)).await)
                    ),
                );
            }
            other => say(start, format!("unknown stage {other:?}, skipped")),
        }
    }

    tokio::time::sleep(Duration::from_millis(1500)).await;
    if stay {
        say(start, "left connected");
    } else {
        say(
            start,
            format!("disconnect: {}", show(backend.disconnect().await)),
        );
    }
    Ok(())
}
