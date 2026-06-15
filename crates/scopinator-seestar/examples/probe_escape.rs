//! Jog the scope off the parked pole (Dec −90°) using open-loop
//! `scope_speed_move` (no polar alignment needed), then map each joystick
//! angle to a signed sky direction (N/S in Dec, E/W in RA).
//!
//! Phases:
//!   0. Verify motion: if the first jog produces no Dec AND no RA change, abort
//!      (fw likely wants the OLD {speed,angle,dur_sec} param form).
//!   1. Escape: find the angle whose jog raises Dec off −90° (the "north"/Dec
//!      axis), then repeat it until Dec clears the pole.
//!   2. Map: at the moderate Dec, jog each angle and classify ΔRA/ΔDec, with a
//!      sidereal-drift baseline subtracted.
//!
//! ⚠️ MOVES THE TELESCOPE. Open-loop jogs (app-style joystick), bounded by short
//! durations. Run:
//!   cargo run -p scopinator-seestar --example probe_escape -- <host> [level] [percent] [dur_sec] [target_dec]

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

async fn read_radec(client: &SeestarClient) -> Result<(f64, f64), Err> {
    let r = client.send_and_validate(Command::ScopeGetEquCoord).await?;
    Ok((
        r["ra"].as_f64().ok_or("no ra")?,
        r["dec"].as_f64().ok_or("no dec")?,
    ))
}

/// Shortest signed RA difference in degrees (handles 24h wrap).
fn dra_degrees(after: f64, before: f64) -> f64 {
    let mut d = after - before;
    if d > 12.0 {
        d -= 24.0;
    } else if d < -12.0 {
        d += 24.0;
    }
    d * 15.0
}

struct Jog {
    level: i32,
    percent: i32,
    dur_sec: i32,
}

impl Jog {
    async fn go(&self, client: &SeestarClient, angle: i32) -> Result<(), Err> {
        client
            .send_and_validate(Command::ScopeSpeedMove(SpeedMoveParams {
                angle,
                level: self.level,
                dur_sec: self.dur_sec,
                percent: self.percent,
            }))
            .await?;
        tokio::time::sleep(Duration::from_secs(self.dur_sec as u64)).await;
        // Explicit stop (percent 0), then settle.
        let _ = client
            .send_and_validate(Command::ScopeSpeedMove(SpeedMoveParams {
                angle,
                level: 0,
                dur_sec: 1,
                percent: 0,
            }))
            .await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Err> {
    let mut a = std::env::args().skip(1);
    let host = a.next().unwrap_or_else(|| "electra.m.bcc.sh".into());
    let level: i32 = a.next().and_then(|s| s.parse().ok()).unwrap_or(2);
    let percent: i32 = a.next().and_then(|s| s.parse().ok()).unwrap_or(60);
    let dur_sec: i32 = a.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    let target_dec: f64 = a.next().and_then(|s| s.parse().ok()).unwrap_or(-65.0);
    let jog = Jog {
        level,
        percent,
        dur_sec,
    };

    let ip = resolve_v4(&host)?;
    println!("connecting to {host} -> {ip}");
    let client = SeestarClient::connect(ip).await?;
    client.wait_for_connection(Duration::from_secs(5)).await?;

    let (ra0, dec0) = read_radec(&client).await?;
    println!(
        "start: RA={ra0:.4}h Dec={dec0:.4}°  (level={level} percent={percent} dur={dur_sec}s)\n"
    );

    // ---- Phase 1: find the Dec-up ("north") axis, abort if nothing moves. ----
    println!("Phase 1 — escaping the pole:");
    let mut any_motion = false;
    let mut north_axis: Option<i32> = None;
    for &angle in ANGLES {
        let (rb, db) = read_radec(&client).await?;
        jog.go(&client, angle).await?;
        let (ra, dec) = read_radec(&client).await?;
        let ddec = dec - db;
        let dra = dra_degrees(ra, rb);
        println!("  angle {angle:3}°: ΔDec={ddec:+.3}°  ΔRA={dra:+.3}°  (Dec now {dec:.3}°)");
        if ddec.abs() > 0.05 || dra.abs() > 0.30 {
            any_motion = true;
        }
        if ddec > 0.10 {
            north_axis = Some(angle);
            break;
        }
    }

    let north = match north_axis {
        Some(n) => n,
        None => {
            if any_motion {
                println!(
                    "\n⚠️ scope_speed_move WORKS (motion seen) but no cardinal angle raised Dec \
                     off the pole — the joystick angles may be offset from the RA/Dec axes; an \
                     intermediate-angle scan is needed."
                );
            } else {
                println!(
                    "\n❌ NO MOTION at any angle. fw 6.70 likely wants the OLD scope_speed_move \
                     form {{speed,angle,dur_sec}} rather than the new {{level,angle,dur_sec,percent}} \
                     scopinator sends. Need to add old-form support to the command."
                );
            }
            client.shutdown().await;
            return Ok(());
        }
    };
    println!("\n  -> angle {north}° raises Dec = NORTH axis. Climbing to Dec > {target_dec}°...");

    // ---- Climb off the pole ----
    for _ in 0..25 {
        let (_, dec) = read_radec(&client).await?;
        if dec > target_dec {
            break;
        }
        jog.go(&client, north).await?;
        let (_, dec2) = read_radec(&client).await?;
        println!("  jog {north}° -> Dec={dec2:.3}°");
    }
    let (_, dec_now) = read_radec(&client).await?;
    if dec_now <= target_dec {
        println!("  (stopped climbing at Dec={dec_now:.3}° after the pulse cap)");
    }
    println!("  off the pole at Dec={dec_now:.3}°\n");

    // ---- Phase 2: signed direction mapping at moderate Dec ----
    println!("Phase 2 — mapping (sidereal drift subtracted):");
    let settle_window = Duration::from_secs(dur_sec as u64 + 1);
    let (bra0, bdec0) = read_radec(&client).await?;
    tokio::time::sleep(settle_window).await;
    let (bra1, bdec1) = read_radec(&client).await?;
    let drift_ra = dra_degrees(bra1, bra0);
    let drift_dec = bdec1 - bdec0;
    println!(
        "  drift over {}s: ΔRA={drift_ra:+.3}° ΔDec={drift_dec:+.3}°\n",
        settle_window.as_secs()
    );

    let mut table: Vec<(i32, &str)> = Vec::new();
    for &angle in ANGLES {
        let (rb, db) = read_radec(&client).await?;
        jog.go(&client, angle).await?;
        let (ra, dec) = read_radec(&client).await?;
        let ddec = (dec - db) - drift_dec;
        // True E/W motion scales RA by cos(Dec).
        let dra = (dra_degrees(ra, rb) - drift_ra) * db.to_radians().cos();
        let dir = if dra.abs() >= ddec.abs() {
            if dra > 0.0 {
                "EAST (RA+)"
            } else {
                "WEST (RA-)"
            }
        } else if ddec > 0.0 {
            "NORTH (Dec+)"
        } else {
            "SOUTH (Dec-)"
        };
        println!("  angle {angle:3}°: ΔDec={ddec:+.3}°  ΔRA·cosDec={dra:+.3}°  => {dir}");
        table.push((angle, dir));
    }

    println!("\n== EQ-mode angle -> direction map ==");
    for (angle, dir) in &table {
        println!("  {angle:3}° => {dir}");
    }
    let (raf, decf) = read_radec(&client).await?;
    println!("\nfinal: RA={raf:.4}h Dec={decf:.4}°");
    client.shutdown().await;
    Ok(())
}
