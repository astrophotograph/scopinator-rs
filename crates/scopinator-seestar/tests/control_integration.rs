//! Integration tests for the control connection's network-IO and concurrency
//! behavior, driving the real `SeestarClient` against the `FakeSeestar` harness.
//!
//! These cover the failure points that unit tests can't reach: command/response
//! correlation under concurrency, event fan-out, disconnect flushing, automatic
//! reconnect, malformed-input resilience, and response timeouts. Event and
//! device-state payloads are drawn from the real captured corpus.
//!
//! # pyscopinator connection edge-case port (tests/unit/test_connection.py, 43 tests)
//!
//! Inventory of the Python oracle's 43 tests and where each landed. Python names
//! are `Class::test`; the oracle itself is pasted in the task spec (the sibling
//! repo is not readable from here).
//!
//! **Ported** (new tests in this file, or unit tests in
//! `src/connection/control.rs::connection_edge_case_tests`):
//!
//! - TestConnectionInit::test_fields -> control.rs unit
//!   `connect_and_heartbeat_timeouts_are_fixed_constants`
//! - TestIsConnected::test_no_reader_writer ->
//!   `never_connected_client_stays_disconnected_and_times_out`
//! - TestIsConnected::test_flag_false_with_streams ->
//!   `auth_failure_leaves_tcp_up_but_never_advertised`
//! - TestOpen::test_open_os_error ->
//!   `never_connected_client_stays_disconnected_and_times_out`
//! - TestWrite::test_write_success -> `command_lines_are_crlf_terminated`
//! - TestWrite::test_write_increments_counter -> `two_commands_write_two_lines`
//! - TestWrite::test_write_not_connected_raises ->
//!   `never_connected_client_stays_disconnected_and_times_out`
//! - TestWrite::test_write_connection_reset_reconnect_fails ->
//!   `reconnect_failure_fails_pending_and_times_out_new_sends`
//! - TestClose::test_close_sets_disconnected ->
//!   `shutdown_marks_disconnected_and_stops_reconnects`
//! - TestClose::test_close_calls_writer_close ->
//!   `shutdown_marks_disconnected_and_stops_reconnects` (pins the difference:
//!   Rust has no writer.close() — the TCP link lingers after shutdown)
//! - TestClose::test_close_handles_connection_reset_on_wait_closed ->
//!   `shutdown_marks_disconnected_and_stops_reconnects` (Rust teardown is
//!   infallible; nothing can raise during close)
//! - TestClose::test_close_without_writer ->
//!   `shutdown_marks_disconnected_and_stops_reconnects` (double shutdown is a
//!   no-op) and `never_connected_client_stays_disconnected_and_times_out`
//!   (shutdown of a never-connected client)
//! - TestRead::test_read_success -> control.rs unit
//!   `handle_response_delivers_to_registered_caller`
//! - TestRead::test_read_strips_whitespace ->
//!   `whitespace_padded_line_is_trimmed_and_delivered`
//! - TestReconnectWithBackoff::test_failed_reconnect_returns_false ->
//!   `reconnect_failure_fails_pending_and_times_out_new_sends`
//!
//! **Already covered** by the pre-existing tests (no duplicate port written):
//!
//! - TestOpen::test_open_success -> `connects_and_correlates_command_response`
//! - TestIsConnected::test_all_set -> `reconnects_after_connection_drop`
//!   (asserts `is_control_connected()` after connect)
//! - TestRead::test_read_connection_reset_triggers_reconnect ->
//!   `reconnects_after_connection_drop`. Note: Python's read() returns None and
//!   the caller retries in place; Rust tears the connection down, reconnects,
//!   and flushes in-flight callers with `Disconnected` (see
//!   `pending_request_flushed_on_disconnect`).
//! - TestWrite::test_write_connection_reset_then_reconnect_success ->
//!   `pending_request_flushed_on_disconnect` + `reconnects_after_connection_drop`.
//!   Note: Rust does NOT retry the failed write — the in-flight caller gets
//!   `Disconnected`; the *next* command goes to the new connection.
//! - TestWrite::test_write_unexpected_error_closes_connection ->
//!   `pending_request_flushed_on_disconnect` + `reconnects_after_connection_drop`.
//!   Note: Rust has no reset-vs-unexpected classification; every write failure
//!   behaves identically (caller gets `Disconnected`, connection torn down,
//!   reconnect attempted). A non-IO "unexpected error" cannot occur in tokio's
//!   write path.
//! - TestReconnectWithBackoff::test_successful_reconnect ->
//!   `reconnects_after_connection_drop`
//!
//! **Skipped** (no meaningful tokio counterpart):
//!
//! - TestConnectionInit::test_str / test_repr: `str`/`repr` of the connection
//!   object; Rust has no `SeestarConnection` type (address formatting is std's
//!   `SocketAddr` Display, not this crate's code).
//! - TestConnectionInit::test_get_connection_stats: no host/port/is_connected/
//!   message-counter stats API in Rust.
//! - TestIsConnected::test_missing_reader / test_missing_writer: reader and
//!   writer are halves of one `TcpStream` and cannot be independently absent;
//!   the mock-injection setup has no counterpart.
//! - TestIsConnectionResetError::test_connection_reset_error,
//!   test_connection_aborted_error, test_broken_pipe_error,
//!   test_incomplete_read_error, test_os_error, test_asyncio_timeout,
//!   test_value_error, test_runtime_error (all 8): Rust has no error-
//!   classification seam — ANY reader/writer failure or EOF simply ends the
//!   connection and triggers reconnect (pinned by
//!   `reconnects_after_connection_drop`). `io::Error` has no ValueError/
//!   RuntimeError analog to classify, and the reconnect decision never
//!   depends on the error kind.
//! - TestOpen::test_open_timeout: the 10 s `CONNECT_TIMEOUT` path needs a peer
//!   that neither refuses nor completes; loopback TCP either gets an immediate
//!   RST (refused) or completes via the kernel handshake (syncookies), so a
//!   hanging connect cannot be produced in this sandbox. The constant itself
//!   is pinned in the control.rs unit test above.
//! - TestRead::test_read_not_connected_returns_none: Python's read() on an
//!   unconnected connection returns None; Rust exposes no read API — events
//!   flow via broadcast, and a never-connected client simply receives nothing.
//! - TestRead::test_read_unexpected_error_returns_none: a non-EOF read error
//!   cannot be produced on loopback TCP; Rust treats any reader failure as
//!   fatal for the connection (see `reconnects_after_connection_drop`) rather
//!   than returning None.
//! - TestReadExactly::test_read_exactly_not_connected_returns_none,
//!   test_read_exactly_success,
//!   test_read_exactly_connection_reset_triggers_reconnect (all 3): the
//!   control connection has no read_exactly. The binary imaging path's
//!   read_exact semantics (EOF, partial read, truncation) are already pinned
//!   in imaging.rs: `read_frame_eof_before_header_returns_none`,
//!   `read_frame_partial_header_returns_none`,
//!   `read_frame_truncated_payload_returns_none`.
//! - TestReconnectWithBackoff::test_already_connected_returns_true: no
//!   conditional/public reconnect API; the internal loop only reconnects
//!   after a connection dies, so "reconnect while connected" cannot arise.
//! - TestReconnectWithBackoff::test_callback_returns_false_skips_reconnect: no
//!   `_should_reconnect_callback`; the only way to suppress reconnecting is
//!   `shutdown()` (pinned in `shutdown_marks_disconnected_and_stops_reconnects`).
//! - TestReconnectWithBackoff::test_reboot_detection_after_3_attempts: reboot
//!   detection is a Python-only feature (see
//!   docs/bugs/reconnect-backoff-parity.md); Rust's failure counter and backoff
//!   schedule are pinned in `src/connection/reconnect.rs` tests.
//!
//! **Supplementary** (no Python original):
//!
//! - `split_line_reassembles_into_one_event` — the "partial reads" goal on the
//!   control path: a JSON line split across two writes reassembles into one
//!   event. asyncio's `readuntil` does this implicitly in Python; imaging.rs's
//!   `read_frame_split_reads_reassemble_correctly` pins the binary equivalent.
//! - control.rs unit `handle_response_for_unknown_id_is_dropped_without_panic`
//!   — id routing has no Python original (Python hands raw text to callers,
//!   so correlation is the caller's problem there).

mod common;

use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::time::Duration;

use common::{FakeFrame, FakeSeestar, device_state_response, load_corpus};
use scopinator_seestar::command::Command;
use scopinator_seestar::command::params::SettingParams;
use scopinator_seestar::error::SeestarError;
use scopinator_seestar::event::SeestarEvent;
use scopinator_seestar::protocol::json_rpc;
use scopinator_seestar::{
    ClockSync, InteropKey, SeestarClient, SeestarConfig, SettingOutcome, Verification,
};
use serde_json::{Value, json};

const LOCALHOST: Ipv4Addr = Ipv4Addr::LOCALHOST;

async fn connect(fake: &FakeSeestar) -> SeestarClient {
    let client = SeestarClient::connect_with_ports(LOCALHOST, fake.control_addr, fake.imaging_addr)
        .await
        .expect("connect");
    client
        .wait_for_connection(Duration::from_secs(2))
        .await
        .expect("control connected");
    client
}

/// The raw lines the fake received after the connect-time negotiation, whose
/// single `get_verify_str` line (CRLF-terminated like any other) comes first.
fn command_lines(fake: &FakeSeestar) -> Vec<String> {
    let mut raw = fake.raw_lines();
    let first = raw.remove(0);
    assert!(first.contains("get_verify_str"), "got {first:?}");
    assert!(
        first.ends_with("\r\n"),
        "negotiation line is CRLF-terminated"
    );
    raw
}

/// A 127.0.0.1 address nothing listens on: bind a listener, take its port,
/// drop it. Connecting there fails immediately with ECONNREFUSED — the
/// connection-refused stand-in for an unreachable telescope.
async fn dead_addr() -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr
}

/// First telescope event of the given name in the corpus (real captured payload).
fn corpus_event(name: &str) -> Value {
    for session in load_corpus() {
        for msg in session.telescope_messages() {
            if json_rpc::event_name(&msg) == Some(name) {
                return msg;
            }
        }
    }
    panic!("no {name} event in corpus");
}

#[tokio::test]
async fn connects_and_correlates_command_response() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_view_state", json!({"View": {"state": "idle"}}));
    let client = connect(&fake).await;

    let resp = client
        .send_command(Command::GetViewState)
        .await
        .expect("response");
    assert_eq!(resp.code, 0);
    assert!(resp.is_success());
    assert_eq!(resp.result.unwrap()["View"]["state"], "idle");

    client.shutdown().await;
}

#[tokio::test]
async fn concurrent_commands_have_no_crosstalk() {
    // The default echo response embeds the request id in the result. If the
    // registry ever delivered a response to the wrong waiter, the echoed id
    // would not match — catching ID-correlation races under concurrency.
    let fake = FakeSeestar::start().await;
    let client = std::sync::Arc::new(connect(&fake).await);

    let mut handles = Vec::new();
    for _ in 0..64 {
        let client = client.clone();
        handles.push(tokio::spawn(async move {
            let resp = client.send_command(Command::GetDeviceState).await.unwrap();
            let echoed = resp.result.unwrap()["echo_id"].as_u64().unwrap();
            (resp.id, echoed)
        }));
    }

    let mut ids = Vec::new();
    for h in handles {
        let (id, echoed) = h.await.unwrap();
        assert_eq!(id, echoed, "response delivered to the wrong waiter");
        ids.push(id);
    }
    // Every command got a distinct id.
    ids.sort_unstable();
    let unique = {
        let mut v = ids.clone();
        v.dedup();
        v.len()
    };
    assert_eq!(unique, ids.len(), "duplicate command ids issued");

    client.shutdown().await;
}

#[tokio::test]
async fn error_response_maps_to_command_failed() {
    // Real captured error: set_setting -> code 210 "not supported".
    let fake = FakeSeestar::start().await;
    fake.respond_error("get_setting", 210, "not supported");
    let client = connect(&fake).await;

    let err = client
        .send_and_validate(Command::GetSetting)
        .await
        .unwrap_err();
    match err {
        SeestarError::CommandFailed { code, message } => {
            assert_eq!(code, 210);
            assert_eq!(message, "not supported");
        }
        other => panic!("expected CommandFailed, got {other:?}"),
    }
    client.shutdown().await;
}

#[tokio::test]
async fn events_are_broadcast_to_subscribers() {
    let fake = FakeSeestar::start().await;
    let client = connect(&fake).await;
    let mut events = client.subscribe_events();

    // Push real captured event payloads.
    fake.send_event(corpus_event("PiStatus"));
    fake.send_event(corpus_event("ScopeTrack"));

    let mut got_pi_status = false;
    let mut got_scope_track = false;
    for _ in 0..2 {
        let ev = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("event within timeout")
            .expect("event recv");
        match ev {
            SeestarEvent::PiStatus(_) => got_pi_status = true,
            SeestarEvent::ScopeTrack(_) => got_scope_track = true,
            other => panic!("unexpected event {}", other.name()),
        }
    }
    assert!(got_pi_status && got_scope_track);
    client.shutdown().await;
}

#[tokio::test]
async fn pending_request_flushed_on_disconnect() {
    // Server receives the command but drops the connection without replying;
    // the in-flight send must resolve to Disconnected, not hang.
    // (Not `get_device_state`: the client sends that by itself at connect.)
    let fake = FakeSeestar::start().await;
    fake.drop_on("get_view_state");
    let client = connect(&fake).await;

    let err = client
        .send_command(Command::GetViewState)
        .await
        .unwrap_err();
    assert!(matches!(err, SeestarError::Disconnected), "got {err:?}");
    client.shutdown().await;
}

#[tokio::test]
async fn reconnects_after_connection_drop() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_view_state", json!({"View": {"state": "idle"}}));
    let client = connect(&fake).await;
    assert_eq!(fake.connection_count(), 1);

    // Drop the live connection; the client should reconnect on its own.
    fake.drop_connection();

    // Wait for a fresh connection to be accepted.
    let mut reconnected = false;
    for _ in 0..50 {
        if fake.connection_count() >= 2 && client.is_control_connected() {
            reconnected = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        reconnected,
        "client did not reconnect (connections={})",
        fake.connection_count()
    );

    // And it works again on the new connection.
    let resp = client
        .send_command(Command::GetViewState)
        .await
        .expect("post-reconnect response");
    assert!(resp.is_success());
    client.shutdown().await;
}

#[tokio::test]
async fn malformed_json_line_is_skipped() {
    // A garbage line must not break the reader; a valid event after it arrives.
    let fake = FakeSeestar::start().await;
    let client = connect(&fake).await;
    let mut events = client.subscribe_events();

    fake.send_raw_line("this is not json {{{");
    fake.send_event(corpus_event("PiStatus"));

    let ev = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("event within timeout")
        .expect("event recv");
    assert!(matches!(ev, SeestarEvent::PiStatus(_)));
    client.shutdown().await;
}

// The Unknown-event half of docs/bugs/response-drop-timeout.md, now closed.
// An event whose `Event` tag no `SeestarEvent` variant models reaches
// broadcast subscribers as `SeestarEvent::Unknown` with its wire name and
// payload, and the stream keeps flowing — a modeled event sent afterwards
// still arrives.
#[tokio::test]
async fn unknown_event_broadcast_as_unknown_and_stream_continues() {
    let fake = FakeSeestar::start().await;
    let client = connect(&fake).await;
    let mut events = client.subscribe_events();

    fake.send_event(json!({
        "Event": "SomeFutureEvent",
        "Timestamp": "1234567890.0",
        "state": "complete",
        "count": 3
    }));

    let ev = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("event within timeout")
        .expect("event recv");
    assert_eq!(ev.name(), "SomeFutureEvent");
    match &ev {
        SeestarEvent::Unknown { payload, .. } => {
            assert_eq!(payload["state"], "complete");
            assert_eq!(payload["count"], 3);
        }
        other => panic!("expected Unknown, got {}", other.name()),
    }

    // The subscriber stream continues after the unknown event.
    fake.send_event(corpus_event("PiStatus"));
    let ev = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("event within timeout")
        .expect("event recv");
    assert!(matches!(ev, SeestarEvent::PiStatus(_)));
    client.shutdown().await;
}

#[tokio::test]
async fn send_command_times_out_when_unanswered() {
    let fake = FakeSeestar::start().await;
    fake.silent_on("get_device_state");
    let config = SeestarConfig {
        response_timeout: Some(Duration::from_millis(250)),
        ..Default::default()
    };
    let client = SeestarClient::connect_with_ports_and_config(
        LOCALHOST,
        fake.control_addr,
        fake.imaging_addr,
        config,
    )
    .await
    .expect("connect");
    client
        .wait_for_connection(Duration::from_secs(2))
        .await
        .unwrap();

    let start = std::time::Instant::now();
    let err = client
        .send_command(Command::GetDeviceState)
        .await
        .unwrap_err();
    assert!(matches!(err, SeestarError::Timeout(_)), "got {err:?}");
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "timeout took too long"
    );

    // The server did receive the command (it just never answered).
    assert!(
        fake.received()
            .iter()
            .any(|m| m.get("method").and_then(Value::as_str) == Some("get_device_state"))
    );
    client.shutdown().await;
}

// Forge task "Add timeout interplay tests with paused time", case 2.
// While the control connection is down (reconnect backoff is 1 s), a caller
// whose response timeout (500 ms) is shorter than the backoff gets
// `Timeout`, not a hang and not `Disconnected`. The abandoned request is then
// dropped: it must not be written to the telescope after the reconnect, when
// nobody is waiting for it any more. (It used to be; a goto that "timed out"
// could fire later.)
#[tokio::test(start_paused = true)]
async fn response_timeout_fires_during_reconnect_window() {
    let fake = FakeSeestar::start().await;
    let config = SeestarConfig {
        response_timeout: Some(Duration::from_millis(500)),
        ..Default::default()
    };
    let client = SeestarClient::connect_with_ports_and_config(
        LOCALHOST,
        fake.control_addr,
        fake.imaging_addr,
        config,
    )
    .await
    .expect("connect");
    client
        .wait_for_connection(Duration::from_secs(2))
        .await
        .unwrap();

    fake.drop_connection();
    // Wait (bounded, virtual) until the client notices the drop.
    let mut noticed = false;
    for _ in 0..50 {
        if !client.is_control_connected() {
            noticed = true;
            break;
        }
        tokio::time::advance(Duration::from_millis(5)).await;
        tokio::task::yield_now().await;
    }
    assert!(noticed, "client never noticed the dropped connection");

    let start = tokio::time::Instant::now();
    let err = tokio::time::timeout(
        Duration::from_secs(5),
        client.send_command(Command::GetDeviceState),
    )
    .await
    .expect("send_command must not hang")
    .unwrap_err();
    assert!(
        matches!(err, SeestarError::Timeout(d) if d == Duration::from_millis(500)),
        "got {err:?}"
    );
    assert!(start.elapsed() < Duration::from_secs(1));

    // After the reconnect, the abandoned request is not delivered.
    let mut reconnected = false;
    for _ in 0..200 {
        if fake.connection_count() >= 2 && client.is_control_connected() {
            reconnected = true;
            break;
        }
        tokio::time::advance(Duration::from_millis(50)).await;
        tokio::task::yield_now().await;
    }
    assert!(reconnected, "client did not reconnect");
    // Give the new connection's writer ample opportunity to drain the queue
    // (virtual time is paused, so step it and let real loopback IO happen).
    for _ in 0..40 {
        tokio::time::advance(Duration::from_millis(5)).await;
        std::thread::sleep(Duration::from_millis(2));
        tokio::task::yield_now().await;
    }
    assert!(
        !fake
            .received()
            .iter()
            .any(|m| m.get("method").and_then(Value::as_str) == Some("get_device_state")),
        "abandoned request was sent to the telescope after the reconnect"
    );
    client.shutdown().await;
}

// docs/bugs/response-drop-timeout.md. A response-shaped message (id + code
// present, so `json_rpc::is_response` classifies it) whose body fails
// `CommandResponse` deserialization used to be dropped, leaving the caller to
// wait out the whole response timeout. It is now delivered to the caller that
// is waiting on that id as `MalformedResponse`, at once, with the raw line;
// the connection stays healthy and the reader keeps processing.
#[tokio::test(start_paused = true)]
async fn malformed_response_for_pending_id_fails_the_caller_at_once() {
    let fake = FakeSeestar::start().await;
    fake.silent_on("get_view_state");
    let config = SeestarConfig {
        response_timeout: Some(Duration::from_secs(30)),
        ..Default::default()
    };
    let client = std::sync::Arc::new(
        SeestarClient::connect_with_ports_and_config(
            LOCALHOST,
            fake.control_addr,
            fake.imaging_addr,
            config,
        )
        .await
        .expect("connect"),
    );
    client
        .wait_for_connection(Duration::from_secs(2))
        .await
        .expect("control connected");
    let mut events = client.subscribe_events();

    let start = tokio::time::Instant::now();
    let send = {
        let client = std::sync::Arc::clone(&client);
        tokio::spawn(async move { client.send_command(Command::GetViewState).await })
    };

    // Wait (bounded, virtual time) until the command reaches the fake, then
    // read back the id the registry allocated for it. By then the request is
    // registered as pending, so the garbage below reaches a live waiter.
    let mut id = None;
    for _ in 0..200 {
        if let Some(msg) = fake
            .received()
            .iter()
            .find(|m| m.get("method").and_then(Value::as_str) == Some("get_view_state"))
        {
            id = msg.get("id").and_then(Value::as_u64);
            break;
        }
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
    }
    let id = id.expect("fake never received the command");

    // Response-shaped for the pending id, but with a body that cannot
    // deserialize: `code` is present (so `is_response` classifies it) yet
    // string-typed where `CommandResponse::code: i32` wants an integer.
    fake.send_raw_line(&format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"get_view_state","code":"garbage"}}"#
    ));

    // The reader carries on: a later event still flows.
    fake.send_event(corpus_event("PiStatus"));

    let err = tokio::time::timeout(Duration::from_secs(5), send)
        .await
        .expect("send_command must not hang")
        .expect("send task join")
        .unwrap_err();
    // Well inside the 30 s response timeout.
    assert!(start.elapsed() < Duration::from_secs(5));
    match err {
        SeestarError::MalformedResponse {
            id: got,
            reason,
            raw,
        } => {
            assert_eq!(got, id);
            assert!(!reason.is_empty());
            assert!(raw.contains("garbage"), "raw line kept: {raw}");
        }
        other => panic!("got {other:?}"),
    }

    let ev = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("event within timeout")
        .expect("event recv");
    assert!(matches!(ev, SeestarEvent::PiStatus(_)));
    client.shutdown().await;
}

#[tokio::test]
async fn firmware_detected_then_used_for_subsequent_commands() {
    // After a get_device_state carrying fw 2706, the client keeps injecting
    // verify (2706 > threshold). We assert the wire form the server observed.
    let fake = FakeSeestar::start().await;
    fake.respond("get_device_state", device_state_response(2706, "7.06"));
    let client = connect(&fake).await;

    client
        .send_command(Command::GetDeviceState)
        .await
        .expect("device state");
    client
        .send_command(Command::PiStationState)
        .await
        .expect("station state");

    let received = fake.received();
    let station = received
        .iter()
        .find(|m| m.get("method").and_then(Value::as_str) == Some("pi_station_state"))
        .expect("server saw pi_station_state");
    // Verify injection produced ["verify"] for the no-arg command.
    assert_eq!(station["params"], json!(["verify"]));
    client.shutdown().await;
}

// ============================================================================
// Ports of pyscopinator connection edge-case tests (see the header comment
// for the full 43-test inventory: ported / already covered / skipped).
// ============================================================================

// Python test_connection.py TestIsConnected::test_no_reader_writer (a
// connection that never opened reports not-connected), TestOpen::
// test_open_os_error (open against a refusing port errors, stays down) and
// TestWrite::test_write_not_connected_raises (write when not connected).
//
// DIFFERENCE vs Python, pinned: Python's write() raises ConnectionError
// immediately when not connected (and the reconnect wrapper turns that into
// "Failed to reconnect" when the retry fails). Rust has no not-connected guard
// and no reconnect-failure error: the command queues in the channel, no
// writer ever drains it, and the caller eats the full response timeout
// (`Timeout`), while the loop keeps retrying the refused connect forever.
#[tokio::test]
async fn never_connected_client_stays_disconnected_and_times_out() {
    let addr = dead_addr().await;
    let config = SeestarConfig {
        response_timeout: Some(Duration::from_millis(250)),
        ..Default::default()
    };
    let client = SeestarClient::connect_with_ports_and_config(LOCALHOST, addr, addr, config)
        .await
        .expect("client spawns");

    // Never connected: is_control_connected() is false (test_no_reader_writer).
    assert!(!client.is_control_connected());

    // wait_for_connection gives up with Timeout, not a connect error
    // (test_open_os_error: the error stays inside the retry loop).
    let err = client
        .wait_for_connection(Duration::from_millis(300))
        .await
        .unwrap_err();
    assert!(matches!(err, SeestarError::Timeout(_)), "got {err:?}");

    // A write with no connection (test_write_not_connected_raises): Rust
    // queues it and the caller times out, instead of raising immediately.
    let err = client
        .send_command(Command::GetDeviceState)
        .await
        .unwrap_err();
    assert!(
        matches!(err, SeestarError::Timeout(d) if d == Duration::from_millis(250)),
        "got {err:?}"
    );
    assert!(!client.is_control_connected());

    // Shutting down a client that never connected is a no-op, not an error
    // (test_close_without_writer: close with nothing to close must not raise).
    client.shutdown().await;
}

// Python TestIsConnected::test_flag_false_with_streams: the reader and writer
// exist, but the internal connected flag is still False, so is_connected()
// is False. The Rust analog under the tokio architecture: an interop-key
// client against a plain (non-authenticating) fake. The TCP connection IS
// established — the fake accepted it — but negotiation fails on a
// `get_verify_str` reply with no challenge (no result.str), so `connected` is
// never advertised true.
#[tokio::test]
async fn auth_failure_leaves_tcp_up_but_never_advertised() {
    use rsa::pkcs8::EncodePrivateKey;

    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_verify_str", json!({}));
    let private = rsa::RsaPrivateKey::new(&mut rand::rngs::OsRng, 512).unwrap();
    let pem = private.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF).unwrap();
    let config = SeestarConfig {
        interop_key: Some(InteropKey::from_pem(pem.as_str()).unwrap()),
        response_timeout: Some(Duration::from_millis(250)),
        ..Default::default()
    };
    let client = SeestarClient::connect_with_ports_and_config(
        LOCALHOST,
        fake.control_addr,
        fake.imaging_addr,
        config,
    )
    .await
    .expect("client spawns");

    // The TCP link is real: bounded wait for the fake to accept it.
    let mut accepted = false;
    for _ in 0..50 {
        if fake.connection_count() >= 1 {
            accepted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(accepted, "fake never saw the TCP connection");

    // ...but it is never advertised as connected: auth cannot complete
    // against the echo response.
    assert!(!client.is_control_connected());
    let err = client
        .wait_for_connection(Duration::from_millis(300))
        .await
        .unwrap_err();
    assert!(matches!(err, SeestarError::Timeout(_)), "got {err:?}");

    // Commands queue (no writer ever runs) and time out, exactly like the
    // never-connected case.
    let err = client
        .send_command(Command::GetDeviceState)
        .await
        .unwrap_err();
    assert!(matches!(err, SeestarError::Timeout(_)), "got {err:?}");
    client.shutdown().await;
}

// Python TestWrite::test_write_success: writer.write is called once with the
// payload plus "\r\n" (and the written counter increments). Rust has no
// written_messages counter and no raw-string write API — commands are typed
// and serialized by serialize_command. Pin the wire bytes instead: one
// command produces exactly one CRLF-terminated JSON-RPC line.
#[tokio::test]
async fn command_lines_are_crlf_terminated() {
    let fake = FakeSeestar::start().await;
    let client = connect(&fake).await;

    client
        .send_command(Command::GetDeviceState)
        .await
        .expect("response");

    let raw = command_lines(&fake);
    assert_eq!(raw.len(), 1, "one command must write exactly one line");
    assert!(raw[0].ends_with("\r\n"), "line must be CRLF-terminated");
    let msg: Value = serde_json::from_str(raw[0].trim()).expect("line is valid JSON");
    assert_eq!(msg["method"], "get_device_state");
    assert_eq!(msg["id"].as_u64(), Some(json_rpc::INITIAL_COMMAND_ID));
    client.shutdown().await;
}

// Python TestWrite::test_write_increments_counter: two writes bump
// written_messages to 2. No counter in Rust; pin that two commands write two
// lines, in order, with monotonically allocated ids.
#[tokio::test]
async fn two_commands_write_two_lines() {
    let fake = FakeSeestar::start().await;
    let client = connect(&fake).await;

    client
        .send_command(Command::GetDeviceState)
        .await
        .expect("response 1");
    client
        .send_command(Command::GetViewState)
        .await
        .expect("response 2");

    let raw = command_lines(&fake);
    assert_eq!(raw.len(), 2);
    let msgs: Vec<Value> = raw
        .iter()
        .map(|l| serde_json::from_str(l.trim()).expect("line is valid JSON"))
        .collect();
    let methods: Vec<&str> = msgs
        .iter()
        .map(|m| m["method"].as_str().expect("method"))
        .collect();
    assert_eq!(methods, ["get_device_state", "get_view_state"]);
    let ids: Vec<u64> = msgs.iter().map(|m| m["id"].as_u64().expect("id")).collect();
    assert_eq!(
        ids,
        [
            json_rpc::INITIAL_COMMAND_ID,
            json_rpc::INITIAL_COMMAND_ID + 1
        ]
    );
    for line in &raw {
        assert!(
            line.ends_with("\r\n"),
            "every written line is CRLF-terminated"
        );
    }
    client.shutdown().await;
}

// Python TestWrite::test_write_connection_reset_reconnect_fails (a write that
// hits a dead connection, with the subsequent reconnect failing, surfaces an
// error to the caller) and TestReconnectWithBackoff::
// test_failed_reconnect_returns_false (a failed reconnect reports failure).
//
// DIFFERENCE vs Python, pinned: Python's write path retries once after the
// reconnect and raises ConnectionError("Failed to reconnect") when that
// fails. Rust has no retry and no reconnect-failure error: the in-flight
// caller is flushed with `Disconnected` as soon as the reader notices EOF, a
// command issued during the outage queues until the response timeout
// (`Timeout`), and the loop keeps retrying the refused connect forever
// without ever reporting failure to anyone.
#[tokio::test]
async fn reconnect_failure_fails_pending_and_times_out_new_sends() {
    let fake = FakeSeestar::start().await;
    let config = SeestarConfig {
        response_timeout: Some(Duration::from_millis(500)),
        ..Default::default()
    };
    let client = SeestarClient::connect_with_ports_and_config(
        LOCALHOST,
        fake.control_addr,
        fake.imaging_addr,
        config,
    )
    .await
    .expect("connect");
    client
        .wait_for_connection(Duration::from_secs(2))
        .await
        .expect("control connected");

    // Take the scope down for good: stop accepting, then kill the live link
    // when the next command arrives (received, dropped without replying).
    fake.stop_accepting();
    fake.drop_on("get_device_state");

    // In-flight caller: Python raises ConnectionError; Rust flushes
    // Disconnected.
    let err = client
        .send_command(Command::GetDeviceState)
        .await
        .unwrap_err();
    assert!(matches!(err, SeestarError::Disconnected), "got {err:?}");

    // Bounded wait until the client notices the dead link.
    let mut noticed = false;
    for _ in 0..50 {
        if !client.is_control_connected() {
            noticed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(noticed, "client never noticed the dropped connection");

    // A new command during the outage: queued (no writer will ever run while
    // nothing accepts), so the caller eats the full response timeout.
    let err = client
        .send_command(Command::GetViewState)
        .await
        .unwrap_err();
    assert!(
        matches!(err, SeestarError::Timeout(d) if d == Duration::from_millis(500)),
        "got {err:?}"
    );

    // Still down, still retrying, no error surfaced anywhere: no second
    // connection was ever accepted.
    assert!(!client.is_control_connected());
    assert_eq!(fake.connection_count(), 1);
    client.shutdown().await;
}

// Python TestClose::test_close_sets_disconnected (close marks the connection
// down), TestClose::test_close_calls_writer_close (close tears the writer
// down), TestClose::test_close_handles_connection_reset_on_wait_closed (close
// tolerates a dead/reset connection) and TestClose::test_close_without_writer
// (close with nothing to close must not raise).
//
// DIFFERENCE vs Python, pinned: Python's close() calls writer.close() /
// wait_closed(), tearing the TCP socket down (tolerating a reset during it).
// Rust's shutdown() only signals the writer task; the detached reader task
// still holds its half of the TcpStream, so the TCP link LINGERS after
// shutdown — the fake's connection stays established — until the runtime
// itself is torn down. Rust's teardown is infallible (a dropped socket
// cannot raise), so there is no reset-during-close to tolerate: shutdown
// after the link is already dead is just a no-op.
#[tokio::test(start_paused = true)]
async fn shutdown_marks_disconnected_and_stops_reconnects() {
    let fake = FakeSeestar::start().await;
    let client = connect(&fake).await;
    assert!(client.is_control_connected());
    assert_eq!(fake.active_control_connections(), 1);

    // Idempotent: a second shutdown is a no-op (test_close_without_writer).
    client.shutdown().await;
    client.shutdown().await;

    // Bounded (virtual-time) wait for the loop to mark the link dead.
    let mut down = false;
    for _ in 0..200 {
        if !client.is_control_connected() {
            down = true;
            break;
        }
        tokio::time::advance(Duration::from_millis(5)).await;
        tokio::task::yield_now().await;
    }
    assert!(down, "shutdown never marked the connection disconnected");

    // The lingering link: the fake's connection is still established after
    // shutdown — Rust has no writer.close() (see the comment above).
    assert_eq!(
        fake.active_control_connections(),
        1,
        "TCP link should linger after shutdown"
    );

    // Killing the lingering link afterwards must NOT bring the client back:
    // the run loop has exited, so no reconnect happens. A live loop would
    // reconnect within the 1 s policy backoff, so step virtual time well
    // past it before asserting.
    fake.drop_connection();
    for _ in 0..100 {
        tokio::time::advance(Duration::from_millis(50)).await;
        tokio::task::yield_now().await;
    }
    assert_eq!(
        fake.connection_count(),
        1,
        "client reconnected after shutdown"
    );
    assert!(!client.is_control_connected());
}

// Python TestRead::test_read_strips_whitespace: readuntil returns
// b"  hello  \r\n" and the caller sees "hello". Rust's reader_task trims each
// line before parsing; pin that a whitespace-padded event line still delivers.
// (There is no raw-text read API — the trimmed line has to parse.)
#[tokio::test]
async fn whitespace_padded_line_is_trimmed_and_delivered() {
    let fake = FakeSeestar::start().await;
    let client = connect(&fake).await;
    let mut events = client.subscribe_events();

    let event = corpus_event("PiStatus");
    fake.send_raw_line(&format!("   {}   \r\n", event));

    let ev = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("event within timeout")
        .expect("event recv");
    assert!(matches!(ev, SeestarEvent::PiStatus(_)));
    client.shutdown().await;
}

// No Python original in test_connection.py (asyncio's readuntil reassembles
// partial reads implicitly) — the "partial reads" goal, for the control
// path. A JSON line delivered in two separate writes must reassemble into
// ONE message: the reader may not treat the first fragment as a line. Mirrors
// imaging.rs's read_frame_split_reads_reassemble_correctly for the binary path.
#[tokio::test]
async fn split_line_reassembles_into_one_event() {
    let fake = FakeSeestar::start().await;
    let client = connect(&fake).await;
    let mut events = client.subscribe_events();

    let event = corpus_event("PiStatus");
    let line = event.to_string();
    let (first, second) = line.split_at(line.len() / 2);
    // The first half carries no terminator (send_raw_fragment appends none),
    // so the client's read_line must buffer it; the second half goes through
    // send_raw_line, which completes the line with "\r\n".
    fake.send_raw_fragment(first);
    fake.send_raw_line(second);

    let ev = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("event within timeout")
        .expect("event recv");
    assert!(matches!(ev, SeestarEvent::PiStatus(_)));
    client.shutdown().await;
}

// ============================================================================
// Verification negotiation at connect (Forge task "Negotiate verification at
// connect"). The device behaviours modelled here were observed on an S50,
// firmware 6.70, on 2026-11-14; the 103 and key-envelope cases follow the
// seestar-spec session-handshake chapter (R28c, R29).
// ============================================================================

fn test_key() -> InteropKey {
    use rsa::pkcs8::EncodePrivateKey;
    let private = rsa::RsaPrivateKey::new(&mut rand::rngs::OsRng, 512).unwrap();
    let pem = private.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF).unwrap();
    InteropKey::from_pem(pem.as_str()).unwrap()
}

async fn connect_with(fake: &FakeSeestar, config: SeestarConfig) -> SeestarClient {
    SeestarClient::connect_with_ports_and_config(
        LOCALHOST,
        fake.control_addr,
        fake.imaging_addr,
        config,
    )
    .await
    .expect("client spawns")
}

fn methods(received: &[Value]) -> Vec<&str> {
    received
        .iter()
        .map(|m| m["method"].as_str().unwrap_or(""))
        .collect()
}

// A legacy device answers get_verify_str with 103. Nothing else is probed and
// commands keep the firmware-rule marker, with or without a key configured: a
// key against a legacy device must not loop on a failing handshake.
#[tokio::test]
async fn legacy_device_connects_with_or_without_a_key() {
    for key in [None, Some(test_key())] {
        let fake = FakeSeestar::start().await;
        let client = connect_with(
            &fake,
            SeestarConfig {
                interop_key: key,
                response_timeout: None,
                ..Default::default()
            },
        )
        .await;
        client
            .wait_for_connection(Duration::from_secs(2))
            .await
            .expect("connected");
        assert_eq!(client.verification(), Some(Verification::Legacy));

        client
            .send_command(Command::TestConnection)
            .await
            .expect("command");
        let received = fake.received();
        assert_eq!(methods(&received), ["get_verify_str", "test_connection"]);
        assert_eq!(received[1]["params"], json!(["verify"]));
        assert_eq!(fake.connection_count(), 1);
        client.shutdown().await;
    }
}

// With a key, the handshake verifies the socket and nothing carries the
// marker afterwards. set_setting in particular must not: the firmware rejects
// the extra member with 109 even on a verified socket.
#[tokio::test]
async fn handshake_mode_sends_no_marker() {
    for verify_result in [json!(0), json!({"key_envelope": {"v": 1}})] {
        let fake = FakeSeestar::start().await;
        fake.respond_ok("get_verify_str", json!({"str": "fake-challenge"}));
        fake.respond_ok("verify_client", verify_result);
        let client = connect_with(
            &fake,
            SeestarConfig {
                interop_key: Some(test_key()),
                response_timeout: None,
                ..Default::default()
            },
        )
        .await;
        client
            .wait_for_connection(Duration::from_secs(2))
            .await
            .expect("connected");
        assert_eq!(client.verification(), Some(Verification::Handshake));

        client
            .send_command(Command::GetDeviceState)
            .await
            .expect("device state");
        client
            .send_command(Command::SetSetting(
                scopinator_seestar::command::params::SettingParams {
                    stack_lenhance: Some(true),
                    ..Default::default()
                },
            ))
            .await
            .expect("set_setting");

        let received = fake.received();
        assert_eq!(
            methods(&received),
            [
                "get_verify_str",
                "verify_client",
                "get_device_state",
                "set_setting"
            ]
        );
        assert_eq!(received[1]["params"]["data"], "fake-challenge");
        assert!(received[2].get("params").is_none(), "{}", received[2]);
        assert!(received[3]["params"].get("verify").is_none());
        client.shutdown().await;
    }
}

// A rejected signature (207) never advertises the connection, and the loop
// retries with backoff rather than proceeding unverified.
#[tokio::test]
async fn rejected_handshake_is_not_connected() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_verify_str", json!({"str": "fake-challenge"}));
    fake.respond_error("verify_client", 207, "fail to operate");
    let client = connect_with(
        &fake,
        SeestarConfig {
            interop_key: Some(test_key()),
            response_timeout: Some(Duration::from_millis(250)),
            ..Default::default()
        },
    )
    .await;

    let err = client
        .wait_for_connection(Duration::from_millis(500))
        .await
        .unwrap_err();
    assert!(matches!(err, SeestarError::Timeout(_)), "got {err:?}");
    assert_eq!(client.verification(), None);
    assert_eq!(
        methods(&fake.received()),
        ["get_verify_str", "verify_client"]
    );
    client.shutdown().await;
}

// No key, and the device serves marked requests on an unverified socket
// (firmware 6.70 / 7.06): marker mode. Every command carries the marker even
// after the firmware is known, and unmarked requests would get no reply.
#[tokio::test]
async fn marker_mode_without_a_key() {
    let fake = FakeSeestar::start().await;
    fake.require_verify_marker();
    fake.respond("get_device_state", device_state_response(2670, "6.70"));
    let client = connect(&fake).await;
    assert_eq!(client.verification(), Some(Verification::Marker));

    client
        .send_command(Command::GetDeviceState)
        .await
        .expect("device state");
    client
        .send_command(Command::PiStationState)
        .await
        .expect("station state");

    let received = fake.received();
    assert_eq!(
        methods(&received),
        [
            "get_verify_str",
            "test_connection",
            "get_device_state",
            "pi_station_state"
        ]
    );
    for msg in &received[1..] {
        assert_eq!(msg["params"], json!(["verify"]), "{msg}");
    }
    client.shutdown().await;
}

// No key, and the device ignores the marker too: every command would be
// silently consumed. The client must say so within the probe timeout rather
// than letting each command run out its 30 s response timeout.
#[tokio::test]
async fn no_key_and_no_marker_reports_verification_required() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_verify_str", json!({"str": "fake-challenge"}));
    fake.silent_on("test_connection");
    let client = connect_with(&fake, SeestarConfig::default()).await;

    // Queued before negotiation finishes: failed as soon as the answer is known.
    let started = std::time::Instant::now();
    let err = client
        .send_command(Command::GetDeviceState)
        .await
        .unwrap_err();
    assert!(
        matches!(err, SeestarError::VerificationRequired),
        "got {err:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(6),
        "took {:?}",
        started.elapsed()
    );

    // Later calls fail at once.
    let err = client
        .wait_for_connection(Duration::from_secs(30))
        .await
        .unwrap_err();
    assert!(matches!(err, SeestarError::VerificationRequired));
    let err = client
        .send_command(Command::GetDeviceState)
        .await
        .unwrap_err();
    assert!(matches!(err, SeestarError::VerificationRequired));
    assert!(!client.is_control_connected());
    assert_eq!(client.verification(), None);
    client.shutdown().await;
}

// An event the device sends while the handshake is in progress is delivered
// to subscribers once the connection is up, not dropped or taken for a reply.
#[tokio::test]
async fn event_during_handshake_reaches_subscribers() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let event = corpus_event("PiStatus").to_string();
    tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let (stream, _) = listener.accept().await.unwrap();
        let (read, mut write) = tokio::io::split(stream);
        let mut reader = BufReader::new(read);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let reply = r#"{"id":1001,"code":103,"error":"method not found"}"#;
        write
            .write_all(format!("{event}\r\n{reply}\r\n").as_bytes())
            .await
            .unwrap();
        // Keep the socket open.
        line.clear();
        while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
            line.clear();
        }
    });

    let imaging = dead_addr().await;
    let client = SeestarClient::connect_with_ports(LOCALHOST, addr, imaging)
        .await
        .expect("client spawns");
    let mut events = client.subscribe_events();
    client
        .wait_for_connection(Duration::from_secs(2))
        .await
        .expect("connected");
    let ev = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("event within timeout")
        .expect("event recv");
    assert!(matches!(ev, SeestarEvent::PiStatus(_)), "got {ev:?}");
    client.shutdown().await;
}

// A goto is `iscope_start_view` in star mode with the target; the firmware has
// no `goto_target` method (103 on an S50, firmware 6.70). RA goes out in hours.
#[tokio::test]
async fn goto_sends_iscope_start_view_with_target() {
    let fake = FakeSeestar::start().await;
    let client = connect(&fake).await;

    let resp = client.goto("M44", 8.69, 19.51).await.expect("goto");
    assert!(resp.is_success());

    let received = fake.received();
    let goto = received.last().expect("goto request");
    assert_eq!(goto["method"], "iscope_start_view");
    assert_eq!(
        goto["params"],
        json!({
            "mode": "star",
            "target_ra_dec": [8.69, 19.51],
            "target_name": "M44",
            "lp_filter": false,
            "verify": true,
        })
    );
    client.shutdown().await;
}

// ============================================================================
// set_setting (Forge task "Make set_setting correct under the verify marker").
// The fake reproduces what an S50 on firmware 6.70 did on 2026-11-14: members
// applied in wire order, no rollback, and the verify marker itself rejected
// with 109 — on a verified socket too.
// ============================================================================

fn setting_store() -> Value {
    json!({
        "auto_af": false,
        "stack_after_goto": false,
        "viewplan_gohome": true,
        "exp_ms": {"stack_l": 10000, "continuous": 500},
    })
}

// The firmware behaviour the client has to cope with, pinned on the fake.
#[tokio::test]
async fn raw_set_setting_under_the_marker_reports_109_even_when_applied() {
    let fake = FakeSeestar::start().await;
    fake.require_verify_marker();
    fake.emulate_settings(setting_store());
    let client = connect(&fake).await;

    // `viewplan_gohome` sorts after `verify`. Written in sorted order the
    // firmware would stop at the marker and never apply it; the client must
    // write the marker last.
    let resp = client
        .send_command(Command::SetSetting(SettingParams {
            viewplan_gohome: Some(false),
            ..Default::default()
        }))
        .await
        .expect("reply");
    assert_eq!(resp.code, 109, "the marker always draws 109");
    assert_eq!(fake.setting("viewplan_gohome"), Some(json!(false)));

    let line = fake.raw_lines().pop().unwrap();
    assert!(
        line.contains(r#""params":{"viewplan_gohome":false,"verify":true}"#),
        "marker must be the last member: {line}"
    );
    client.shutdown().await;
}

// Marker mode through the public API: one request per setting, each 109
// resolved by a readback.
#[tokio::test]
async fn set_setting_in_marker_mode_confirms_by_readback() {
    let fake = FakeSeestar::start().await;
    fake.require_verify_marker();
    fake.emulate_settings(setting_store());
    let client = connect(&fake).await;
    assert_eq!(client.verification(), Some(Verification::Marker));

    let mut params = SettingParams {
        stack_after_goto: Some(true),
        viewplan_gohome: Some(false),
        exp_ms: Some(json!({"stack_l": 20000})),
        ..Default::default()
    };
    // A name this firmware does not know: indistinguishable from the marker's
    // 109 except that the readback cannot confirm it.
    params.extra.insert("no_such_setting".into(), json!(1));

    let results = client.set_setting(params).await.expect("set_setting");
    let outcomes: Vec<(&str, &SettingOutcome)> = results
        .iter()
        .map(|r| (r.key.as_str(), &r.outcome))
        .collect();
    assert_eq!(
        outcomes,
        [
            ("exp_ms", &SettingOutcome::Applied),
            ("no_such_setting", &SettingOutcome::Unconfirmed),
            ("stack_after_goto", &SettingOutcome::Applied),
            ("viewplan_gohome", &SettingOutcome::Applied),
        ]
    );
    assert_eq!(fake.setting("stack_after_goto"), Some(json!(true)));
    assert_eq!(fake.setting("viewplan_gohome"), Some(json!(false)));
    assert_eq!(fake.setting("auto_af"), Some(json!(false)), "untouched");

    // Four single-member requests, then one readback.
    let received = fake.received();
    let sets: Vec<&Value> = received
        .iter()
        .filter(|m| m["method"] == "set_setting")
        .collect();
    assert_eq!(sets.len(), 4);
    for set in sets {
        assert_eq!(set["params"].as_object().unwrap().len(), 2, "{set}");
    }
    assert_eq!(received.last().unwrap()["method"], "get_setting");
    client.shutdown().await;
}

// With the handshake there is no marker, so set_setting is one request with a
// truthful code 0.
#[tokio::test]
async fn set_setting_on_a_verified_socket_is_one_plain_request() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_verify_str", json!({"str": "fake-challenge"}));
    fake.respond_ok("verify_client", json!(0));
    fake.emulate_settings(setting_store());
    let client = connect_with(
        &fake,
        SeestarConfig {
            interop_key: Some(test_key()),
            response_timeout: None,
            ..Default::default()
        },
    )
    .await;
    client
        .wait_for_connection(Duration::from_secs(2))
        .await
        .expect("connected");

    let results = client
        .set_setting(SettingParams {
            stack_after_goto: Some(true),
            viewplan_gohome: Some(false),
            ..Default::default()
        })
        .await
        .expect("set_setting");
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|r| r.is_applied()));
    assert_eq!(fake.setting("stack_after_goto"), Some(json!(true)));
    assert_eq!(fake.setting("viewplan_gohome"), Some(json!(false)));

    let received = fake.received();
    let last = received.last().unwrap();
    assert_eq!(last["method"], "set_setting", "no readback needed");
    assert_eq!(
        last["params"],
        json!({"stack_after_goto": true, "viewplan_gohome": false})
    );

    // A genuine rejection is an error, not a silent success.
    let mut bad = SettingParams::default();
    bad.extra.insert("no_such_setting".into(), json!(1));
    let err = client.set_setting(bad).await.unwrap_err();
    assert!(
        matches!(err, SeestarError::CommandFailed { code: 109, .. }),
        "got {err:?}"
    );
    client.shutdown().await;
}

// ============================================================================
// One-shot image requests on the image socket (Forge task "Send
// get_stacked_img on the image socket and fix Camera::get_image").
// `get_stacked_img` is an image-socket method: on the control port an S50 on
// firmware 6.70 answers 103. Replies are matched by the request-id byte the
// device echoes in the frame header.
// ============================================================================

async fn connect_imaging(fake: &FakeSeestar) -> SeestarClient {
    let client = connect(fake).await;
    for _ in 0..50 {
        if client.is_imaging_connected() {
            return client;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("imaging port never connected");
}

fn zipped(pixels: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .start_file("raw_data", zip::write::SimpleFileOptions::default())
        .unwrap();
    writer.write_all(pixels).unwrap();
    writer.finish().unwrap().into_inner()
}

#[tokio::test]
async fn stacked_image_request_returns_its_own_reply() {
    let pixels: Vec<u8> = (0..=255u8).cycle().take(2 * 4 * 6).collect();
    let fake = FakeSeestar::start().await;
    // A running stream pushes frames carrying the begin_streaming id (21), and
    // a heartbeat echo carries 99. Neither may satisfy the request, even
    // though they arrive first.
    fake.respond_image(
        "get_stacked_img",
        vec![
            FakeFrame::image(2, 1, 4, 6, vec![0xEE; 48]).with_request_id(21),
            FakeFrame {
                data_type: 0,
                img_type: 0,
                width: 0,
                height: 0,
                can_debayer: 0,
                payload: b"server connected!".to_vec(),
                request_id: Some(99),
            },
            FakeFrame::image(3, 1, 4, 6, zipped(&pixels)),
        ],
    );
    let client = connect_imaging(&fake).await;

    let frame = client.get_stacked_image().await.expect("stacked image");
    assert_eq!((frame.header.width, frame.header.height), (4, 6));
    assert_eq!(frame.kind, scopinator_types::FrameKind::Stack);
    assert_eq!(frame.header.bytes_per_pixel(), Some(2));
    assert_eq!(&frame.image_bytes().expect("inflate")[..], &pixels[..]);

    // It went to the image socket, not the control port.
    assert!(
        fake.received()
            .iter()
            .all(|m| m["method"] != "get_stacked_img")
    );

    // A second request uses a different id, so a late reply to an abandoned
    // request cannot be mistaken for it.
    let again = client.get_stacked_image().await.expect("second request");
    assert_ne!(again.header.request_id, frame.header.request_id);
    client.shutdown().await;
}

#[tokio::test]
async fn image_request_error_text_is_a_typed_error() {
    let fake = FakeSeestar::start().await;
    fake.respond_image(
        "get_stacked_img",
        vec![FakeFrame::error("there is no image now")],
    );
    let client = connect_imaging(&fake).await;

    let err = client.get_stacked_image().await.unwrap_err();
    assert!(
        matches!(&err, SeestarError::ImageRequest(m) if m == "there is no image now"),
        "got {err:?}"
    );
    client.shutdown().await;
}

#[tokio::test]
async fn image_request_times_out_when_only_other_frames_arrive() {
    let fake = FakeSeestar::start().await;
    fake.respond_image(
        "get_current_img",
        vec![FakeFrame::image(2, 1, 4, 6, vec![0xEE; 48]).with_request_id(21)],
    );
    let client = SeestarClient::connect_with_ports_and_config(
        LOCALHOST,
        fake.control_addr,
        fake.imaging_addr,
        SeestarConfig {
            interop_key: None,
            response_timeout: Some(Duration::from_millis(300)),
            ..Default::default()
        },
    )
    .await
    .expect("client spawns");
    for _ in 0..100 {
        if client.is_imaging_connected() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let err = client
        .request_image(
            scopinator_seestar::command::ImagingCommand::GetCurrentImage {
                compress: Some(false),
                raw: true,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, SeestarError::Timeout(_)), "got {err:?}");
    client.shutdown().await;
}

// ============================================================================
// Per-connection wire profile (Forge task "Per-connection wire profile for
// verify marker and firmware").
// ============================================================================

// The client asks for the device state by itself as soon as the connection is
// negotiated, framed for the negotiated mode, so the firmware and model are
// known without the caller sending anything.
#[tokio::test]
async fn firmware_and_model_are_learned_at_connect() {
    let fake = FakeSeestar::start().await;
    fake.require_verify_marker();
    fake.respond("get_device_state", device_state_response(2670, "6.70"));
    let client = connect(&fake).await;

    let mut firmware = None;
    for _ in 0..100 {
        firmware = client.firmware_version();
        if firmware.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(firmware.map(|f| f.0), Some(2670));
    assert_eq!(client.product_model().as_deref(), Some("Seestar S50"));

    let all = fake.received_all();
    let probe = all
        .iter()
        .find(|m| m["id"] == common::CONNECT_PROBE_ID)
        .expect("connect-time device state request");
    assert_eq!(probe["method"], "get_device_state");
    assert_eq!(probe["params"], json!(["verify"]), "marker mode marks it");
    // It is the first thing sent after negotiation, and the caller sent nothing.
    assert_eq!(
        methods(&all),
        ["get_verify_str", "test_connection", "get_device_state"]
    );
    client.shutdown().await;
}

// A legacy device on firmware old enough to predate the marker: once the
// connect-time reply reveals the firmware, commands and the heartbeat's
// serializer stop marking, without the caller asking for the device state.
#[tokio::test]
async fn legacy_old_firmware_stops_marking_once_known() {
    let fake = FakeSeestar::start().await;
    fake.respond("get_device_state", device_state_response(2400, "4.00"));
    let client = connect(&fake).await;
    for _ in 0..100 {
        if client.firmware_version().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(client.verification(), Some(Verification::Legacy));

    client
        .send_command(Command::PiStationState)
        .await
        .expect("station state");
    let received = fake.received();
    let station = received.last().unwrap();
    assert_eq!(station["method"], "pi_station_state");
    assert!(station.get("params").is_none(), "{station}");

    // The connect-time request itself went out before the firmware was
    // known, so it was marked.
    let all = fake.received_all();
    let probe = all
        .iter()
        .find(|m| m["id"] == common::CONNECT_PROBE_ID)
        .unwrap();
    assert_eq!(probe["params"], json!(["verify"]));
    client.shutdown().await;
}

// get_disk_volume reads a positional element as a path, so the array marker
// breaks it (255 "file not exist" on an S50, firmware 6.70). It is marked
// with the object form, which the firmware serves.
#[tokio::test]
async fn disk_volume_uses_the_object_marker() {
    let fake = FakeSeestar::start().await;
    fake.require_verify_marker();
    let client = connect(&fake).await;
    client
        .send_command(Command::GetDiskVolume)
        .await
        .expect("disk volume");
    let line = fake.raw_lines().pop().unwrap();
    assert!(
        line.trim_end()
            .ends_with(r#""method":"get_disk_volume","params":{"verify":true}}"#),
        "got {line}"
    );
    client.shutdown().await;
}

// ============================================================================
// Connection limit (Forge task "Back off on connection-limit rejection"). The
// device admits two client addresses (eight with guest mode); beyond that it
// sends Alert 429 to the new socket and closes it.
// ============================================================================

#[tokio::test]
async fn connection_limit_is_reported_and_backed_off() {
    let fake = FakeSeestar::start().await;
    fake.refuse_connections(true);
    let client = connect_with(&fake, SeestarConfig::default()).await;

    // The caller is told why, promptly, instead of waiting out a timeout.
    let started = std::time::Instant::now();
    let err = client
        .wait_for_connection(Duration::from_secs(30))
        .await
        .unwrap_err();
    assert!(matches!(err, SeestarError::ConnectionLimit), "got {err:?}");
    assert!(started.elapsed() < Duration::from_secs(1));
    let err = client
        .send_command(Command::GetDeviceState)
        .await
        .unwrap_err();
    assert!(matches!(err, SeestarError::ConnectionLimit), "got {err:?}");
    assert!(!client.is_control_connected());

    // Backoff grows: waits are 1 s then 2 s, each plus up to 1 s of jitter,
    // so the second attempt lands within 2 s and the third not before 3 s.
    // A loop that reset its backoff on every accepted TCP connection would
    // have made three or more attempts by now.
    tokio::time::sleep(Duration::from_millis(2500).saturating_sub(started.elapsed())).await;
    assert_eq!(fake.connection_count(), 2, "attempts after 2.5 s");

    // Once the device has room again, a later attempt connects and the
    // error clears by itself.
    fake.refuse_connections(false);
    client
        .wait_for_connection(Duration::from_millis(1))
        .await
        .expect_err("still refused until the next attempt");
    let mut connected = false;
    for _ in 0..80 {
        if client.is_control_connected() {
            connected = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(connected, "client did not recover after the limit cleared");
    client
        .send_command(Command::TestConnection)
        .await
        .expect("command after recovery");
    client.shutdown().await;
}

// ============================================================================
// Clock sync at connect (opt-in). A telescope whose clock was never set
// accepts a goto and does not slew (Seestar S30, firmware 9.75), so a client
// can ask for the clock to be set on every connect, as the official app does.
// ============================================================================

fn set_time_requests(fake: &FakeSeestar) -> Vec<Value> {
    fake.received_all()
        .into_iter()
        .filter(|m| m["method"] == "pi_set_time")
        .collect()
}

// Off by default: connecting writes nothing to the device's clock.
#[tokio::test]
async fn clock_is_left_alone_by_default() {
    let fake = FakeSeestar::start().await;
    let client = connect(&fake).await;
    client
        .send_command(Command::TestConnection)
        .await
        .expect("command");
    assert!(set_time_requests(&fake).is_empty());
    client.shutdown().await;
}

// With ClockSync::Utc the clock is set right after negotiation, before the
// device-state request and before anything the caller sends, and again on
// every reconnect.
#[tokio::test]
async fn clock_sync_utc_sets_the_clock_at_every_connect() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_verify_str", json!({"str": "fake-challenge"}));
    fake.respond_ok("verify_client", json!(0));
    let client = connect_with(
        &fake,
        SeestarConfig {
            interop_key: Some(test_key()),
            clock_sync: ClockSync::Utc,
            ..Default::default()
        },
    )
    .await;
    client
        .wait_for_connection(Duration::from_secs(2))
        .await
        .expect("connected");
    client
        .send_command(Command::TestConnection)
        .await
        .expect("command");

    let all = fake.received_all();
    assert_eq!(
        methods(&all),
        [
            "get_verify_str",
            "verify_client",
            "pi_set_time",
            "get_device_state",
            "test_connection"
        ]
    );
    let set = &all[2];
    assert_eq!(set["id"], common::CONNECT_SET_TIME_ID);
    // One settings object in an array, unmarked on a verified socket.
    let time = &set["params"][0];
    assert_eq!(set["params"].as_array().map(Vec::len), Some(1));
    assert_eq!(time["time_zone"], "UTC");
    let year = time["year"].as_i64().unwrap();
    assert!(year >= 2026, "year {year} should be this computer's");
    for member in ["mon", "day", "hour", "min", "sec"] {
        assert!(time[member].is_i64(), "{member} missing: {time}");
    }

    // A reconnect sets it again.
    fake.drop_connection();
    for _ in 0..100 {
        if set_time_requests(&fake).len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(set_time_requests(&fake).len(), 2);
    client.shutdown().await;
}

// ClockSync::With sends exactly what the caller's function returns, e.g.
// local time under the telescope's own zone name. In marker mode the request
// carries the marker after the settings object.
#[tokio::test]
async fn clock_sync_with_sends_the_callers_time() {
    use scopinator_seestar::command::params::SetTimeParams;
    fn local_time() -> SetTimeParams {
        SetTimeParams {
            year: 2026,
            mon: 11,
            day: 14,
            hour: 21,
            min: 0,
            sec: 39,
            time_zone: "America/Chicago".into(),
        }
    }

    let fake = FakeSeestar::start().await;
    fake.require_verify_marker();
    let client = connect_with(
        &fake,
        SeestarConfig {
            clock_sync: ClockSync::With(local_time),
            ..Default::default()
        },
    )
    .await;
    client
        .wait_for_connection(Duration::from_secs(2))
        .await
        .expect("connected");

    let sets = set_time_requests(&fake);
    assert_eq!(sets.len(), 1);
    assert_eq!(
        sets[0]["params"],
        json!([
            {"year": 2026, "mon": 11, "day": 14, "hour": 21, "min": 0, "sec": 39,
             "time_zone": "America/Chicago"},
            "verify"
        ])
    );
    client.shutdown().await;
}

// ============================================================================
// Reconnect races (Forge task "Add reconnect race integration tests"). The
// invariant: a command sent at any point around a disconnect resolves, with a
// response, `Disconnected` or `Timeout`. It is never left pending forever,
// and no response is delivered to the wrong caller.
// ============================================================================

/// Every fake reply echoes the request id in `result.echo_id`; a response
/// routed to the wrong caller would carry a different id than its envelope.
fn assert_own_response(resp: &scopinator_seestar::response::CommandResponse) {
    let echoed = resp.result.as_ref().and_then(|r| r["echo_id"].as_u64());
    assert_eq!(echoed, Some(resp.id), "response crossed over: {resp:?}");
}

async fn wait_for_reconnect(fake: &FakeSeestar, client: &SeestarClient, connections: usize) {
    for _ in 0..100 {
        if fake.connection_count() >= connections && client.is_control_connected() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!(
        "no reconnect: {} connections, connected={}",
        fake.connection_count(),
        client.is_control_connected()
    );
}

// Scenario 1: a command sent while the link is down waits in the queue and is
// answered on the next connection.
#[tokio::test]
async fn command_sent_during_the_disconnect_window_is_answered_after_reconnect() {
    let fake = FakeSeestar::start().await;
    let client = connect(&fake).await;

    fake.drop_connection();
    // Wait until the client has noticed, so the command is sent with no
    // connection at all.
    for _ in 0..100 {
        if !client.is_control_connected() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!client.is_control_connected());

    let started = std::time::Instant::now();
    let resp = client
        .send_command(Command::GetViewState)
        .await
        .expect("answered on the next connection");
    assert_own_response(&resp);
    assert!(fake.connection_count() >= 2);
    // It waited for the reconnect (about 1 to 2 s), not for a timeout.
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    client.shutdown().await;
}

// Scenario 2: several commands are on the wire, unanswered, when the socket
// dies. Every one of them is failed with Disconnected at once.
#[tokio::test]
async fn commands_in_flight_when_the_socket_dies_all_fail_promptly() {
    let fake = FakeSeestar::start().await;
    fake.silent_on("get_view_state");
    let client = std::sync::Arc::new(connect(&fake).await);

    let mut callers = Vec::new();
    for _ in 0..8 {
        let client = std::sync::Arc::clone(&client);
        callers.push(tokio::spawn(async move {
            client.send_command(Command::GetViewState).await
        }));
    }
    // Let all eight reach the fake before the connection is cut.
    for _ in 0..100 {
        let seen = fake
            .received()
            .iter()
            .filter(|m| m["method"] == "get_view_state")
            .count();
        if seen == 8 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let started = std::time::Instant::now();
    fake.drop_connection();

    for caller in callers {
        let result = tokio::time::timeout(Duration::from_secs(5), caller)
            .await
            .expect("caller left pending after the disconnect")
            .unwrap();
        assert!(
            matches!(result, Err(SeestarError::Disconnected)),
            "got {result:?}"
        );
    }
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    client.shutdown().await;
}

// Scenario 3: many senders keep sending while the connection is dropped
// twice. Every call resolves, successes carry their own response, and the
// client works afterwards.
#[tokio::test]
async fn concurrent_senders_across_reconnects_all_resolve_without_crosstalk() {
    let fake = FakeSeestar::start().await;
    let client = std::sync::Arc::new(
        SeestarClient::connect_with_ports_and_config(
            LOCALHOST,
            fake.control_addr,
            fake.imaging_addr,
            SeestarConfig {
                response_timeout: Some(Duration::from_secs(8)),
                ..Default::default()
            },
        )
        .await
        .expect("client spawns"),
    );
    client
        .wait_for_connection(Duration::from_secs(2))
        .await
        .expect("connected");

    let mut senders = Vec::new();
    for worker in 0..12u64 {
        let client = std::sync::Arc::clone(&client);
        senders.push(tokio::spawn(async move {
            let (mut ok, mut failed) = (0usize, 0usize);
            for round in 0..25u64 {
                match client.send_command(Command::GetViewState).await {
                    Ok(resp) => {
                        assert_own_response(&resp);
                        ok += 1;
                    }
                    Err(SeestarError::Disconnected | SeestarError::Timeout(_)) => failed += 1,
                    Err(other) => panic!("worker {worker} round {round}: {other:?}"),
                }
                tokio::time::sleep(Duration::from_millis(20 + worker)).await;
            }
            (ok, failed)
        }));
    }

    // Two drops while the senders are busy.
    tokio::time::sleep(Duration::from_millis(150)).await;
    fake.drop_connection();
    wait_for_reconnect(&fake, &client, 2).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    fake.drop_connection();
    wait_for_reconnect(&fake, &client, 3).await;

    let (mut ok, mut failed) = (0usize, 0usize);
    for sender in senders {
        let (o, f) = tokio::time::timeout(Duration::from_secs(30), sender)
            .await
            .expect("a sender never finished: some call was left pending")
            .unwrap();
        ok += o;
        failed += f;
    }
    assert_eq!(ok + failed, 12 * 25, "every call resolved");
    assert!(ok > 0, "nothing succeeded");

    let resp = client
        .send_command(Command::GetViewState)
        .await
        .expect("client still works");
    assert_own_response(&resp);
    client.shutdown().await;
}

// Scenario 4: the failed-handshake branch. A command queued while the
// handshake keeps failing ends in a bounded Timeout, and once the device
// accepts the handshake the client recovers by itself.
#[tokio::test]
async fn failed_handshake_bounds_queued_commands_and_recovers() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_verify_str", json!({"str": "fake-challenge"}));
    fake.respond_error("verify_client", 207, "fail to operate");
    let client = connect_with(
        &fake,
        SeestarConfig {
            interop_key: Some(test_key()),
            response_timeout: Some(Duration::from_millis(400)),
            ..Default::default()
        },
    )
    .await;

    let started = std::time::Instant::now();
    let err = client
        .send_command(Command::GetViewState)
        .await
        .unwrap_err();
    assert!(matches!(err, SeestarError::Timeout(_)), "got {err:?}");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert!(!client.is_control_connected());

    // The device starts accepting the signature.
    fake.respond_ok("verify_client", json!(0));
    for _ in 0..200 {
        if client.is_control_connected() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(client.verification(), Some(Verification::Handshake));
    // The command that timed out was dropped from the queue with its caller,
    // not replayed: the first command on the new connection is this one.
    let resp = client
        .send_command(Command::TestConnection)
        .await
        .expect("command after recovery");
    assert_own_response(&resp);
    let replayed = fake
        .received()
        .iter()
        .filter(|m| m["method"] == "get_view_state")
        .count();
    assert_eq!(
        replayed, 0,
        "a command whose caller gave up was sent to the device after the reconnect"
    );
    client.shutdown().await;
}

// ============================================================================
// Raw commands (Forge task "Raw command escape hatch through the wire
// profile"): any method by name, framed by the connection's wire profile.
// ============================================================================

#[tokio::test]
async fn send_raw_in_marker_mode_marks_by_params_shape() {
    use scopinator_seestar::command::RawMarker;

    let fake = FakeSeestar::start().await;
    fake.require_verify_marker();
    fake.respond_ok("iscope_get_app_state", json!({"View": {"state": "idle"}}));
    fake.respond_error("no_such_method", 103, "method not found");
    let client = connect(&fake).await;

    let state = client
        .send_raw_and_validate("iscope_get_app_state", None)
        .await
        .expect("app state");
    assert_eq!(state["View"]["state"], "idle");
    client
        .send_raw("set_something", Some(json!({"zeta": 1, "alpha": true})))
        .await
        .expect("object params");
    client
        .send_raw("move_something", Some(json!([1, 2])))
        .await
        .expect("array params");
    client
        .send_raw_with("get_disk_volume", None, RawMarker::Object)
        .await
        .expect("object marker");

    // A device error comes back as a response, or as CommandFailed when validated.
    let resp = client
        .send_raw("no_such_method", None)
        .await
        .expect("reply");
    assert_eq!(resp.code, 103);
    let err = client
        .send_raw_and_validate("no_such_method", None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, SeestarError::CommandFailed { code: 103, .. }),
        "got {err:?}"
    );

    let lines = command_lines(&fake);
    let tail = |i: usize| {
        lines[i]
            .trim_end()
            .split_once(r#""method":"#)
            .unwrap()
            .1
            .to_string()
    };
    // lines[0] is the marker probe (test_connection).
    assert_eq!(tail(1), r#""iscope_get_app_state","params":["verify"]}"#);
    assert_eq!(
        tail(2),
        r#""set_something","params":{"alpha":true,"zeta":1,"verify":true}}"#
    );
    assert_eq!(tail(3), r#""move_something","params":[1,2,"verify"]}"#);
    assert_eq!(tail(4), r#""get_disk_volume","params":{"verify":true}}"#);
    client.shutdown().await;
}

#[tokio::test]
async fn send_raw_on_a_verified_socket_adds_nothing() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_verify_str", json!({"str": "fake-challenge"}));
    fake.respond_ok("verify_client", json!(0));
    let client = connect_with(
        &fake,
        SeestarConfig {
            interop_key: Some(test_key()),
            ..Default::default()
        },
    )
    .await;
    client
        .wait_for_connection(Duration::from_secs(2))
        .await
        .expect("connected");

    client
        .send_raw("iscope_get_app_state", None)
        .await
        .expect("no params");
    client
        .send_raw("set_something", Some(json!({"zeta": 1})))
        .await
        .expect("object params");

    let received = fake.received();
    let n = received.len();
    assert_eq!(received[n - 2]["method"], "iscope_get_app_state");
    assert!(received[n - 2].get("params").is_none());
    assert_eq!(received[n - 1]["params"], json!({"zeta": 1}));
    client.shutdown().await;
}
