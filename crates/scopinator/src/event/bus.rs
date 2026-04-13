use tokio::sync::broadcast;

use super::UnifiedEvent;

/// A broadcast-based event bus for unified telescope events.
///
/// Subscribers receive all events and filter by type on their end.
/// This avoids the complexity of per-type registration and is trivially
/// `Send + Sync`.
#[derive(Debug)]
pub struct UnifiedEventBus {
    tx: broadcast::Sender<UnifiedEvent>,
}

impl UnifiedEventBus {
    /// Create a new event bus with the given channel capacity.
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Emit an event to all subscribers.
    ///
    /// If there are no active subscribers, the event is silently dropped.
    pub fn emit(&self, event: UnifiedEvent) {
        let _ = self.tx.send(event);
    }

    /// Subscribe to all events on this bus.
    pub fn subscribe(&self) -> broadcast::Receiver<UnifiedEvent> {
        self.tx.subscribe()
    }
}

impl Default for UnifiedEventBus {
    fn default() -> Self {
        Self::new(256)
    }
}
