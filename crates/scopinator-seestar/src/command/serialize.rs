use serde_json::{Value, json};

use super::Command;
use scopinator_types::FirmwareVersion;

/// Serialize a command to a JSON-RPC message, ready to send over the wire.
///
/// The returned `Value` has `id`, `method`, and optionally `params` fields.
/// If `firmware_version` indicates the telescope requires verify injection,
/// the "verify" param is automatically added.
pub fn serialize_command(
    cmd: &Command,
    id: u64,
    firmware_version: Option<FirmwareVersion>,
) -> Value {
    let method = cmd.method();
    let params = command_params(cmd);
    let needs_verify = firmware_version.is_none_or(|fw| fw.requires_verify());

    let mut msg = json!({
        "id": id,
        "method": method,
    });

    let final_params = inject_verify(params, needs_verify);
    if let Some(p) = final_params {
        msg["params"] = p;
    }

    msg
}

/// Extract the params value for a command.
fn command_params(cmd: &Command) -> Option<Value> {
    match cmd {
        // No-param commands
        Command::TestConnection
        | Command::PiIsVerified
        | Command::PiReboot
        | Command::PiGetTime
        | Command::GetDeviceState
        | Command::GetViewState
        | Command::GetCameraInfo
        | Command::GetCameraState
        | Command::GetSetting
        | Command::GetStackSetting
        | Command::GetStackInfo
        | Command::GetDiskVolume
        | Command::GetUserLocation
        | Command::GetWheelPosition
        | Command::GetWheelSetting
        | Command::GetWheelState
        | Command::GetLastSolveResult
        | Command::GetSolveResult
        | Command::GetAnnotatedResult
        | Command::ScopeGetEquCoord
        | Command::ScopeGetRaDec
        | Command::ScopeGetHorizCoord
        | Command::ScopePark
        | Command::ScopeMoveToHorizon
        | Command::GetFocuserPosition
        | Command::StartAutoFocus
        | Command::StopAutoFocus
        | Command::BeginStreaming
        | Command::StopStreaming
        | Command::GetStackedImage
        | Command::StartSolve
        | Command::StartScanPlanet => None,

        // Mount mode switch: scope_park with equ_mode param
        Command::ScopeParkMode(eq) => Some(json!({ "equ_mode": eq })),

        // Tuple params
        Command::ScopeSync(ra, dec) => Some(json!([ra, dec])),
        Command::ScopeSetTrackState(enabled) => Some(json!(enabled)),
        Command::SetControlValue(name, value) => Some(json!([name, value])),

        // Struct params
        Command::GotoTarget(p) => serde_json::to_value(p).ok(),
        Command::IscopeStartView(p) => serde_json::to_value(p).ok(),
        Command::IscopeStopView(p) => Some(json!({"stage": format!("{:?}", p.stage)})),
        Command::IscopeStartStack(Some(p)) => serde_json::to_value(p).ok(),
        Command::IscopeStartStack(None) => None,
        Command::ScopeSpeedMove(p) => serde_json::to_value(p).ok(),
        Command::MoveFocuser(p) => serde_json::to_value(p).ok(),
        Command::PiSetTime(p) => Some(json!([p])),
        Command::SetUserLocation(p) => serde_json::to_value(p).ok(),
        Command::SetSetting(p) => serde_json::to_value(p).ok(),
        Command::SetStackSetting(p) => serde_json::to_value(p).ok(),
        Command::PiOutputSet2(v) => Some(v.clone()),
        Command::SetViewPlan(v) => Some(v.clone()),
        Command::StopViewPlan => Some(json!({"name": "ViewPlan"})),
    }
}

/// Inject `"verify"` into command params when required by firmware.
///
/// The injection strategy matches pyscopinator's `_transform_message_for_verify`:
/// - Dict params: adds `"verify": true`
/// - Array params: appends `"verify"` string
/// - Scalar/bool params: wraps in array with `"verify"`
/// - No params: becomes `["verify"]`
fn inject_verify(params: Option<Value>, needs_verify: bool) -> Option<Value> {
    if !needs_verify {
        return params;
    }

    match params {
        None => Some(json!(["verify"])),
        Some(Value::Object(mut map)) => {
            map.insert("verify".to_string(), Value::Bool(true));
            Some(Value::Object(map))
        }
        Some(Value::Array(mut arr)) => {
            arr.push(Value::String("verify".to_string()));
            Some(Value::Array(arr))
        }
        Some(other) => Some(json!([other, "verify"])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::params::GotoTargetParams;

    #[test]
    fn serialize_no_param_command_with_verify() {
        let msg = serialize_command(&Command::TestConnection, 100, None);
        assert_eq!(msg["method"], "test_connection");
        assert_eq!(msg["id"], 100);
        assert_eq!(msg["params"], json!(["verify"]));
    }

    #[test]
    fn serialize_no_param_command_without_verify() {
        let fw = Some(FirmwareVersion(2500));
        let msg = serialize_command(&Command::TestConnection, 100, fw);
        assert_eq!(msg["method"], "test_connection");
        assert!(msg.get("params").is_none());
    }

    #[test]
    fn serialize_goto_with_verify() {
        let cmd = Command::GotoTarget(GotoTargetParams {
            target_name: "M31".into(),
            is_j2000: true,
            ra: 10.68,
            dec: 41.27,
        });
        let msg = serialize_command(&cmd, 200, None);
        assert_eq!(msg["method"], "goto_target");
        assert_eq!(msg["params"]["verify"], true);
        assert_eq!(msg["params"]["target_name"], "M31");
    }

    #[test]
    fn serialize_scope_sync() {
        let cmd = Command::ScopeSync(10.5, 45.0);
        let msg = serialize_command(&cmd, 300, None);
        assert_eq!(msg["method"], "scope_sync");
        // Array params get "verify" appended
        let params = msg["params"].as_array().unwrap();
        assert_eq!(params.len(), 3);
        assert_eq!(params[2], "verify");
    }

    #[test]
    fn serialize_track_state() {
        let cmd = Command::ScopeSetTrackState(true);
        let msg = serialize_command(&cmd, 400, None);
        assert_eq!(msg["method"], "scope_set_track_state");
        // Bool scalar gets wrapped: [true, "verify"]
        let params = msg["params"].as_array().unwrap();
        assert_eq!(params[0], true);
        assert_eq!(params[1], "verify");
    }

    #[test]
    fn serialize_stop_view_plan() {
        let cmd = Command::StopViewPlan;
        let msg = serialize_command(&cmd, 500, None);
        assert_eq!(msg["method"], "stop_func");
        assert_eq!(msg["params"]["name"], "ViewPlan");
        assert_eq!(msg["params"]["verify"], true);
    }
}
