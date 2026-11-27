use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, warn};

use std::sync::Arc;

use scopinator_types::{
    Coordinates, DecDegrees, ExposureSettings, RaDegrees, StackSettings, Target,
};

use super::context::{ExecutionContext, FrameSink};
use crate::device::smart::LiveStacker;
use crate::device::traits::Camera;
use crate::device::wait::{
    WaitOptions, wait_for_acquisition, wait_for_autofocus, wait_for_filter_wheel, wait_for_frames,
    wait_for_image, wait_for_park, wait_for_slew,
};
use crate::error::ScopinatorError;

/// Status of a sequencer command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
    Paused,
}

/// A command in an observation sequence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command_type")]
#[non_exhaustive]
pub enum SequencerCommand {
    /// Slew to a target by RA/Dec and wait for the mount to arrive.
    GoToTarget {
        name: String,
        /// RA in hours (0-24).
        ra_hours: f64,
        /// Dec in degrees (-90 to 90).
        dec_deg: f64,
        /// How long to wait for the mount to arrive before failing the
        /// command. Defaults to 300 seconds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_seconds: Option<f64>,
        /// Extra time to wait after the mount has arrived. Defaults to none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        settle_seconds: Option<f64>,
    },
    /// Take frames.
    ///
    /// On a telescope that stacks (the context has a live stacker), start a
    /// stack with this sub-exposure and gain. With a `count` the command
    /// waits for that many stacked frames and stops the stack; without one
    /// it returns once the stack has started, and `StopImaging` ends it.
    ///
    /// Otherwise take single exposures with the camera, handing each frame
    /// to the context's frame sink. With a `count` the command takes that
    /// many and returns; without one it keeps taking them in the background
    /// until `StopImaging`.
    StartImaging {
        /// Exposure time in seconds.
        exposure_seconds: f64,
        /// Gain (camera-specific).
        gain: Option<i32>,
        /// Number of frames (None = until `StopImaging`).
        count: Option<u32>,
    },
    /// Stop the stack, or the camera's imaging run and its exposure.
    StopImaging,
    /// Focus with the telescope's own autofocus and wait for it to finish.
    AutoFocus {
        /// How long to wait before failing the command. Defaults to 300
        /// seconds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_seconds: Option<f64>,
    },
    /// Turn the filter wheel to a filter, by position (from zero) or by
    /// name, and wait for it to stop. Exactly one of the two must be given.
    SetFilter {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// Park the mount and wait for it to finish.
    Park,
    /// Take the mount out of park.
    Unpark,
    /// Wait for a fixed number of minutes.
    WaitMinutes { minutes: f64 },
    /// Wait until a specific UTC time.
    WaitUntilTime { target_time: DateTime<Utc> },
    /// A nested sequence of commands.
    Sequence {
        commands: Vec<SequencerCommand>,
        #[serde(default = "default_true")]
        stop_on_error: bool,
    },
}

fn default_true() -> bool {
    true
}

/// Runtime state of a command during execution.
#[derive(Debug)]
pub struct CommandExecution {
    pub command: SequencerCommand,
    pub status: CommandStatus,
    pub error: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl CommandExecution {
    pub fn new(command: SequencerCommand) -> Self {
        Self {
            command,
            status: CommandStatus::Pending,
            error: None,
            started_at: None,
            completed_at: None,
        }
    }

    pub fn mark_started(&mut self) {
        self.status = CommandStatus::Running;
        self.started_at = Some(Utc::now());
    }

    pub fn mark_completed(&mut self) {
        self.status = CommandStatus::Completed;
        self.completed_at = Some(Utc::now());
    }

    pub fn mark_failed(&mut self, error: String) {
        self.status = CommandStatus::Failed;
        self.error = Some(error);
        self.completed_at = Some(Utc::now());
    }

    pub fn mark_cancelled(&mut self) {
        self.status = CommandStatus::Cancelled;
        self.completed_at = Some(Utc::now());
    }
}

/// Execute a single sequencer command.
pub fn execute_command<'a>(
    cmd: &'a SequencerCommand,
    ctx: &'a ExecutionContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), ScopinatorError>> + Send + 'a>> {
    Box::pin(execute_command_inner(cmd, ctx))
}

async fn execute_command_inner(
    cmd: &SequencerCommand,
    ctx: &ExecutionContext,
) -> Result<(), ScopinatorError> {
    match cmd {
        SequencerCommand::GoToTarget {
            name,
            ra_hours,
            dec_deg,
            timeout_seconds,
            settle_seconds,
        } => {
            debug!(target = name, ra = ra_hours, dec = dec_deg, "goto target");
            let seconds = |name: &str, value: Option<f64>, default: Duration| match value {
                None => Ok(default),
                Some(s) => Duration::try_from_secs_f64(s).map_err(|_| {
                    ScopinatorError::InvalidArgument(format!("{name} is not a duration: {s}"))
                }),
            };
            let mut wait = WaitOptions::default();
            wait.timeout = seconds("timeout_seconds", *timeout_seconds, wait.timeout)?;
            wait.settle = seconds("settle_seconds", *settle_seconds, wait.settle)?;
            let ra = RaDegrees::from_hours(*ra_hours)
                .map_err(|e| ScopinatorError::InvalidArgument(e.to_string()))?;
            let dec = DecDegrees::new(*dec_deg)
                .map_err(|e| ScopinatorError::InvalidArgument(e.to_string()))?;
            let coords = Coordinates::new(ra, dec);
            // A smart telescope finds the target itself: slew, plate solve
            // and re-centre. Arriving is being on target, not having slewed.
            if let Some(observer) = &ctx.observer {
                let target = Target::Sky {
                    name: Some(name.clone()),
                    coordinates: coords,
                };
                observer.acquire(&target).await?;
                return wait_for_acquisition(observer.as_ref(), wait).await;
            }
            // The slew is only accepted at this point; the next command
            // must not start until the mount has arrived.
            ctx.mount.slew_to_coordinates(&coords).await?;
            match wait_for_slew(ctx.mount.as_ref(), wait).await {
                Err(e) if e.is_not_supported() => {
                    warn!(
                        target = name,
                        "this mount cannot say whether it is slewing; \
                         continuing without waiting for it to arrive"
                    );
                    Ok(())
                }
                outcome => outcome,
            }
        }

        SequencerCommand::StartImaging {
            exposure_seconds,
            gain,
            count,
        } => {
            let exposure = Duration::try_from_secs_f64(*exposure_seconds).map_err(|_| {
                ScopinatorError::InvalidArgument(format!(
                    "exposure_seconds is not a duration: {exposure_seconds}"
                ))
            })?;
            debug!(exposure = exposure_seconds, gain = ?gain, count = ?count, "start imaging");
            if let Some(stacker) = &ctx.live_stacker {
                return stack(stacker.as_ref(), exposure, *gain, *count).await;
            }
            let camera = ctx.camera.clone().ok_or_else(|| {
                ScopinatorError::NotSupported("no stacker and no camera in context".into())
            })?;
            let mut settings = ExposureSettings::new(*exposure_seconds);
            settings.gain = *gain;
            // One imaging run at a time.
            ctx.end_imaging();
            let sink = ctx.frame_sink.clone();
            match count {
                Some(count) => take_frames(camera, sink, settings, Some(*count)).await,
                None => {
                    let run = tokio::spawn(async move {
                        if let Err(e) = take_frames(camera, sink, settings, None).await {
                            error!(error = %e, "imaging run ended");
                        }
                    });
                    *ctx.imaging
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(run);
                    Ok(())
                }
            }
        }

        SequencerCommand::StopImaging => {
            debug!("stop imaging");
            if let Some(stacker) = &ctx.live_stacker {
                return stacker.stop_stacking().await;
            }
            let camera = ctx.camera.as_ref().ok_or_else(|| {
                ScopinatorError::NotSupported("no stacker and no camera in context".into())
            })?;
            ctx.end_imaging();
            camera.abort_exposure().await
        }

        SequencerCommand::AutoFocus { timeout_seconds } => {
            let focus = ctx
                .auto_focus
                .as_ref()
                .ok_or_else(|| ScopinatorError::NotSupported("no autofocus in context".into()))?;
            let timeout = match timeout_seconds {
                None => WaitOptions::default().timeout,
                Some(s) => Duration::try_from_secs_f64(*s).map_err(|_| {
                    ScopinatorError::InvalidArgument(format!(
                        "timeout_seconds is not a duration: {s}"
                    ))
                })?,
            };
            debug!("autofocus");
            focus.start_autofocus().await?;
            let position =
                wait_for_autofocus(focus.as_ref(), WaitOptions::with_timeout(timeout)).await?;
            debug!(?position, "autofocus finished");
            Ok(())
        }

        SequencerCommand::SetFilter { position, name } => {
            let wheel = ctx.filter_wheel.as_ref().ok_or_else(|| {
                ScopinatorError::NotSupported("no filter wheel in context".into())
            })?;
            let position = match (position, name) {
                (Some(position), None) => *position,
                (None, Some(name)) => {
                    let names = wheel.get_filter_names().await?;
                    let found = names.iter().position(|n| n.eq_ignore_ascii_case(name));
                    let found = found.ok_or_else(|| {
                        ScopinatorError::InvalidArgument(format!(
                            "no filter named {name:?}; the wheel has {names:?}"
                        ))
                    })?;
                    found as u32
                }
                _ => {
                    return Err(ScopinatorError::InvalidArgument(
                        "SetFilter takes a position or a name, one of the two".into(),
                    ));
                }
            };
            debug!(position, "set filter");
            wheel.set_filter_position(position).await?;
            wait_for_filter_wheel(wheel.as_ref(), WaitOptions::default()).await
        }

        SequencerCommand::Park => {
            debug!("park");
            ctx.mount.park().await?;
            match wait_for_park(ctx.mount.as_ref(), WaitOptions::default()).await {
                Err(e) if e.is_not_supported() => {
                    warn!(
                        "this mount cannot say whether it is parked; \
                         continuing without waiting for it"
                    );
                    Ok(())
                }
                outcome => outcome,
            }
        }

        SequencerCommand::Unpark => {
            debug!("unpark");
            ctx.mount.unpark().await
        }

        SequencerCommand::WaitMinutes { minutes } => {
            debug!(minutes, "waiting");
            let duration = Duration::from_secs_f64(minutes * 60.0);
            tokio::time::sleep(duration).await;
            Ok(())
        }

        SequencerCommand::WaitUntilTime { target_time } => {
            let now = Utc::now();
            if *target_time > now {
                let wait = (*target_time - now).to_std().unwrap_or(Duration::ZERO);
                debug!(
                    ?target_time,
                    wait_secs = wait.as_secs(),
                    "waiting until time"
                );
                tokio::time::sleep(wait).await;
            } else {
                debug!(?target_time, "target time already passed, skipping wait");
            }
            Ok(())
        }

        SequencerCommand::Sequence {
            commands,
            stop_on_error,
        } => {
            for (i, subcmd) in commands.iter().enumerate() {
                debug!(step = i, "executing sub-command");
                match execute_command(subcmd, ctx).await {
                    Ok(()) => {}
                    Err(e) => {
                        error!(step = i, error = %e, "sub-command failed");
                        if *stop_on_error {
                            return Err(e);
                        }
                    }
                }
            }
            Ok(())
        }
    }
}

/// Start a stack; with a `count`, see it through to that many frames.
async fn stack(
    stacker: &dyn LiveStacker,
    exposure: Duration,
    gain: Option<i32>,
    count: Option<u32>,
) -> Result<(), ScopinatorError> {
    let limits_itself = stacker.capabilities().can_limit_frames;
    let mut settings = StackSettings::default();
    settings.exposure = Some(exposure);
    settings.gain = gain;
    settings.frame_limit = count.filter(|_| limits_itself);
    match stacker.start_stacking(&settings).await {
        // The telescope stacks with the sub-exposure it is set to: better
        // that than no stack. Tried once.
        Err(e) if e.is_not_supported() => {
            warn!(
                error = %e,
                "the stacker refused these settings; \
                 stacking with its own sub-exposure instead"
            );
            settings.exposure = None;
            stacker.start_stacking(&settings).await?;
        }
        outcome => outcome?,
    }
    let Some(count) = count else {
        return Ok(());
    };
    if limits_itself {
        return Ok(());
    }
    // Frames are dropped, and the telescope may focus before the first one:
    // allow several times the exposure time, and never less than ten minutes.
    let timeout = (exposure * count * 4 + Duration::from_secs(300)).max(Duration::from_secs(600));
    let waited = wait_for_frames(stacker, count, WaitOptions::with_timeout(timeout)).await;
    // The stack is stopped whether or not it got there.
    let stopped = stacker.stop_stacking().await;
    waited?;
    stopped
}

/// Take `count` single exposures (or keep going, without one), handing each
/// frame to `sink`.
async fn take_frames(
    camera: Arc<dyn Camera>,
    sink: Option<Arc<dyn FrameSink>>,
    settings: ExposureSettings,
    count: Option<u32>,
) -> Result<(), ScopinatorError> {
    let exposure = Duration::try_from_secs_f64(settings.duration_seconds).unwrap_or_default();
    let wait = WaitOptions::with_timeout(exposure + Duration::from_secs(120));
    let mut taken = 0u32;
    while count.is_none_or(|count| taken < count) {
        camera.start_exposure(&settings).await?;
        wait_for_image(camera.as_ref(), wait).await?;
        let image = camera.get_image().await?;
        if let Some(sink) = &sink {
            sink.frame(image).await?;
        }
        taken += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_goto_command() {
        let cmd = SequencerCommand::GoToTarget {
            name: "M31".into(),
            ra_hours: 0.712,
            dec_deg: 41.27,
            timeout_seconds: None,
            settle_seconds: None,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"command_type\":\"GoToTarget\""));
        assert!(json.contains("\"name\":\"M31\""));
    }

    #[test]
    fn serialize_nested_sequence() {
        let cmd = SequencerCommand::Sequence {
            commands: vec![
                SequencerCommand::GoToTarget {
                    name: "M42".into(),
                    ra_hours: 5.59,
                    dec_deg: -5.39,
                    timeout_seconds: None,
                    settle_seconds: None,
                },
                SequencerCommand::StartImaging {
                    exposure_seconds: 10.0,
                    gain: Some(80),
                    count: Some(30),
                },
                SequencerCommand::WaitMinutes { minutes: 5.0 },
                SequencerCommand::StopImaging,
            ],
            stop_on_error: true,
        };
        let json = serde_json::to_string_pretty(&cmd).unwrap();
        let roundtrip: SequencerCommand = serde_json::from_str(&json).unwrap();
        match roundtrip {
            SequencerCommand::Sequence {
                commands,
                stop_on_error,
            } => {
                assert_eq!(commands.len(), 4);
                assert!(stop_on_error);
            }
            _ => panic!("expected Sequence"),
        }
    }

    #[test]
    fn serialize_wait_until_time() {
        let cmd = SequencerCommand::WaitUntilTime {
            target_time: chrono::DateTime::parse_from_rfc3339("2026-04-13T04:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("2026-04-13"));
    }

    #[test]
    fn command_execution_lifecycle() {
        let mut exec = CommandExecution::new(SequencerCommand::StopImaging);
        assert_eq!(exec.status, CommandStatus::Pending);

        exec.mark_started();
        assert_eq!(exec.status, CommandStatus::Running);
        assert!(exec.started_at.is_some());

        exec.mark_completed();
        assert_eq!(exec.status, CommandStatus::Completed);
        assert!(exec.completed_at.is_some());
    }

    #[test]
    fn command_execution_failure() {
        let mut exec = CommandExecution::new(SequencerCommand::StopImaging);
        exec.mark_started();
        exec.mark_failed("telescope lost connection".into());
        assert_eq!(exec.status, CommandStatus::Failed);
        assert_eq!(exec.error.as_deref(), Some("telescope lost connection"));
    }

    // --- GoToTarget waits for the mount ---

    use std::sync::Arc;

    use crate::device::mock::{Answer, MockCamera, MockMount};
    use crate::sequencer::context::ExecutionContext;

    fn goto(timeout_seconds: Option<f64>, settle_seconds: Option<f64>) -> SequencerCommand {
        SequencerCommand::GoToTarget {
            name: "M31".into(),
            ra_hours: 0.712,
            dec_deg: 41.27,
            timeout_seconds,
            settle_seconds,
        }
    }

    fn context(mount: &Arc<MockMount>, camera: &Arc<MockCamera>) -> ExecutionContext {
        ExecutionContext::new(Arc::clone(mount) as Arc<dyn crate::device::Mount>)
            .with_camera(Arc::clone(camera) as Arc<dyn crate::device::Camera>)
    }

    #[tokio::test(start_paused = true)]
    async fn goto_completes_only_when_the_mount_has_arrived() {
        let mount = Arc::new(MockMount::default());
        let camera = Arc::new(MockCamera::default());
        // Slewing for the first ten polls (five seconds), then arrived.
        mount
            .slewing
            .script(std::iter::repeat_n(Answer::Is(true), 10).chain([Answer::Is(false)]));
        let ctx = context(&mount, &camera);
        let sequence = SequencerCommand::Sequence {
            commands: vec![
                goto(None, None),
                SequencerCommand::StartImaging {
                    exposure_seconds: 10.0,
                    gain: None,
                    count: Some(1),
                },
            ],
            stop_on_error: true,
        };
        camera.ready.script([Answer::Is(true)]);

        let run = tokio::spawn(async move { execute_command(&sequence, &ctx).await });
        // Four seconds in: the slew was sent, the mount is still moving, and
        // imaging has not started.
        tokio::time::sleep(Duration::from_secs(4)).await;
        assert_eq!(mount.slews.lock().unwrap().len(), 1);
        assert!(camera.exposures.lock().unwrap().is_empty());
        assert!(!run.is_finished());

        run.await.unwrap().unwrap();
        assert_eq!(camera.exposures.lock().unwrap().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn goto_fails_with_timeout_when_the_mount_never_arrives() {
        let mount = Arc::new(MockMount::default());
        let camera = Arc::new(MockCamera::default());
        mount.slewing.script([Answer::Is(true)]);
        let ctx = context(&mount, &camera);
        let start = tokio::time::Instant::now();
        let err = execute_command(&goto(Some(30.0), None), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ScopinatorError::Timeout), "{err:?}");
        assert_eq!(start.elapsed(), Duration::from_secs(30));
    }

    #[tokio::test(start_paused = true)]
    async fn goto_fails_when_the_slew_fails_under_way() {
        let mount = Arc::new(MockMount::default());
        let camera = Arc::new(MockCamera::default());
        mount
            .slewing
            .script([Answer::Is(true), Answer::Fails("goto failed at ScopeGoto")]);
        let ctx = context(&mount, &camera);
        let err = execute_command(&goto(None, None), &ctx).await.unwrap_err();
        assert_eq!(err.to_string(), "backend error: goto failed at ScopeGoto");
    }

    #[tokio::test(start_paused = true)]
    async fn goto_does_not_wait_on_a_mount_that_cannot_say_if_it_is_slewing() {
        let mount = Arc::new(MockMount::default());
        let camera = Arc::new(MockCamera::default());
        mount.slewing.script([Answer::NotSupported]);
        let ctx = context(&mount, &camera);
        let start = tokio::time::Instant::now();
        execute_command(&goto(None, None), &ctx).await.unwrap();
        assert_eq!(start.elapsed(), Duration::ZERO);
        assert_eq!(mount.slews.lock().unwrap().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn goto_waits_the_settle_time_after_arrival() {
        let mount = Arc::new(MockMount::default());
        let camera = Arc::new(MockCamera::default());
        let ctx = context(&mount, &camera);
        let start = tokio::time::Instant::now();
        execute_command(&goto(None, Some(5.0)), &ctx).await.unwrap();
        // One poll interval to confirm "not slewing", then the settle time.
        assert_eq!(start.elapsed(), Duration::from_millis(5500));
    }

    #[tokio::test(start_paused = true)]
    async fn a_refused_slew_fails_without_waiting() {
        let mount = Arc::new(MockMount::default());
        let camera = Arc::new(MockCamera::default());
        *mount.refuse_slew.lock().unwrap() = Some("below horizon");
        let ctx = context(&mount, &camera);
        let err = execute_command(&goto(None, None), &ctx).await.unwrap_err();
        assert_eq!(err.to_string(), "backend error: below horizon");
        assert_eq!(mount.slewing.asked(), 0);
    }

    #[tokio::test]
    async fn a_negative_timeout_is_an_invalid_argument() {
        let mount = Arc::new(MockMount::default());
        let camera = Arc::new(MockCamera::default());
        let ctx = context(&mount, &camera);
        let err = execute_command(&goto(Some(-1.0), None), &ctx)
            .await
            .unwrap_err();
        assert!(
            matches!(err, ScopinatorError::InvalidArgument(_)),
            "{err:?}"
        );
        // Checked before anything is sent to the mount.
        assert!(mount.slews.lock().unwrap().is_empty());
    }

    #[test]
    fn goto_json_without_the_wait_fields_still_deserializes() {
        let json = r#"{"command_type":"GoToTarget","name":"M31","ra_hours":0.712,"dec_deg":41.27}"#;
        let cmd: SequencerCommand = serde_json::from_str(json).unwrap();
        match cmd {
            SequencerCommand::GoToTarget {
                timeout_seconds,
                settle_seconds,
                ..
            } => {
                assert_eq!(timeout_seconds, None);
                assert_eq!(settle_seconds, None);
            }
            other => panic!("expected GoToTarget, got {other:?}"),
        }
        // And the fields are left out again when unset.
        let cmd: SequencerCommand = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_string(&cmd).unwrap(), json);
    }
    use std::sync::Mutex;

    use async_trait::async_trait;
    use scopinator_types::{
        AcquisitionState, AutoFocusState, ImageData, StackProgress, StackState,
    };

    use crate::device::mock::{MockAutoFocus, MockFilterWheel, MockObserver, MockStacker};
    use crate::device::traits::FilterWheel;

    fn imaging(count: Option<u32>) -> SequencerCommand {
        SequencerCommand::StartImaging {
            exposure_seconds: 10.0,
            gain: Some(80),
            count,
        }
    }

    fn stacked(state: StackState, frames: u32) -> Result<StackProgress, &'static str> {
        let mut progress = StackProgress::default();
        progress.state = state;
        progress.stacked = frames;
        Ok(progress)
    }

    #[derive(Default)]
    struct Frames(Mutex<Vec<ImageData>>);

    #[async_trait]
    impl FrameSink for Frames {
        async fn frame(&self, image: ImageData) -> Result<(), ScopinatorError> {
            self.0.lock().unwrap().push(image);
            Ok(())
        }
    }

    #[tokio::test(start_paused = true)]
    async fn imaging_on_a_stacker_waits_for_the_count_and_stops_the_stack() {
        let stacker = Arc::new(MockStacker::default());
        stacker.progress.script([
            stacked(StackState::Stacking, 0),
            stacked(StackState::Stacking, 1),
            stacked(StackState::Stacking, 3),
        ]);
        let camera = Arc::new(MockCamera::default());
        let ctx = ExecutionContext::new(Arc::new(MockMount::default()))
            .with_camera(camera.clone())
            .with_live_stacker(stacker.clone());

        execute_command(&imaging(Some(3)), &ctx).await.unwrap();

        let started = stacker.started.lock().unwrap().clone();
        assert_eq!(started.len(), 1);
        assert_eq!(started[0].exposure, Some(Duration::from_secs(10)));
        assert_eq!(started[0].gain, Some(80));
        // This stacker cannot count for itself: the sequencer does.
        assert_eq!(started[0].frame_limit, None);
        assert_eq!(*stacker.stops.lock().unwrap(), 1);
        // The camera is not involved when there is a stacker.
        assert!(camera.exposures.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn imaging_without_a_count_starts_the_stack_and_returns() {
        let stacker = Arc::new(MockStacker::default());
        let ctx = ExecutionContext::new(Arc::new(MockMount::default()))
            .with_live_stacker(stacker.clone());

        execute_command(&imaging(None), &ctx).await.unwrap();
        assert_eq!(stacker.started.lock().unwrap().len(), 1);
        assert_eq!(*stacker.stops.lock().unwrap(), 0);
        assert_eq!(stacker.progress.asked(), 0);

        execute_command(&SequencerCommand::StopImaging, &ctx)
            .await
            .unwrap();
        assert_eq!(*stacker.stops.lock().unwrap(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_stacker_that_counts_frames_is_given_the_count() {
        let stacker = Arc::new(MockStacker::default());
        *stacker.limits_frames.lock().unwrap() = true;
        let ctx = ExecutionContext::new(Arc::new(MockMount::default()))
            .with_live_stacker(stacker.clone());

        execute_command(&imaging(Some(30)), &ctx).await.unwrap();
        assert_eq!(stacker.started.lock().unwrap()[0].frame_limit, Some(30));
        assert_eq!(*stacker.stops.lock().unwrap(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_refused_sub_exposure_is_dropped_and_the_stack_started_without_it() {
        let stacker = Arc::new(MockStacker::default());
        *stacker.refuse_exposure.lock().unwrap() = true;
        let ctx = ExecutionContext::new(Arc::new(MockMount::default()))
            .with_live_stacker(stacker.clone());

        execute_command(&imaging(None), &ctx).await.unwrap();
        let started = stacker.started.lock().unwrap().clone();
        assert_eq!(started.len(), 1);
        assert_eq!(started[0].exposure, None);
        assert_eq!(started[0].gain, Some(80));
    }

    #[tokio::test(start_paused = true)]
    async fn a_stack_that_ends_short_fails_the_command_and_is_still_stopped() {
        let stacker = Arc::new(MockStacker::default());
        stacker.progress.script([
            stacked(StackState::Stacking, 1),
            stacked(StackState::Idle, 1),
        ]);
        let ctx = ExecutionContext::new(Arc::new(MockMount::default()))
            .with_live_stacker(stacker.clone());

        let err = execute_command(&imaging(Some(5)), &ctx).await.unwrap_err();
        assert!(matches!(err, ScopinatorError::InvalidState(_)), "{err:?}");
        assert_eq!(*stacker.stops.lock().unwrap(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn imaging_on_a_camera_takes_the_count_and_hands_over_each_frame() {
        let camera = Arc::new(MockCamera::default());
        camera.ready.script([Answer::Is(true)]);
        let frames = Arc::new(Frames::default());
        let ctx = ExecutionContext::new(Arc::new(MockMount::default()))
            .with_camera(camera.clone())
            .with_frame_sink(frames.clone());

        execute_command(&imaging(Some(4)), &ctx).await.unwrap();

        let exposures = camera.exposures.lock().unwrap().clone();
        assert_eq!(exposures.len(), 4);
        assert_eq!(exposures[0].duration_seconds, 10.0);
        assert_eq!(exposures[0].gain, Some(80));
        assert_eq!(frames.0.lock().unwrap().len(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn imaging_on_a_camera_without_a_count_runs_until_it_is_stopped() {
        let camera = Arc::new(MockCamera::default());
        // Two polls per frame: an exposure takes a moment.
        camera.ready.script([Answer::Is(true)]);
        let frames = Arc::new(Frames::default());
        let ctx = ExecutionContext::new(Arc::new(MockMount::default()))
            .with_camera(camera.clone())
            .with_frame_sink(frames.clone());

        execute_command(&imaging(None), &ctx).await.unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
        let before = frames.0.lock().unwrap().len();
        assert!(before > 1, "{before} frames");

        execute_command(&SequencerCommand::StopImaging, &ctx)
            .await
            .unwrap();
        assert_eq!(*camera.aborts.lock().unwrap(), 1);
        tokio::time::sleep(Duration::from_secs(30)).await;
        let after = frames.0.lock().unwrap().len();
        assert!(after <= before + 1, "{before} then {after} frames");
        tokio::time::sleep(Duration::from_secs(30)).await;
        assert_eq!(frames.0.lock().unwrap().len(), after);
    }

    #[tokio::test(start_paused = true)]
    async fn cancelling_a_sequence_interrupts_a_wait_for_frames() {
        use crate::sequencer::{Sequence, SequenceState};

        let stacker = Arc::new(MockStacker::default());
        // Never gets past one frame.
        stacker.progress.script([stacked(StackState::Stacking, 1)]);
        let ctx = Arc::new(
            ExecutionContext::new(Arc::new(MockMount::default()))
                .with_live_stacker(stacker.clone()),
        );
        let mut sequence = Sequence::new("stuck", vec![imaging(Some(50))]);
        sequence.start(ctx);
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert_eq!(sequence.state(), SequenceState::Running);
        let asked = stacker.progress.asked();
        assert!(asked > 1);

        sequence.stop();
        assert_eq!(sequence.state(), SequenceState::Cancelled);
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert!(stacker.progress.asked() <= asked + 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_goto_with_an_observer_waits_until_the_telescope_is_on_target() {
        let mount = Arc::new(MockMount::default());
        let observer = Arc::new(MockObserver::default());
        observer.acquisition.script([
            Ok(AcquisitionState::Slewing),
            Ok(AcquisitionState::Centring),
            Ok(AcquisitionState::OnTarget),
        ]);
        let ctx = ExecutionContext::new(mount.clone()).with_observer(observer.clone());
        let goto = SequencerCommand::GoToTarget {
            name: "M31".into(),
            ra_hours: 0.712,
            dec_deg: 41.27,
            timeout_seconds: None,
            settle_seconds: None,
        };

        execute_command(&goto, &ctx).await.unwrap();
        assert_eq!(observer.targets.lock().unwrap().len(), 1);
        // The observer does the slewing.
        assert!(mount.slews.lock().unwrap().is_empty());

        observer
            .acquisition
            .script([Err("goto failed at AutoGoto: solve failed (code 251)")]);
        let err = execute_command(&goto, &ctx).await.unwrap_err();
        assert!(err.to_string().contains("solve failed"), "{err}");
    }

    #[tokio::test(start_paused = true)]
    async fn autofocus_waits_for_the_run_and_fails_with_it() {
        let focus = Arc::new(MockAutoFocus::default());
        focus.state.script([
            Ok(AutoFocusState::Running),
            Ok(AutoFocusState::Done {
                position: Some(1250),
            }),
        ]);
        let ctx =
            ExecutionContext::new(Arc::new(MockMount::default())).with_auto_focus(focus.clone());
        let command = SequencerCommand::AutoFocus {
            timeout_seconds: None,
        };
        execute_command(&command, &ctx).await.unwrap();

        focus.state.script([Err("autofocus failed")]);
        assert!(execute_command(&command, &ctx).await.is_err());

        let none = ExecutionContext::new(Arc::new(MockMount::default()));
        let err = execute_command(&command, &none).await.unwrap_err();
        assert!(err.is_not_supported(), "{err:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_filter_is_chosen_by_position_or_by_name() {
        let wheel = Arc::new(MockFilterWheel::default());
        let ctx =
            ExecutionContext::new(Arc::new(MockMount::default())).with_filter_wheel(wheel.clone());
        let set = |position, name: Option<&str>| SequencerCommand::SetFilter {
            position,
            name: name.map(str::to_string),
        };

        execute_command(&set(Some(2), None), &ctx).await.unwrap();
        assert_eq!(wheel.filter_position().await.unwrap().unwrap().position, 2);
        execute_command(&set(None, Some("b")), &ctx).await.unwrap();
        assert_eq!(wheel.filter_position().await.unwrap().unwrap().position, 3);

        for bad in [
            set(None, None),
            set(Some(1), Some("R")),
            set(None, Some("Ha")),
        ] {
            let err = execute_command(&bad, &ctx).await.unwrap_err();
            assert!(
                matches!(err, ScopinatorError::InvalidArgument(_)),
                "{err:?}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn park_waits_for_the_mount_to_be_parked() {
        let mount = Arc::new(MockMount::default());
        mount
            .parked
            .script([Answer::Is(false), Answer::Is(false), Answer::Is(true)]);
        let ctx = ExecutionContext::new(mount.clone());
        execute_command(&SequencerCommand::Park, &ctx)
            .await
            .unwrap();
        assert!(mount.parked.asked() >= 3);

        // A mount that cannot say is taken at its word.
        mount.parked.script([Answer::NotSupported]);
        execute_command(&SequencerCommand::Park, &ctx)
            .await
            .unwrap();
    }

    #[test]
    fn the_new_commands_have_the_json_they_are_documented_with() {
        let json = r#"[
            {"command_type": "AutoFocus"},
            {"command_type": "SetFilter", "name": "LP"},
            {"command_type": "SetFilter", "position": 1},
            {"command_type": "Park"},
            {"command_type": "Unpark"}
        ]"#;
        let commands: Vec<SequencerCommand> = serde_json::from_str(json).unwrap();
        assert_eq!(commands.len(), 5);
        assert_eq!(
            serde_json::to_string(&commands[1]).unwrap(),
            r#"{"command_type":"SetFilter","name":"LP"}"#
        );
    }
}
