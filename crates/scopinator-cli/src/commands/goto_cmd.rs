use std::net::Ipv4Addr;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{Datelike, Local, Timelike};
use scopinator_seestar::command::Command;
use scopinator_seestar::command::params::SetTimeParams;
use scopinator_seestar::event::{EventState, SeestarEvent};
use scopinator_seestar::{InteropKey, SeestarClient, SeestarConfig};
use scopinator_types::Coordinates;
use serde_json::Value;

/// When `goto` sets the telescope's clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockPolicy {
    /// Only when it is clearly wrong (the default).
    IfWrong,
    /// Every time.
    Always,
    /// Never; warn when it is wrong.
    Never,
}

/// How long to wait for the mount to start moving, or to report why not.
const OUTCOME_TIMEOUT: Duration = Duration::from_secs(12);

pub async fn goto(
    host: Ipv4Addr,
    ra_hours: f64,
    dec_deg: f64,
    name: &str,
    interop_key: Option<InteropKey>,
    clock: ClockPolicy,
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

    let result = run(&client, name, coords, clock).await;
    client.shutdown().await;
    result
}

async fn run(
    client: &SeestarClient,
    name: &str,
    coords: Coordinates,
    clock: ClockPolicy,
) -> Result<()> {
    prepare_clock(client, clock).await?;

    println!("Slewing to {name} ({coords})...");
    let mut events = client.subscribe_events();
    let response = client
        .goto(name, coords.ra.as_hours(), coords.dec.as_degrees())
        .await?;
    if !response.is_success() {
        bail!(
            "the telescope refused the goto: {} (code {})",
            response.error.unwrap_or_default(),
            response.code
        );
    }

    // A code 0 reply only means the request was accepted. Whether the mount
    // moves is reported afterwards, in events and in the view state.
    let deadline = tokio::time::Instant::now() + OUTCOME_TIMEOUT;
    let mut poll = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                bail!(
                    "the telescope accepted the goto but the mount has not started moving \
                     after {} s. Check its clock and location, and that the target is above \
                     the horizon.",
                    OUTCOME_TIMEOUT.as_secs()
                );
            }
            event = events.recv() => {
                if let Ok(event) = event
                    && let Some(outcome) = outcome_from_event(&event)
                {
                    return outcome.report();
                }
            }
            _ = poll.tick() => {
                if let Some(outcome) = outcome_from_state(client).await {
                    return outcome.report();
                }
            }
        }
    }
}

/// What became of an accepted goto.
#[derive(Debug, PartialEq)]
enum Outcome {
    Moving,
    Failed {
        stage: String,
        code: i64,
        error: String,
    },
}

impl Outcome {
    fn report(self) -> Result<()> {
        match self {
            Outcome::Moving => {
                println!("The mount is slewing.");
                Ok(())
            }
            Outcome::Failed { stage, code, error } => {
                bail!("the goto failed at {stage}: {error} (code {code})")
            }
        }
    }
}

fn outcome_from_event(event: &SeestarEvent) -> Option<Outcome> {
    let failed = |stage: &str, code: Option<i64>, error: Option<&str>| Outcome::Failed {
        stage: stage.to_string(),
        code: code.unwrap_or(-1),
        error: error.unwrap_or("no reason given").to_string(),
    };
    match event {
        SeestarEvent::ScopeGoto(data) => match data.state {
            Some(EventState::Working) => Some(Outcome::Moving),
            Some(EventState::Fail) => Some(failed(
                "ScopeGoto",
                data.code.map(i64::from),
                data.error.as_deref(),
            )),
            _ => None,
        },
        SeestarEvent::AutoGoto(data) if data.state == Some(EventState::Fail) => Some(failed(
            "AutoGoto",
            data.code.map(i64::from),
            data.error.as_deref(),
        )),
        SeestarEvent::AutoGotoStep(value) => {
            (value.get("state").and_then(Value::as_str) == Some("fail")).then(|| {
                failed(
                    "AutoGotoStep",
                    value.get("code").and_then(Value::as_i64),
                    value.get("error").and_then(Value::as_str),
                )
            })
        }
        _ => None,
    }
}

/// Ask the device: is the mount slewing, or does the view state hold a
/// failed stage?
async fn outcome_from_state(client: &SeestarClient) -> Option<Outcome> {
    if let Ok(view) = client.send_and_validate(Command::GetViewState).await
        && let Some(failure) = find_failure(&view, "View")
    {
        return Some(failure);
    }
    let state = client
        .send_raw_and_validate(
            "get_device_state",
            Some(serde_json::json!({"keys": ["mount"]})),
        )
        .await
        .ok()?;
    (state.pointer("/mount/move_type").and_then(Value::as_str) == Some("ScopeGoto"))
        .then_some(Outcome::Moving)
}

/// The first stage in a view state that reports `state: fail`, by depth-first
/// search. Stages nest under their own names (`View` > `AutoGoto` >
/// `ScopeGoto`), and a failed inner stage can sit under an outer one that
/// still says `working`.
fn find_failure(value: &Value, name: &str) -> Option<Outcome> {
    let object = value.as_object()?;
    if object.get("state").and_then(Value::as_str) == Some("fail") {
        return Some(Outcome::Failed {
            stage: name.to_string(),
            code: object.get("code").and_then(Value::as_i64).unwrap_or(-1),
            error: object
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("no reason given")
                .to_string(),
        });
    }
    object
        .iter()
        .find_map(|(key, child)| find_failure(child, key))
}

/// Check the telescope's clock and set it according to the policy.
///
/// The telescope works out where a target is in the sky from its own clock.
/// With a wrong clock a goto is accepted and the mount does not move: a
/// Seestar S30 on firmware 9.75 loses its clock when powered off and comes
/// back reading 2020. The official app sets the clock on every connect.
async fn prepare_clock(client: &SeestarClient, policy: ClockPolicy) -> Result<()> {
    let now = local_time();
    let wrong = match telescope_date(client).await {
        Some(theirs) => dates_far_apart(theirs, (now.year.into(), now.mon.into(), now.day.into()))
            .then_some(theirs),
        None => None,
    };
    let set = match policy {
        ClockPolicy::Always => true,
        ClockPolicy::IfWrong => wrong.is_some(),
        ClockPolicy::Never => false,
    };
    if let Some((year, mon, day)) = wrong {
        let action = if set {
            "Setting it"
        } else {
            "With a wrong clock the telescope may accept the goto and not move. Not setting it \
             (--no-sync-clock)"
        };
        eprintln!(
            "warning: the telescope's clock reads {year}-{mon:02}-{day:02}, this computer's \
             {}-{:02}-{:02}. {action}.",
            now.year, now.mon, now.day
        );
    }
    if set {
        let zone = now.time_zone.clone();
        client
            .send_and_validate(Command::PiSetTime(now))
            .await
            .context("could not set the telescope's clock")?;
        println!("Telescope clock set from this computer ({zone}).");
    }
    Ok(())
}

/// This computer's local time and time zone, for `pi_set_time`. The zone name
/// replaces the telescope's, so the local one is sent rather than UTC; it
/// falls back to UTC when the system zone cannot be read.
fn local_time() -> SetTimeParams {
    match iana_time_zone::get_timezone() {
        Ok(zone) => {
            let now = Local::now();
            SetTimeParams {
                year: now.year(),
                mon: now.month() as i32,
                day: now.day() as i32,
                hour: now.hour() as i32,
                min: now.minute() as i32,
                sec: now.second() as i32,
                time_zone: zone,
            }
        }
        Err(_) => SetTimeParams::from_unix_utc(chrono::Utc::now().timestamp()),
    }
}

/// The telescope's calendar date (year, month, day), if it reports one.
async fn telescope_date(client: &SeestarClient) -> Option<(i64, i64, i64)> {
    let time = client.send_and_validate(Command::PiGetTime).await.ok()?;
    let field = |name: &str| time.get(name).and_then(|v| v.as_i64());
    Some((field("year")?, field("mon")?, field("day")?))
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
    use super::*;
    use serde_json::json;

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

    // The nesting a Seestar S30 (firmware 9.75) reported for a goto with its
    // clock unset: the inner stage failed while the outer ones said working.
    #[test]
    fn a_failed_inner_stage_is_found_under_working_ones() {
        let view = json!({"View": {"state": "working", "mode": "star", "stage": "AutoGoto",
            "AutoGoto": {"state": "working", "stage": "ScopeGoto",
                "ScopeGoto": {"state": "fail", "code": 501, "error": "mount goto failed"}}}});
        assert_eq!(
            find_failure(&view, "View"),
            Some(Outcome::Failed {
                stage: "ScopeGoto".into(),
                code: 501,
                error: "mount goto failed".into(),
            })
        );
    }

    #[test]
    fn a_working_view_has_no_failure() {
        let view = json!({"View": {"state": "working", "stage": "AutoGoto",
            "AutoGoto": {"state": "working"}}});
        assert_eq!(find_failure(&view, "View"), None);
    }

    #[test]
    fn goto_events_give_the_outcome() {
        let event = |v: Value| serde_json::from_value::<SeestarEvent>(v).unwrap();
        assert_eq!(
            outcome_from_event(&event(json!({"Event": "ScopeGoto", "state": "working",
                "cur_ra_dec": [1.0, 2.0], "dist_deg": 30.0}))),
            Some(Outcome::Moving)
        );
        assert_eq!(
            outcome_from_event(&event(json!({"Event": "AutoGotoStep", "state": "fail",
                "code": 501, "error": "mount goto failed"}))),
            Some(Outcome::Failed {
                stage: "AutoGotoStep".into(),
                code: 501,
                error: "mount goto failed".into(),
            })
        );
        assert_eq!(
            outcome_from_event(&event(json!({"Event": "AutoGoto", "state": "start"}))),
            None
        );
    }
}
