//! Toggle one boolean setting and restore it, through
//! `SeestarClient::set_setting`, printing the per-setting outcome.
//!
//! Exercises both verification modes against real hardware: without a key the
//! connection uses the `verify` marker (every raw `set_setting` draws 109 and
//! the outcome comes from a readback); with a key it is a plain request.
//!
//! ```text
//! cargo run -p scopinator-seestar --example probe_setting -- 192.168.x.x
//! SEESTAR_INTEROP_PEM=client.pem cargo run -p scopinator-seestar \
//!     --example probe_setting -- 192.168.x.x [setting_name]
//! ```
//!
//! The default setting is `stack_after_goto`, which only changes behaviour
//! after a later goto. The original value is always written back.

use std::time::Duration;

use scopinator_seestar::command::Command;
use scopinator_seestar::command::params::SettingParams;
use scopinator_seestar::{InteropKey, SeestarClient, SeestarConfig};

type Err = Box<dyn std::error::Error>;

async fn read(client: &SeestarClient, key: &str) -> Result<bool, Err> {
    let settings = client.send_and_validate(Command::GetSetting).await?;
    settings
        .get(key)
        .and_then(|v| v.as_bool())
        .ok_or_else(|| format!("get_setting has no boolean `{key}`").into())
}

async fn write(client: &SeestarClient, key: &str, value: bool) -> Result<(), Err> {
    let mut params = SettingParams::default();
    params.extra.insert(key.to_string(), value.into());
    let results = client.set_setting(params).await?;
    println!("  set {key}={value} -> {results:?}");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Err> {
    let mut args = std::env::args().skip(1);
    let host = args.next().ok_or("usage: probe_setting <host> [setting]")?;
    let key = args
        .next()
        .unwrap_or_else(|| "stack_after_goto".to_string());

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

    let original = read(&client, &key).await?;
    println!("{key} is {original}");

    write(&client, &key, !original).await?;
    println!("  readback: {}", read(&client, &key).await?);

    write(&client, &key, original).await?;
    let restored = read(&client, &key).await?;
    println!("  readback: {restored}");

    client.shutdown().await;
    if restored != original {
        return Err(format!("{key} was not restored to {original}").into());
    }
    Ok(())
}
