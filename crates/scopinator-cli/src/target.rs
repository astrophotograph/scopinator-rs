//! Which telescope a command is for: its address and the backend to use.

use std::net::Ipv4Addr;
use std::str::FromStr;

use anyhow::{Context, Result, bail};

/// The kinds of telescope this tool can talk to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum BackendKind {
    /// A ZWO Seestar, over its own protocol.
    Seestar,
    /// Any mount, camera, focuser or filter wheel behind an ASCOM Alpaca
    /// server.
    Alpaca,
}

impl BackendKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Seestar => "seestar",
            Self::Alpaca => "alpaca",
        }
    }
}

/// The default port of an Alpaca server.
pub const ALPACA_DEFAULT_PORT: u16 = 11111;

/// What `--host` was given: a bare address, or a URL that names the backend
/// (`alpaca://192.168.1.50:11111`, `seestar://192.168.1.100`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostArg {
    backend: Option<BackendKind>,
    host: Ipv4Addr,
    port: Option<u16>,
}

impl FromStr for HostArg {
    type Err = anyhow::Error;

    fn from_str(text: &str) -> Result<Self> {
        let (backend, rest) = match text.split_once("://") {
            Some(("seestar", rest)) => (Some(BackendKind::Seestar), rest),
            Some(("alpaca" | "http", rest)) => (Some(BackendKind::Alpaca), rest),
            Some((scheme, _)) => {
                bail!("unknown backend `{scheme}://` (use seestar:// or alpaca://)")
            }
            None => (None, text),
        };
        let rest = rest.trim_end_matches('/');
        let (host, port) = match rest.split_once(':') {
            Some((host, port)) => (
                host,
                Some(
                    port.parse::<u16>()
                        .with_context(|| format!("`{port}` is not a port number"))?,
                ),
            ),
            None => (rest, None),
        };
        let host = host
            .parse::<Ipv4Addr>()
            .with_context(|| format!("`{host}` is not an IPv4 address"))?;
        Ok(Self {
            backend,
            host,
            port,
        })
    }
}

/// A telescope to connect to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Seestar { host: Ipv4Addr },
    Alpaca { host: Ipv4Addr, port: u16 },
}

impl Target {
    /// Combine `--host` with `--backend` and `--port`. A URL's backend and
    /// port win over the options; a contradiction between a URL and
    /// `--backend` is an error.
    pub fn resolve(
        host: &HostArg,
        backend: Option<BackendKind>,
        port: Option<u16>,
    ) -> Result<Self> {
        if let (Some(in_url), Some(option)) = (host.backend, backend)
            && in_url != option
        {
            bail!(
                "--host names a {} telescope but --backend says {}",
                in_url.name(),
                option.name()
            );
        }
        match host.backend.or(backend).unwrap_or(BackendKind::Seestar) {
            BackendKind::Seestar => {
                if host.port.or(port).is_some() {
                    bail!("a Seestar is reached on fixed ports; drop the port");
                }
                Ok(Self::Seestar { host: host.host })
            }
            BackendKind::Alpaca => Ok(Self::Alpaca {
                host: host.host,
                port: host.port.or(port).unwrap_or(ALPACA_DEFAULT_PORT),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(host: &str, backend: Option<BackendKind>, port: Option<u16>) -> Result<Target> {
        Target::resolve(&host.parse()?, backend, port)
    }

    const IP: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 50);

    #[test]
    fn a_bare_address_is_a_seestar_unless_told_otherwise() {
        assert_eq!(
            target("192.168.1.50", None, None).unwrap(),
            Target::Seestar { host: IP }
        );
        assert_eq!(
            target("192.168.1.50", Some(BackendKind::Alpaca), None).unwrap(),
            Target::Alpaca {
                host: IP,
                port: 11111
            }
        );
        assert_eq!(
            target("192.168.1.50", Some(BackendKind::Alpaca), Some(32323)).unwrap(),
            Target::Alpaca {
                host: IP,
                port: 32323
            }
        );
    }

    #[test]
    fn a_url_names_the_backend_and_the_port() {
        assert_eq!(
            target("alpaca://192.168.1.50:32323", None, None).unwrap(),
            Target::Alpaca {
                host: IP,
                port: 32323
            }
        );
        assert_eq!(
            target("alpaca://192.168.1.50", None, None).unwrap(),
            Target::Alpaca {
                host: IP,
                port: 11111
            }
        );
        // What a browser would show for an Alpaca server.
        assert_eq!(
            target("http://192.168.1.50:11111/", None, None).unwrap(),
            Target::Alpaca {
                host: IP,
                port: 11111
            }
        );
        assert_eq!(
            target("seestar://192.168.1.50", None, None).unwrap(),
            Target::Seestar { host: IP }
        );
        // A bare address with a port, plus --backend.
        assert_eq!(
            target("192.168.1.50:5000", Some(BackendKind::Alpaca), None).unwrap(),
            Target::Alpaca {
                host: IP,
                port: 5000
            }
        );
    }

    #[test]
    fn the_port_in_a_url_wins_over_the_port_option() {
        assert_eq!(
            target("alpaca://192.168.1.50:32323", None, Some(1)).unwrap(),
            Target::Alpaca {
                host: IP,
                port: 32323
            }
        );
    }

    #[test]
    fn contradictions_and_nonsense_are_errors() {
        assert!(target("alpaca://192.168.1.50", Some(BackendKind::Seestar), None).is_err());
        assert!(target("seestar://192.168.1.50:4700", None, None).is_err());
        assert!(target("192.168.1.50", None, Some(4700)).is_err());
        assert!(target("indi://192.168.1.50", None, None).is_err());
        assert!(target("telescope.local", None, None).is_err());
        assert!(target("192.168.1.50:port", Some(BackendKind::Alpaca), None).is_err());
        assert!(target("", None, None).is_err());
    }
}
