use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, warn};

use scopinator_types::{Coordinates, DecDegrees, ExposureSettings, RaDegrees};

use super::context::ExecutionContext;
use crate::device::wait::{WaitOptions, wait_for_slew};
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
    /// Start camera imaging/stacking.
    StartImaging {
        /// Exposure time in seconds.
        exposure_seconds: f64,
        /// Gain (camera-specific).
        gain: Option<i32>,
        /// Number of frames (None = unlimited).
        count: Option<u32>,
    },
    /// Stop camera imaging.
    StopImaging,
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
            count: _,
        } => {
            let camera = ctx
                .camera
                .as_ref()
                .ok_or_else(|| ScopinatorError::NotSupported("no camera in context".into()))?;
            debug!(exposure = exposure_seconds, gain = ?gain, "start imaging");
            let mut settings = ExposureSettings::new(*exposure_seconds);
            settings.gain = *gain;
            camera.start_exposure(&settings).await?;
            Ok(())
        }

        SequencerCommand::StopImaging => {
            let camera = ctx
                .camera
                .as_ref()
                .ok_or_else(|| ScopinatorError::NotSupported("no camera in context".into()))?;
            debug!("stop imaging");
            camera.abort_exposure().await?;
            Ok(())
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
        ExecutionContext {
            mount: Arc::clone(mount) as Arc<dyn crate::device::Mount>,
            camera: Some(Arc::clone(camera) as Arc<dyn crate::device::Camera>),
        }
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
                    count: None,
                },
            ],
            stop_on_error: true,
        };

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
}
