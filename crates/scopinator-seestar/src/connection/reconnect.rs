use std::time::Duration;

use tracing::debug;

/// Reconnection state with exponential backoff.
pub struct ReconnectPolicy {
    /// Current backoff duration.
    backoff: Duration,
    /// Minimum backoff.
    min_backoff: Duration,
    /// Maximum backoff.
    max_backoff: Duration,
    /// Number of consecutive failures.
    failures: u32,
}

impl ReconnectPolicy {
    pub fn new() -> Self {
        Self {
            backoff: Duration::from_secs(1),
            min_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(60),
            failures: 0,
        }
    }

    /// Record a failure and return the duration to wait before retrying.
    pub fn next_backoff(&mut self) -> Duration {
        self.failures += 1;
        let wait = self.backoff;

        // Double the backoff, capped at max
        self.backoff = (self.backoff * 2).min(self.max_backoff);

        debug!(
            failures = self.failures,
            wait_secs = wait.as_secs_f32(),
            "reconnect backoff"
        );
        wait
    }

    /// Record a successful connection, resetting the backoff.
    pub fn reset(&mut self) {
        self.backoff = self.min_backoff;
        self.failures = 0;
    }

    /// Number of consecutive failures.
    pub fn failures(&self) -> u32 {
        self.failures
    }
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self::new()
    }
}
