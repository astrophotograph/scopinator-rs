//! The typed `get_device_state` / `get_view_state` results against real
//! replies: sanitized captures from a Seestar S50 on firmware 6.70 and a
//! Seestar S30 on firmware 9.75 (2026-11-14, `tests/fixtures`), and every
//! such reply in the conformance corpus (firmware 6.70 and 7.06).
//!
//! Serial numbers, CPU ids, network names, addresses, passwords and the
//! observing location are redacted in the fixtures.

mod common;

use common::load_corpus;
use scopinator_seestar::DeviceSummary;
use scopinator_seestar::event::{ChargerStatus, EventState};
use scopinator_seestar::response::{DeviceStateResult, ViewStateResult};
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")))
        .unwrap()
}

fn device_state(name: &str) -> (Value, DeviceStateResult) {
    let raw = fixture(name)["get_device_state"]["result"].clone();
    let typed = serde_json::from_value(raw.clone()).expect("device state parses");
    (raw, typed)
}

#[test]
fn s50_fw670_device_state() {
    let (raw, state) = device_state("state_s50_fw670.json");
    let device = state.device.as_ref().unwrap();
    assert_eq!(device.firmware_ver_int, Some(2670));
    assert_eq!(device.product_model.as_deref(), Some("Seestar S50"));
    assert_eq!(device.focal_len, Some(250.0));
    assert_eq!(device.capability("can_wide_cam_af"), Some(false));
    // 6.70 has only the three original flags.
    assert_eq!(device.capability("can_dual_capture"), None);

    let camera = state.camera.as_ref().unwrap();
    assert_eq!(camera.chip_size, Some((1080, 1920)));
    assert_eq!(camera.pixel_size_um, Some(2.9));
    assert_eq!(camera.debayer_pattern.as_deref(), Some("GR"));
    assert!(camera.hpc_num.is_some());
    assert!(camera.focal_len.is_none(), "6.70 has no camera.focal_len");
    assert!(state.second_camera.is_none());

    let pi = state.pi_status.as_ref().unwrap();
    assert_eq!(pi.charger_status, Some(ChargerStatus::Discharging));
    assert_eq!(pi.battery_temp_type.as_deref(), Some("normal"));
    assert!(pi.battery_temp.is_some() && pi.is_overtemp == Some(false));

    let mount = state.mount.as_ref().unwrap();
    assert_eq!((mount.close, mount.equ_mode), (Some(true), Some(true)));
    assert_eq!(mount.move_type.as_deref(), Some("none"));
    assert_eq!(state.focuser.as_ref().unwrap().max_step, Some(2600));
    assert_eq!(
        state.location_lon_lat,
        Some((0.0, 0.0)),
        "redacted in the fixture"
    );

    // Every section the device sent is either typed or kept as a value.
    for section in raw.as_object().unwrap().keys() {
        assert!(
            [
                "device",
                "setting",
                "camera",
                "second_camera",
                "focuser",
                "ap",
                "station",
                "storage",
                "balance_sensor",
                "compass_sensor",
                "mount",
                "pi_status",
                "location_lon_lat",
            ]
            .contains(&section.as_str()),
            "section `{section}` is not represented in DeviceStateResult"
        );
    }
}

#[test]
fn s30_fw975_device_state() {
    let (raw, state) = device_state("state_s30_fw975.json");
    let device = state.device.as_ref().unwrap();
    assert_eq!(device.firmware_ver_int, Some(2975));
    assert_eq!(device.product_model.as_deref(), Some("Seestar S30"));
    assert!(device.ios_ver.is_some());
    // Flags 9.75 added are reachable by name.
    assert_eq!(device.capability("can_4k_quality"), Some(true));
    assert_eq!(device.capability("can_dual_capture"), Some(false));
    assert_eq!(device.capability("can_star_mode_sel_cam"), Some(false));

    let camera = state.camera.as_ref().unwrap();
    assert_eq!(camera.chip_size, Some((1080, 1920)));
    assert_eq!((camera.focal_len, camera.gain), (Some(150.0), Some(200)));
    let wide = state.second_camera.as_ref().expect("two-camera model");
    assert_eq!(wide.chip_size, Some((1920, 1080)));
    assert_eq!(wide.pixel_size_um, Some(2.7));
    assert_eq!(wide.focal_len, Some(14.0));
    assert_eq!(wide.debayer_pattern.as_deref(), Some("RG"));

    // The forgiving summary and the strict struct agree on the same reply.
    let mut summary = DeviceSummary::default();
    assert!(summary.update_from_device_state(&raw));
    assert_eq!(summary.firmware.map(|f| f.0), device.firmware_ver_int);
    assert_eq!(summary.model, device.product_model);
    assert_eq!(summary.chip_size, camera.chip_size);
    assert_eq!(summary.pixel_size_um, camera.pixel_size_um);
    assert_eq!(summary.has_second_camera, Some(true));
    let summary_wide = summary.second_camera.as_ref().unwrap();
    assert_eq!(summary_wide.chip_size, wide.chip_size);
    assert_eq!(summary_wide.focal_length_mm, wide.focal_len);
    for (name, value) in &summary.capabilities {
        assert_eq!(device.capability(name), Some(*value), "{name}");
    }
    assert_eq!(summary.capabilities.len(), 10);
}

#[test]
fn view_state_after_a_view_and_during_a_goto() {
    // Idle after a cancelled view, on both firmware versions.
    for (name, gain, has_target) in [
        ("state_s50_fw670.json", 80, false),
        ("state_s30_fw975.json", 200, true),
    ] {
        let raw = fixture(name)["get_view_state"]["result"].clone();
        let state: ViewStateResult = serde_json::from_value(raw).expect("view state parses");
        let view = state.view.expect("View");
        assert_eq!(view.state, Some(EventState::Cancel));
        assert_eq!(view.mode.as_deref(), Some("none"));
        assert_eq!((view.cam_id, view.gain), (Some(0), Some(gain)));
        assert_eq!(view.stage.as_deref(), Some("ContinuousExposure"));
        assert!(view.continuous_exposure.is_some());
        assert_eq!(view.target_ra_dec.is_some(), has_target);
        assert!(state.second_view.is_none());
    }

    // During a goto the stage is AutoGoto, with ScopeGoto nested in it.
    let raw = fixture("view_state_goto_s30_fw975.json")["result"].clone();
    let state: ViewStateResult = serde_json::from_value(raw).unwrap();
    let view = state.view.unwrap();
    assert_eq!(view.state, Some(EventState::Working));
    assert_eq!(view.mode.as_deref(), Some("star"));
    assert_eq!(view.target_name.as_deref(), Some("Deneb"));
    assert_eq!(view.target_ra_dec, Some((20.690001, 45.280003)));
    assert_eq!(view.stage.as_deref(), Some("AutoGoto"));
    let goto = view.auto_goto.expect("AutoGoto");
    assert_eq!(goto["ScopeGoto"]["state"], "working");
    assert!(view.continuous_exposure.is_none());

    // Before any view has run the result is an empty object.
    let empty: ViewStateResult = serde_json::from_value(serde_json::json!({})).unwrap();
    assert!(empty.view.is_none() && empty.second_view.is_none());

    // The wide camera's process reports its view as SecondView.
    let wide: ViewStateResult = serde_json::from_value(serde_json::json!({
        "SecondView": {"state": "working", "lapse_ms": 8009, "mode": "star", "cam_id": 1,
            "lp_filter": false, "gain": 200,
            "ContinuousExposure": {"state": "working", "lapse_ms": 7341, "fps": 1.049475},
            "stage": "ContinuousExposure"}
    }))
    .unwrap();
    assert_eq!(wide.second_view.unwrap().cam_id, Some(1));
}

// Every device-state and view-state reply in the corpus (firmware 6.70 and
// 7.06) parses into the typed results.
#[test]
fn corpus_state_replies_parse() {
    let (mut device_states, mut view_states) = (0, 0);
    for session in load_corpus() {
        for msg in session.telescope_messages() {
            let Some(result) = msg.get("result").filter(|r| r.is_object()) else {
                continue;
            };
            match msg.get("method").and_then(Value::as_str) {
                Some("get_device_state") => {
                    serde_json::from_value::<DeviceStateResult>(result.clone()).unwrap_or_else(
                        |e| panic!("{}: device state: {e}\n{result}", session.name),
                    );
                    device_states += 1;
                }
                Some("get_view_state") => {
                    serde_json::from_value::<ViewStateResult>(result.clone())
                        .unwrap_or_else(|e| panic!("{}: view state: {e}\n{result}", session.name));
                    view_states += 1;
                }
                _ => {}
            }
        }
    }
    assert!(device_states > 0, "corpus has no get_device_state replies");
    println!("{device_states} device states, {view_states} view states parsed");
}
