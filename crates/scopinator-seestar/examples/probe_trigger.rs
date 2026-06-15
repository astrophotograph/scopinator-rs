//! Empirically confirm what starts and stops real preview frames over the
//! imaging port.
//!
//! Assumes the scope is already in an active view (iscope_start_view done by
//! someone else). Phase A: baseline (no trigger). Phase B: `begin_streaming` on
//! the imaging port (4800) via [`SeestarClient::begin_streaming`]. Phase C:
//! `stop_streaming` via `send_imaging(ImagingCommand::StopStreaming)`. Reports a
//! per-phase frame id histogram so we can see whether real
//! PREVIEW(21)/VIEW(20)/STACK(23) frames appear/disappear vs only heartbeat
//! echoes, and prints a verdict on whether stop_streaming actually stops frames.
//!
//! (The control-port (4700) `begin_streaming` path was verified to be rejected
//! with code 103, which is why `Command::BeginStreaming` no longer exists.)
//!
//! Run: `cargo run -p scopinator-seestar --example probe_trigger -- <host>`

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use scopinator_seestar::SeestarClient;
use scopinator_seestar::command::{Command, ImagingCommand};
use scopinator_seestar::connection::imaging::ImageFrame;

type Err = Box<dyn std::error::Error>;

fn resolve_v4(host: &str) -> Result<Ipv4Addr, Err> {
    for sa in (host, 4700u16).to_socket_addrs()? {
        if let IpAddr::V4(v4) = sa.ip() {
            return Ok(v4);
        }
    }
    Err("no IPv4".into())
}

/// Drain frames for `secs`, returning (id -> count) and the largest payload.
async fn collect(
    frames: &mut tokio::sync::broadcast::Receiver<Arc<ImageFrame>>,
    secs: u64,
) -> (BTreeMap<u8, u64>, usize) {
    let mut hist: BTreeMap<u8, u64> = BTreeMap::new();
    let mut max_payload = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        match tokio::time::timeout_at(deadline, frames.recv()).await {
            Ok(Ok(f)) => {
                *hist.entry(f.header.id).or_default() += 1;
                max_payload = max_payload.max(f.data.len());
            }
            Ok(Err(_)) => continue, // lagged
            Err(_) => break,        // deadline
        }
    }
    (hist, max_payload)
}

/// Print a per-phase summary and return the number of real image frames.
fn report(label: &str, hist: &BTreeMap<u8, u64>, max_payload: usize) -> u64 {
    let total: u64 = hist.values().sum();
    // id 20=VIEW, 21=PREVIEW, 23=STACK are real image frames; others are status.
    let images: u64 = hist
        .iter()
        .filter(|(id, _)| matches!(**id, 20 | 21 | 23))
        .map(|(_, c)| c)
        .sum();
    println!(
        "  [{label}] total={total} image_frames(id20/21/23)={images} max_payload={max_payload}B"
    );
    println!("    id histogram: {hist:?}");
    images
}

#[tokio::main]
async fn main() -> Result<(), Err> {
    let host = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "electra.m.bcc.sh".into());
    let ip = resolve_v4(&host)?;
    println!("connecting to {host} -> {ip}");
    let client = SeestarClient::connect(ip).await?;
    client.wait_for_connection(Duration::from_secs(5)).await?;

    // Confirm a view is active.
    let vs = client.send_command(Command::GetViewState).await?;
    let state = vs
        .result
        .as_ref()
        .and_then(|r| r.get("View"))
        .and_then(|v| v.get("state"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    println!("view state = {state}\n");

    let mut frames = client.subscribe_frames();

    println!("Phase A — baseline (no trigger), 7s:");
    let (h, mp) = collect(&mut frames, 7).await;
    report("baseline", &h, mp);

    println!("\nPhase B — begin_streaming on IMAGING port (4800), 12s:");
    client.begin_streaming().await?;
    let (h, mp) = collect(&mut frames, 12).await;
    let started = report("after 4800 begin_streaming", &h, mp);

    println!("\nPhase C — stop_streaming on IMAGING port (4800), 14s:");
    client.send_imaging(ImagingCommand::StopStreaming).await?;
    let (h, mp) = collect(&mut frames, 14).await;
    let after_stop = report("after 4800 stop_streaming", &h, mp);

    println!("\n== verdict ==");
    if started == 0 {
        println!(
            "INCONCLUSIVE: no image frames flowed in phase B, so there was no \
             stream to stop (is a star-mode view active?)."
        );
    } else if after_stop == 0 {
        println!(
            "stop_streaming WORKS: {started} image frames while streaming, 0 after \
             stop_streaming. The frame stream halted on the imaging port (4800)."
        );
    } else {
        println!(
            "stop_streaming did NOT halt frames: {started} before, {after_stop} after. \
             Either it's the wrong method/port, or another client re-started the stream."
        );
    }

    client.shutdown().await;
    Ok(())
}
