use std::net::Ipv4Addr;
use std::time::Duration;

use anyhow::{Context, Result};
use scopinator_seestar::command::Command;
use scopinator_seestar::command::params::SetTimeParams;
use scopinator_seestar::{InteropKey, SeestarClient, SeestarConfig};
use scopinator_types::Coordinates;

pub async fn goto(
    host: Ipv4Addr,
    ra_hours: f64,
    dec_deg: f64,
    name: &str,
    interop_key: Option<InteropKey>,
) -> Result<()> {
    let coords = Coordinates::from_hours(ra_hours, dec_deg).context("invalid coordinates")?;

    println!("Connecting to {host}...");
    let client = SeestarClient::connect_with_config(
        host,
        SeestarConfig {
            interop_key,
            ..Default::default()
        },
    )
    .await?;
    client
        .wait_for_connection(Duration::from_secs(10))
        .await
        .context("could not connect to the telescope")?;

    warn_if_clock_is_off(&client).await;

    println!("Slewing to {name} ({coords})...");

    let response = client
        .goto(name, coords.ra.as_hours(), coords.dec.as_degrees())
        .await?;
    if response.is_success() {
        println!("Goto command accepted. Telescope is slewing.");
    } else {
        println!(
            "Goto failed: {} (code {})",
            response.error.unwrap_or_default(),
            response.code
        );
    }

    client.shutdown().await;
    Ok(())
}

/// Warn when the telescope's clock is far from this computer's.
///
/// The telescope works out where a target is in the sky from its own clock.
/// With a wrong clock it can accept a goto and then not slew at all, with no
/// error: a Seestar S30 on firmware 9.75 whose clock read 2020 entered star
/// view and stayed parked. The official app sets the clock on every connect;
/// `SeestarClient::startup` does the same.
async fn warn_if_clock_is_off(client: &SeestarClient) {
    let Ok(time) = client.send_and_validate(Command::PiGetTime).await else {
        return;
    };
    let field = |name: &str| time.get(name).and_then(|v| v.as_i64());
    let (Some(year), Some(mon), Some(day)) = (field("year"), field("mon"), field("day")) else {
        return;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let ours = SetTimeParams::from_unix_utc(now);
    if dates_far_apart(
        (year, mon, day),
        (ours.year.into(), ours.mon.into(), ours.day.into()),
    ) {
        eprintln!(
            "warning: the telescope's clock reads {year}-{mon:02}-{day:02}, this computer's \
             {}-{:02}-{:02}. With a wrong clock the telescope may accept the goto and not move. \
             Set its clock first (the Seestar app does this when it connects).",
            ours.year, ours.mon, ours.day
        );
    }
}

/// True if two calendar dates (year, month, day) are clearly different days,
/// by a coarse count that treats every month as 31 days. A time zone shifts
/// the date by one at most (which this count can see as up to four at the end
/// of February), and an unset clock is years off.
fn dates_far_apart(a: (i64, i64, i64), b: (i64, i64, i64)) -> bool {
    let ordinal = |(y, m, d): (i64, i64, i64)| y * 372 + m * 31 + d;
    (ordinal(a) - ordinal(b)).abs() > 4
}

#[cfg(test)]
mod tests {
    use super::dates_far_apart;

    #[test]
    fn unset_clock_is_far_and_time_zone_shift_is_not() {
        // The S30's unset clock against the real date.
        assert!(dates_far_apart((2020, 10, 10), (2026, 11, 15)));
        // Local evening vs UTC next day, including across a month and a year.
        assert!(!dates_far_apart((2026, 11, 14), (2026, 11, 15)));
        assert!(!dates_far_apart((2026, 11, 30), (2026, 12, 1)));
        assert!(!dates_far_apart((2026, 12, 31), (2027, 1, 1)));
        assert!(!dates_far_apart((2026, 2, 28), (2026, 3, 1)));
        assert!(dates_far_apart((2026, 11, 10), (2026, 11, 15)));
    }
}
