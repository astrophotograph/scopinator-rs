use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use scopinator_seestar::InteropKey;

mod commands;
mod target;

use target::{BackendKind, HostArg, Target};

#[derive(Parser)]
#[command(name = "scopinator", about = "Telescope control CLI")]
struct Cli {
    /// Logging verbosity (-v for debug, -vv for trace)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Path to RSA interoperability PEM key for the verify_client handshake (firmware 6.70+)
    #[arg(long, env = "SEESTAR_INTEROP_PEM", value_name = "PATH")]
    interop_pem: Option<PathBuf>,

    /// The kind of telescope at --host. A URL in --host says the same:
    /// `alpaca://192.168.1.50:11111`, `seestar://192.168.1.100`. Without
    /// either, a Seestar.
    #[arg(long, global = true, env = "SCOPINATOR_BACKEND", value_enum)]
    backend: Option<BackendKind>,

    /// The Alpaca server's port, when --host has none (default 11111).
    #[arg(long, global = true, value_name = "PORT")]
    port: Option<u16>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Discover telescopes on the local network (Seestars and Alpaca servers)
    Discover {
        /// Discovery timeout in seconds
        #[arg(short, long, default_value = "3")]
        timeout: u64,
    },
    /// Show telescope status
    Status {
        /// Telescope address: an IPv4 address, or a URL naming the backend
        /// (alpaca://HOST:PORT, seestar://HOST)
        #[arg(short = 'H', long)]
        host: HostArg,
    },
    /// Slew to coordinates
    ///
    /// On an Alpaca mount the coordinates are J2000 unless --jnow is given,
    /// tracking is turned on if it is off, and the command waits for the
    /// mount to arrive. A Seestar is sent the coordinates as given and
    /// reports when it starts moving.
    Goto {
        /// Telescope address: an IPv4 address, or a URL naming the backend
        /// (alpaca://HOST:PORT, seestar://HOST)
        #[arg(short = 'H', long)]
        host: HostArg,
        /// Right ascension in hours (0-24)
        #[arg(long)]
        ra: f64,
        /// Declination in degrees (-90 to 90)
        #[arg(long, allow_negative_numbers = true)]
        dec: f64,
        /// Target name
        #[arg(short, long, default_value = "Target")]
        name: String,
        /// Set the telescope's clock from this computer even if it looks
        /// right. By default the clock is set only when it is clearly wrong
        /// (some models lose it when powered off), because the telescope
        /// cannot find a target without it.
        #[arg(long, conflicts_with = "no_sync_clock")]
        sync_clock: bool,
        /// Never set the telescope's clock; only warn when it looks wrong.
        #[arg(long)]
        no_sync_clock: bool,
        /// The coordinates are in the equinox of date, not J2000 (Alpaca).
        #[arg(long)]
        jnow: bool,
        /// How long to wait for the mount to arrive, in seconds (Alpaca).
        #[arg(long, default_value = "300")]
        timeout: u64,
    },
    /// Park the telescope
    Park {
        /// Telescope address: an IPv4 address, or a URL naming the backend
        /// (alpaca://HOST:PORT, seestar://HOST)
        #[arg(short = 'H', long)]
        host: HostArg,
        /// How long to wait for the mount to park, in seconds (Alpaca).
        #[arg(long, default_value = "300")]
        timeout: u64,
    },
    /// Take the mount out of park (a Seestar opens by itself on a goto)
    Unpark {
        /// Telescope address: an IPv4 address, or a URL naming the backend
        /// (alpaca://HOST:PORT, seestar://HOST)
        #[arg(short = 'H', long)]
        host: HostArg,
    },
    /// Send any Seestar control method by name and print the reply as JSON.
    ///
    /// For methods this tool has no command for. Nothing is checked: what
    /// the method does to the telescope is up to you.
    Raw {
        /// Telescope address: an IPv4 address, or a URL naming the backend
        /// (alpaca://HOST:PORT, seestar://HOST)
        #[arg(short = 'H', long)]
        host: HostArg,
        /// Method name as the firmware spells it, e.g. iscope_get_app_state
        method: String,
        /// Params as JSON, e.g. '{"keys":["mount"]}'. Omit to send none.
        params: Option<String>,
        /// For a method with no params that reads a positional element
        /// (get_disk_volume, get_annotate_result): when the connection needs
        /// the verify marker, send it as an object.
        #[arg(long)]
        object_marker: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let log_level = match cli.verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level)),
        )
        .init();

    // Load interop key once; fail fast if the file exists but is unparseable.
    let interop_key: Option<InteropKey> = if let Some(path) = &cli.interop_pem {
        let pem = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read interop PEM from {}", path.display()))?;
        let key = InteropKey::from_pem(&pem)
            .with_context(|| format!("failed to parse interop PEM from {}", path.display()))?;
        Some(key)
    } else {
        None
    };

    let target = |host: &HostArg| Target::resolve(host, cli.backend, cli.port);

    match cli.command {
        Commands::Discover { timeout } => {
            commands::discover(Duration::from_secs(timeout)).await?;
        }
        Commands::Status { host } => match target(&host)? {
            Target::Seestar { host } => commands::status(host, interop_key).await?,
            Target::Alpaca { host, port } => {
                let backend = commands::neutral::connect_alpaca(host, port).await?;
                commands::neutral::status(backend).await?;
            }
        },
        Commands::Goto {
            host,
            ra,
            dec,
            name,
            sync_clock,
            no_sync_clock,
            jnow,
            timeout,
        } => match target(&host)? {
            Target::Seestar { host } => {
                let clock = match (sync_clock, no_sync_clock) {
                    (true, _) => commands::ClockPolicy::Always,
                    (_, true) => commands::ClockPolicy::Never,
                    _ => commands::ClockPolicy::IfWrong,
                };
                commands::goto(host, ra, dec, &name, interop_key, clock).await?;
            }
            Target::Alpaca { host, port } => {
                if sync_clock || no_sync_clock {
                    anyhow::bail!("--sync-clock and --no-sync-clock are for a Seestar");
                }
                let mut coords = scopinator_types::Coordinates::from_hours(ra, dec)
                    .context("invalid coordinates")?;
                if jnow {
                    coords.epoch = scopinator_types::Epoch::JNow;
                }
                let backend = commands::neutral::connect_alpaca(host, port).await?;
                commands::neutral::goto(backend, coords, Duration::from_secs(timeout)).await?;
            }
        },
        Commands::Park { host, timeout } => match target(&host)? {
            Target::Seestar { host } => commands::park(host, interop_key).await?,
            Target::Alpaca { host, port } => {
                let backend = commands::neutral::connect_alpaca(host, port).await?;
                commands::neutral::park(backend, Duration::from_secs(timeout)).await?;
            }
        },
        Commands::Unpark { host } => match target(&host)? {
            Target::Seestar { .. } => anyhow::bail!(
                "a Seestar has no unpark request: it opens its arm when it is sent a goto"
            ),
            Target::Alpaca { host, port } => {
                let backend = commands::neutral::connect_alpaca(host, port).await?;
                commands::neutral::unpark(backend).await?;
            }
        },
        Commands::Raw {
            host,
            method,
            params,
            object_marker,
        } => match target(&host)? {
            Target::Seestar { host } => {
                commands::raw(host, &method, params.as_deref(), object_marker, interop_key).await?;
            }
            Target::Alpaca { .. } => {
                anyhow::bail!("`raw` sends Seestar control methods; it has no Alpaca form")
            }
        },
    }

    Ok(())
}
