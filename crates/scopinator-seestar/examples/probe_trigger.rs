//! Empirically confirm what triggers real preview frames over the imaging port.
//!
//! Assumes the scope is already in an active view (iscope_start_view done by
//! someone else). Phase A: baseline (no trigger). Phase B: `begin_streaming` on
//! the imaging port (4800) via [`SeestarClient::begin_streaming`]. Reports a
//! per-phase frame id histogram so we can see whether real
//! PREVIEW(21)/VIEW(20)/STACK(23) frames appear vs only heartbeat echoes.
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
use scopinator_seestar::command::Command;
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

fn report(label: &str, hist: &BTreeMap<u8, u64>, max_payload: usize) {
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

    println!("\nPhase B — begin_streaming on IMAGING port (4800), 10s:");
    client.begin_streaming().await?;
    let (h, mp) = collect(&mut frames, 10).await;
    report("after 4800 begin_streaming", &h, mp);

    client.shutdown().await;
    Ok(())
}
