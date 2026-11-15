//! Fetch one-shot images from the image socket (4800) and print what came
//! back: the current preview frame as plain raw and as zip-compressed raw,
//! then the stacked image (which the scope refuses when nothing is stacked).
//!
//! ```text
//! cargo run -p scopinator-seestar --example probe_image -- 192.168.x.x
//! cargo run -p scopinator-seestar --example probe_image -- 192.168.x.x view
//! ```
//!
//! Without `view` nothing moves: the requests are sent as the scope is, and a
//! parked scope answers `there is no image now`. With `view` the scope is put
//! into star mode first (the arm opens), and afterwards the view is stopped
//! and the scope parked again. `SEESTAR_INTEROP_PEM` may name a key file.

use std::time::Duration;

use scopinator_seestar::command::params::{StartViewParams, ViewMode};
use scopinator_seestar::command::{Command, ImagingCommand};
use scopinator_seestar::{InteropKey, SeestarClient, SeestarConfig};

type Err = Box<dyn std::error::Error>;

async fn fetch(client: &SeestarClient, label: &str, cmd: ImagingCommand) {
    match client.request_image(cmd).await {
        Ok(frame) => {
            let h = &frame.header;
            let pixels = frame.image_bytes().map(|b| b.len());
            println!(
                "  {label}: {:?} {}x{} imgType={} bytes/px={:?} canDebayer={} imageId={} \
                 requestId={} payload={}B pixels={pixels:?}",
                frame.payload_kind(),
                h.width,
                h.height,
                h.img_type,
                h.bytes_per_pixel(),
                h.can_debayer,
                h.image_id,
                h.request_id,
                frame.data.len(),
            );
        }
        Err(e) => println!("  {label}: {e}"),
    }
}

#[tokio::main]
async fn main() -> Result<(), Err> {
    let mut args = std::env::args().skip(1);
    let host = args.next().ok_or("usage: probe_image <host> [view]")?;
    let view = args.next().as_deref() == Some("view");

    let interop_key = match std::env::var("SEESTAR_INTEROP_PEM") {
        Ok(path) => Some(InteropKey::from_pem(&std::fs::read_to_string(path)?)?),
        Err(_) => None,
    };
    let client = SeestarClient::connect_with_config(
        host.parse()?,
        SeestarConfig {
            interop_key,
            response_timeout: Some(Duration::from_secs(15)),
        },
    )
    .await?;
    client.wait_for_connection(Duration::from_secs(10)).await?;
    println!("verification: {:?}", client.verification());

    if view {
        let params = StartViewParams {
            mode: Some(ViewMode::Star),
            target_name: None,
            target_ra_dec: None,
            target_type: None,
            lp_filter: None,
        };
        let r = client
            .send_command(Command::IscopeStartView(params))
            .await?;
        println!("iscope_start_view star -> code {}", r.code);
        // Give the arm time to open and continuous exposure time to start.
        tokio::time::sleep(Duration::from_secs(12)).await;
    }
    let state = client.send_and_validate(Command::GetViewState).await?;
    println!(
        "view: mode={} stage={}",
        state["View"]["mode"], state["View"]["stage"]
    );

    let raw = ImagingCommand::GetCurrentImage {
        compress: Some(false),
        raw: true,
    };
    let zipped = ImagingCommand::GetCurrentImage {
        compress: Some(true),
        raw: true,
    };
    fetch(&client, "get_current_img raw", raw).await;
    fetch(&client, "get_current_img zip", zipped).await;
    fetch(&client, "get_stacked_img", ImagingCommand::GetStackedImage).await;

    if view {
        let r = client.send_command(Command::IscopeStopView(None)).await?;
        println!("iscope_stop_view -> code {}", r.code);
        tokio::time::sleep(Duration::from_secs(2)).await;
        let r = client.send_command(Command::ScopePark).await?;
        println!("scope_park -> code {}", r.code);
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let mount = client.send_and_validate(Command::GetDeviceState).await?;
            let mount = &mount["mount"];
            if mount["close"] == true && mount["move_type"] == "none" {
                println!("parked: {mount}");
                break;
            }
        }
    }
    client.shutdown().await;
    Ok(())
}
