use serde::{Deserialize, Serialize};

/// Focuser position and state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FocuserPosition {
    pub position: i32,
    pub max_position: i32,
    pub temperature: Option<f64>,
    pub is_moving: bool,
}
