use serde::{Deserialize, Serialize};

/// Filter wheel position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilterPosition {
    pub position: u32,
    pub name: Option<String>,
}
