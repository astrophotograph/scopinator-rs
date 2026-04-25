use serde::{Deserialize, Serialize};

/// Filter wheel position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilterPosition {
    pub position: u32,
    pub name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn filter_position_roundtrip(
            position in any::<u32>(),
            name in proptest::option::of(".{0,32}"),
        ) {
            let fp = FilterPosition { position, name };
            let s = serde_json::to_string(&fp).unwrap();
            let back: FilterPosition = serde_json::from_str(&s).unwrap();
            prop_assert_eq!(fp, back);
        }
    }
}
