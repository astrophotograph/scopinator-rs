//! The `scopinator` binary against a fake Alpaca server.

#[path = "../../scopinator/tests/alpaca_common/mod.rs"]
mod alpaca_common;

use std::process::Output;

use alpaca_common::{FakeAlpaca, full_telescope};
use serde_json::json;

/// A server with one mount, idle and unparked.
async fn mount_server() -> FakeAlpaca {
    let fake = FakeAlpaca::start().await;
    fake.set_devices(json!([
        {"DeviceName": "Mount", "DeviceType": "Telescope", "DeviceNumber": 0, "UniqueID": "t0"},
    ]));
    fake.set_all("telescope/0", full_telescope());
    fake
}

/// Run the binary with `args`, with `SCOPINATOR_BACKEND` unset.
async fn scopinator(args: &[&str]) -> Output {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_scopinator"))
        .args(args)
        .env_remove("SCOPINATOR_BACKEND")
        .env_remove("SEESTAR_INTEROP_PEM")
        .output()
        .await
        .expect("run scopinator")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn url(fake: &FakeAlpaca) -> String {
    format!("alpaca://{}", fake.addr)
}

#[tokio::test(flavor = "multi_thread")]
async fn status_describes_an_alpaca_mount() {
    let fake = mount_server().await;
    let output = scopinator(&["status", "--host", &url(&fake)]).await;
    let text = stdout(&output);
    assert!(output.status.success(), "{text}\n{}", stderr(&output));
    assert!(text.contains("(alpaca)"), "{text}");
    assert!(text.contains("Fake Mount"), "{text}");
    assert!(text.contains("State:    tracking"), "{text}");
    assert!(text.contains("Site:     41.8800, -87.6300"), "{text}");
    assert!(text.contains("Clock:    2026-11-15 17:18:54 UTC"), "{text}");
    // Looking does not disconnect the mount for other clients.
    let connected = fake.puts("telescope/0/connected");
    assert!(
        connected.iter().all(|put| put["Connected"] == "True"),
        "{connected:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_backend_option_and_port_work_like_the_url() {
    let fake = mount_server().await;
    let port = fake.addr.port().to_string();
    let output = scopinator(&[
        "--backend",
        "alpaca",
        "--port",
        &port,
        "status",
        "--host",
        "127.0.0.1",
    ])
    .await;
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("Fake Mount"));
}

#[tokio::test(flavor = "multi_thread")]
async fn goto_slews_waits_and_reports_arrival() {
    let fake = mount_server().await;
    // The mount "arrives" at once: it reports the target as its position and
    // never says it is slewing.
    fake.on_put("telescope/0/slewtocoordinatesasync", |values, form| {
        let number = |name: &str| form[name].parse::<f64>().unwrap();
        values.insert(
            "telescope/0/rightascension".into(),
            json!(number("RightAscension")),
        );
        values.insert(
            "telescope/0/declination".into(),
            json!(number("Declination")),
        );
    });
    let output = scopinator(&[
        "goto",
        "--host",
        &url(&fake),
        "--ra",
        "5.5",
        "--dec",
        "-5.4",
    ])
    .await;
    let text = stdout(&output);
    assert!(output.status.success(), "{text}\n{}", stderr(&output));
    assert!(text.contains("Slewing to"), "{text}");
    assert!(text.contains("Arrived at RA 5h 30m"), "{text}");

    let slew = &fake.puts("telescope/0/slewtocoordinatesasync")[0];
    assert_eq!(slew["RightAscension"].parse::<f64>().unwrap(), 5.5);
    assert_eq!(slew["Declination"].parse::<f64>().unwrap(), -5.4);
    // Already tracking, so tracking was not touched.
    assert!(fake.puts("telescope/0/tracking").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn goto_turns_tracking_on_and_jnow_marks_the_coordinates() {
    let fake = mount_server().await;
    fake.set("telescope/0/tracking", json!(false));
    fake.on_put("telescope/0/tracking", |values, _| {
        values.insert("telescope/0/tracking".into(), json!(true));
    });
    // The fake mount works in J2000, so coordinates given as the equinox of
    // date are precessed back: right ascension gets smaller.
    let output = scopinator(&[
        "goto",
        "--host",
        &url(&fake),
        "--ra",
        "6.0",
        "--dec",
        "20",
        "--jnow",
    ])
    .await;
    let text = stdout(&output);
    assert!(output.status.success(), "{text}\n{}", stderr(&output));
    assert!(text.contains("Tracking turned on."), "{text}");
    assert_eq!(fake.puts("telescope/0/tracking")[0]["Tracking"], "True");
    let sent = fake.puts("telescope/0/slewtocoordinatesasync")[0]["RightAscension"]
        .parse::<f64>()
        .unwrap();
    assert!(sent < 6.0 - 0.02, "{sent}");
}

#[tokio::test(flavor = "multi_thread")]
async fn goto_on_a_parked_mount_fails_and_says_to_unpark() {
    let fake = mount_server().await;
    fake.set("telescope/0/atpark", json!(true));
    let output = scopinator(&[
        "goto",
        "--host",
        &url(&fake),
        "--ra",
        "5.5",
        "--dec",
        "-5.4",
    ])
    .await;
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("scopinator unpark"),
        "{}",
        stderr(&output)
    );
    assert!(fake.puts("telescope/0/slewtocoordinatesasync").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn goto_gives_up_after_the_timeout_with_a_failing_exit_code() {
    let fake = mount_server().await;
    fake.on_put("telescope/0/slewtocoordinatesasync", |values, _| {
        values.insert("telescope/0/slewing".into(), json!(true));
    });
    let output = scopinator(&[
        "goto",
        "--host",
        &url(&fake),
        "--ra",
        "5.5",
        "--dec",
        "-5.4",
        "--timeout",
        "1",
    ])
    .await;
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(text.contains("did not finish"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn park_waits_for_the_mount_and_unpark_takes_it_out() {
    let fake = mount_server().await;
    fake.on_put("telescope/0/park", |values, _| {
        values.insert("telescope/0/atpark".into(), json!(true));
    });
    fake.on_put("telescope/0/unpark", |values, _| {
        values.insert("telescope/0/atpark".into(), json!(false));
    });

    let output = scopinator(&["park", "--host", &url(&fake)]).await;
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("Parked."));

    // Parking again is not an error and sends nothing.
    let output = scopinator(&["park", "--host", &url(&fake)]).await;
    assert!(
        stdout(&output).contains("already parked"),
        "{}",
        stdout(&output)
    );
    assert_eq!(fake.puts("telescope/0/park").len(), 1);

    let output = scopinator(&["unpark", "--host", &url(&fake)]).await;
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("Unparked."));
    assert_eq!(fake.puts("telescope/0/unpark").len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn seestar_only_requests_say_so_on_alpaca() {
    let fake = mount_server().await;
    let raw = scopinator(&["raw", "--host", &url(&fake), "get_device_state"]).await;
    assert!(!raw.status.success());
    assert!(stderr(&raw).contains("no Alpaca form"), "{}", stderr(&raw));

    let clock = scopinator(&[
        "goto",
        "--host",
        &url(&fake),
        "--ra",
        "1",
        "--dec",
        "1",
        "--sync-clock",
    ])
    .await;
    assert!(!clock.status.success());
    assert!(
        stderr(&clock).contains("for a Seestar"),
        "{}",
        stderr(&clock)
    );
    assert_eq!(fake.request_count(), 0, "nothing was sent for either");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_is_not_there_is_a_clear_failure() {
    // A port nothing listens on. Not one the system has just handed out
    // and taken back: a test running beside this one can be given it.
    let port = 1;
    let output = scopinator(&["status", "--host", &format!("alpaca://127.0.0.1:{port}")]).await;
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("could not connect to the Alpaca server"),
        "{}",
        stderr(&output)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_contradictory_host_and_backend_is_refused() {
    let output = scopinator(&[
        "--backend",
        "seestar",
        "status",
        "--host",
        "alpaca://127.0.0.1:1",
    ])
    .await;
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("--backend says seestar"),
        "{}",
        stderr(&output)
    );
}
