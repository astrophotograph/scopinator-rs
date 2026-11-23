//! Finding instruments on the local network, whatever kind they are.
//!
//! Each backend has its own way of being found (a Seestar answers a UDP
//! broadcast on one port, an Alpaca server on another). A [`Discoverer`]
//! wraps one of them, and [`discover_all`] runs every one this build has.
//! What comes back can be handed to
//! [`DeviceManager::connect`](crate::DeviceManager::connect).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::error::ScopinatorError;

/// An instrument that answered a discovery probe.
///
/// Fields may be added; build one with [`Discovered::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Discovered {
    /// Which backend found it and can connect to it: `seestar`, `alpaca`.
    pub backend_kind: &'static str,
    /// Where to connect.
    pub address: SocketAddr,
    /// The name the instrument gives itself.
    pub name: Option<String>,
    /// The product model, e.g. `Seestar S50`.
    pub model: Option<String>,
    pub serial: Option<String>,
}

impl Discovered {
    pub fn new(backend_kind: &'static str, address: SocketAddr) -> Self {
        Self {
            backend_kind,
            address,
            name: None,
            model: None,
            serial: None,
        }
    }
}

/// One way of finding instruments.
#[async_trait]
pub trait Discoverer: Send + Sync {
    /// The backend kind of what this finds.
    fn kind(&self) -> &'static str;

    /// Probe the local network and collect answers for `timeout`.
    async fn discover(&self, timeout: Duration) -> Result<Vec<Discovered>, ScopinatorError>;
}

/// The discoverers of every backend compiled into this build.
// One push per feature-gated backend: a `vec![]` cannot hold `#[cfg]` items.
#[allow(clippy::vec_init_then_push, unused_mut)]
pub fn discoverers() -> Vec<Arc<dyn Discoverer>> {
    let mut all: Vec<Arc<dyn Discoverer>> = Vec::new();
    #[cfg(feature = "seestar")]
    all.push(Arc::new(crate::backend::seestar::SeestarDiscoverer));
    all
}

/// Look for instruments of every kind this build supports, at the same
/// time, for `timeout`. A kind whose search fails is logged and left out;
/// the others still report.
pub async fn discover_all(timeout: Duration) -> Vec<Discovered> {
    discover_with(discoverers(), timeout).await
}

/// [`discover_all`] with a chosen set of discoverers. Results are ordered by
/// backend kind, then address.
pub async fn discover_with(
    discoverers: Vec<Arc<dyn Discoverer>>,
    timeout: Duration,
) -> Vec<Discovered> {
    let mut searches = tokio::task::JoinSet::new();
    for discoverer in discoverers {
        searches.spawn(async move {
            let kind = discoverer.kind();
            (kind, discoverer.discover(timeout).await)
        });
    }
    let mut found = Vec::new();
    while let Some(search) = searches.join_next().await {
        match search {
            Ok((_, Ok(devices))) => found.extend(devices),
            Ok((kind, Err(e))) => tracing::warn!(kind, error = %e, "discovery failed"),
            Err(e) => tracing::warn!(error = %e, "discovery task failed"),
        }
    }
    found.sort_by(|a, b| (a.backend_kind, a.address).cmp(&(b.backend_kind, b.address)));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Finds(&'static str, Vec<&'static str>);

    #[async_trait]
    impl Discoverer for Finds {
        fn kind(&self) -> &'static str {
            self.0
        }

        async fn discover(&self, _timeout: Duration) -> Result<Vec<Discovered>, ScopinatorError> {
            Ok(self
                .1
                .iter()
                .map(|address| Discovered::new(self.0, address.parse().unwrap()))
                .collect())
        }
    }

    struct Fails;

    #[async_trait]
    impl Discoverer for Fails {
        fn kind(&self) -> &'static str {
            "broken"
        }

        async fn discover(&self, _timeout: Duration) -> Result<Vec<Discovered>, ScopinatorError> {
            Err(ScopinatorError::Backend("no network".into()))
        }
    }

    /// Takes the whole timeout, as a real UDP search does.
    struct Slow(&'static str);

    #[async_trait]
    impl Discoverer for Slow {
        fn kind(&self) -> &'static str {
            self.0
        }

        async fn discover(&self, timeout: Duration) -> Result<Vec<Discovered>, ScopinatorError> {
            tokio::time::sleep(timeout).await;
            Ok(vec![Discovered::new(self.0, "10.0.0.9:1".parse().unwrap())])
        }
    }

    #[tokio::test]
    async fn one_failing_search_does_not_hide_the_others() {
        let found = discover_with(
            vec![
                Arc::new(Finds("seestar", vec!["10.0.0.2:4700"])),
                Arc::new(Fails),
                Arc::new(Finds("alpaca", vec!["10.0.0.7:11111", "10.0.0.3:32323"])),
            ],
            Duration::from_secs(1),
        )
        .await;
        let summary: Vec<(&str, String)> = found
            .iter()
            .map(|d| (d.backend_kind, d.address.to_string()))
            .collect();
        // Ordered by kind, then address.
        assert_eq!(
            summary,
            [
                ("alpaca", "10.0.0.3:32323".to_string()),
                ("alpaca", "10.0.0.7:11111".to_string()),
                ("seestar", "10.0.0.2:4700".to_string()),
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn searches_run_at_the_same_time() {
        let start = tokio::time::Instant::now();
        let found = discover_with(
            vec![
                Arc::new(Slow("a")),
                Arc::new(Slow("b")),
                Arc::new(Slow("c")),
            ],
            Duration::from_secs(3),
        )
        .await;
        assert_eq!(found.len(), 3);
        // Three searches of three seconds each took three seconds, not nine.
        assert_eq!(start.elapsed(), Duration::from_secs(3));
    }

    #[tokio::test]
    async fn no_discoverers_find_nothing() {
        assert!(
            discover_with(Vec::new(), Duration::from_secs(1))
                .await
                .is_empty()
        );
    }
}
