use std::sync::Arc;

use async_trait::async_trait;
use scopinator_seestar::SeestarClient;
use scopinator_types::{DeviceId, DeviceInfo, DeviceKind, FocuserPosition};
use serde_json::{Value, json};

use crate::device::capabilities::FocuserCapabilities;
use crate::device::status::DeviceStatus;
use crate::device::traits::{Device, Focuser};
use crate::error::ScopinatorError;

/// A Seestar's focuser: absolute, in motor steps from zero to the
/// `max_step` the telescope reports (2600 on an S30).
///
/// The telescope's own autofocus is
/// [`SeestarAutoFocus`](super::SeestarAutoFocus), not this.
pub struct SeestarFocuser {
    client: Arc<SeestarClient>,
    device_id: DeviceId,
}

/// The `focuser` section of the device state.
struct FocuserState {
    step: i32,
    max_step: Option<i32>,
    moving: bool,
}

impl SeestarFocuser {
    pub(crate) fn new(client: Arc<SeestarClient>, device_id: DeviceId) -> Self {
        Self { client, device_id }
    }

    async fn state(&self) -> Result<FocuserState, ScopinatorError> {
        let state = self
            .client
            .send_raw_and_validate("get_device_state", Some(json!({"keys": ["focuser"]})))
            .await?;
        focuser_state(&state)
    }
}

fn focuser_state(state: &Value) -> Result<FocuserState, ScopinatorError> {
    let focuser = state.get("focuser").ok_or_else(|| {
        ScopinatorError::Backend(format!("get_device_state reply has no `focuser`: {state}"))
    })?;
    let step = focuser
        .get("step")
        .and_then(Value::as_i64)
        .and_then(|step| i32::try_from(step).ok())
        .ok_or_else(|| {
            ScopinatorError::Backend(format!("`focuser.step` is not a position: {state}"))
        })?;
    Ok(FocuserState {
        step,
        max_step: focuser
            .get("max_step")
            .and_then(Value::as_i64)
            .and_then(|max| i32::try_from(max).ok()),
        // `idle` or `moving`; anything else is taken as not at rest.
        moving: focuser.get("state").and_then(Value::as_str) != Some("idle"),
    })
}

#[async_trait]
impl Device for SeestarFocuser {
    fn id(&self) -> DeviceId {
        self.device_id.clone()
    }

    fn info(&self) -> DeviceInfo {
        let summary = self.client.device_info();
        let mut info = DeviceInfo::new(
            self.device_id.clone(),
            DeviceKind::Focuser,
            "Seestar focuser",
        );
        info.model = summary.model;
        info.firmware = summary.firmware_string;
        info
    }

    async fn connect(&self) -> Result<(), ScopinatorError> {
        Ok(())
    }

    async fn disconnect(&self) -> Result<(), ScopinatorError> {
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.client.is_control_connected()
    }

    async fn get_status(&self) -> Result<DeviceStatus, ScopinatorError> {
        Ok(DeviceStatus {
            connected: self.is_connected(),
            name: Some(self.device_id.to_string()),
            description: Some("Seestar focuser".into()),
        })
    }
}

#[async_trait]
impl Focuser for SeestarFocuser {
    /// The end of the travel is not known without asking the telescope:
    /// [`focuser_position`](Self::focuser_position) has it.
    fn capabilities(&self) -> FocuserCapabilities {
        FocuserCapabilities {
            absolute: true,
            ..FocuserCapabilities::default()
        }
    }

    async fn focuser_position(&self) -> Result<FocuserPosition, ScopinatorError> {
        let state = self.state().await?;
        Ok(FocuserPosition {
            position: state.step,
            max_position: state.max_step,
            temperature: None,
            is_moving: state.moving,
        })
    }

    /// `move_focuser` to `position`. A position outside the travel is
    /// refused here: the telescope would not refuse it, it would run to the
    /// end of its travel instead.
    async fn move_to(&self, position: i32) -> Result<(), ScopinatorError> {
        let max = self.state().await?.max_step;
        if position < 0 || max.is_some_and(|max| position > max) {
            return Err(ScopinatorError::OutOfRange(match max {
                Some(max) => format!("focuser position {position} is outside 0..={max}"),
                None => format!("focuser position {position} is negative"),
            }));
        }
        self.client
            .send_raw_and_validate(
                "move_focuser",
                Some(json!({"step": position, "ret_step": true})),
            )
            .await?;
        Ok(())
    }

    async fn move_relative(&self, offset: i32) -> Result<(), ScopinatorError> {
        let position = self.state().await?.step;
        let target = position.checked_add(offset).ok_or_else(|| {
            ScopinatorError::OutOfRange(format!("focuser move of {offset} from {position}"))
        })?;
        self.move_to(target).await
    }

    async fn is_moving(&self) -> Result<bool, ScopinatorError> {
        Ok(self.state().await?.moving)
    }

    /// `stop_focuser`: the focuser stops where it is.
    async fn halt(&self) -> Result<(), ScopinatorError> {
        self.client
            .send_raw_and_validate("stop_focuser", None)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shapes from an S30 on firmware 9.75.
    #[test]
    fn the_focuser_section_gives_position_travel_and_motion() {
        let idle = json!({"focuser": {"max_step": 2600, "state": "idle", "step": 1253}});
        let state = focuser_state(&idle).unwrap();
        assert_eq!(
            (state.step, state.max_step, state.moving),
            (1253, Some(2600), false)
        );

        let moving = json!({"focuser": {"max_step": 2600, "state": "moving", "step": 2379}});
        assert!(focuser_state(&moving).unwrap().moving);
    }

    #[test]
    fn a_reply_without_the_focuser_is_an_error_and_not_position_zero() {
        assert!(focuser_state(&json!({})).is_err());
        assert!(focuser_state(&json!({"focuser": {"state": "idle"}})).is_err());
    }
}
