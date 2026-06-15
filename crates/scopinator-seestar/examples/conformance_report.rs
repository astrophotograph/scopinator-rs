//! Emit a normalized conformance report for the session corpus, as scopinator-rs
//! sees it. The companion `conformance/parity/pyscopinator_report.py` emits the
//! same schema for pyscopinator, and `conformance/parity/compare.py` diffs them
//! to verify the two implementations stay in parity.
//!
//! Usage:
//!     cargo run -p scopinator-seestar --example conformance_report [-- <sessions_dir>]
//!
//! Prints JSON to stdout. Schema (version 1):
//!   { "schema": 1, "impl": "scopinator-rs",
//!     "sessions": [ { "session": str, "firmware_int": int|null,
//!                     "messages": [ <obs>, ... ] } ] }
//! where each <obs> is one of:
//!   client:    { "i": int, "dir": "client", "method": str, "modeled": bool }
//!   telescope: { "i": int, "dir": "telescope", "class": "event"|"response"|"unknown",
//!                "parse_ok": bool, "event": str?, "method": str?, "id": int?, "code": int? }

use std::path::{Path, PathBuf};

use scopinator_seestar::command::command_method_names;
use scopinator_seestar::event::SeestarEvent;
use scopinator_seestar::protocol::json_rpc;
use scopinator_seestar::response::CommandResponse;
use serde_json::{Value, json};

fn corpus_dir() -> PathBuf {
    if let Some(arg) = std::env::args().nth(1) {
        return PathBuf::from(arg);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("conformance")
        .join("sessions")
}

fn is_modeled(method: &str) -> bool {
    command_method_names().contains(&method)
}

fn observe_client(i: usize, msg: &Value) -> Value {
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    json!({
        "i": i,
        "dir": "client",
        "method": method,
        "modeled": is_modeled(method),
    })
}

fn observe_telescope(i: usize, msg: &Value) -> Value {
    if json_rpc::is_event(msg) {
        let parsed = serde_json::from_value::<SeestarEvent>(msg.clone());
        let parse_ok = matches!(&parsed, Ok(e) if !matches!(e, SeestarEvent::Unknown));
        json!({
            "i": i,
            "dir": "telescope",
            "class": "event",
            "event": json_rpc::event_name(msg),
            "parse_ok": parse_ok,
        })
    } else if json_rpc::is_response(msg) {
        let parse_ok = serde_json::from_value::<CommandResponse>(msg.clone()).is_ok();
        json!({
            "i": i,
            "dir": "telescope",
            "class": "response",
            "method": msg.get("method").and_then(Value::as_str),
            "id": json_rpc::json_rpc_id(msg),
            "code": json_rpc::response_code(msg),
            "parse_ok": parse_ok,
        })
    } else {
        json!({ "i": i, "dir": "telescope", "class": "unknown", "parse_ok": false })
    }
}

fn main() {
    let dir = corpus_dir();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read corpus {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("control.jsonl").is_file())
        .collect();
    entries.sort();

    let mut sessions = Vec::new();
    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(path.join("control.jsonl")).expect("read control.jsonl");
        let mut messages = Vec::new();
        let mut firmware_int: Option<u64> = None;

        for (i, line) in text.lines().filter(|l| !l.trim().is_empty()).enumerate() {
            let rec: Value = serde_json::from_str(line).expect("record");
            let raw = rec.get("raw").and_then(Value::as_str).expect("raw");
            let msg: Value = serde_json::from_str(raw).expect("embedded json");

            if firmware_int.is_none() {
                firmware_int = msg
                    .get("result")
                    .and_then(|r| r.get("device"))
                    .and_then(|d| d.get("firmware_ver_int"))
                    .and_then(Value::as_u64);
            }

            let dir_field = rec.get("direction").and_then(Value::as_str).unwrap_or("");
            messages.push(match dir_field {
                "client" => observe_client(i, &msg),
                _ => observe_telescope(i, &msg),
            });
        }

        sessions.push(json!({
            "session": name,
            "firmware_int": firmware_int,
            "messages": messages,
        }));
    }

    let report = json!({
        "schema": 1,
        "impl": "scopinator-rs",
        "sessions": sessions,
    });
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}
