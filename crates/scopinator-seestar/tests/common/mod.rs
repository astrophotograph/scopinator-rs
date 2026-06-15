//! Shared helpers for loading the language-neutral conformance corpus.
//!
//! The corpus lives at `<repo>/conformance/sessions/<name>/control.jsonl` and is
//! replayed by both this crate's tests and the Python (pyscopinator) parity
//! harness. Each line is one captured message:
//!
//! ```json
//! {"timestamp": 1775012248.33, "direction": "client", "raw": "<json-rpc string>"}
//! ```
//!
//! `direction` is from the proxy's vantage point: `client` = app→telescope
//! (a command), `telescope` = telescope→app (a response or an async event).

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

/// One captured control-channel message.
#[derive(Debug, Clone, Deserialize)]
pub struct Record {
    pub timestamp: f64,
    pub direction: String,
    /// The raw JSON-RPC payload, as a string (it is JSON-within-JSON).
    pub raw: String,
}

impl Record {
    pub fn is_client(&self) -> bool {
        self.direction == "client"
    }
    pub fn is_telescope(&self) -> bool {
        self.direction == "telescope"
    }
    /// Parse the embedded `raw` payload into a JSON value.
    pub fn message(&self) -> Value {
        serde_json::from_str(&self.raw)
            .unwrap_or_else(|e| panic!("corpus has malformed raw JSON: {e}\n  raw = {}", self.raw))
    }
}

/// A loaded session: its directory name plus all parsed records.
pub struct Session {
    pub name: String,
    pub records: Vec<Record>,
}

impl Session {
    pub fn client_messages(&self) -> impl Iterator<Item = Value> + '_ {
        self.records
            .iter()
            .filter(|r| r.is_client())
            .map(|r| r.message())
    }
    pub fn telescope_messages(&self) -> impl Iterator<Item = Value> + '_ {
        self.records
            .iter()
            .filter(|r| r.is_telescope())
            .map(|r| r.message())
    }
}

/// Absolute path to `conformance/sessions`, resolved from this crate's manifest.
pub fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("conformance")
        .join("sessions")
}

/// Load every session directory in the corpus, sorted by name.
///
/// Panics if the corpus is missing or empty — the corpus is committed, so its
/// absence is a real test-environment failure, not a skip condition.
pub fn load_corpus() -> Vec<Session> {
    let dir = corpus_dir();
    let mut sessions = Vec::new();
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read corpus dir {}: {e}", dir.display()));
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("control.jsonl").is_file())
        .collect();
    paths.sort();
    for p in paths {
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(p.join("control.jsonl")).expect("read control.jsonl");
        let records = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<Record>(l).expect("parse corpus record"))
            .collect();
        sessions.push(Session { name, records });
    }
    assert!(
        !sessions.is_empty(),
        "conformance corpus is empty at {}",
        dir.display()
    );
    sessions
}

/// Extract `firmware_ver_int` from a `get_device_state` response, if present.
pub fn firmware_int_from_response(msg: &Value) -> Option<u64> {
    if msg.get("method").and_then(Value::as_str) != Some("get_device_state") {
        return None;
    }
    msg.get("result")?
        .get("device")?
        .get("firmware_ver_int")?
        .as_u64()
}

/// Result of removing the firmware-injected `verify` marker from a params value.
pub struct Stripped {
    /// True if the captured message carried a `verify` marker.
    pub had_verify: bool,
    /// The base params with `verify` removed; `None` means "no params".
    pub base: Option<Value>,
}

/// Invert verify injection to recover the semantic (base) params of a command.
///
/// Mirrors the injection strategy in `command::serialize::inject_verify`:
/// - `["verify"]`                 -> no params
/// - array ending in `"verify"`   -> array without the trailing marker
/// - object with `"verify": true` -> object without that key
/// - anything else                -> unchanged, no verify
pub fn strip_verify(params: Option<&Value>) -> Stripped {
    match params {
        None | Some(Value::Null) => Stripped {
            had_verify: false,
            base: None,
        },
        Some(Value::Array(arr)) => {
            let ends_with_verify = arr.last() == Some(&Value::String("verify".into()));
            if !ends_with_verify {
                return Stripped {
                    had_verify: false,
                    base: Some(Value::Array(arr.clone())),
                };
            }
            let rest = &arr[..arr.len() - 1];
            let base = if rest.is_empty() {
                None
            } else {
                Some(Value::Array(rest.to_vec()))
            };
            Stripped {
                had_verify: true,
                base,
            }
        }
        Some(Value::Object(map)) => {
            if map.get("verify") == Some(&Value::Bool(true)) {
                let mut m = map.clone();
                m.remove("verify");
                Stripped {
                    had_verify: true,
                    base: Some(Value::Object(m)),
                }
            } else {
                Stripped {
                    had_verify: false,
                    base: Some(Value::Object(map.clone())),
                }
            }
        }
        Some(other) => Stripped {
            had_verify: false,
            base: Some(other.clone()),
        },
    }
}

// ============================================================================
// FakeSeestar: a programmable localhost stand-in for a Seestar telescope.
//
// It serves a control port (line-delimited JSON-RPC) and an idle imaging port.
// Tests configure canned responses per method, then drive the real
// `SeestarClient` against it to exercise connect/correlate/event/disconnect/
// reconnect paths — the network-IO and concurrency surface.
// ============================================================================

/// Action pushed to the currently-connected control socket.
#[derive(Clone, Debug)]
enum ServerAction {
    /// Write a raw line (terminator appended if missing).
    SendLine(String),
    /// Close the current connection (EOF to the client).
    Drop,
}

struct Shared {
    /// method -> full response template; the request's `id` is substituted in.
    responses: Mutex<HashMap<String, Value>>,
    /// Methods that cause the server to drop the connection without replying.
    drop_on: Mutex<HashSet<String>>,
    /// Methods the server receives but deliberately never answers (timeout tests).
    silent_on: Mutex<HashSet<String>>,
    /// Every client message the server has received, in order.
    received: Mutex<Vec<Value>>,
    /// Count of accepted control connections (for reconnect assertions).
    connections: AtomicUsize,
    /// Broadcast of actions to the active connection.
    actions: broadcast::Sender<ServerAction>,
}

/// Handle to a running fake telescope.
pub struct FakeSeestar {
    pub control_addr: SocketAddr,
    pub imaging_addr: SocketAddr,
    shared: Arc<Shared>,
}

impl FakeSeestar {
    /// Start a fake telescope with no canned responses (every command gets a
    /// default success echo). Use the `respond*`/`drop_on` builders to customize.
    pub async fn start() -> Self {
        let control = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind control");
        let imaging = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind imaging");
        let control_addr = control.local_addr().unwrap();
        let imaging_addr = imaging.local_addr().unwrap();

        let (actions, _) = broadcast::channel(256);
        let shared = Arc::new(Shared {
            responses: Mutex::new(HashMap::new()),
            drop_on: Mutex::new(HashSet::new()),
            silent_on: Mutex::new(HashSet::new()),
            received: Mutex::new(Vec::new()),
            connections: AtomicUsize::new(0),
            actions,
        });

        // Control accept loop.
        {
            let shared = Arc::clone(&shared);
            tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = control.accept().await else {
                        break;
                    };
                    shared.connections.fetch_add(1, Ordering::SeqCst);
                    let shared = Arc::clone(&shared);
                    tokio::spawn(async move { serve_control(stream, shared).await });
                }
            });
        }
        // Imaging accept loop — accept and idle so the imaging task stays quiet.
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = imaging.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    use tokio::io::AsyncReadExt;
                    let mut s = stream;
                    while let Ok(n) = s.read(&mut buf).await {
                        if n == 0 {
                            break;
                        }
                    }
                });
            }
        });

        FakeSeestar {
            control_addr,
            imaging_addr,
            shared,
        }
    }

    /// Register a full response template for `method`. `id` is overwritten per request.
    pub fn respond(&self, method: &str, template: Value) -> &Self {
        self.shared
            .responses
            .lock()
            .unwrap()
            .insert(method.to_string(), template);
        self
    }

    /// Register a successful response carrying `result` for `method`.
    pub fn respond_ok(&self, method: &str, result: Value) -> &Self {
        self.respond(
            method,
            json!({"jsonrpc": "2.0", "method": method, "code": 0, "result": result, "id": 0}),
        )
    }

    /// Register an error response for `method`.
    pub fn respond_error(&self, method: &str, code: i32, error: &str) -> &Self {
        self.respond(
            method,
            json!({"jsonrpc": "2.0", "method": method, "code": code, "error": error, "id": 0}),
        )
    }

    /// Make `method` drop the connection instead of replying.
    pub fn drop_on(&self, method: &str) -> &Self {
        self.shared
            .drop_on
            .lock()
            .unwrap()
            .insert(method.to_string());
        self
    }

    /// Make `method` be received but never answered (so the caller times out).
    pub fn silent_on(&self, method: &str) -> &Self {
        self.shared
            .silent_on
            .lock()
            .unwrap()
            .insert(method.to_string());
        self
    }

    /// Push an async event to the connected client.
    pub fn send_event(&self, event: Value) {
        let _ = self
            .shared
            .actions
            .send(ServerAction::SendLine(event.to_string()));
    }

    /// Push an arbitrary raw line (e.g. malformed JSON) to the client.
    pub fn send_raw_line(&self, line: &str) {
        let _ = self
            .shared
            .actions
            .send(ServerAction::SendLine(line.to_string()));
    }

    /// Force-drop the current connection (to test disconnect/reconnect).
    pub fn drop_connection(&self) {
        let _ = self.shared.actions.send(ServerAction::Drop);
    }

    /// Snapshot of every client message received so far.
    pub fn received(&self) -> Vec<Value> {
        self.shared.received.lock().unwrap().clone()
    }

    /// Number of control connections accepted.
    pub fn connection_count(&self) -> usize {
        self.shared.connections.load(Ordering::SeqCst)
    }
}

async fn serve_control(stream: TcpStream, shared: Arc<Shared>) {
    let _ = stream.set_nodelay(true);
    let (read_half, mut write_half) = tokio::io::split(stream);
    let mut reader = BufReader::new(read_half);
    let mut actions = shared.actions.subscribe();
    let mut line = String::new();

    loop {
        line.clear();
        tokio::select! {
            res = reader.read_line(&mut line) => {
                match res {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() { continue; }
                        let Ok(msg) = serde_json::from_str::<Value>(trimmed) else { continue };
                        let method = msg.get("method").and_then(Value::as_str).unwrap_or("").to_string();
                        let id = msg.get("id").cloned().unwrap_or(Value::Null);
                        shared.received.lock().unwrap().push(msg);

                        if shared.drop_on.lock().unwrap().contains(&method) {
                            return; // drop without replying
                        }
                        if shared.silent_on.lock().unwrap().contains(&method) {
                            continue; // received, but never answered
                        }
                        // Heartbeats (pi_get_time id:1) are answered generically.
                        let mut response = shared
                            .responses
                            .lock()
                            .unwrap()
                            .get(&method)
                            .cloned()
                            .unwrap_or_else(|| json!({
                                "jsonrpc": "2.0",
                                "method": method,
                                "code": 0,
                                "result": {"echo_id": id},
                                "id": 0,
                            }));
                        response["id"] = id;
                        let out = format!("{response}\r\n");
                        if write_half.write_all(out.as_bytes()).await.is_err() {
                            return;
                        }
                    }
                }
            }
            action = actions.recv() => {
                match action {
                    Ok(ServerAction::SendLine(s)) => {
                        let s = if s.ends_with('\n') { s } else { format!("{s}\r\n") };
                        if write_half.write_all(s.as_bytes()).await.is_err() { return; }
                    }
                    Ok(ServerAction::Drop) => return,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        }
    }
}

/// A real (sanitized) `get_device_state` response carrying the given firmware
/// int, suitable for `respond("get_device_state", device_state_response(..))`.
pub fn device_state_response(firmware_ver_int: u32, firmware_ver_string: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "get_device_state",
        "result": {
            "device": {
                "name": "fake",
                "firmware_ver_int": firmware_ver_int,
                "firmware_ver_string": firmware_ver_string,
                "product_model": "Seestar S50"
            }
        },
        "code": 0,
        "id": 0
    })
}
