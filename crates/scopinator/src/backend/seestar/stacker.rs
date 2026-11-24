use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use scopinator_seestar::SeestarClient;
use scopinator_seestar::client::SettingOutcome;
use scopinator_seestar::command::Command;
use scopinator_seestar::command::params::{
    SettingParams, StartStackParams, StopStage, StopViewParams,
};
use scopinator_types::{BayerPattern, ImageData, StackProgress, StackSettings, StackState};
use serde_json::Value;

use crate::device::smart::{LiveStacker, StackerCapabilities};
use crate::error::ScopinatorError;

/// The filter a Seestar stacks through with `stack_lenhance` off.
const FILTER_IR_CUT: &str = "IR cut";
/// The light-pollution filter (`stack_lenhance` on).
const FILTER_LP: &str = "LP";

/// Live stacking on a Seestar: `iscope_start_stack`, the `Stack` stage of
/// the view state, and the stacked image from the image socket.
///
/// A Seestar stacks what its star view is showing, so a view has to be
/// running first (a goto leaves one running).
pub struct SeestarStacker {
    client: Arc<SeestarClient>,
    /// A stack was started through this stacker and has not been seen to
    /// end: its failure is still to be reported.
    started: AtomicBool,
}

impl SeestarStacker {
    pub(crate) fn new(client: Arc<SeestarClient>) -> Self {
        Self {
            client,
            started: AtomicBool::new(false),
        }
    }

    async fn view_state(&self) -> Result<Value, ScopinatorError> {
        Ok(self.client.send_and_validate(Command::GetViewState).await?)
    }

    /// Change one device setting and insist that it took.
    async fn apply(&self, what: &str, params: SettingParams) -> Result<(), ScopinatorError> {
        for result in self.client.set_setting(params).await? {
            match result.outcome {
                SettingOutcome::Applied => {}
                SettingOutcome::Rejected { code, message } => {
                    return Err(ScopinatorError::OutOfRange(format!(
                        "the telescope rejected {what}: {message} (code {code})"
                    )));
                }
                SettingOutcome::Unconfirmed => {
                    return Err(ScopinatorError::Backend(format!(
                        "could not confirm that the telescope applied {what}"
                    )));
                }
            }
        }
        Ok(())
    }
}

/// The stacker's progress from a `get_view_state` reply: the `Stack` stage
/// under `View`. No such stage means nothing is being stacked.
fn progress_from(view: &Value) -> Result<StackProgress, String> {
    let mut progress = StackProgress::default();
    let Some(stack) = view.pointer("/View/Stack") else {
        return Ok(progress);
    };
    let count = |name: &str| {
        stack
            .get(name)
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0)
    };
    progress.stacked = count("stacked_frame");
    progress.dropped = count("dropped_frame");
    // The sub-exposure in use is under the `Exposure` stage.
    progress.integration = stack
        .pointer("/Exposure/exp_ms")
        .and_then(Value::as_f64)
        .filter(|ms| ms.is_finite() && *ms > 0.0)
        .map(|ms| Duration::from_secs_f64(ms / 1000.0 * f64::from(progress.stacked)));
    match stack.get("state").and_then(Value::as_str) {
        Some("start" | "working") => progress.state = StackState::Stacking,
        Some("fail") => {
            let error = stack
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("no reason given");
            return Err(match stack.get("code").and_then(Value::as_i64) {
                Some(code) => format!("{error} (code {code})"),
                None => error.to_string(),
            });
        }
        _ => {}
    }
    Ok(progress)
}

#[async_trait]
impl LiveStacker for SeestarStacker {
    fn capabilities(&self) -> StackerCapabilities {
        StackerCapabilities {
            filters: vec![FILTER_IR_CUT.to_string(), FILTER_LP.to_string()],
            can_limit_frames: false,
            ..StackerCapabilities::default()
        }
    }

    /// Apply the settings, then `iscope_start_stack`.
    ///
    /// The sub-exposure is the `exp_ms.stack_l` device setting and the
    /// filter is `stack_lenhance`; both stay as set after the stack ends.
    /// A Seestar cannot stop after a number of frames.
    async fn start_stacking(&self, settings: &StackSettings) -> Result<(), ScopinatorError> {
        // Everything is checked before anything is changed.
        if settings.frame_limit.is_some() {
            return Err(ScopinatorError::NotSupported(
                "a frame limit: a Seestar stacks until it is stopped".into(),
            ));
        }
        let lp_filter = match settings.filter.as_deref() {
            None => None,
            Some(FILTER_IR_CUT) => Some(false),
            Some(FILTER_LP) => Some(true),
            Some(other) => {
                return Err(ScopinatorError::InvalidArgument(format!(
                    "no filter `{other}` (a Seestar has `{FILTER_IR_CUT}` and `{FILTER_LP}`)"
                )));
            }
        };
        let exposure_ms = match settings.exposure {
            None => None,
            Some(exposure) => {
                let ms = exposure.as_millis();
                if ms == 0 {
                    return Err(ScopinatorError::OutOfRange("an exposure of zero".into()));
                }
                Some(u64::try_from(ms).map_err(|_| {
                    ScopinatorError::OutOfRange(format!("an exposure of {exposure:?}"))
                })?)
            }
        };
        let view = self.view_state().await?;
        let viewing = matches!(
            view.pointer("/View/state").and_then(Value::as_str),
            Some("start" | "working")
        );
        if !viewing || view.pointer("/View/mode").and_then(Value::as_str) != Some("star") {
            return Err(ScopinatorError::InvalidState(
                "a Seestar stacks its star view, and none is running; find a target first".into(),
            ));
        }

        if let Some(ms) = exposure_ms {
            let params = SettingParams {
                exp_ms: Some(serde_json::json!({"stack_l": ms})),
                ..SettingParams::default()
            };
            self.apply("the exposure", params).await?;
        }
        if let Some(on) = lp_filter {
            let params = SettingParams {
                stack_lenhance: Some(on),
                ..SettingParams::default()
            };
            self.apply("the filter", params).await?;
        }
        if let Some(gain) = settings.gain {
            self.client
                .send_and_validate(Command::SetControlValue("gain".into(), gain))
                .await?;
        }
        let params = settings.restart.then_some(StartStackParams {
            restart: Some(true),
        });
        self.client
            .send_and_validate(Command::IscopeStartStack(params))
            .await?;
        self.started.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn stop_stacking(&self) -> Result<(), ScopinatorError> {
        self.started.store(false, Ordering::SeqCst);
        self.client
            .send_and_validate(Command::IscopeStopView(Some(StopViewParams {
                stage: StopStage::Stack,
            })))
            .await?;
        Ok(())
    }

    async fn progress(&self) -> Result<StackProgress, ScopinatorError> {
        match progress_from(&self.view_state().await?) {
            Ok(progress) => Ok(progress),
            // The failed stage stays in the view state: report the failure
            // of a stack started here once, and after that the stack is
            // simply not running.
            Err(failure) => {
                if self.started.swap(false, Ordering::SeqCst) {
                    Err(ScopinatorError::Backend(format!(
                        "stacking failed: {failure}"
                    )))
                } else {
                    Ok(StackProgress::default())
                }
            }
        }
    }

    async fn stacked_image(&self) -> Result<ImageData, ScopinatorError> {
        use scopinator_seestar::SeestarError;
        let frame = match self.client.get_stacked_image().await {
            Ok(frame) => frame,
            // The telescope's answer when it has no stack.
            Err(SeestarError::ImageRequest(message)) => {
                return Err(ScopinatorError::InvalidState(format!(
                    "no stacked image: {message}"
                )));
            }
            Err(e) => return Err(e.into()),
        };
        let pattern = super::bayer_pattern(self.client.device_info().debayer_pattern.as_deref())
            .unwrap_or(BayerPattern::Grbg);
        match super::image_from_frame(&frame, pattern) {
            Err(ScopinatorError::Seestar(SeestarError::ImageRequest(message))) => Err(
                ScopinatorError::InvalidState(format!("no stacked image: {message}")),
            ),
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    // The shape is from the 6.70 corpus: a star view of M44, 225 frames in.
    fn stacking_view() -> Value {
        json!({"View": {"state": "working", "lapse_ms": 3257511, "mode": "star", "cam_id": 0,
            "target_name": "M44", "lp_filter": false, "gain": 200,
            "Stack": {"state": "working", "lapse_ms": 3243809, "frame_errcode": 0,
                "stacked_frame": 225, "dropped_frame": 16, "can_annotate": true,
                "Exposure": {"state": "working", "lapse_ms": 6925, "exp_ms": 10000.0},
                "stage": "Exposure"}}})
    }

    #[test]
    fn progress_is_read_from_the_stack_stage_of_the_view() {
        let progress = progress_from(&stacking_view()).unwrap();
        assert_eq!(progress.state, StackState::Stacking);
        assert_eq!((progress.stacked, progress.dropped), (225, 16));
        // 225 frames of ten seconds.
        assert_eq!(progress.integration, Some(Duration::from_secs(2250)));
        assert_eq!(progress.frame_limit, None);
    }

    #[test]
    fn no_stack_stage_is_an_idle_empty_stack() {
        for view in [
            json!({}),
            json!({"View": {"state": "working", "mode": "star"}}),
        ] {
            assert_eq!(progress_from(&view).unwrap(), StackProgress::default());
        }
    }

    #[test]
    fn a_stopped_stack_keeps_its_counts() {
        let mut view = stacking_view();
        view["View"]["Stack"]["state"] = json!("cancel");
        let progress = progress_from(&view).unwrap();
        assert_eq!(progress.state, StackState::Idle);
        assert_eq!(progress.stacked, 225);
    }

    #[test]
    fn a_failed_stack_is_an_error_with_its_reason() {
        let view = json!({"View": {"state": "working", "mode": "star",
            "Stack": {"state": "fail", "error": "no star detected", "code": 268}}});
        assert_eq!(
            progress_from(&view).unwrap_err(),
            "no star detected (code 268)"
        );
    }

    #[test]
    fn a_frame_error_is_not_a_failed_stack() {
        // From the corpus: one bad frame while the stack carries on.
        let mut view = stacking_view();
        view["View"]["Stack"]["frame_errcode"] = json!(-1);
        assert_eq!(progress_from(&view).unwrap().state, StackState::Stacking);
    }
}
