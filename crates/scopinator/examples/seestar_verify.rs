//! Check on a real Seestar what the neutral traits assume about it. **This
//! moves the telescope**: it opens the arm, slews, focuses, stacks and parks.
//! Run it at night with the telescope under the sky.
//!
//! ```text
//! SEESTAR_INTEROP_PEM=client.pem cargo run -p scopinator --example seestar_verify -- \
//!     192.168.x.x <stage>... [--ra HOURS --dec DEGREES --name NAME] [--out DIR]
//! ```
//!
//! Stages run in the order given:
//!
//! - `state`: print the raw device state, view state, coordinates and clock. Read-only.
//! - `ready`: `Observer::readiness`, then `prepare`, then `readiness` again.
//! - `goto`: `Observer::acquire` of the target (J2000, converted by the
//!   backend), printing every change in `is_slewing`, `is_parked`, the mount
//!   status and the acquisition stage until `OnTarget`.
//! - `goto-raw`: the same target sent as its J2000 numbers, unconverted, with
//!   `SeestarClient::goto`. Comparing where the star lands in the frame after
//!   `goto` and after `goto-raw` says which epoch the telescope works in.
//! - `solve`: `PlateSolver::start_solve` and the solution.
//! - `focus`: `AutoFocus::start_autofocus` to its end.
//! - `stack`: `LiveStacker` for a few frames, then the stacked image.
//! - `frame`: fetch the current image and save it.
//! - `camera`: the main camera as a neutral `Camera`: start the live view,
//!   fetch two frames, stop it. Does not move the mount.
//! - `focuser`: read the focuser, move it 20 steps, start a 600-step move and
//!   halt it, ask for a position beyond the travel, and put it back.
//! - `wheel`: read the filter wheel, turn it one position on and back.
//! - `switch`: read the dew heater, switch it over for a second and back.
//! - `preview`: fetch the current preview frame, raw, and save it.
//! - `stop`: `Observer::stop`.
//! - `park`: `Mount::park` and wait for it.
//! - `park-eq`: park and switch the mount to equatorial mode
//!   (`scope_park {"equ_mode": true}`), for a telescope on a wedge. On an
//!   S30 (9.75) under cloud this parked and left the mode as it was.
//!
//! `--goto-timeout SECONDS` (default 240) is how long `goto` watches;
//! `--frames N` (default 3) how many frames `stack` waits for.
//!
//! The default target is Aldebaran. Images are written to `--out` (default
//! the current directory) as raw sample bytes, with their size and format in
//! the file name.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use scopinator::backend::seestar::SeestarBackend;
use scopinator::types::{Coordinates, Epoch, ExposureSettings, ImageData, StackSettings, Target};
use scopinator::{
    AutoFocus, Backend, Camera, LiveStacker, Mount, Observer, PlateSolver, ScopinatorError,
    UnifiedEventBus, WaitOptions, wait_for_acquisition, wait_for_autofocus, wait_for_filter_wheel,
    wait_for_focuser, wait_for_frames, wait_for_park, wait_for_solution,
};
use scopinator_seestar::command::ImagingCommand;
use scopinator_seestar::{InteropKey, SeestarClient, SeestarConfig};

type AnyResult<T> = Result<T, Box<dyn std::error::Error>>;

fn say(start: Instant, text: impl AsRef<str>) {
    println!("[{:7.1}s] {}", start.elapsed().as_secs_f64(), text.as_ref());
}

fn show<T: std::fmt::Debug>(result: &Result<T, ScopinatorError>) -> String {
    match result {
        Ok(value) => format!("{value:?}"),
        Err(e) => format!("ERR {e}"),
    }
}

/// The reply to a raw request on one line, or the error.
async fn raw(client: &SeestarClient, method: &str) -> String {
    match client.send_raw(method, None).await {
        Ok(response) => serde_json::to_string(&response.result)
            .unwrap_or_else(|e| format!("unprintable reply: {e}")),
        Err(e) => format!("ERR {e}"),
    }
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

/// How far a position the telescope reports is from the target in each epoch.
fn compare(start: Instant, what: &str, got: &Coordinates, j2000: &Coordinates) {
    let jnow = j2000.to_epoch(Epoch::JNow, Utc::now());
    say(
        start,
        format!(
            "{what}: {got} (labelled {:?}); {:.4} deg from the J2000 position, {:.4} deg from the JNow position",
            got.epoch,
            separation_deg(got, j2000),
            separation_deg(got, &jnow),
        ),
    );
}

fn save(start: Instant, dir: &Path, label: &str, image: &ImageData) -> AnyResult<()> {
    let format = format!("{:?}", image.format)
        .replace(['(', ')'], "-")
        .trim_end_matches('-')
        .to_lowercase();
    let path = dir.join(format!(
        "{label}-{}x{}-{format}.bin",
        image.width, image.height
    ));
    std::fs::write(&path, &image.data)?;
    say(
        start,
        format!(
            "saved {} ({} bytes, {:?}, {:?})",
            path.display(),
            image.data.len(),
            image.format,
            image.frame_kind
        ),
    );
    Ok(())
}

/// Print the mount and acquisition state whenever it changes, until `done`
/// says so or the time runs out.
async fn watch(
    start: Instant,
    mount: &dyn Mount,
    observer: &dyn Observer,
    client: &SeestarClient,
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    let mut last = String::new();
    let mut on_target_since: Option<Instant> = None;
    while Instant::now() < deadline {
        let acquisition = observer.acquisition().await;
        let line = format!(
            "is_slewing={} is_parked={} slew_state={} acquisition={}",
            show(&mount.is_slewing().await),
            show(&mount.is_parked().await),
            show(&mount.get_mount_status().await.map(|s| s.slew_state)),
            show(&acquisition),
        );
        if line != last {
            say(start, &line);
            say(
                start,
                format!("  view: {}", raw(client, "get_view_state").await),
            );
            last = line;
        }
        if format!("{acquisition:?}").contains("OnTarget") {
            let since = *on_target_since.get_or_insert_with(Instant::now);
            if since.elapsed() > Duration::from_secs(5) {
                return;
            }
        } else {
            on_target_since = None;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    say(start, "watch: timed out");
}

#[tokio::main]
async fn main() -> AnyResult<()> {
    let mut host = None;
    let mut stages = Vec::new();
    let (mut ra, mut dec, mut name) = (4.598_677, 16.509_30, "Aldebaran".to_string());
    let mut out = PathBuf::from(".");
    let mut frames = 3u32;
    let mut goto_timeout = 240u64;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--ra" => ra = value()?.parse()?,
            "--dec" => dec = value()?.parse()?,
            "--name" => name = value()?,
            "--out" => out = PathBuf::from(value()?),
            "--frames" => frames = value()?.parse()?,
            "--goto-timeout" => goto_timeout = value()?.parse()?,
            _ if host.is_none() => host = Some(arg),
            _ => stages.push(arg),
        }
    }
    let host = host.ok_or("usage: seestar_verify <host> <stage>... (see the file's header)")?;
    let target = Coordinates::from_hours(ra, dec)?;

    let interop_key = match std::env::var("SEESTAR_INTEROP_PEM") {
        Ok(path) => Some(InteropKey::from_pem(&std::fs::read_to_string(path)?)?),
        Err(_) => None,
    };
    let config = SeestarConfig {
        interop_key,
        ..Default::default()
    };
    let bus = Arc::new(UnifiedEventBus::new(256));
    let backend = SeestarBackend::connect_with_config(host.parse()?, config, bus).await?;
    let client = backend.client().clone();
    client.wait_for_connection(Duration::from_secs(10)).await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let start = Instant::now();
    let info = backend.device_info();
    say(
        start,
        format!(
            "connected: {:?} firmware {:?}, verification {:?}; computer clock {}",
            info.model,
            info.firmware_string,
            client.verification(),
            Utc::now()
        ),
    );
    // What the telescope says about a goto as a whole, as it says it.
    let mut events = client.subscribe_events();
    tokio::spawn(async move {
        while let Ok(event) = events.recv().await {
            let text = format!("{event:?}");
            if text.starts_with("AutoGoto") || text.starts_with("PlateSolve") {
                let short: String = text.chars().take(400).collect();
                say(start, format!("  event: {short}"));
            }
        }
    });
    let mount = backend.mount();
    let observer: Arc<dyn Observer> = Backend::observer(&backend).ok_or("no observer")?;
    let stacker: Arc<dyn LiveStacker> = Backend::live_stacker(&backend).ok_or("no stacker")?;
    let focus: Arc<dyn AutoFocus> = Backend::auto_focus(&backend).ok_or("no autofocus")?;
    let solver: Arc<dyn PlateSolver> = Backend::plate_solver(&backend).ok_or("no solver")?;

    for stage in &stages {
        say(start, format!("===== {stage}"));
        match stage.as_str() {
            "state" => {
                for method in [
                    "get_view_state",
                    "scope_get_equ_coord",
                    "scope_get_horiz_coord",
                    "pi_get_time",
                    "get_stack_info",
                    "get_focuser_position",
                ] {
                    say(start, format!("{method}: {}", raw(&client, method).await));
                }
                let state = client
                    .send_raw(
                        "get_device_state",
                        Some(serde_json::json!({"keys": ["mount", "balance_sensor"]})),
                    )
                    .await;
                say(
                    start,
                    format!(
                        "device state mount: {}",
                        state.map_or_else(
                            |e| format!("ERR {e}"),
                            |r| serde_json::to_string(&r.result).unwrap_or_default()
                        )
                    ),
                );
                say(
                    start,
                    format!(
                        "is_parked={} is_slewing={} is_tracking={} status={}",
                        show(&mount.is_parked().await),
                        show(&mount.is_slewing().await),
                        show(&mount.is_tracking().await),
                        show(&mount.get_mount_status().await),
                    ),
                );
                say(
                    start,
                    format!("get_coordinates: {}", show(&mount.get_coordinates().await)),
                );
                say(
                    start,
                    format!("get_alt_az: {}", show(&mount.get_alt_az().await)),
                );
                say(start, format!("get_utc: {}", show(&mount.get_utc().await)));
                say(
                    start,
                    format!("acquisition: {}", show(&observer.acquisition().await)),
                );
                say(
                    start,
                    format!("stack progress: {}", show(&stacker.progress().await)),
                );
                say(
                    start,
                    format!("autofocus: {}", show(&focus.autofocus_state().await)),
                );
                say(
                    start,
                    format!("solution: {}", show(&solver.solution().await)),
                );
            }
            "ready" => {
                say(
                    start,
                    format!("pi_get_time before: {}", raw(&client, "pi_get_time").await),
                );
                say(
                    start,
                    format!("readiness before: {}", show(&observer.readiness().await)),
                );
                say(
                    start,
                    format!("prepare: {}", show(&observer.prepare().await)),
                );
                tokio::time::sleep(Duration::from_secs(2)).await;
                say(
                    start,
                    format!("pi_get_time after: {}", raw(&client, "pi_get_time").await),
                );
                say(
                    start,
                    format!("readiness after: {}", show(&observer.readiness().await)),
                );
                say(start, format!("get_utc: {}", show(&mount.get_utc().await)));
            }
            "goto" | "goto-raw" => {
                let jnow = target.to_epoch(Epoch::JNow, Utc::now());
                say(start, format!("target {name}: J2000 {target}, JNow {jnow}"));
                let sent = if stage == "goto" {
                    let sky = Target::Sky {
                        name: Some(name.clone()),
                        coordinates: target,
                    };
                    show(&observer.acquire(&sky).await)
                } else {
                    let reply = client
                        .goto(&name, target.ra.as_hours(), target.dec.as_degrees())
                        .await;
                    format!("{:?}", reply.map(|r| (r.code, r.result)))
                };
                say(start, format!("request: {sent}"));
                watch(
                    start,
                    &mount,
                    observer.as_ref(),
                    &client,
                    Duration::from_secs(goto_timeout),
                )
                .await;
                let outcome = wait_for_acquisition(
                    observer.as_ref(),
                    WaitOptions::with_timeout(Duration::from_secs(5)),
                )
                .await;
                say(start, format!("wait_for_acquisition: {}", show(&outcome)));
                say(
                    start,
                    format!(
                        "scope_get_equ_coord: {}",
                        raw(&client, "scope_get_equ_coord").await
                    ),
                );
                match mount.get_coordinates().await {
                    Ok(got) => compare(start, "mount reports", &got, &target),
                    Err(e) => say(start, format!("get_coordinates: ERR {e}")),
                }
                say(
                    start,
                    format!(
                        "is_tracking={} status={}",
                        show(&mount.is_tracking().await),
                        show(&mount.get_mount_status().await)
                    ),
                );
            }
            "solve" => {
                say(
                    start,
                    format!("solution before: {}", show(&solver.solution().await)),
                );
                say(
                    start,
                    format!("start_solve: {}", show(&solver.start_solve().await)),
                );
                let solution = wait_for_solution(
                    solver.as_ref(),
                    WaitOptions::with_timeout(Duration::from_secs(90)),
                )
                .await;
                say(start, format!("solution: {}", show(&solution)));
                say(
                    start,
                    format!(
                        "get_solve_result: {}",
                        raw(&client, "get_solve_result").await
                    ),
                );
                if let Ok(solution) = solution {
                    compare(start, "solved centre", &solution.center, &target);
                }
            }
            "focus" => {
                say(
                    start,
                    format!(
                        "position before: {}",
                        raw(&client, "get_focuser_position").await
                    ),
                );
                say(
                    start,
                    format!("state before: {}", show(&focus.autofocus_state().await)),
                );
                say(
                    start,
                    format!("start_autofocus: {}", show(&focus.start_autofocus().await)),
                );
                tokio::time::sleep(Duration::from_secs(1)).await;
                say(
                    start,
                    format!("state just after: {}", show(&focus.autofocus_state().await)),
                );
                let done = wait_for_autofocus(
                    focus.as_ref(),
                    WaitOptions::with_timeout(Duration::from_secs(180)),
                )
                .await;
                say(start, format!("wait_for_autofocus: {}", show(&done)));
                say(
                    start,
                    format!(
                        "position after: {}",
                        raw(&client, "get_focuser_position").await
                    ),
                );
            }
            "stack" => {
                say(start, format!("capabilities: {:?}", stacker.capabilities()));
                say(
                    start,
                    format!("progress before: {}", show(&stacker.progress().await)),
                );
                let mut settings = StackSettings::default();
                settings.restart = true;
                say(
                    start,
                    format!(
                        "start_stacking: {}",
                        show(&stacker.start_stacking(&settings).await)
                    ),
                );
                tokio::time::sleep(Duration::from_secs(2)).await;
                say(
                    start,
                    format!("view: {}", raw(&client, "get_view_state").await),
                );
                let progress = wait_for_frames(
                    stacker.as_ref(),
                    frames,
                    WaitOptions::with_timeout(Duration::from_secs(60 + 40 * u64::from(frames))),
                )
                .await;
                say(
                    start,
                    format!("wait_for_frames({frames}): {}", show(&progress)),
                );
                say(
                    start,
                    format!("view: {}", raw(&client, "get_view_state").await),
                );
                say(
                    start,
                    format!("get_stack_info: {}", raw(&client, "get_stack_info").await),
                );
                match stacker.stacked_image().await {
                    Ok(image) => save(start, &out, "stack", &image)?,
                    Err(e) => say(start, format!("stacked_image: ERR {e}")),
                }
                say(
                    start,
                    format!("stop_stacking: {}", show(&stacker.stop_stacking().await)),
                );
                tokio::time::sleep(Duration::from_secs(3)).await;
                say(
                    start,
                    format!("progress after stop: {}", show(&stacker.progress().await)),
                );
                say(
                    start,
                    format!("view: {}", raw(&client, "get_view_state").await),
                );
            }
            "frame" => {
                let label = format!("frame-{}", Utc::now().format("%H%M%S"));
                match client.get_stacked_image().await {
                    Ok(_) => match stacker.stacked_image().await {
                        Ok(image) => save(start, &out, &label, &image)?,
                        Err(e) => say(start, format!("stacked_image: ERR {e}")),
                    },
                    Err(e) => say(start, format!("get_stacked_image: ERR {e}")),
                }
            }
            "preview" => {
                let command = ImagingCommand::GetCurrentImage {
                    compress: None,
                    raw: true,
                };
                match client.request_image(command).await {
                    Ok(frame) => {
                        let path = out.join(format!(
                            "preview-{}-{}x{}.bin",
                            Utc::now().format("%H%M%S"),
                            frame.header.width,
                            frame.header.height
                        ));
                        std::fs::write(&path, &frame.data)?;
                        say(
                            start,
                            format!(
                                "saved {} ({} bytes, {:?}, header {:?})",
                                path.display(),
                                frame.data.len(),
                                frame.payload_kind(),
                                frame.header
                            ),
                        );
                    }
                    Err(e) => say(start, format!("get_current_img: ERR {e}")),
                }
                // The same from the live stream, which is what the app shows.
                let mut frames = client.subscribe_frames();
                say(
                    start,
                    format!("begin_streaming: {:?}", client.begin_streaming().await),
                );
                let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
                loop {
                    match tokio::time::timeout_at(deadline, frames.recv()).await {
                        Ok(Ok(frame)) if frame.header.width > 0 && frame.text().is_none() => {
                            let path = out.join(format!(
                                "stream-{}-{}x{}.bin",
                                Utc::now().format("%H%M%S"),
                                frame.header.width,
                                frame.header.height
                            ));
                            std::fs::write(&path, &frame.data)?;
                            say(
                                start,
                                format!(
                                    "saved {} ({} bytes, {:?}, header {:?})",
                                    path.display(),
                                    frame.data.len(),
                                    frame.payload_kind(),
                                    frame.header
                                ),
                            );
                            break;
                        }
                        Ok(Ok(frame)) => say(start, format!("stream text: {:?}", frame.text())),
                        Ok(Err(e)) => say(start, format!("stream: {e}")),
                        Err(_) => {
                            say(start, "no stream frame in 15 s");
                            break;
                        }
                    }
                }
                say(
                    start,
                    format!(
                        "stop streaming: {:?}",
                        client.send_imaging(ImagingCommand::StopStreaming).await
                    ),
                );
            }
            "camera" => {
                let camera = backend.camera();
                say(
                    start,
                    format!("image_ready before: {}", show(&camera.image_ready().await)),
                );
                say(
                    start,
                    format!(
                        "start_exposure: {}",
                        show(&camera.start_exposure(&ExposureSettings::new(1.0)).await)
                    ),
                );
                say(
                    start,
                    format!("view: {}", raw(&client, "get_view_state").await),
                );
                say(
                    start,
                    format!("image_ready: {}", show(&camera.image_ready().await)),
                );
                for _ in 0..2 {
                    match camera.get_image().await {
                        Ok(image) => save(start, &out, "camera", &image)?,
                        Err(e) => say(start, format!("get_image: ERR {e}")),
                    }
                }
                say(
                    start,
                    format!("abort_exposure: {}", show(&camera.abort_exposure().await)),
                );
                tokio::time::sleep(Duration::from_secs(2)).await;
                say(
                    start,
                    format!("image_ready after: {}", show(&camera.image_ready().await)),
                );
                say(
                    start,
                    format!("view: {}", raw(&client, "get_view_state").await),
                );
            }
            "focuser" => {
                let focuser = Backend::focuser(&backend).ok_or("no focuser")?;
                let wait = || WaitOptions::with_timeout(Duration::from_secs(60));
                let home = focuser.focuser_position().await?;
                say(
                    start,
                    format!("position: {home:?}, {:?}", focuser.capabilities()),
                );
                say(
                    start,
                    format!(
                        "move_relative(+20): {}",
                        show(&focuser.move_relative(20).await)
                    ),
                );
                say(
                    start,
                    format!(
                        "wait_for_focuser: {}",
                        show(&wait_for_focuser(focuser.as_ref(), wait()).await)
                    ),
                );
                say(
                    start,
                    format!("position: {}", show(&focuser.focuser_position().await)),
                );
                // A long move, halted part of the way.
                say(
                    start,
                    format!(
                        "move_relative(+600): {}",
                        show(&focuser.move_relative(600).await)
                    ),
                );
                tokio::time::sleep(Duration::from_millis(1500)).await;
                say(
                    start,
                    format!("is_moving: {}", show(&focuser.is_moving().await)),
                );
                say(start, format!("halt: {}", show(&focuser.halt().await)));
                tokio::time::sleep(Duration::from_secs(1)).await;
                say(
                    start,
                    format!(
                        "position after halt: {}",
                        show(&focuser.focuser_position().await)
                    ),
                );
                say(
                    start,
                    format!(
                        "move_to(beyond the travel): {}",
                        show(&focuser.move_to(100_000).await)
                    ),
                );
                say(
                    start,
                    format!(
                        "back to {}: {}",
                        home.position,
                        show(&focuser.move_to(home.position).await)
                    ),
                );
                say(
                    start,
                    format!(
                        "wait_for_focuser: {}",
                        show(&wait_for_focuser(focuser.as_ref(), wait()).await)
                    ),
                );
                say(
                    start,
                    format!("position: {}", show(&focuser.focuser_position().await)),
                );
            }
            "wheel" => {
                let wheel = Backend::filter_wheel(&backend).ok_or("no filter wheel")?;
                let wait = || WaitOptions::with_timeout(Duration::from_secs(30));
                say(
                    start,
                    format!("names: {}", show(&wheel.get_filter_names().await)),
                );
                let home = wheel.filter_position().await?;
                say(
                    start,
                    format!("position: {home:?}, {:?}", wheel.capabilities()),
                );
                let Some(home) = home else {
                    say(start, "the wheel is turning; not touching it");
                    continue;
                };
                let slots = wheel.capabilities().slot_count;
                for position in [(home.position + 1) % slots, home.position] {
                    say(
                        start,
                        format!(
                            "set_filter_position({position}): {}",
                            show(&wheel.set_filter_position(position).await)
                        ),
                    );
                    say(
                        start,
                        format!(
                            "just after: is_moving={} position={}",
                            show(&wheel.is_moving().await),
                            show(&wheel.filter_position().await)
                        ),
                    );
                    say(
                        start,
                        format!(
                            "wait_for_filter_wheel: {}",
                            show(&wait_for_filter_wheel(wheel.as_ref(), wait()).await)
                        ),
                    );
                    say(
                        start,
                        format!("position: {}", show(&wheel.filter_position().await)),
                    );
                }
                say(
                    start,
                    format!(
                        "set_filter_position({slots}): {}",
                        show(&wheel.set_filter_position(slots).await)
                    ),
                );
            }
            "switch" => {
                let switch = Backend::switch(&backend).ok_or("no switch")?;
                say(
                    start,
                    format!("switches: {}", show(&switch.switches().await)),
                );
                let was_on = switch.get_bool(0).await?;
                say(start, format!("switch 0 on: {was_on}"));
                for on in [!was_on, was_on] {
                    say(
                        start,
                        format!("set_bool(0, {on}): {}", show(&switch.set_bool(0, on).await)),
                    );
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    say(
                        start,
                        format!(
                            "get: {} pi_output_get2: {}",
                            show(&switch.get(0).await),
                            raw(&client, "pi_output_get2").await
                        ),
                    );
                }
                say(
                    start,
                    format!("set(0, 5.0): {}", show(&switch.set(0, 5.0).await)),
                );
                say(
                    start,
                    format!("set(1, 1.0): {}", show(&switch.set(1, 1.0).await)),
                );
            }
            "stop" => {
                say(start, format!("stop: {}", show(&observer.stop().await)));
                tokio::time::sleep(Duration::from_secs(3)).await;
                say(
                    start,
                    format!("acquisition: {}", show(&observer.acquisition().await)),
                );
                say(
                    start,
                    format!("view: {}", raw(&client, "get_view_state").await),
                );
            }
            "park" => {
                say(start, format!("park: {}", show(&mount.park().await)));
                let mut last = String::new();
                let deadline = Instant::now() + Duration::from_secs(90);
                while Instant::now() < deadline {
                    let line = format!(
                        "is_parked={} is_slewing={} slew_state={}",
                        show(&mount.is_parked().await),
                        show(&mount.is_slewing().await),
                        show(&mount.get_mount_status().await.map(|s| s.slew_state)),
                    );
                    if line != last {
                        say(start, &line);
                        last = line;
                    }
                    if matches!(mount.is_parked().await, Ok(true)) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                let parked =
                    wait_for_park(&mount, WaitOptions::with_timeout(Duration::from_secs(10))).await;
                say(start, format!("wait_for_park: {}", show(&parked)));
            }
            "park-eq" => {
                // Park and put the mount in equatorial mode in one command.
                let reply = client
                    .send_raw("scope_park", Some(serde_json::json!({"equ_mode": true})))
                    .await;
                say(
                    start,
                    format!(
                        "scope_park equ_mode=true: {:?}",
                        reply.map(|r| (r.code, r.result))
                    ),
                );
                tokio::time::sleep(Duration::from_secs(2)).await;
                let parked =
                    wait_for_park(&mount, WaitOptions::with_timeout(Duration::from_secs(120)))
                        .await;
                say(start, format!("wait_for_park: {}", show(&parked)));
                say(
                    start,
                    format!(
                        "device state mount: {:?}",
                        client
                            .send_raw_and_validate(
                                "get_device_state",
                                Some(serde_json::json!({"keys": ["mount"]})),
                            )
                            .await
                    ),
                );
            }
            other => say(start, format!("unknown stage {other:?}, skipped")),
        }
    }

    Backend::disconnect(&backend).await?;
    Ok(())
}
