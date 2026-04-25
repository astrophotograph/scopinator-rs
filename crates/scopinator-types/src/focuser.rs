use serde::{Deserialize, Serialize};

/// Focuser position and state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FocuserPosition {
    pub position: i32,
    pub max_position: i32,
    pub temperature: Option<f64>,
    pub is_moving: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // Temperatures with 0.01°C precision: always exactly representable as f64
    // and roundtrip cleanly through serde_json's Ryu/parser combination.
    fn temperatures() -> impl Strategy<Value = Option<f64>> {
        proptest::option::of((-50_000i32..50_000).prop_map(|x| f64::from(x) / 100.0))
    }

    proptest! {
        #[test]
        fn focuser_position_roundtrip(
            position in any::<i32>(),
            max_position in any::<i32>(),
            temperature in temperatures(),
            is_moving in any::<bool>(),
        ) {
            let fp = FocuserPosition { position, max_position, temperature, is_moving };
            let s = serde_json::to_string(&fp).unwrap();
            let back: FocuserPosition = serde_json::from_str(&s).unwrap();
            prop_assert_eq!(fp, back);
        }
    }
}
