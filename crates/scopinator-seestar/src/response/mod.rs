use serde::Deserialize;

/// A response to a command sent on the control port.
#[derive(Debug, Clone, Deserialize)]
pub struct CommandResponse {
    pub id: u64,
    #[serde(default)]
    pub jsonrpc: String,
    #[serde(rename = "Timestamp")]
    pub timestamp: Option<String>,
    pub method: Option<String>,
    #[serde(default)]
    pub code: i32,
    pub error: Option<String>,
    pub result: Option<serde_json::Value>,
}

impl CommandResponse {
    /// Returns true if the command succeeded (code == 0).
    pub fn is_success(&self) -> bool {
        self.code == 0
    }

    /// Returns an error if the command failed.
    pub fn into_result(self) -> Result<serde_json::Value, CommandError> {
        if self.is_success() {
            Ok(self.result.unwrap_or(serde_json::Value::Null))
        } else {
            Err(CommandError {
                code: self.code,
                message: self
                    .error
                    .unwrap_or_else(|| format!("command failed with code {}", self.code)),
                method: self.method,
            })
        }
    }
}

/// An error returned by a failed command.
#[derive(Debug, Clone, thiserror::Error)]
#[error("command {method:?} failed (code {code}): {message}")]
pub struct CommandError {
    pub code: i32,
    pub message: String,
    pub method: Option<String>,
}

/// Device state returned by `get_device_state`.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceStateResult {
    pub device: Option<DeviceInfo>,
    pub setting: Option<serde_json::Value>,
    pub camera: Option<CameraInfo>,
    pub focuser: Option<FocuserInfo>,
    pub ap: Option<serde_json::Value>,
    pub station: Option<serde_json::Value>,
    pub storage: Option<serde_json::Value>,
    pub balance_sensor: Option<serde_json::Value>,
    pub compass_sensor: Option<serde_json::Value>,
    pub mount: Option<MountInfo>,
    pub pi_status: Option<PiStatusInfo>,
}

/// Device identification info.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceInfo {
    pub name: Option<String>,
    pub firmware_ver_int: Option<u32>,
    pub firmware_ver_string: Option<String>,
    pub sn: Option<String>,
    #[serde(rename = "cpuId")]
    pub cpu_id: Option<String>,
    pub product_model: Option<String>,
    pub user_product_model: Option<String>,
    pub focal_len: Option<f64>,
    pub fnumber: Option<f64>,
    pub can_star_mode_sel_cam: Option<bool>,
    pub can_wide_cam_af: Option<bool>,
    pub can_wide_cam_roi: Option<bool>,
    pub is_verified: Option<bool>,
}

/// Camera sensor info.
#[derive(Debug, Clone, Deserialize)]
pub struct CameraInfo {
    pub chip_size: Option<(f64, f64)>,
    pub pixel_size_um: Option<f64>,
    pub debayer_pattern: Option<String>,
}

/// Focuser info.
#[derive(Debug, Clone, Deserialize)]
pub struct FocuserInfo {
    pub state: Option<String>,
    pub max_step: Option<i32>,
    pub step: Option<i32>,
}

/// Mount info.
#[derive(Debug, Clone, Deserialize)]
pub struct MountInfo {
    pub move_type: Option<String>,
    pub tracking: Option<bool>,
    pub equ_mode: Option<bool>,
    pub close: Option<bool>,
}

/// Pi status info (temperatures, battery, charge).
#[derive(Debug, Clone, Deserialize)]
pub struct PiStatusInfo {
    pub temp: Option<f64>,
    pub battery_capacity: Option<i32>,
    pub charger_status: Option<String>,
    pub charge_online: Option<bool>,
}

/// View state returned by `get_view_state`.
#[derive(Debug, Clone, Deserialize)]
pub struct ViewStateResult {
    #[serde(rename = "View")]
    pub view: Option<ViewInfo>,
}

/// View info within the view state.
#[derive(Debug, Clone, Deserialize)]
pub struct ViewInfo {
    pub state: Option<String>,
    pub mode: Option<String>,
    pub gain: Option<i32>,
    #[serde(rename = "ContinuousExposure")]
    pub continuous_exposure: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_success_response() {
        let json = r#"{
            "id": 100,
            "jsonrpc": "2.0",
            "Timestamp": "1234567890.0",
            "method": "test_connection",
            "code": 0,
            "result": "server connected!"
        }"#;
        let resp: CommandResponse = serde_json::from_str(json).unwrap();
        assert!(resp.is_success());
        assert_eq!(resp.id, 100);
        assert_eq!(resp.method.as_deref(), Some("test_connection"));
    }

    #[test]
    fn deserialize_error_response() {
        let json = r#"{
            "id": 200,
            "jsonrpc": "2.0",
            "method": "unknown_method",
            "code": 210,
            "error": "method not supported"
        }"#;
        let resp: CommandResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.is_success());
        let err = resp.into_result().unwrap_err();
        assert_eq!(err.code, 210);
    }

    // ---- Differential tests vs pyscopinator `commands/common.py::CommandResponse`
    // (Forge task "Add differential response parsing tests").
    // Python fields: id, jsonrpc, Timestamp, method (required str), code, error, result.

    fn parse(json: &str) -> Result<CommandResponse, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Python: test_command_response_requires_method (DIVERGENCE).
    /// Python declares `method: str` (required), so a response lacking `method`
    /// fails validation. Rust declares `method: Option<String>`, so it parses.
    /// Pinned as current behaviour; see docs/bugs/response-method-optionality.md.
    #[test]
    fn diff_response_without_method_parses_in_rust() {
        let resp = parse(r#"{"id": 1, "jsonrpc": "2.0", "code": 0}"#).unwrap();
        assert_eq!(resp.method, None);
        assert!(resp.is_success());
    }

    /// Same divergence, explicit `"method": null`.
    #[test]
    fn diff_response_with_null_method_parses_in_rust() {
        let resp = parse(r#"{"id": 1, "method": null}"#).unwrap();
        assert_eq!(resp.method, None);
    }

    /// With no method, a failed response carries `method: None` into the error.
    #[test]
    fn diff_methodless_error_response_into_result() {
        let err = parse(r#"{"id": 1, "code": 5, "error": "boom"}"#)
            .unwrap()
            .into_result()
            .unwrap_err();
        assert_eq!(err.method, None);
        assert_eq!(err.code, 5);
        assert_eq!(err.message, "boom");
    }

    /// `id` is required in both ports: missing id fails.
    #[test]
    fn diff_response_missing_id_fails() {
        assert!(parse(r#"{"jsonrpc": "2.0", "method": "x", "code": 0}"#).is_err());
        assert!(parse(r#"{"id": null, "method": "x"}"#).is_err());
    }

    /// Only `id` is required by serde in Rust; everything else defaults.
    #[test]
    fn diff_response_minimal_id_only() {
        let resp = parse(r#"{"id": 7}"#).unwrap();
        assert_eq!(resp.id, 7);
        assert_eq!(resp.jsonrpc, "");
        assert_eq!(resp.timestamp, None);
        assert_eq!(resp.code, 0);
        assert_eq!(resp.error, None);
        assert!(resp.result.is_none());
        assert!(resp.is_success());
    }

    /// Error/code combinations, per current serde behaviour.
    #[test]
    fn diff_response_error_code_combinations() {
        // code != 0 with error text: failure, message from `error`.
        let err = parse(r#"{"id": 1, "method": "m", "code": 210, "error": "not supported"}"#)
            .unwrap()
            .into_result()
            .unwrap_err();
        assert_eq!((err.code, err.message.as_str()), (210, "not supported"));
        assert_eq!(err.method.as_deref(), Some("m"));

        // code != 0 without error text: synthesized message.
        let err = parse(r#"{"id": 1, "method": "m", "code": 109}"#)
            .unwrap()
            .into_result()
            .unwrap_err();
        assert_eq!(err.message, "command failed with code 109");

        // error text present but code == 0 (or absent): still success; error ignored.
        let resp = parse(r#"{"id": 1, "method": "m", "error": "stray"}"#).unwrap();
        assert!(resp.is_success());
        assert_eq!(resp.error.as_deref(), Some("stray"));
        assert_eq!(resp.into_result().unwrap(), serde_json::Value::Null);

        // success returns the result payload.
        let v = parse(r#"{"id": 1, "method": "m", "code": 0, "result": {"a": 1}}"#)
            .unwrap()
            .into_result()
            .unwrap();
        assert_eq!(v["a"], 1);
    }

    /// Type strictness: wrong-typed fields fail rather than coerce.
    #[test]
    fn diff_response_wrong_types_fail() {
        assert!(parse(r#"{"id": "1"}"#).is_err());
        assert!(parse(r#"{"id": 1, "code": "0"}"#).is_err());
        assert!(parse(r#"{"id": 1, "error": 5}"#).is_err());
        assert!(parse(r#"{"id": 1, "Timestamp": 12.5}"#).is_err());
        assert!(parse(r#"{"id": 1, "method": 3}"#).is_err());
    }

    /// Real corpus response lines (telescope -> client, with `jsonrpc`) parse.
    /// Inlined from conformance/sessions/*/control.jsonl.
    #[test]
    fn diff_corpus_inline_response_lines_parse() {
        let ok = r#"{"jsonrpc":"2.0","Timestamp":"13679.955123311","method":"set_setting","error":"not supported","code":210,"id":100002}"#;
        let resp = parse(ok).unwrap();
        assert_eq!(resp.id, 100002);
        assert_eq!(resp.code, 210);
        assert_eq!(resp.error.as_deref(), Some("not supported"));
        assert_eq!(resp.timestamp.as_deref(), Some("13679.955123311"));
        assert!(!resp.is_success());
    }

    /// Every telescope-side JSON-RPC response in every corpus session parses,
    /// and all of them carry `method` (so the Python-required shape holds on real data).
    #[test]
    fn diff_corpus_all_sessions_responses_parse() {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conformance/sessions");
        let mut parsed = 0usize;
        for entry in std::fs::read_dir(&root).expect("conformance/sessions exists") {
            let path = entry.unwrap().path().join("control.jsonl");
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for line in text.lines() {
                let rec: serde_json::Value = serde_json::from_str(line).unwrap();
                if rec["direction"] != "telescope" {
                    continue;
                }
                let raw = rec["raw"].as_str().unwrap();
                let Ok(val) = serde_json::from_str::<serde_json::Value>(raw) else {
                    continue;
                };
                // Events have "Event" and no "jsonrpc"; only responses are in scope.
                if val.get("jsonrpc").is_none() {
                    continue;
                }
                let resp =
                    parse(raw).unwrap_or_else(|e| panic!("{}: {e}: {raw:.200}", path.display()));
                assert!(resp.method.is_some(), "corpus response lacks method");
                parsed += 1;
            }
        }
        assert!(parsed > 0, "no corpus responses found");
    }

    #[test]
    fn deserialize_device_state() {
        let json = r#"{
            "device": {
                "name": "Seestar",
                "firmware_ver_int": 3000,
                "firmware_ver_string": "3.0.0",
                "sn": "ABC123",
                "product_model": "Seestar S50",
                "focal_len": 250.0,
                "fnumber": 5.0
            },
            "focuser": {
                "state": "idle",
                "max_step": 80000,
                "step": 40000
            },
            "mount": {
                "tracking": true
            }
        }"#;
        let state: DeviceStateResult = serde_json::from_str(json).unwrap();
        assert_eq!(state.device.as_ref().unwrap().firmware_ver_int, Some(3000));
        assert_eq!(state.focuser.as_ref().unwrap().step, Some(40000));
        assert_eq!(state.mount.as_ref().unwrap().tracking, Some(true));
    }
}
