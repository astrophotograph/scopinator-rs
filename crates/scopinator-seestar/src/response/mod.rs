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

    /// The device's error code by name, or `None` on success.
    pub fn error_code(&self) -> Option<crate::error::ErrorCode> {
        crate::error::ErrorCode::from_response_code(self.code)
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

/// The `result` of `get_device_state`.
///
/// Checked against real replies from an S50 on firmware 6.70 and an S30 on
/// firmware 9.75 (`tests/fixtures/state_*.json`), and the 7.06 corpus. A
/// keyed request (`{"keys": [...]}`) returns only the sections asked for, so
/// every section is optional. Sections and members this struct does not name
/// are still in the raw `result`.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceStateResult {
    pub device: Option<DeviceInfo>,
    pub setting: Option<serde_json::Value>,
    pub camera: Option<CameraInfo>,
    /// The wide camera, on two-camera models only.
    pub second_camera: Option<CameraInfo>,
    pub focuser: Option<FocuserInfo>,
    pub ap: Option<serde_json::Value>,
    pub station: Option<serde_json::Value>,
    pub storage: Option<serde_json::Value>,
    /// `{code, data: {x, y, z, angle}}`.
    pub balance_sensor: Option<serde_json::Value>,
    /// `{code, data: {x, y, z, direction, cali, ...}}`.
    pub compass_sensor: Option<serde_json::Value>,
    pub mount: Option<MountInfo>,
    pub pi_status: Option<PiStatusInfo>,
    /// Observing site as `[longitude, latitude]` in degrees.
    pub location_lon_lat: Option<(f64, f64)>,
}

/// Device identification info (`device` section).
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
    /// Focal length in millimetres.
    pub focal_len: Option<f64>,
    pub fnumber: Option<f64>,
    pub can_star_mode_sel_cam: Option<bool>,
    pub can_wide_cam_af: Option<bool>,
    pub can_wide_cam_roi: Option<bool>,
    /// Device activation, not verification of a connection.
    pub is_verified: Option<bool>,
    /// Hardware / OS image identifier (seen on firmware 9.75).
    pub ios_ver: Option<String>,
    /// Every other member, by wire name. Newer firmware adds capability
    /// flags here (`can_4k_quality`, `can_dual_capture`, ... on 9.75); read
    /// them with [`DeviceInfo::capability`].
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

impl DeviceInfo {
    /// A `can_*` capability flag by its wire name, whether it is one of the
    /// named fields or one a newer firmware added.
    pub fn capability(&self, name: &str) -> Option<bool> {
        match name {
            "can_star_mode_sel_cam" => self.can_star_mode_sel_cam,
            "can_wide_cam_af" => self.can_wide_cam_af,
            "can_wide_cam_roi" => self.can_wide_cam_roi,
            _ => self.other.get(name).and_then(|v| v.as_bool()),
        }
    }
}

/// Camera sensor info (`camera` and `second_camera` sections).
#[derive(Debug, Clone, Deserialize)]
pub struct CameraInfo {
    /// Sensor size as (width, height) in pixels. The main sensor is
    /// portrait (1080 x 1920); an S30's wide sensor is 1920 x 1080.
    pub chip_size: Option<(u32, u32)>,
    pub pixel_size_um: Option<f64>,
    /// e.g. `GR`, `RG`.
    pub debayer_pattern: Option<String>,
    /// Hot-pixel count.
    pub hpc_num: Option<i64>,
    /// Focal length in millimetres (firmware 9.75).
    pub focal_len: Option<f64>,
    pub fnumber: Option<f64>,
    pub gain: Option<i32>,
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
    /// What is moving the mount: `none`, `ScopeGoto`, ...
    pub move_type: Option<String>,
    pub tracking: Option<bool>,
    pub equ_mode: Option<bool>,
    /// True when the arm is closed (parked).
    pub close: Option<bool>,
}

/// Pi status info (temperatures, battery, charge).
#[derive(Debug, Clone, Deserialize)]
pub struct PiStatusInfo {
    /// CPU temperature in degrees Celsius.
    pub temp: Option<f64>,
    pub battery_capacity: Option<i32>,
    pub charger_status: Option<crate::event::ChargerStatus>,
    pub charge_online: Option<bool>,
    /// Battery temperature in degrees Celsius.
    pub battery_temp: Option<f64>,
    /// e.g. `normal`.
    pub battery_temp_type: Option<String>,
    pub battery_overtemp: Option<bool>,
    pub is_overtemp: Option<bool>,
    pub is_typec_connected: Option<bool>,
}

/// View state returned by `get_view_state`. It is `{}` until a view has run.
#[derive(Debug, Clone, Deserialize)]
pub struct ViewStateResult {
    /// The main camera's view.
    #[serde(rename = "View")]
    pub view: Option<ViewInfo>,
    /// The wide camera's view, as reported by its own process (port 4702).
    #[serde(rename = "SecondView")]
    pub second_view: Option<ViewInfo>,
}

/// View info within the view state.
///
/// The stage that is running (or ran last) is named by `stage` and described
/// by a member of that name: `ContinuousExposure`, or `AutoGoto` with a
/// nested `ScopeGoto` during a goto. Other stages are in the raw `result`.
#[derive(Debug, Clone, Deserialize)]
pub struct ViewInfo {
    pub state: Option<crate::event::EventState>,
    #[serde(default)]
    pub lapse_ms: i64,
    /// `star`, `scenery`, `sun`, ... or `none` after the view ended.
    pub mode: Option<String>,
    /// 0 for the main camera, 1 for the wide camera.
    pub cam_id: Option<i32>,
    /// Target as (RA hours, Dec degrees), when the view has one.
    pub target_ra_dec: Option<(f64, f64)>,
    pub target_name: Option<String>,
    pub lp_filter: Option<bool>,
    pub gain: Option<i32>,
    /// Name of the current stage.
    pub stage: Option<String>,
    #[serde(rename = "ContinuousExposure")]
    pub continuous_exposure: Option<serde_json::Value>,
    #[serde(rename = "AutoGoto")]
    pub auto_goto: Option<serde_json::Value>,
}

/// What the device says about itself in `get_device_state`: identity, optics
/// and sensor.
///
/// This is a flattened, forgiving view of the same reply that
/// [`DeviceStateResult`] types strictly: it is built from the raw JSON member
/// by member, so one member of an unexpected type cannot hide the rest, and
/// it accumulates across keyed (partial) replies. The client fills this in at connect and keeps it up to date
/// from every later `get_device_state` reply; see
/// [`SeestarClient::device_info`](crate::client::SeestarClient::device_info).
///
/// Every field is optional because replies can be partial (a keyed
/// `get_device_state` returns only the sections asked for) and because
/// firmware versions differ in what they report.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceSummary {
    /// `device.firmware_ver_int`, e.g. 2670 for firmware 6.70.
    pub firmware: Option<scopinator_types::FirmwareVersion>,
    /// `device.firmware_ver_string`, e.g. `6.70`.
    pub firmware_string: Option<String>,
    /// `device.product_model`, e.g. `Seestar S50`.
    pub model: Option<String>,
    /// `device.focal_len`: focal length in millimetres.
    pub focal_length_mm: Option<f64>,
    /// `device.fnumber`: focal ratio.
    pub f_number: Option<f64>,
    /// `camera.chip_size` as (width, height) in pixels, in the orientation
    /// the image socket delivers frames (1080 x 1920 on an S50).
    pub chip_size: Option<(u32, u32)>,
    /// `camera.pixel_size_um`.
    pub pixel_size_um: Option<f64>,
    /// `camera.debayer_pattern`, e.g. `GR`.
    pub debayer_pattern: Option<String>,
    /// True if the reply had a `second_camera` section (the wide camera on
    /// dual-camera models).
    pub has_second_camera: Option<bool>,
    /// The wide camera's sensor and optics, from the `second_camera` section.
    pub second_camera: Option<SensorSummary>,
    /// The `device.can_*` capability flags, by their wire names.
    pub capabilities: std::collections::BTreeMap<String, bool>,
}

/// Sensor and optics of one camera, as reported in `get_device_state`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SensorSummary {
    /// `chip_size` as (width, height) in pixels.
    pub chip_size: Option<(u32, u32)>,
    pub pixel_size_um: Option<f64>,
    /// `focal_len`: focal length in millimetres.
    pub focal_length_mm: Option<f64>,
    /// `debayer_pattern`, e.g. `RG`.
    pub debayer_pattern: Option<String>,
}

impl SensorSummary {
    fn from_section(section: &serde_json::Value) -> Self {
        let size = section.get("chip_size").and_then(|v| v.as_array());
        let chip_size = match size.map(Vec::as_slice) {
            Some([w, h]) => w
                .as_u64()
                .zip(h.as_u64())
                .map(|(w, h)| (w as u32, h as u32)),
            _ => None,
        };
        Self {
            chip_size,
            pixel_size_um: section.get("pixel_size_um").and_then(|v| v.as_f64()),
            focal_length_mm: section.get("focal_len").and_then(|v| v.as_f64()),
            debayer_pattern: section
                .get("debayer_pattern")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        }
    }
}

impl DeviceSummary {
    /// Update from the `result` of a `get_device_state` reply. Members that
    /// are absent from this reply keep their previous values. Returns true
    /// if the reply carried a `device` or `camera` section.
    pub fn update_from_device_state(&mut self, result: &serde_json::Value) -> bool {
        let device = result.get("device");
        let camera = result.get("camera");
        if let Some(device) = device {
            if let Some(v) = device.get("firmware_ver_int").and_then(|v| v.as_u64()) {
                self.firmware = Some(scopinator_types::FirmwareVersion(v as u32));
            }
            if let Some(v) = device.get("firmware_ver_string").and_then(|v| v.as_str()) {
                self.firmware_string = Some(v.to_string());
            }
            if let Some(v) = device.get("product_model").and_then(|v| v.as_str()) {
                self.model = Some(v.to_string());
            }
            if let Some(v) = device.get("focal_len").and_then(|v| v.as_f64()) {
                self.focal_length_mm = Some(v);
            }
            if let Some(v) = device.get("fnumber").and_then(|v| v.as_f64()) {
                self.f_number = Some(v);
            }
            if let Some(members) = device.as_object() {
                for (name, value) in members {
                    if let (true, Some(flag)) = (name.starts_with("can_"), value.as_bool()) {
                        self.capabilities.insert(name.clone(), flag);
                    }
                }
            }
        }
        if let Some(camera) = camera {
            let size = camera.get("chip_size").and_then(|v| v.as_array());
            if let Some([w, h]) = size.map(Vec::as_slice)
                && let (Some(w), Some(h)) = (w.as_u64(), h.as_u64())
            {
                self.chip_size = Some((w as u32, h as u32));
            }
            if let Some(v) = camera.get("pixel_size_um").and_then(|v| v.as_f64()) {
                self.pixel_size_um = Some(v);
            }
            if let Some(v) = camera.get("debayer_pattern").and_then(|v| v.as_str()) {
                self.debayer_pattern = Some(v.to_string());
            }
        }
        // A full (unkeyed) reply always has both sections; only then does the
        // absence of `second_camera` mean there is none.
        if let Some(second) = result.get("second_camera") {
            self.second_camera = Some(SensorSummary::from_section(second));
            self.has_second_camera = Some(true);
        } else if device.is_some() && camera.is_some() {
            self.has_second_camera = Some(false);
        }
        device.is_some() || camera.is_some()
    }
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
