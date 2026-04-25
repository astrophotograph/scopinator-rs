use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use super::command::{SequencerCommand, execute_command};
use super::context::ExecutionContext;

/// State of a sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum SequenceState {
    Idle = 0,
    Running = 1,
    Paused = 2,
    Completed = 3,
    Failed = 4,
    Cancelled = 5,
}

impl SequenceState {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Idle,
            1 => Self::Running,
            2 => Self::Paused,
            3 => Self::Completed,
            4 => Self::Failed,
            5 => Self::Cancelled,
            _ => Self::Idle,
        }
    }
}

/// An observation sequence: an ordered list of commands with lifecycle management.
pub struct Sequence {
    pub name: String,
    pub description: Option<String>,
    pub commands: Vec<SequencerCommand>,
    state: Arc<AtomicU8>,
    resume_notify: Arc<Notify>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    execution_handle: Option<JoinHandle<()>>,
}

impl Sequence {
    /// Create a new sequence.
    pub fn new(name: impl Into<String>, commands: Vec<SequencerCommand>) -> Self {
        Self {
            name: name.into(),
            description: None,
            commands,
            state: Arc::new(AtomicU8::new(SequenceState::Idle as u8)),
            resume_notify: Arc::new(Notify::new()),
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            execution_handle: None,
        }
    }

    /// Get the current state.
    pub fn state(&self) -> SequenceState {
        SequenceState::from_u8(self.state.load(Ordering::Acquire))
    }

    fn set_state(&self, s: SequenceState) {
        self.state.store(s as u8, Ordering::Release);
    }

    /// Start the sequence.
    ///
    /// The context is consumed by the background task. Commands execute sequentially.
    pub fn start(&mut self, ctx: Arc<ExecutionContext>) {
        if self.state() != SequenceState::Idle {
            return;
        }

        self.set_state(SequenceState::Running);
        self.started_at = Some(Utc::now());

        let commands = self.commands.clone();
        let state = Arc::clone(&self.state);
        let resume_notify = Arc::clone(&self.resume_notify);
        let name = self.name.clone();

        self.execution_handle = Some(tokio::spawn(async move {
            info!(sequence = name, "sequence started");
            run_sequence(commands, ctx, state, resume_notify, &name).await;
        }));
    }

    /// Stop (cancel) the sequence.
    pub fn stop(&mut self) {
        if self.state() != SequenceState::Running && self.state() != SequenceState::Paused {
            return;
        }

        self.set_state(SequenceState::Cancelled);
        self.completed_at = Some(Utc::now());

        if let Some(handle) = self.execution_handle.take() {
            handle.abort();
        }

        // Wake any paused waiter
        self.resume_notify.notify_one();
        info!(sequence = self.name, "sequence cancelled");
    }

    /// Pause the sequence.
    pub fn pause(&self) {
        if self.state() == SequenceState::Running {
            self.set_state(SequenceState::Paused);
            debug!(sequence = self.name, "sequence paused");
        }
    }

    /// Resume a paused sequence.
    pub fn resume(&self) {
        if self.state() == SequenceState::Paused {
            self.set_state(SequenceState::Running);
            self.resume_notify.notify_one();
            debug!(sequence = self.name, "sequence resumed");
        }
    }

    /// Returns true if the sequence has finished (completed, failed, or cancelled).
    pub fn is_finished(&self) -> bool {
        matches!(
            self.state(),
            SequenceState::Completed | SequenceState::Failed | SequenceState::Cancelled
        )
    }
}

impl Drop for Sequence {
    fn drop(&mut self) {
        if let Some(handle) = self.execution_handle.take() {
            handle.abort();
        }
    }
}

/// Internal execution loop for a sequence.
async fn run_sequence(
    commands: Vec<SequencerCommand>,
    ctx: Arc<ExecutionContext>,
    state: Arc<AtomicU8>,
    resume_notify: Arc<Notify>,
    name: &str,
) {
    for (i, cmd) in commands.iter().enumerate() {
        // Check for cancellation
        let current = SequenceState::from_u8(state.load(Ordering::Acquire));
        if current == SequenceState::Cancelled {
            debug!(sequence = name, step = i, "sequence cancelled, stopping");
            return;
        }

        // Handle pause — wait on Notify instead of busy-polling
        while SequenceState::from_u8(state.load(Ordering::Acquire)) == SequenceState::Paused {
            debug!(sequence = name, step = i, "paused, waiting for resume");
            resume_notify.notified().await;

            // Check if we were cancelled while paused
            if SequenceState::from_u8(state.load(Ordering::Acquire)) == SequenceState::Cancelled {
                return;
            }
        }

        debug!(sequence = name, step = i, "executing command");

        match execute_command(cmd, &ctx).await {
            Ok(()) => {
                debug!(sequence = name, step = i, "command completed");
            }
            Err(e) => {
                error!(sequence = name, step = i, error = %e, "command failed");
                state.store(SequenceState::Failed as u8, Ordering::Release);
                return;
            }
        }
    }

    // All commands completed successfully
    let current = SequenceState::from_u8(state.load(Ordering::Acquire));
    if current == SequenceState::Running {
        state.store(SequenceState::Completed as u8, Ordering::Release);
        info!(sequence = name, "sequence completed");
    }
}
