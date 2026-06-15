//! Live probe against a running Seestar (or seestar-proxy): connect, check
//! status, and collect imaging frames. Secret-ish fields in the device state
//! are redacted before printing.
//!
//! Run: `cargo run -p scopinator-seestar --example probe_proxy -- <host> [secs]`
//! Defaults: host=electra.m.bcc.sh, secs=12

use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};
use std::time::Duration;

use scopinator_seestar::SeestarClient;
use scopinator_seestar::command::Command;
use serde_json::Value;

type Err = Box<dyn std::error::Error>;

fn resolve_v4(host: &str) -> Result<Ipv4Addr, Err> {
    for sa in (host, 4700u16).to_socket_addrs()? {
        if let IpAddr::V4(v4) = sa.ip() {
            return Ok(v4);
        }
    }
    Err(format!("no IPv4 address for {host}").into())
}

/// Recursively blank out secret-ish keys so we never print/save real secrets.
fn redact(v: &mut Value) {
    const SECRET: &[&str] = &[
        "sn",
        "ssid",
        "passwd",
        "password",
        "ip",
        "gateway",
        "netmask",
        "bssid",
        "mac",
        "cpuId",
        "cpu_id",
        "cli_name",
        "location_lon_lat",
        "user_name",
        "host",
    ];
    match v {
        Value::Object(m) => {
            for (k, val) in m.iter_mut() {
                if SECRET.contains(&k.as_str()) {
                    *val = Value::String("[redacted]".into());
                } else {
                    redact(val);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(redact),
        _ => {}
    }
}

#[tokio::main]
async fn main() -> Result<(), Err> {
    let mut args = std::env::args().skip(1);
    let host = args.next().unwrap_or_else(|| "electra.m.bcc.sh".into());
    let secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(12);

    let ip = resolve_v4(&host)?;
    println!("== resolve ==\n{host} -> {ip}  (control :4700, imaging :4800)\n");

    // 1) Connect.
    println!("== connect ==");
    let client = SeestarClient::connect(ip).await?;
    client.wait_for_connection(Duration::from_secs(5)).await?;
    println!(
        "control_connected={}  imaging_connected={}\n",
        client.is_control_connected(),
        client.is_imaging_connected()
    );

    // Subscribe to frames/events BEFORE issuing commands so nothing is missed.
    let mut frames = client.subscribe_frames();
    let mut events = client.subscribe_events();

    // 2) Status: device state (firmware/model) + view state (stacking/tracking).
    println!("== status ==");
    let ds = client.send_command(Command::GetDeviceState).await?;
    println!("get_device_state -> code={}", ds.code);
    if let Some(mut result) = ds.result.clone() {
        redact(&mut result);
        // Pull a couple of headline fields if present.
        let dev = result.get("device");
        if let Some(dev) = dev {
            println!(
                "  firmware_ver_int={}  product_model={}",
                dev.get("firmware_ver_int").unwrap_or(&Value::Null),
                dev.get("product_model").unwrap_or(&Value::Null),
            );
        }
        println!("  (redacted device state) {}", result);
    }

    let vs = client.send_command(Command::GetViewState).await?;
    println!("get_view_state   -> code={}", vs.code);
    if let Some(result) = &vs.result {
        println!("  {}\n", result);
    } else {
        println!();
    }

    // 3) Imaging frames: collect for `secs` seconds.
    println!("== imaging frames (collecting for {secs}s) ==");
    let mut n: u64 = 0;
    let mut bytes: u64 = 0;
    let mut evt_n: u64 = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        tokio::select! {
            r = tokio::time::timeout_at(deadline, frames.recv()) => match r {
                Ok(Ok(f)) => {
                    n += 1;
                    bytes += f.data.len() as u64;
                    if n <= 6 {
                        println!(
                            "  frame #{n}: kind={:?} hdr.size={} {}x{} id={} code={} payload={}B",
                            f.kind, f.header.size, f.header.width, f.header.height,
                            f.header.id, f.header.code, f.data.len()
                        );
                    }
                }
                Ok(Err(e)) => { println!("  frame channel closed: {e}"); break; }
                Err(_) => break, // deadline reached
            },
            r = tokio::time::timeout_at(deadline, events.recv()) => match r {
                Ok(Ok(_)) => { evt_n += 1; }
                Ok(Err(_)) => {}
                Err(_) => break,
            },
        }
    }

    let rate = n as f64 / secs as f64;
    println!(
        "\n== summary ==\nframes={n}  ({rate:.2}/s, {bytes} bytes total)  control_events={evt_n}"
    );
    println!(
        "imaging_connected={}  control_connected={}",
        client.is_imaging_connected(),
        client.is_control_connected()
    );

    client.shutdown().await;
    Ok(())
}
