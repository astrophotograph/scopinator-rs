//! Integration tests for the control connection's network-IO and concurrency
//! behavior, driving the real `SeestarClient` against the `FakeSeestar` harness.
//!
//! These cover the failure points that unit tests can't reach: command/response
//! correlation under concurrency, event fan-out, disconnect flushing, automatic
//! reconnect, malformed-input resilience, and response timeouts. Event and
//! device-state payloads are drawn from the real captured corpus.

mod common;

use std::net::Ipv4Addr;
use std::time::Duration;

use common::{FakeSeestar, device_state_response, load_corpus};
use scopinator_seestar::command::Command;
use scopinator_seestar::error::SeestarError;
use scopinator_seestar::event::SeestarEvent;
use scopinator_seestar::protocol::json_rpc;
use scopinator_seestar::{SeestarClient, SeestarConfig};
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
