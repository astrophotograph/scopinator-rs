//! Get the scope OFF the parked pole (Dec ±90, where RA is singular) so the
//! angle-mapping probe can run. Mirrors seestar_alp's flow:
//!   1. enter star mode (`iscope_start_view {mode:star}`) — unparks/activates.
//!   2. if still poleward, goto a near-zenith target via
//!      `iscope_start_view {mode:star, target_ra_dec:[RA_h, Dec_deg], ...}`
//!      (seestar_alp's actual "goto" — a bare goto_target is what the firmware
//!      rejects as invalid).
//!
//! The target is the meridian at the observer's latitude (≈ zenith), computed
//! from the device's location + system UTC, so it's above the horizon. Absolute
//! pointing accuracy is irrelevant here — we only need a moderate *reported*
//! Dec so RA/Dec jog deltas become measurable.
//!
//! ⚠️ THIS MOVES THE TELESCOPE. Run: cargo run -p scopinator-seestar --example probe_goto -- <host>

use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use scopinator_seestar::SeestarClient;
use scopinator_seestar::command::Command;
use scopinator_seestar::command::params::{StartViewParams, ViewMode};

type Err = Box<dyn std::error::Error>;

fn resolve_v4(host: &str) -> Result<Ipv4Addr, Err> {
    for sa in (host, 4700u16).to_socket_addrs()? {
        if let IpAddr::V4(v4) = sa.ip() {
            return Ok(v4);
        }
    }
    Err("no IPv4".into())
}

async fn read_dec(client: &SeestarClient) -> Result<(f64, f64), Err> {
    let r = client.send_and_validate(Command::ScopeGetEquCoord).await?;
    Ok((
        r["ra"].as_f64().unwrap_or(f64::NAN),
        r["dec"].as_f64().unwrap_or(f64::NAN),
    ))
}

/// Local sidereal time (hours) from system UTC + east longitude (deg).
fn lst_hours(lon_deg: f64) -> f64 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let jd = secs / 86400.0 + 2440587.5;
    let d = jd - 2451545.0;
    let gmst = (280.46061837 + 360.98564736629 * d).rem_euclid(360.0);
    ((gmst + lon_deg).rem_euclid(360.0)) / 15.0
}

async fn enter_star_view(client: &SeestarClient, target: Option<(f64, f64)>) -> Result<(), Err> {
    let params = StartViewParams {
        mode: Some(ViewMode::Star),
        target_name: target.map(|_| "probe_zenith".to_string()),
        target_ra_dec: target,
        target_type: None,
        lp_filter: target.map(|_| false),
    };
    match client.send_command(Command::IscopeStartView(params)).await {
        Ok(r) => {
            println!("  iscope_start_view -> code={}", r.code);
            Ok(())
        }
        Err(e) => {
            println!("  iscope_start_view -> error: {e}");
            Err(e.into())
        }
    }
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

    // Read location (lon, lat) for the zenith target; coords for the start state.
    let ds = client.send_and_validate(Command::GetDeviceState).await?;
    let loc = ds.get("location_lon_lat").and_then(|v| v.as_array());
    let (lon, lat) = match loc {
        Some(a) if a.len() == 2 => (a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0)),
        _ => (0.0, 0.0),
    };
    println!(
        "mount equ_mode={}  (lat/lon read for zenith target; not printed)",
        ds["mount"]
            .get("equ_mode")
            .unwrap_or(&serde_json::Value::Null)
    );

    let (ra0, dec0) = read_dec(&client).await?;
    println!("start: RA={ra0:.4}h Dec={dec0:.4}°");

    // Stage 1: enter star mode (unpark/activate).
    println!("\nStage 1: entering star mode...");
    enter_star_view(&client, None).await.ok();
    tokio::time::sleep(Duration::from_secs(6)).await;
    let (ra1, dec1) = read_dec(&client).await?;
    println!("  after star mode: RA={ra1:.4}h Dec={dec1:.4}°");

    if dec1.abs() < 85.0 {
        println!(
            "\n✅ Off the pole (Dec={dec1:.2}°). Ready for the angle-mapping probe \
             (probe_move ... arm)."
        );
        client.shutdown().await;
        return Ok(());
    }

    // Stage 2: goto a near-zenith target so reported Dec becomes moderate.
    let (t_ra, t_dec) = if lon != 0.0 || lat != 0.0 {
        (lst_hours(lon), lat.clamp(-75.0, 75.0))
    } else {
        // No location — fall back to a moderate Dec at the current RA.
        println!("  (no device location; using fallback target)");
        (if ra1.is_finite() { ra1 } else { 0.0 }, 45.0)
    };
    println!(
        "\nStage 2: still at the pole — goto near-zenith target RA={t_ra:.4}h Dec={t_dec:.4}°..."
    );
    enter_star_view(&client, Some((t_ra, t_dec))).await.ok();

    // Slews can take a while; poll Dec until it leaves the pole or we time out.
    let mut moved = false;
    for i in 0..20 {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let (ra, dec) = read_dec(&client).await?;
        println!("  t+{:>2}s: RA={ra:.4}h Dec={dec:.4}°", (i + 1) * 3);
        if dec.abs() < 85.0 {
            moved = true;
            break;
        }
    }

    let (_raf, decf) = read_dec(&client).await?;
    if moved {
        println!("\n✅ Off the pole (Dec={decf:.2}°). Ready for probe_move ... arm.");
    } else {
        println!(
            "\n⚠️ Still poleward (Dec={decf:.2}°). The goto may need polar alignment \
             first, or the firmware rejected the target — check the codes above."
        );
    }
    client.shutdown().await;
    Ok(())
}
