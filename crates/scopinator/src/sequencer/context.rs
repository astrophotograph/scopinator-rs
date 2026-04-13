use std::sync::Arc;

use crate::device::traits::{Camera, Mount};

/// Execution context provided to sequencer commands.
///
/// Contains references to the devices needed for observation sequences.
pub struct ExecutionContext {
    /// The mount to use for slewing.
    pub mount: Arc<dyn Mount>,
    /// The camera to use for imaging (optional).
    pub camera: Option<Arc<dyn Camera>>,
}
