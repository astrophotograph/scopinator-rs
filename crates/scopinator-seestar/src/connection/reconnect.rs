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

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const MIN_BACKOFF: Duration = Duration::from_secs(1);
    const MAX_BACKOFF: Duration = Duration::from_secs(60);

    #[test]
    fn first_backoff_is_min() {
        let mut p = ReconnectPolicy::new();
        assert_eq!(p.next_backoff(), MIN_BACKOFF);
    }

    #[test]
    fn backoff_doubles_each_failure() {
        let mut p = ReconnectPolicy::new();
        assert_eq!(p.next_backoff(), Duration::from_secs(1));
        assert_eq!(p.next_backoff(), Duration::from_secs(2));
        assert_eq!(p.next_backoff(), Duration::from_secs(4));
        assert_eq!(p.next_backoff(), Duration::from_secs(8));
        assert_eq!(p.next_backoff(), Duration::from_secs(16));
        assert_eq!(p.next_backoff(), Duration::from_secs(32));
        // 64 capped at 60
        assert_eq!(p.next_backoff(), MAX_BACKOFF);
        assert_eq!(p.next_backoff(), MAX_BACKOFF);
    }

    #[test]
    fn reset_returns_to_initial_state() {
        let mut p = ReconnectPolicy::new();
        for _ in 0..10 {
            p.next_backoff();
        }
        p.reset();
        assert_eq!(p.failures(), 0);
        assert_eq!(p.next_backoff(), MIN_BACKOFF);
    }

    proptest! {
        #[test]
        fn backoff_never_exceeds_max(failures in 0u32..256) {
            let mut p = ReconnectPolicy::new();
            for _ in 0..failures {
                let wait = p.next_backoff();
                prop_assert!(wait <= MAX_BACKOFF, "exceeded max: {wait:?}");
            }
        }

        #[test]
        fn backoff_never_below_min(failures in 1u32..256) {
            let mut p = ReconnectPolicy::new();
            for _ in 0..failures {
                let wait = p.next_backoff();
                prop_assert!(wait >= MIN_BACKOFF, "below min: {wait:?}");
            }
        }

        #[test]
        fn backoff_monotonic_until_cap(failures in 1u32..32) {
            let mut p = ReconnectPolicy::new();
            let mut prev = Duration::ZERO;
            for _ in 0..failures {
                let cur = p.next_backoff();
                // Either non-decreasing, or already at the cap.
                prop_assert!(
                    cur >= prev || cur == MAX_BACKOFF,
                    "non-monotonic: {prev:?} -> {cur:?}"
                );
                prev = cur;
            }
        }

        #[test]
        fn failure_count_matches_call_count(calls in 0u32..64) {
            let mut p = ReconnectPolicy::new();
            for _ in 0..calls {
                p.next_backoff();
            }
            prop_assert_eq!(p.failures(), calls);
        }

        #[test]
        fn reset_then_retry_starts_fresh(prefix in 0u32..32, suffix in 1u32..16) {
            let mut p = ReconnectPolicy::new();
            for _ in 0..prefix {
                p.next_backoff();
            }
            p.reset();
            prop_assert_eq!(p.failures(), 0);
            // First backoff after reset is always min.
            prop_assert_eq!(p.next_backoff(), MIN_BACKOFF);
            // Subsequent backoffs follow the same doubling pattern.
            let mut expected = MIN_BACKOFF;
            for _ in 1..suffix {
                expected = (expected * 2).min(MAX_BACKOFF);
                prop_assert_eq!(p.next_backoff(), expected);
            }
        }
    }
}
