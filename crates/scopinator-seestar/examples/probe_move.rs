//! Empirically map `scope_speed_move` ANGLE -> sky direction (RA/Dec) in the
//! telescope's current mount mode (e.g. EQ). For each test angle it jogs the
//! scope briefly and measures the change in `scope_get_equ_coord`, classifying
//! the motion as North/South (Dec) and East/West (RA increases eastward).
//!
//! ⚠️ THIS MOVES THE TELESCOPE. Safe by default: without the `arm` argument it
//! only READS the current position and exits. You must pass `arm` to move.
//!
//!   Read-only (no motion):  cargo run -p scopinator-seestar --example probe_move -- <host>
//!   Armed (MOVES SCOPE):    cargo run -p scopinator-seestar --example probe_move -- <host> arm [level] [percent] [dur_sec]
//!
//! Defaults when armed: level=2, percent=60, dur_sec=2 — gentle, short jogs.
//! The four test angles (0/90/180/270) with equal magnitude roughly cancel, so
//! net displacement is small. A sidereal-drift baseline is measured first and
//! subtracted, so the reported deltas reflect the jog, not Earth rotation.
//!
//! NOTE: scopinator models the *new* scope_speed_move param form
//! ({angle,level,dur_sec,percent}). If the scope shows no motion, this firmware
//! may want the *old* form ({speed,angle,dur_sec}) — the probe flags that case.

use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};
use std::time::Duration;

use scopinator_seestar::SeestarClient;
use scopinator_seestar::command::Command;
use scopinator_seestar::command::params::SpeedMoveParams;

type Err = Box<dyn std::error::Error>;

const ANGLES: &[i32] = &[0, 90, 180, 270];

fn resolve_v4(host: &str) -> Result<Ipv4Addr, Err> {
    for sa in (host, 4700u16).to_socket_addrs()? {
        if let IpAddr::V4(v4) = sa.ip() {
            return Ok(v4);
        }
    }
    Err("no IPv4".into())
}

/// Read (ra_hours, dec_degrees) from scope_get_equ_coord.
async fn read_radec(client: &SeestarClient) -> Result<(f64, f64), Err> {
    let r = client.send_and_validate(Command::ScopeGetEquCoord).await?;
    let ra = r["ra"].as_f64().ok_or("no ra in scope_get_equ_coord")?;
    let dec = r["dec"].as_f64().ok_or("no dec in scope_get_equ_coord")?;
    Ok((ra, dec))
}

/// Shortest signed RA difference in hours (handles 24h wrap), then to degrees.
fn dra_degrees(after: f64, before: f64) -> f64 {
    let mut d = after - before;
    if d > 12.0 {
        d -= 24.0;
    } else if d < -12.0 {
        d += 24.0;
    }
    d * 15.0 // hours -> degrees
}

#[tokio::main]
async fn main() -> Result<(), Err> {
    let mut args = std::env::args().skip(1);
    let host = args.next().unwrap_or_else(|| "electra.m.bcc.sh".into());
    let armed = args.next().as_deref() == Some("arm");
    let level: i32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(2);
    let percent: i32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(60);
    let dur_sec: i32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(2);

    let ip = resolve_v4(&host)?;
    println!("connecting to {host} -> {ip}");
    let client = SeestarClient::connect(ip).await?;
    client.wait_for_connection(Duration::from_secs(5)).await?;

    // Report mount mode / tracking so results are interpretable.
    if let Ok(ds) = client.send_and_validate(Command::GetDeviceState).await {
        let mount = &ds["mount"];
        println!(
            "mount: equ_mode={}  tracking={}  (tracking ON gives cleaner deltas)",
            mount.get("equ_mode").unwrap_or(&serde_json::Value::Null),
            mount.get("tracking").unwrap_or(&serde_json::Value::Null),
        );
    }

    let (ra0, dec0) = read_radec(&client).await?;
    println!("start position: RA={ra0:.5}h  Dec={dec0:.5}°");

    if !armed {
        println!(
            "\n[read-only] Not moving. Re-run with `arm` to jog the scope and map angles:\n  \
             cargo run -p scopinator-seestar --example probe_move -- {host} arm [level] [percent] [dur_sec]"
        );
        client.shutdown().await;
        return Ok(());
    }

    // Near the poles RA is singular — E/W (RA) deltas are meaningless there.
    if dec0.abs() > 85.0 {
        println!(
            "\nABORT: scope is at Dec={dec0:.2}° (near the pole, likely parked). RA is \
             degenerate here, so E/W mapping would be garbage. Slew to a target at a \
             moderate Dec (ideally near the equator) with tracking ON, then re-run."
        );
        client.shutdown().await;
        return Ok(());
    }

    println!(
        "\n⚠️  ARMED — jogging at angles {ANGLES:?} (level={level}, percent={percent}, dur_sec={dur_sec}).\n"
    );
    let settle = Duration::from_secs(1);
    let window = Duration::from_secs(dur_sec as u64) + settle;

    // Sidereal-drift baseline: how much RA/Dec move on their own over `window`.
    let (bra0, bdec0) = read_radec(&client).await?;
    tokio::time::sleep(window).await;
    let (bra1, bdec1) = read_radec(&client).await?;
    let drift_ra = dra_degrees(bra1, bra0);
    let drift_dec = bdec1 - bdec0;
    println!(
        "drift baseline over {}s: ΔRA={:+.4}°  ΔDec={:+.4}° (subtracted below)\n",
        window.as_secs(),
        drift_ra,
        drift_dec
    );

    let stop = |angle: i32| {
        Command::ScopeSpeedMove(SpeedMoveParams {
            angle,
            level: 0,
            dur_sec: 1,
            percent: 0, // percent 0 = stop
        })
    };

    let mut results: Vec<(i32, f64, f64)> = Vec::new();
    for &angle in ANGLES {
        let (before_ra, before_dec) = read_radec(&client).await?;
        client
            .send_and_validate(Command::ScopeSpeedMove(SpeedMoveParams {
                angle,
                level,
                dur_sec,
                percent,
            }))
            .await?;
        tokio::time::sleep(Duration::from_secs(dur_sec as u64)).await;
        let _ = client.send_and_validate(stop(angle)).await; // explicit halt
        tokio::time::sleep(settle).await;
        let (after_ra, after_dec) = read_radec(&client).await?;

        // Jog motion = raw delta minus the sidereal drift over the same window.
        let dra = dra_degrees(after_ra, before_ra) - drift_ra;
        let ddec = (after_dec - before_dec) - drift_dec;
        results.push((angle, dra, ddec));

        let ns = if ddec > 0.0 { "North" } else { "South" };
        let ew = if dra > 0.0 { "East" } else { "West" };
        let dominant = if dra.abs() >= ddec.abs() { ew } else { ns };
        println!(
            "angle {angle:3}°: ΔRA={:+.4}° ({ew})  ΔDec={:+.4}° ({ns})  -> dominant: {dominant}",
            dra, ddec
        );
    }

    println!("\n== mapping summary (current mount mode) ==");
    let moved = results
        .iter()
        .any(|(_, dra, ddec)| dra.abs() > 0.01 || ddec.abs() > 0.01);
    if !moved {
        println!(
            "NO MOTION detected at any angle. This firmware may want the OLD \
             scope_speed_move form ({{speed,angle,dur_sec}}) rather than the new \
             ({{level,angle,dur_sec,percent}}) form scopinator sends — or level/percent \
             were too low. Try larger `percent`, or add old-form support to the command."
        );
    } else {
        for (angle, dra, ddec) in &results {
            let dir = if dra.abs() >= ddec.abs() {
                if *dra > 0.0 {
                    "EAST (RA+)"
                } else {
                    "WEST (RA-)"
                }
            } else if *ddec > 0.0 {
                "NORTH (Dec+)"
            } else {
                "SOUTH (Dec-)"
            };
            println!("  angle {angle:3}° => {dir}");
        }
    }

    let (raf, decf) = read_radec(&client).await?;
    println!("\nfinal position: RA={raf:.5}h Dec={decf:.5}° (started RA={ra0:.5}h Dec={dec0:.5}°)");
    client.shutdown().await;
    Ok(())
}
