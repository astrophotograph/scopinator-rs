//! Run a short one-target view plan, then stop it, clear it and park.
//!
//! **This moves the telescope and replaces the plan the device holds.** It
//! refuses to run if the device already holds a plan, unless `--replace` is
//! given. The target starts at the device's current local time.
//!
//! ```text
//! cargo run -p scopinator-seestar --example probe_plan -- 192.168.x.x <ra_hours> <dec_deg> [--replace]
//! ```

use std::time::Duration;

use scopinator_seestar::command::Command;
use scopinator_seestar::command::params::{PlanCameraMode, ViewPlan, ViewPlanTarget};
use scopinator_seestar::event::SeestarEvent;
use scopinator_seestar::{InteropKey, SeestarClient, SeestarConfig};

type Err = Box<dyn std::error::Error>;

async fn show_plan(client: &SeestarClient, label: &str) -> Result<bool, Err> {
    match client.view_plan_state().await? {
        None => {
            println!("{label}: device holds no plan");
            Ok(false)
        }
        Some(state) => {
            let plan = state.plan.unwrap_or_default();
            let targets: Vec<String> = plan
                .list
                .iter()
                .map(|t| {
                    format!(
                        "{}:{}",
                        t.target_name.clone().unwrap_or_default(),
                        t.extra.get("state").and_then(|s| s.as_str()).unwrap_or("-")
                    )
                })
                .collect();
            println!(
                "{label}: plan {:?} state={:?} stage={:?} targets={targets:?}",
                plan.plan_name, state.state, state.stage
            );
            Ok(true)
        }
    }
}

async fn watch(events: &mut tokio::sync::broadcast::Receiver<SeestarEvent>, secs: u64) {
    let end = tokio::time::Instant::now() + Duration::from_secs(secs);
    let mut last = String::new();
    while let Ok(Ok(event)) = tokio::time::timeout_at(end, events.recv()).await {
        let line = match &event {
            SeestarEvent::ViewPlan(d) => format!(
                "ViewPlan state={:?} stage={:?} code={:?}",
                d.state, d.stage, d.code
            ),
            SeestarEvent::Target(d) => format!(
                "Target id={:?} state={:?} stage={:?} code={:?} error={:?}",
                d.target_id, d.state, d.stage, d.code, d.error
            ),
            SeestarEvent::AutoGoto(d) => format!("AutoGoto state={:?}", d.state),
            SeestarEvent::ScopeGoto(d) => format!("ScopeGoto state={:?}", d.state),
            SeestarEvent::View(d) => format!("View state={:?} mode={:?}", d.state, d.mode),
            SeestarEvent::PiStatus(_) | SeestarEvent::BalanceSensor(_) => continue,
            other => other.name().to_string(),
        };
        if line != last {
            println!("    event: {line}");
            last = line;
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Err> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: probe_plan <host> <ra_hours> <dec_deg> [--replace]";
    let host = args.first().ok_or(usage)?;
    let ra: f64 = args.get(1).ok_or(usage)?.parse()?;
    let dec: f64 = args.get(2).ok_or(usage)?.parse()?;
    let replace = args.iter().any(|a| a == "--replace");

    let interop_key = match std::env::var("SEESTAR_INTEROP_PEM") {
        Ok(path) => Some(InteropKey::from_pem(&std::fs::read_to_string(path)?)?),
        Err(_) => None,
    };
    let client = SeestarClient::connect_with_config(
        host.parse()?,
        SeestarConfig {
            interop_key,
            ..Default::default()
        },
    )
    .await?;
    client.wait_for_connection(Duration::from_secs(10)).await?;
    println!("verification: {:?}", client.verification());
    let mut events = client.subscribe_events();

    if show_plan(&client, "before").await? && !replace {
        println!("refusing to replace the held plan; pass --replace to do so");
        client.shutdown().await;
        return Ok(());
    }

    let time = client.send_and_validate(Command::PiGetTime).await?;
    let now_min =
        (time["hour"].as_i64().unwrap_or(0) * 60 + time["min"].as_i64().unwrap_or(0)) as i32;
    println!(
        "device local time {}:{:02} -> start_min {now_min}",
        now_min / 60,
        now_min % 60
    );

    let plan = ViewPlan {
        plan_name: "scopinator probe".into(),
        plan_target_af: Some(false),
        viewplan_gohome: Some(false),
        list: vec![ViewPlanTarget {
            target_id: 1_700_000_001,
            target_name: Some("probe target".into()),
            alias_name: Some("probe".into()),
            target_ra_dec: Some((ra, dec)),
            start_min: now_min,
            duration_min: 5,
            camera_mode: Some(PlanCameraMode::Tele),
            lp_filter: Some(false),
            skip: Some(false),
            ..Default::default()
        }],
        ..Default::default()
    };
    println!("set_view_plan -> {:?}", client.set_view_plan(plan).await);
    watch(&mut events, 25).await;
    show_plan(&client, "running").await?;
    let mount = client.send_and_validate(Command::GetDeviceState).await?;
    println!("mount: {}", mount["mount"]);

    println!("stop_view_plan -> {:?}", client.stop_view_plan().await);
    watch(&mut events, 6).await;
    let held_after_stop = show_plan(&client, "after stop").await?;

    println!("clear_view_plan -> {:?}", client.clear_view_plan().await);
    watch(&mut events, 4).await;
    let held_after_clear = show_plan(&client, "after clear").await?;
    println!("plan held after stop: {held_after_stop}; after clear: {held_after_clear}");

    // Leave any view the plan started, then park.
    let r = client.send_command(Command::IscopeStopView(None)).await?;
    println!("iscope_stop_view -> code {}", r.code);
    tokio::time::sleep(Duration::from_secs(2)).await;
    let r = client.send_command(Command::ScopePark).await?;
    println!("scope_park -> code {}", r.code);
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let state = client.send_and_validate(Command::GetDeviceState).await?;
        let mount = &state["mount"];
        if mount["close"] == true && mount["move_type"] == "none" {
            println!("parked: {mount}");
            break;
        }
    }
    client.shutdown().await;
    Ok(())
}
