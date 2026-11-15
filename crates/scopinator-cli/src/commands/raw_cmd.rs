use std::net::Ipv4Addr;
use std::time::Duration;

use anyhow::{Context, Result};
use scopinator_seestar::command::RawMarker;
use scopinator_seestar::{InteropKey, SeestarClient, SeestarConfig};

/// Send one control method by name and print the reply as JSON.
///
/// Returns an error (non-zero exit) when the telescope answers with a
/// non-zero code, after printing the reply.
pub async fn raw(
    host: Ipv4Addr,
    method: &str,
    params: Option<&str>,
    object_marker: bool,
    interop_key: Option<InteropKey>,
) -> Result<()> {
    let params = params
        .map(serde_json::from_str::<serde_json::Value>)
        .transpose()
        .context("params must be valid JSON")?;

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

    let marker = if object_marker {
        RawMarker::Object
    } else {
        RawMarker::Auto
    };
    let response = client.send_raw_with(method, params, marker).await?;
    client.shutdown().await;

    let reply = serde_json::json!({
        "code": response.code,
        "error": response.error,
        "result": response.result,
    });
    println!("{}", serde_json::to_string_pretty(&reply)?);
    if response.code != 0 {
        anyhow::bail!(
            "{method} failed: {} (code {})",
            response.error.unwrap_or_default(),
            response.code
        );
    }
    Ok(())
}
