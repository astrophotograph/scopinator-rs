use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, error};

use scopinator_types::{Coordinates, DecDegrees, ExposureSettings, RaDegrees};

use super::context::ExecutionContext;
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
    /// Slew to a target by RA/Dec.
    GoToTarget {
        name: String,
        /// RA in hours (0-24).
        ra_hours: f64,
        /// Dec in degrees (-90 to 90).
        dec_deg: f64,
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
        } => {
            debug!(target = name, ra = ra_hours, dec = dec_deg, "goto target");
            let ra = RaDegrees::from_hours(*ra_hours)
                .map_err(|e| ScopinatorError::InvalidArgument(e.to_string()))?;
            let dec = DecDegrees::new(*dec_deg)
                .map_err(|e| ScopinatorError::InvalidArgument(e.to_string()))?;
            let coords = Coordinates::new(ra, dec);
            ctx.mount.slew_to_coordinates(&coords).await?;
            Ok(())
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
            let settings = ExposureSettings {
                duration_seconds: *exposure_seconds,
                gain: *gain,
                ..Default::default()
            };
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
}
