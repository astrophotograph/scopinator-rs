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

use common::{FakeSeestar, device_state_response, load_corpus};
use scopinator_seestar::command::Command;
use scopinator_seestar::error::SeestarError;
use scopinator_seestar::event::SeestarEvent;
use scopinator_seestar::protocol::json_rpc;
use scopinator_seestar::{InteropKey, SeestarClient, SeestarConfig};
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
    let fake = FakeSeestar::start().await;
    fake.drop_on("get_device_state");
    let client = connect(&fake).await;

    let err = client
        .send_command(Command::GetDeviceState)
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

// Characterizes the Unknown-event half of docs/bugs/response-drop-timeout.md
// (Forge task "Characterize silent drop paths and write bug report"). An event
// whose `Event` tag no `SeestarEvent` variant models still reaches broadcast
// subscribers — but only as `SeestarEvent::Unknown`: serde's `#[serde(other)]`
// requires a unit variant, so the wire tag and payload are discarded. Pinned:
// subscribers get `Unknown` (nothing else to see), and the broadcast stream
// keeps flowing — a modeled event sent afterwards still arrives.
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
    // GAP (characterized, not fixed): the payload — and even the wire tag —
    // are inaccessible on the delivered value.
    assert!(matches!(&ev, SeestarEvent::Unknown));
    assert_eq!(ev.name(), "Unknown");

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
// `Timeout`, not a hang and not `Disconnected`. Pinned side effect: the
// request stays queued and is still written to the telescope after the
// reconnect, even though the caller already gave up.
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

    // After the reconnect, the abandoned request is still delivered.
    let mut delivered = false;
    for _ in 0..100 {
        if fake.connection_count() >= 2
            && fake
                .received()
                .iter()
                .any(|m| m.get("method").and_then(Value::as_str) == Some("get_device_state"))
        {
            delivered = true;
            break;
        }
        tokio::time::advance(Duration::from_millis(50)).await;
        tokio::task::yield_now().await;
    }
    assert!(delivered, "stale request was not delivered after reconnect");
    client.shutdown().await;
}

// Characterizes docs/bugs/response-drop-timeout.md (Forge task "Characterize
// silent drop paths and write bug report"). A response-shaped message (id +
// code present, so `json_rpc::is_response` classifies it) whose body fails
// `CommandResponse` deserialization is warn-and-dropped in `handle_response`
// (control.rs) — even when its id matches a pending request. Pinned: the
// waiting caller gets no correlation error and eats the full configured
// response timeout (500 ms here; `DEFAULT_RESPONSE_TIMEOUT` = 30 s in
// production), while the connection stays healthy — the error is `Timeout`,
// not `Disconnected`, and the reader keeps processing (the event pushed after
// the garbage body still arrives).
#[tokio::test(start_paused = true)]
async fn malformed_response_for_pending_id_is_dropped_and_caller_times_out() {
    let fake = FakeSeestar::start().await;
    fake.silent_on("get_view_state");
    let config = SeestarConfig {
        response_timeout: Some(Duration::from_millis(500)),
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
    // registered as pending, so the garbage below really is dropped for a
    // live waiter.
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

    // The reader survives the warn-and-drop: a later event still flows.
    fake.send_event(corpus_event("PiStatus"));

    let err = tokio::time::timeout(Duration::from_secs(5), send)
        .await
        .expect("send_command must not hang")
        .expect("send task join")
        .unwrap_err();
    // The caller ate the full timeout rather than erroring promptly.
    assert!(start.elapsed() >= Duration::from_millis(500));
    assert!(
        matches!(err, SeestarError::Timeout(d) if d == Duration::from_millis(500)),
        "got {err:?}"
    );

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
// established — the fake accepted it — but authenticate() fails on the fake's
// echo reply (no result.str), so `connected` is never advertised true.
#[tokio::test]
async fn auth_failure_leaves_tcp_up_but_never_advertised() {
    use rsa::pkcs8::EncodePrivateKey;

    let fake = FakeSeestar::start().await;
    let private = rsa::RsaPrivateKey::new(&mut rand::rngs::OsRng, 512).unwrap();
    let pem = private.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF).unwrap();
    let config = SeestarConfig {
        interop_key: Some(InteropKey::from_pem(pem.as_str()).unwrap()),
        response_timeout: Some(Duration::from_millis(250)),
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

    let raw = fake.raw_lines();
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

    let raw = fake.raw_lines();
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
