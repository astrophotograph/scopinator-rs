//! Registry of pending values keyed by an auto-allocated u64 ID.
//!
//! Used for correlating in-flight commands with their responses on the
//! control connection: each outbound command gets a unique ID via
//! [`Registry::register`], and the matching incoming response is dispatched
//! via [`Registry::take`]. On disconnect the registry is [`Registry::drain`]ed
//! so all pending senders can be notified of the failure.
//!
//! Concurrency is tested via `std::thread`-based stress tests in the
//! `tests` module below. Exhaustive loom-based testing would require
//! extracting this type to a tokio-free sub-crate, since `--cfg loom`
//! propagates to tokio and disables `tokio::net`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// Map of pending values keyed by an auto-allocated u64 ID.
pub struct Registry<T> {
    map: Mutex<HashMap<u64, T>>,
    next_id: AtomicU64,
}

impl<T> Registry<T> {
    /// Create a new registry whose first allocated ID will be `initial_id`.
    pub fn with_start(initial_id: u64) -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(initial_id),
        }
    }

    /// Allocate the next ID and store `value` under it. Returns the allocated ID.
    pub fn register(&self, value: T) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut map = self.map.lock().expect("registry mutex poisoned");
        map.insert(id, value);
        id
    }

    /// Take and return the value registered under `id`, if any.
    pub fn take(&self, id: u64) -> Option<T> {
        let mut map = self.map.lock().expect("registry mutex poisoned");
        map.remove(&id)
    }

    /// Drain all registered values, returning them in unspecified order.
    pub fn drain(&self) -> Vec<T> {
        let mut map = self.map.lock().expect("registry mutex poisoned");
        map.drain().map(|(_, v)| v).collect()
    }

    /// Number of entries currently registered.
    pub fn len(&self) -> usize {
        self.map.lock().expect("registry mutex poisoned").len()
    }

    /// True when no entries are registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T> Default for Registry<T> {
    fn default() -> Self {
        Self::with_start(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn register_then_take_returns_value() {
        let r = Registry::<&'static str>::with_start(0);
        let id = r.register("hello");
        assert_eq!(r.take(id), Some("hello"));
        assert_eq!(r.take(id), None);
    }

    #[test]
    fn ids_are_monotonic_within_a_thread() {
        let r = Registry::<u32>::with_start(100);
        let id1 = r.register(1);
        let id2 = r.register(2);
        let id3 = r.register(3);
        assert_eq!(id1, 100);
        assert_eq!(id2, 101);
        assert_eq!(id3, 102);
    }

    #[test]
    fn drain_returns_all_values_and_empties_the_map() {
        let r = Registry::<u32>::with_start(0);
        r.register(1);
        r.register(2);
        r.register(3);
        let mut drained = r.drain();
        drained.sort();
        assert_eq!(drained, vec![1, 2, 3]);
        assert!(r.is_empty());
    }

    #[test]
    fn take_after_drain_returns_none() {
        let r = Registry::<u32>::with_start(0);
        let id = r.register(42);
        let _ = r.drain();
        assert_eq!(r.take(id), None);
    }

    #[test]
    fn len_tracks_inserts_and_removes() {
        let r = Registry::<u32>::with_start(0);
        assert_eq!(r.len(), 0);
        let a = r.register(1);
        let b = r.register(2);
        assert_eq!(r.len(), 2);
        r.take(a);
        assert_eq!(r.len(), 1);
        r.take(b);
        assert_eq!(r.len(), 0);
    }

    proptest! {
        // Allocated IDs are always unique within a sequence.
        #[test]
        fn ids_unique(values in proptest::collection::vec(any::<u32>(), 1..64)) {
            let r = Registry::<u32>::with_start(0);
            let mut ids: Vec<u64> = values.iter().copied().map(|v| r.register(v)).collect();
            let original_len = ids.len();
            ids.sort();
            ids.dedup();
            prop_assert_eq!(ids.len(), original_len);
        }

        // Each (id, value) inserted is exactly recoverable by id.
        #[test]
        fn register_take_roundtrip(values in proptest::collection::vec(any::<u32>(), 0..32)) {
            let r = Registry::<u32>::with_start(0);
            let pairs: Vec<(u64, u32)> = values.iter().copied().map(|v| (r.register(v), v)).collect();
            for (id, expected) in &pairs {
                prop_assert_eq!(r.take(*id), Some(*expected));
            }
            prop_assert!(r.is_empty());
        }

        // Drain returns exactly the multiset of registered values.
        #[test]
        fn drain_captures_everything(values in proptest::collection::vec(any::<u32>(), 0..32)) {
            let r = Registry::<u32>::with_start(0);
            for v in &values {
                r.register(*v);
            }
            let mut drained = r.drain();
            drained.sort();
            let mut expected = values.clone();
            expected.sort();
            prop_assert_eq!(drained, expected);
            prop_assert!(r.is_empty());
        }

        // Registering N items advances next_id by exactly N.
        #[test]
        fn next_id_advances_monotonically(start in any::<u32>(), n in 0u32..32) {
            let r = Registry::<u32>::with_start(start as u64);
            let ids: Vec<u64> = (0..n).map(|i| r.register(i)).collect();
            for (i, id) in ids.iter().enumerate() {
                prop_assert_eq!(*id, start as u64 + i as u64);
            }
        }
    }
}

#[cfg(test)]
mod stress_tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    // N threads concurrently register M values each. All N*M IDs must be
    // distinct, and the map must contain exactly N*M entries.
    #[test]
    fn concurrent_register_produces_unique_ids() {
        const THREADS: usize = 8;
        const PER_THREAD: usize = 256;

        let registry = Arc::new(Registry::<u32>::with_start(0));
        let mut handles = Vec::new();

        for _ in 0..THREADS {
            let r = Arc::clone(&registry);
            handles.push(thread::spawn(move || {
                let mut ids = Vec::with_capacity(PER_THREAD);
                for v in 0..PER_THREAD as u32 {
                    ids.push(r.register(v));
                }
                ids
            }));
        }

        let mut all_ids: Vec<u64> = Vec::with_capacity(THREADS * PER_THREAD);
        for h in handles {
            all_ids.extend(h.join().unwrap());
        }

        let total = all_ids.len();
        all_ids.sort();
        all_ids.dedup();
        assert_eq!(all_ids.len(), total, "duplicate ID detected");
        assert_eq!(registry.len(), total);
    }

    // drain races with register: every registered value must end up either
    // in the drained vec or still in the map. Total count is conserved.
    #[test]
    fn drain_under_concurrent_register_does_not_lose_values() {
        const ITERATIONS: usize = 100;
        const WRITERS: usize = 4;
        const PER_WRITER: usize = 64;

        for _ in 0..ITERATIONS {
            let registry = Arc::new(Registry::<u32>::with_start(0));
            let mut handles = Vec::new();

            for w in 0..WRITERS {
                let r = Arc::clone(&registry);
                handles.push(thread::spawn(move || {
                    for v in 0..PER_WRITER as u32 {
                        r.register(w as u32 * 1000 + v);
                    }
                }));
            }

            // Race a drain against the writers.
            let drained = registry.drain();
            for h in handles {
                h.join().unwrap();
            }
            // Anything not drained should still be in the map.
            let remaining = registry.len();
            assert_eq!(
                drained.len() + remaining,
                WRITERS * PER_WRITER,
                "value lost: drained={} remaining={}",
                drained.len(),
                remaining
            );
        }
    }

    // Two threads call take on the same id. Exactly one must succeed.
    #[test]
    fn concurrent_take_same_id_yields_one_winner() {
        const ITERATIONS: usize = 1000;

        for _ in 0..ITERATIONS {
            let registry = Arc::new(Registry::<u32>::with_start(0));
            let id = registry.register(123);

            let r1 = Arc::clone(&registry);
            let r2 = Arc::clone(&registry);
            let h1 = thread::spawn(move || r1.take(id));
            let h2 = thread::spawn(move || r2.take(id));

            let a = h1.join().unwrap();
            let b = h2.join().unwrap();

            let winners = a.iter().count() + b.iter().count();
            assert_eq!(winners, 1, "expected exactly one take to succeed");
        }
    }

    // Many writers concurrently register; one reader takes by ID. Each
    // taken value matches the one that was registered with that ID.
    #[test]
    fn register_take_pairs_match_under_contention() {
        use std::sync::Mutex as StdMutex;

        const WRITERS: usize = 4;
        const PER_WRITER: usize = 64;

        let registry = Arc::new(Registry::<u32>::with_start(0));
        let pairs: Arc<StdMutex<Vec<(u64, u32)>>> = Arc::new(StdMutex::new(Vec::new()));

        let mut handles = Vec::new();
        for w in 0..WRITERS {
            let r = Arc::clone(&registry);
            let pairs = Arc::clone(&pairs);
            handles.push(thread::spawn(move || {
                for v in 0..PER_WRITER as u32 {
                    let value = (w as u32) * 1000 + v;
                    let id = r.register(value);
                    pairs.lock().unwrap().push((id, value));
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        // Sequentially verify every (id, value) pair recovers exactly.
        let pairs = pairs.lock().unwrap().clone();
        for (id, value) in pairs {
            assert_eq!(registry.take(id), Some(value));
        }
        assert!(registry.is_empty());
    }
}
