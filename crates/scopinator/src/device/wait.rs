//! Waiting for an operation that takes time to finish.
//!
//! The methods that start a slew, a park, an exposure or a focuser or filter
//! move return when the device has accepted the request (see the
//! [`traits`](super::traits) module). These helpers wait for the operation
//! to end by polling its state query, so they work the same way for every
//! backend.

use std::future::Future;
use std::time::Duration;

use tokio::time::Instant;

use super::traits::{Camera, FilterWheel, Focuser, Mount};
use crate::error::ScopinatorError;

/// How to wait.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaitOptions {
    /// Time between two state queries.
    pub poll_interval: Duration,
    /// Give up with [`ScopinatorError::Timeout`] after this long.
    pub timeout: Duration,
    /// Extra time to wait after the operation is over, for the instrument to
    /// stop shaking.
    pub settle: Duration,
}

impl Default for WaitOptions {
    /// Poll twice a second, give up after five minutes, no settling time.
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(500),
            timeout: Duration::from_secs(300),
            settle: Duration::ZERO,
        }
    }
}

impl WaitOptions {
    /// The defaults with another timeout.
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            timeout,
            ..Self::default()
        }
    }
}

/// How many polls in a row must say "finished". One is not enough: a device
/// can report its idle state for a moment between accepting a request and
/// starting on it.
const POLLS_TO_CONFIRM: u32 = 2;

/// Poll `finished` until it has been true [`POLLS_TO_CONFIRM`] times in a
/// row. An error from the query ends the wait with that error.
async fn wait_until<F, Fut>(opts: WaitOptions, mut finished: F) -> Result<(), ScopinatorError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<bool, ScopinatorError>>,
{
    let deadline = Instant::now() + opts.timeout;
    let mut confirmed = 0;
    loop {
        if finished().await? {
            confirmed += 1;
            if confirmed >= POLLS_TO_CONFIRM {
                break;
            }
        } else {
            confirmed = 0;
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(ScopinatorError::Timeout);
        }
        tokio::time::sleep_until((now + opts.poll_interval).min(deadline)).await;
    }
    tokio::time::sleep(opts.settle).await;
    Ok(())
}

/// Wait for the mount to finish slewing: until [`Mount::is_slewing`] is
/// false. Returns the slew's error if it failed under way.
pub async fn wait_for_slew(mount: &dyn Mount, opts: WaitOptions) -> Result<(), ScopinatorError> {
    wait_until(opts, || async { Ok(!mount.is_slewing().await?) }).await
}

/// Wait for the mount to reach its park position: until
/// [`Mount::is_parked`] is true.
pub async fn wait_for_park(mount: &dyn Mount, opts: WaitOptions) -> Result<(), ScopinatorError> {
    wait_until(opts, || mount.is_parked()).await
}

/// Wait for the camera to have an image: until [`Camera::image_ready`] is
/// true.
pub async fn wait_for_image(camera: &dyn Camera, opts: WaitOptions) -> Result<(), ScopinatorError> {
    wait_until(opts, || camera.image_ready()).await
}

/// Wait for the focuser to stop: until [`Focuser::is_moving`] is false.
pub async fn wait_for_focuser(
    focuser: &dyn Focuser,
    opts: WaitOptions,
) -> Result<(), ScopinatorError> {
    wait_until(opts, || async { Ok(!focuser.is_moving().await?) }).await
}

/// Wait for the filter wheel to stop: until [`FilterWheel::is_moving`] is
/// false.
pub async fn wait_for_filter_wheel(
    wheel: &dyn FilterWheel,
    opts: WaitOptions,
) -> Result<(), ScopinatorError> {
    wait_until(opts, || async { Ok(!wheel.is_moving().await?) }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::mock::{Answer, MockCamera, MockFilterWheel, MockFocuser, MockMount};

    const POLL: Duration = Duration::from_millis(500);

    fn elapsed_since(start: Instant) -> Duration {
        Instant::now() - start
    }

    #[tokio::test(start_paused = true)]
    async fn slew_wait_ends_when_the_mount_stops() {
        let mount = MockMount::default();
        mount
            .slewing
            .script([Answer::Is(true), Answer::Is(true), Answer::Is(false)]);
        let start = Instant::now();
        wait_for_slew(&mount, WaitOptions::default()).await.unwrap();
        // true, true, false, false: the fourth query confirms the third.
        assert_eq!(mount.slewing.asked(), 4);
        assert_eq!(elapsed_since(start), POLL * 3);
    }

    #[tokio::test(start_paused = true)]
    async fn one_idle_reading_before_the_mount_starts_does_not_end_the_wait() {
        let mount = MockMount::default();
        // Accepted, briefly still "not slewing", then moving, then done.
        mount.slewing.script([
            Answer::Is(false),
            Answer::Is(true),
            Answer::Is(true),
            Answer::Is(false),
        ]);
        wait_for_slew(&mount, WaitOptions::default()).await.unwrap();
        assert_eq!(mount.slewing.asked(), 5);
    }

    #[tokio::test(start_paused = true)]
    async fn a_wait_times_out() {
        let mount = MockMount::default();
        mount.slewing.script([Answer::Is(true)]);
        let start = Instant::now();
        let err = wait_for_slew(&mount, WaitOptions::with_timeout(Duration::from_secs(10)))
            .await
            .unwrap_err();
        assert!(matches!(err, ScopinatorError::Timeout), "{err:?}");
        assert_eq!(elapsed_since(start), Duration::from_secs(10));
    }

    #[tokio::test(start_paused = true)]
    async fn a_timeout_shorter_than_the_poll_interval_is_kept() {
        let mount = MockMount::default();
        mount.slewing.script([Answer::Is(true)]);
        let start = Instant::now();
        let opts = WaitOptions::with_timeout(Duration::from_millis(200));
        assert!(matches!(
            wait_for_slew(&mount, opts).await,
            Err(ScopinatorError::Timeout)
        ));
        assert_eq!(elapsed_since(start), Duration::from_millis(200));
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_slew_ends_the_wait_with_its_error() {
        let mount = MockMount::default();
        mount.slewing.script([
            Answer::Is(true),
            Answer::Fails("goto failed"),
            Answer::Is(false),
        ]);
        let err = wait_for_slew(&mount, WaitOptions::default())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "backend error: goto failed");
        // Not retried.
        assert_eq!(mount.slewing.asked(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn settle_time_is_added_after_the_operation() {
        let mount = MockMount::default();
        mount.slewing.script([Answer::Is(false)]);
        let start = Instant::now();
        let opts = WaitOptions {
            settle: Duration::from_secs(3),
            ..WaitOptions::default()
        };
        wait_for_slew(&mount, opts).await.unwrap();
        assert_eq!(elapsed_since(start), POLL + Duration::from_secs(3));
    }

    #[tokio::test(start_paused = true)]
    async fn park_wait_ends_when_the_mount_is_parked() {
        let mount = MockMount::default();
        mount
            .parked
            .script([Answer::Is(false), Answer::Is(false), Answer::Is(true)]);
        wait_for_park(&mount, WaitOptions::default()).await.unwrap();
        assert_eq!(mount.parked.asked(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn image_wait_ends_when_an_image_is_ready() {
        let camera = MockCamera::default();
        camera.ready.script([Answer::Is(false), Answer::Is(true)]);
        wait_for_image(&camera, WaitOptions::default())
            .await
            .unwrap();
        assert_eq!(camera.ready.asked(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn focuser_and_filter_wheel_waits_end_when_they_stop() {
        let focuser = MockFocuser::default();
        focuser.moving.script([Answer::Is(true), Answer::Is(false)]);
        wait_for_focuser(&focuser, WaitOptions::default())
            .await
            .unwrap();
        assert_eq!(focuser.moving.asked(), 3);

        let wheel = MockFilterWheel::default();
        wheel.moving.script([Answer::Is(true), Answer::Is(false)]);
        wait_for_filter_wheel(&wheel, WaitOptions::default())
            .await
            .unwrap();
        assert_eq!(wheel.moving.asked(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn a_device_that_cannot_tell_ends_the_wait_with_not_supported() {
        let mount = MockMount::default();
        mount.slewing.script([Answer::NotSupported]);
        let err = wait_for_slew(&mount, WaitOptions::default())
            .await
            .unwrap_err();
        assert!(err.is_not_supported(), "{err:?}");
    }
}
