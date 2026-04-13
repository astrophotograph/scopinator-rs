use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use scopinator_seestar::InteropKey;

mod commands;

#[derive(Parser)]
#[command(name = "scopinator", about = "Telescope control CLI")]
struct Cli {
    /// Logging verbosity (-v for debug, -vv for trace)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Path to RSA interoperability PEM key for firmware 7.18+ authentication
    #[arg(long, env = "SEESTAR_INTEROP_PEM", value_name = "PATH")]
    interop_pem: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Discover Seestar telescopes on the local network
    Discover {
        /// Discovery timeout in seconds
        #[arg(short, long, default_value = "3")]
        timeout: u64,
    },
    /// Show telescope status
    Status {
        /// Telescope IP address
        #[arg(short = 'H', long)]
        host: Ipv4Addr,
    },
    /// Slew to coordinates
    Goto {
        /// Telescope IP address
        #[arg(short = 'H', long)]
        host: Ipv4Addr,
        /// Right ascension in hours (0-24)
        #[arg(long)]
        ra: f64,
        /// Declination in degrees (-90 to 90)
        #[arg(long)]
        dec: f64,
        /// Target name
        #[arg(short, long, default_value = "Target")]
        name: String,
    },
    /// Park the telescope
    Park {
        /// Telescope IP address
        #[arg(short = 'H', long)]
        host: Ipv4Addr,
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

    match cli.command {
        Commands::Discover { timeout } => {
            commands::discover(Duration::from_secs(timeout)).await?;
        }
        Commands::Status { host } => {
            commands::status(host, interop_key).await?;
        }
        Commands::Goto {
            host,
            ra,
            dec,
            name,
        } => {
            commands::goto(host, ra, dec, &name, interop_key).await?;
        }
        Commands::Park { host } => {
            commands::park(host, interop_key).await?;
        }
    }

    Ok(())
}
