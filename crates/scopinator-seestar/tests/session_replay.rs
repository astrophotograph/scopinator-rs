//! Replay the conformance corpus through this crate's protocol models.
//!
//! These tests treat the captured (sanitized) sessions as an oracle:
//!   * every telescope→app message must classify and deserialize cleanly into a
//!     known event or a command response (no parse failures, no Unknown events);
//!   * every app→telescope command for a modeled method must round-trip — our
//!     `Command` + `serialize_command` must reproduce the captured wire bytes;
//!   * firmware version must be detected and drive verify injection.
//!
//! When a future firmware capture is added that adds/drops/changes a command or
//! event, one of these tests fails, pinpointing the drift.

mod common;

use common::{firmware_int_from_response, load_corpus, strip_verify};

use scopinator_seestar::command::Command;
use scopinator_seestar::command::params::{
    PlaySoundParams, SequenceSettingParams, SettingParams, StartStackParams, StartViewParams,
    StopViewParams,
};
use scopinator_seestar::command::serialize::{WireProfile, serialize_command};
use scopinator_seestar::event::SeestarEvent;
use scopinator_seestar::protocol::json_rpc;
use scopinator_seestar::response::CommandResponse;
use scopinator_types::FirmwareVersion;
use serde_json::Value;

// ----------------------------------------------------------------------------
// Telescope → app: events and responses must all parse.
// ----------------------------------------------------------------------------

#[test]
fn every_telescope_message_parses_cleanly() {
    let corpus = load_corpus();
    let mut events = 0usize;
    let mut responses = 0usize;
    let mut unknown_events: Vec<String> = Vec::new();
    let mut parse_failures: Vec<String> = Vec::new();
    let mut unclassified: Vec<String> = Vec::new();

    for session in &corpus {
        for msg in session.telescope_messages() {
            if json_rpc::is_event(&msg) {
                match serde_json::from_value::<SeestarEvent>(msg.clone()) {
                    Ok(SeestarEvent::Unknown { .. }) => {
                        unknown_events.push(json_rpc::event_name(&msg).unwrap_or("?").to_string())
                    }
                    Ok(_) => events += 1,
                    Err(e) => parse_failures.push(format!("{}: event error {e}", session.name)),
                }
            } else if json_rpc::is_response(&msg) {
                match serde_json::from_value::<CommandResponse>(msg.clone()) {
                    Ok(_) => responses += 1,
                    Err(e) => parse_failures.push(format!("{}: response error {e}", session.name)),
                }
            } else {
                unclassified.push(format!("{}: {}", session.name, truncate(&msg)));
            }
        }
    }

    println!(
        "parsed {events} events + {responses} responses across {} sessions",
        corpus.len()
    );
    assert!(
        parse_failures.is_empty(),
        "parse failures:\n{}",
        parse_failures.join("\n")
    );
    assert!(
        unknown_events.is_empty(),
        "unmodeled (Unknown) event types in corpus — model them: {:?}",
        dedup(unknown_events)
    );
    assert!(
        unclassified.is_empty(),
        "unclassified telescope messages:\n{}",
        unclassified.join("\n")
    );
    assert!(
        events + responses > 0,
        "corpus produced no telescope messages"
    );
}

// ----------------------------------------------------------------------------
// Firmware detection.
// ----------------------------------------------------------------------------

#[test]
fn firmware_detected_from_device_state_and_requires_verify() {
    let corpus = load_corpus();
    let mut detected = 0usize;

    for session in &corpus {
        let fw_int = session
            .telescope_messages()
            .find_map(|m| firmware_int_from_response(&m));
        let Some(fw_int) = fw_int else { continue };
        detected += 1;

        let fw = FirmwareVersion(fw_int as u32);
        // Both firmwares in the corpus are above the verify threshold.
        assert!(
            fw.requires_verify(),
            "{}: firmware {fw_int} unexpectedly below verify threshold",
            session.name
        );
        // Session names encode the firmware: s50_fw670_* -> 2670, s30_fw706_* -> 2706.
        if session.name.contains("fw670") {
            assert_eq!(fw_int, 2670, "{}: expected fw int 2670", session.name);
        } else if session.name.contains("fw706") {
            assert_eq!(fw_int, 2706, "{}: expected fw int 2706", session.name);
        }
    }

    assert!(
        detected >= 2,
        "expected device-state responses in at least 2 sessions, got {detected}"
    );
}

// ----------------------------------------------------------------------------
// App → telescope: modeled commands must reproduce the captured wire bytes.
// ----------------------------------------------------------------------------

enum Rebuild {
    Cmd(Command),
    Skip(&'static str),
}

/// Reconstruct a `Command` from a captured method + its verify-stripped params.
fn rebuild(method: &str, base: Option<&Value>) -> Result<Rebuild, String> {
    let parse =
        |v: Option<&Value>| -> Result<Value, String> { Ok(v.cloned().unwrap_or(Value::Null)) };
    let cmd = match method {
        "get_device_state" => Command::GetDeviceState,
        "get_focuser_position" => Command::GetFocuserPosition,
        "get_setting" => Command::GetSetting,
        "get_view_state" => Command::GetViewState,
        "pi_station_state" => Command::PiStationState,
        "scope_get_equ_coord" => Command::ScopeGetEquCoord,
        "play_sound" => {
            let p: PlaySoundParams = serde_json::from_value(parse(base)?)
                .map_err(|e| format!("play_sound params: {e}"))?;
            Command::PlaySound(p)
        }
        "iscope_start_stack" => match base {
            None => Command::IscopeStartStack(None),
            Some(v) => {
                let p: StartStackParams = serde_json::from_value(v.clone())
                    .map_err(|e| format!("iscope_start_stack params: {e}"))?;
                Command::IscopeStartStack(Some(p))
            }
        },
        "iscope_start_view" => {
            let p: StartViewParams = serde_json::from_value(parse(base)?)
                .map_err(|e| format!("iscope_start_view params: {e}"))?;
            Command::IscopeStartView(p)
        }
        "iscope_stop_view" => match base {
            None => Command::IscopeStopView(None),
            Some(v) => {
                let p: StopViewParams = serde_json::from_value(v.clone())
                    .map_err(|e| format!("iscope_stop_view params: {e}"))?;
                Command::IscopeStopView(Some(p))
            }
        },
        "set_sequence_setting" => {
            // Known client bug in the capture: the app-side client sent the
            // nested form [[{group_name: ...}]], which the firmware rejected
            // with 107 "expected object param" (also confirmed on an S50,
            // firmware 6.70). It is not a round-trip target; this crate sends
            // the settings object.
            if base.and_then(|v| v.as_array()).is_some() {
                return Ok(Rebuild::Skip(
                    "set_sequence_setting nested-array form (firmware-rejected)",
                ));
            }
            let p: SequenceSettingParams = serde_json::from_value(parse(base)?)
                .map_err(|e| format!("set_sequence_setting params: {e}"))?;
            Command::SetSequenceSetting(p)
        }
        "set_setting" => {
            // cli_name / master_cli are app-specific keys the firmware rejects
            // (codes 210/109) and we deliberately do not model them.
            if let Some(Value::Object(m)) = base
                && (m.contains_key("cli_name") || m.contains_key("master_cli"))
            {
                return Ok(Rebuild::Skip(
                    "set_setting app-specific param (firmware-rejected)",
                ));
            }
            let p: SettingParams = serde_json::from_value(parse(base)?)
                .map_err(|e| format!("set_setting params: {e}"))?;
            Command::SetSetting(p)
        }
        _ => return Ok(Rebuild::Skip("method not in round-trip set")),
    };
    Ok(Rebuild::Cmd(cmd))
}

#[test]
fn client_commands_round_trip_to_captured_bytes() {
    let corpus = load_corpus();
    let mut matched = 0usize;
    let mut skipped: std::collections::BTreeMap<String, usize> = Default::default();
    let mut mismatches: Vec<String> = Vec::new();

    for session in &corpus {
        for msg in session.client_messages() {
            let Some(method) = msg.get("method").and_then(Value::as_str) else {
                continue;
            };
            let stripped = strip_verify(msg.get("params"));

            let rebuilt = match rebuild(method, stripped.base.as_ref()) {
                Ok(r) => r,
                Err(e) => {
                    mismatches.push(format!("{} [{method}] rebuild failed: {e}", session.name));
                    continue;
                }
            };
            let cmd = match rebuilt {
                Rebuild::Cmd(c) => c,
                Rebuild::Skip(reason) => {
                    *skipped.entry(format!("{method}: {reason}")).or_default() += 1;
                    continue;
                }
            };

            let id = msg.get("id").and_then(Value::as_u64).unwrap_or(0);
            // Reproduce the captured verify convention: messages that carried a
            // verify marker were sent by a verify-injecting client (firmware
            // above threshold); ones without it were not.
            let fw = if stripped.had_verify {
                None
            } else {
                Some(FirmwareVersion(2500))
            };
            let produced = serialize_command(&cmd, id, &WireProfile::legacy(fw));

            if produced != msg {
                mismatches.push(format!(
                    "{} [{method}]\n   captured: {msg}\n   produced: {produced}",
                    session.name
                ));
            } else {
                matched += 1;
            }
        }
    }

    println!("client round-trip: {matched} matched");
    for (reason, n) in &skipped {
        println!("  skipped x{n}: {reason}");
    }
    assert!(
        mismatches.is_empty(),
        "{} client command(s) did not round-trip:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
    assert!(matched > 0, "no client commands were round-tripped");
}

// ----------------------------------------------------------------------------
// Firmware-matrix pins: catch added/dropped commands & events across versions.
// ----------------------------------------------------------------------------

#[test]
fn corpus_covers_both_firmware_versions_and_models() {
    let corpus = load_corpus();
    let names: Vec<&str> = corpus.iter().map(|s| s.name.as_str()).collect();
    assert!(
        names.iter().any(|n| n.contains("fw670")),
        "corpus lacks a 6.70 session"
    );
    assert!(
        names.iter().any(|n| n.contains("fw706")),
        "corpus lacks a 7.06 session"
    );
    assert!(
        names.iter().any(|n| n.contains("s50")),
        "corpus lacks an S50 session"
    );
    assert!(
        names.iter().any(|n| n.contains("s30")),
        "corpus lacks an S30 session"
    );
}

#[test]
fn firmware_specific_command_drift_is_pinned() {
    let corpus = load_corpus();
    let methods_for = |pred: &dyn Fn(&str) -> bool| -> std::collections::BTreeSet<String> {
        corpus
            .iter()
            .filter(|s| pred(&s.name))
            .flat_map(|s| s.client_messages().collect::<Vec<_>>())
            .filter_map(|m| m.get("method").and_then(Value::as_str).map(str::to_string))
            .collect()
    };

    let fw670 = methods_for(&|n| n.contains("fw670"));
    let fw706 = methods_for(&|n| n.contains("fw706"));

    // Added in 7.06 (absent from the 6.70 captures).
    for added in [
        "play_sound",
        "set_sequence_setting",
        "iscope_start_stack",
        "get_setting",
    ] {
        assert!(fw706.contains(added), "expected {added} in 7.06 captures");
        assert!(
            !fw670.contains(added),
            "did not expect {added} in 6.70 captures"
        );
    }
    // Present in 6.70 captures, dropped from the 7.06 app's vocabulary.
    assert!(
        fw670.contains("iscope_stop_view"),
        "expected iscope_stop_view in 6.70 captures"
    );
    assert!(
        !fw706.contains("iscope_stop_view"),
        "did not expect iscope_stop_view in 7.06 captures"
    );
}

#[test]
fn firmware_specific_event_drift_is_pinned() {
    let corpus = load_corpus();
    let events_for = |pred: &dyn Fn(&str) -> bool| -> std::collections::BTreeSet<String> {
        corpus
            .iter()
            .filter(|s| pred(&s.name))
            .flat_map(|s| s.telescope_messages().collect::<Vec<_>>())
            .filter_map(|m| json_rpc::event_name(&m).map(str::to_string))
            .collect()
    };
    let fw706 = events_for(&|n| n.contains("fw706"));
    // These event types only appear in the 7.06 captures.
    for added in ["Stack", "Exposure", "ScopeGoto", "WheelMove"] {
        assert!(
            fw706.contains(added),
            "expected {added} event in 7.06 captures"
        );
    }
}

// ----------------------------------------------------------------------------

fn truncate(v: &Value) -> String {
    let s = v.to_string();
    if s.len() > 160 {
        format!("{}…", &s[..160])
    } else {
        s
    }
}

fn dedup(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v.dedup();
    v
}

// The device's self-description is read from real `get_device_state` replies:
// the S50 and S30 captures give different models, firmware and optics, which
// is what the high-level backend builds its capabilities from.
#[test]
fn device_summary_is_read_from_captured_device_state() {
    use scopinator_seestar::DeviceSummary;

    let mut by_model: std::collections::BTreeMap<String, DeviceSummary> = Default::default();
    for session in load_corpus() {
        for msg in session.telescope_messages() {
            if msg.get("method").and_then(Value::as_str) != Some("get_device_state") {
                continue;
            }
            let Some(result) = msg.get("result") else {
                continue;
            };
            let mut summary = DeviceSummary::default();
            if summary.update_from_device_state(result)
                && let Some(model) = summary.model.clone()
            {
                by_model
                    .entry(model)
                    .or_default()
                    .update_from_device_state(result);
            }
        }
    }

    let s50 = &by_model["Seestar S50"];
    let s30 = &by_model["Seestar S30"];
    assert_eq!(s50.firmware, Some(FirmwareVersion(2670)));
    assert_eq!(s30.firmware, Some(FirmwareVersion(2706)));
    assert_eq!(s50.focal_length_mm, Some(250.0));
    assert_eq!(s30.focal_length_mm, Some(150.0));
    for summary in [s50, s30] {
        assert_eq!(summary.chip_size, Some((1080, 1920)));
        assert_eq!(summary.pixel_size_um, Some(2.9));
        assert_eq!(summary.debayer_pattern.as_deref(), Some("GR"));
        assert_eq!(summary.capabilities.get("can_wide_cam_af"), Some(&false));
    }
    // Only the S30 has a second (wide) camera section.
    assert_eq!(s50.has_second_camera, Some(false));
    assert_eq!(s30.has_second_camera, Some(true));

    // A keyed reply with only one section updates that section and keeps the rest.
    let mut partial = s50.clone();
    assert!(
        partial
            .update_from_device_state(&serde_json::json!({"device": {"firmware_ver_int": 2700}}))
    );
    assert_eq!(partial.firmware, Some(FirmwareVersion(2700)));
    assert_eq!(partial.chip_size, s50.chip_size);
    assert_eq!(partial.has_second_camera, s50.has_second_camera);
    assert!(!partial.update_from_device_state(&serde_json::json!({"mount": {}})));
}
