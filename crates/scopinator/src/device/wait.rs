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

use scopinator_types::{
    AcquisitionState, AutoFocusState, PlateSolution, Readiness, StackProgress, StackState,
};

use super::smart::{AutoFocus, LiveStacker, Observer, PlateSolver};
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

/// Poll `ready` until it has a value. Unlike [`wait_until`], one answer is
/// enough: a value is not something a device reports by accident. An error
/// from the query ends the wait with that error.
async fn wait_for<T, F, Fut>(opts: WaitOptions, mut ready: F) -> Result<T, ScopinatorError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Option<T>, ScopinatorError>>,
{
    let deadline = Instant::now() + opts.timeout;
    loop {
        if let Some(value) = ready().await? {
            tokio::time::sleep(opts.settle).await;
            return Ok(value);
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(ScopinatorError::Timeout);
        }
        tokio::time::sleep_until((now + opts.poll_interval).min(deadline)).await;
    }
}

/// Wait for a telescope to be ready to find targets, for example after
/// [`Observer::prepare`]: until [`Observer::readiness`] is `Ready`.
pub async fn wait_for_ready(
    observer: &dyn Observer,
    opts: WaitOptions,
) -> Result<(), ScopinatorError> {
    wait_until(opts, || async {
        Ok(observer.readiness().await? == Readiness::Ready)
    })
    .await
}

/// Wait for a telescope to have its target in view: until
/// [`Observer::acquisition`] is `OnTarget`. Returns the acquisition's error
/// if it failed, and [`InvalidState`](ScopinatorError::InvalidState) if it
/// ended without reaching the target (it was stopped, or never started).
pub async fn wait_for_acquisition(
    observer: &dyn Observer,
    opts: WaitOptions,
) -> Result<(), ScopinatorError> {
    // Idle ends the wait too, but has to be seen twice: a telescope can be
    // idle for a moment between accepting a target and starting on it.
    wait_until(opts, || async {
        Ok(matches!(
            observer.acquisition().await?,
            AcquisitionState::OnTarget | AcquisitionState::Idle
        ))
    })
    .await?;
    match observer.acquisition().await? {
        AcquisitionState::OnTarget => Ok(()),
        state => Err(ScopinatorError::InvalidState(format!(
            "the telescope stopped looking for the target ({state:?})"
        ))),
    }
}

/// Wait for an autofocus run to finish and return the focuser position it
/// ended at, if the telescope reports one. Returns the run's error if it
/// failed, and [`InvalidState`](ScopinatorError::InvalidState) if no run
/// took place.
pub async fn wait_for_autofocus(
    focus: &dyn AutoFocus,
    opts: WaitOptions,
) -> Result<Option<i32>, ScopinatorError> {
    wait_until(opts, || async {
        Ok(focus.autofocus_state().await? != AutoFocusState::Running)
    })
    .await?;
    match focus.autofocus_state().await? {
        AutoFocusState::Done { position } => Ok(position),
        state => Err(ScopinatorError::InvalidState(format!(
            "no autofocus run finished ({state:?})"
        ))),
    }
}

/// Wait for a plate solve to finish and return its solution.
pub async fn wait_for_solution(
    solver: &dyn PlateSolver,
    opts: WaitOptions,
) -> Result<PlateSolution, ScopinatorError> {
    wait_for(opts, || solver.solution()).await
}

/// Wait until a stack has at least `frames` frames in it and return its
/// progress then. Returns the stack's error if it failed, and
/// [`InvalidState`](ScopinatorError::InvalidState) if stacking ended with
/// fewer frames.
pub async fn wait_for_frames(
    stacker: &dyn LiveStacker,
    frames: u32,
    opts: WaitOptions,
) -> Result<StackProgress, ScopinatorError> {
    // A stacker that is not stacking is given one more look before the wait
    // gives up, for the moment between a start being accepted and the
    // telescope acting on it.
    let deadline = Instant::now() + opts.timeout;
    let mut seen_stopped = false;
    loop {
        let progress = stacker.progress().await?;
        if progress.stacked >= frames {
            tokio::time::sleep(opts.settle).await;
            return Ok(progress);
        }
        if progress.state != StackState::Stacking {
            if seen_stopped {
                return Err(ScopinatorError::InvalidState(format!(
                    "stacking ended with {} of {frames} frames",
                    progress.stacked
                )));
            }
            seen_stopped = true;
        } else {
            seen_stopped = false;
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(ScopinatorError::Timeout);
        }
        tokio::time::sleep_until((now + opts.poll_interval).min(deadline)).await;
    }
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

    #[tokio::test]
    async fn a_mount_without_the_optional_requests_says_not_supported() {
        use scopinator_types::{Coordinates, MountAxis};

        let mount = MockMount::default();
        let somewhere = Coordinates::from_hours(1.0, 2.0).unwrap();
        assert!(mount.unpark().await.unwrap_err().is_not_supported());
        assert!(
            mount
                .sync_to_coordinates(&somewhere)
                .await
                .unwrap_err()
                .is_not_supported()
        );
        assert!(
            mount
                .move_axis(MountAxis::Primary, 0.5)
                .await
                .unwrap_err()
                .is_not_supported()
        );
        assert!(mount.stop_motion().await.unwrap_err().is_not_supported());
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

    // --- Smart telescopes ---

    use crate::device::mock::{MockAutoFocus, MockObserver, MockSolver, MockStacker};
    use scopinator_types::{Coordinates, Prerequisite};

    fn stacking(stacked: u32) -> Result<StackProgress, &'static str> {
        let mut progress = StackProgress::default();
        progress.state = StackState::Stacking;
        progress.stacked = stacked;
        Ok(progress)
    }

    fn stopped(stacked: u32) -> Result<StackProgress, &'static str> {
        let mut progress = StackProgress::default();
        progress.stacked = stacked;
        Ok(progress)
    }

    #[tokio::test(start_paused = true)]
    async fn ready_wait_ends_when_nothing_is_needed_any_more() {
        let observer = MockObserver::default();
        observer.readiness.script([
            Ok(Readiness::Needs(vec![Prerequisite::Calibration])),
            Ok(Readiness::Needs(vec![Prerequisite::Calibration])),
            Ok(Readiness::Ready),
        ]);
        wait_for_ready(&observer, WaitOptions::default())
            .await
            .unwrap();
        assert_eq!(observer.readiness.asked(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn a_telescope_that_never_gets_ready_times_out() {
        let observer = MockObserver::default();
        observer.readiness.script([Ok(Readiness::Unknown)]);
        let err = wait_for_ready(&observer, WaitOptions::with_timeout(Duration::from_secs(5)))
            .await
            .unwrap_err();
        assert!(matches!(err, ScopinatorError::Timeout), "{err:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn acquisition_wait_goes_through_slewing_and_centring_to_on_target() {
        let observer = MockObserver::default();
        observer.acquisition.script([
            // Accepted, not started yet.
            Ok(AcquisitionState::Idle),
            Ok(AcquisitionState::Slewing),
            Ok(AcquisitionState::Centring),
            Ok(AcquisitionState::OnTarget),
        ]);
        wait_for_acquisition(&observer, WaitOptions::default())
            .await
            .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn an_acquisition_that_fails_ends_the_wait_with_its_error() {
        let observer = MockObserver::default();
        observer.acquisition.script([
            Ok(AcquisitionState::Slewing),
            Err("plate solve failed"),
            Ok(AcquisitionState::Idle),
        ]);
        let err = wait_for_acquisition(&observer, WaitOptions::default())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "backend error: plate solve failed");
    }

    #[tokio::test(start_paused = true)]
    async fn an_acquisition_that_was_stopped_is_an_invalid_state_not_a_success() {
        let observer = MockObserver::default();
        observer
            .acquisition
            .script([Ok(AcquisitionState::Slewing), Ok(AcquisitionState::Idle)]);
        let err = wait_for_acquisition(&observer, WaitOptions::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ScopinatorError::InvalidState(_)), "{err:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn autofocus_wait_returns_where_the_focuser_ended() {
        let focus = MockAutoFocus::default();
        focus.state.script([
            Ok(AutoFocusState::Running),
            Ok(AutoFocusState::Running),
            Ok(AutoFocusState::Done {
                position: Some(1580),
            }),
        ]);
        let position = wait_for_autofocus(&focus, WaitOptions::default())
            .await
            .unwrap();
        assert_eq!(position, Some(1580));
    }

    #[tokio::test(start_paused = true)]
    async fn waiting_for_an_autofocus_that_never_ran_is_an_invalid_state() {
        let focus = MockAutoFocus::default();
        focus.state.script([Ok(AutoFocusState::Idle)]);
        let err = wait_for_autofocus(&focus, WaitOptions::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ScopinatorError::InvalidState(_)), "{err:?}");

        let failing = MockAutoFocus::default();
        failing
            .state
            .script([Ok(AutoFocusState::Running), Err("no stars")]);
        let err = wait_for_autofocus(&failing, WaitOptions::default())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "backend error: no stars");
    }

    #[tokio::test(start_paused = true)]
    async fn solution_wait_returns_the_solution() {
        let solver = MockSolver::default();
        let centre = Coordinates::from_hours(8.69, 19.5).unwrap();
        solver
            .solution
            .script([Ok(None), Ok(None), Ok(Some(PlateSolution::new(centre)))]);
        let start = Instant::now();
        let solution = wait_for_solution(&solver, WaitOptions::default())
            .await
            .unwrap();
        assert_eq!(solution.center, centre);
        // A value needs no second look.
        assert_eq!(solver.solution.asked(), 3);
        assert_eq!(elapsed_since(start), POLL * 2);

        let failing = MockSolver::default();
        failing.solution.script([Ok(None), Err("too few stars")]);
        assert!(
            wait_for_solution(&failing, WaitOptions::default())
                .await
                .is_err()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn frames_wait_ends_at_the_frame_count() {
        let stacker = MockStacker::default();
        stacker.progress.script([
            stacking(0),
            stacking(3),
            stacking(9),
            stacking(10),
            stacking(11),
        ]);
        let progress = wait_for_frames(&stacker, 10, WaitOptions::default())
            .await
            .unwrap();
        assert_eq!(progress.stacked, 10);
        assert_eq!(stacker.progress.asked(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn a_stack_that_ends_short_is_an_invalid_state() {
        let stacker = MockStacker::default();
        // Not started for one poll, stacks four frames, then stops.
        stacker
            .progress
            .script([stopped(0), stacking(2), stacking(4), stopped(4)]);
        let err = wait_for_frames(&stacker, 10, WaitOptions::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ScopinatorError::InvalidState(_)), "{err:?}");
        assert!(err.to_string().contains("4 of 10"), "{err}");
        // One idle reading before stacking began did not end the wait.
        assert_eq!(stacker.progress.asked(), 5);
    }

    #[tokio::test(start_paused = true)]
    async fn a_stack_already_long_enough_returns_at_once_even_if_stopped() {
        let stacker = MockStacker::default();
        stacker.progress.script([stopped(30)]);
        let start = Instant::now();
        let progress = wait_for_frames(&stacker, 10, WaitOptions::default())
            .await
            .unwrap();
        assert_eq!(progress.stacked, 30);
        assert_eq!(elapsed_since(start), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_stack_times_out() {
        let stacker = MockStacker::default();
        stacker.progress.script([stacking(1)]);
        let err = wait_for_frames(
            &stacker,
            10,
            WaitOptions::with_timeout(Duration::from_secs(20)),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ScopinatorError::Timeout), "{err:?}");
    }
}
