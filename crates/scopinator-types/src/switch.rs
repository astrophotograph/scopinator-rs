use serde::{Deserialize, Serialize};

/// One output of a [switch device](https://ascom-standards.org/newdocs/switch.html):
/// a dew heater, a power port, a flat panel's brightness.
///
/// Every switch has a value in a range. One that is only on or off has the
/// range 0 to 1 in one step, which is what [`SwitchInfo::on_off`] builds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SwitchInfo {
    /// Its number on the device, counted from zero.
    pub index: u32,
    pub name: String,
    pub description: Option<String>,
    /// The value that means off, or the least.
    pub min: f64,
    /// The value that means fully on.
    pub max: f64,
    /// The difference between two values the switch can tell apart.
    pub step: f64,
    /// False for a switch that can only be read, a sensor for example.
    pub writable: bool,
}

impl SwitchInfo {
    /// A switch with values from `min` to `max` in steps of `step`.
    pub fn ranged(index: u32, name: impl Into<String>, min: f64, max: f64, step: f64) -> Self {
        Self {
            index,
            name: name.into(),
            description: None,
            min,
            max,
            step,
            writable: true,
        }
    }

    /// A switch that is on (1) or off (0).
    pub fn on_off(index: u32, name: impl Into<String>) -> Self {
        Self::ranged(index, name, 0.0, 1.0, 1.0)
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Mark the switch as one that can only be read.
    pub fn read_only(mut self) -> Self {
        self.writable = false;
        self
    }

    /// True if `value` counts as on: at or above the middle of the range.
    pub fn is_on(&self, value: f64) -> bool {
        value >= (self.min + self.max) / 2.0
    }

    /// True if `value` is a number within the range.
    pub fn accepts(&self, value: f64) -> bool {
        value.is_finite() && value >= self.min && value <= self.max
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_on_off_switch_is_on_at_one() {
        let heater = SwitchInfo::on_off(0, "Dew heater");
        assert!(heater.writable);
        assert!(heater.is_on(1.0));
        assert!(!heater.is_on(0.0));
        assert!(heater.accepts(0.0) && heater.accepts(1.0));
        assert!(!heater.accepts(2.0) && !heater.accepts(-1.0) && !heater.accepts(f64::NAN));
    }

    #[test]
    fn a_ranged_switch_is_on_from_the_middle_of_its_range() {
        let panel = SwitchInfo::ranged(1, "Flat panel", 0.0, 255.0, 1.0).read_only();
        assert!(!panel.writable);
        assert!(panel.is_on(127.5));
        assert!(!panel.is_on(127.0));
    }
}
