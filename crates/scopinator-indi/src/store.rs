//! The client's copy of what the server has defined.

use std::collections::BTreeMap;
use std::sync::Mutex;

use tokio::sync::watch;

use crate::protocol::message::{
    IndiMessage, MemberValue, PropertyDefinition, PropertyUpdate, VectorKind,
};
use crate::protocol::property::{PropertyState, SwitchState};

/// A property as the client knows it now: its definition, with the values,
/// state, timestamp and message of the latest update.
pub type Property = PropertyDefinition;

impl PropertyDefinition {
    /// The value of member `name`, if the property has one by that name.
    pub fn member(&self, name: &str) -> Option<&MemberValue> {
        self.members
            .iter()
            .find(|member| member.name == name)
            .map(|member| &member.value)
    }

    pub fn number(&self, name: &str) -> Option<f64> {
        match self.member(name)? {
            MemberValue::Number(value) => Some(*value),
            _ => None,
        }
    }

    pub fn switch(&self, name: &str) -> Option<SwitchState> {
        match self.member(name)? {
            MemberValue::Switch(state) => Some(*state),
            _ => None,
        }
    }

    pub fn text(&self, name: &str) -> Option<&str> {
        match self.member(name)? {
            MemberValue::Text(text) => Some(text),
            _ => None,
        }
    }

    pub fn light(&self, name: &str) -> Option<PropertyState> {
        match self.member(name)? {
            MemberValue::Light(state) => Some(*state),
            _ => None,
        }
    }

    /// True if member `name` is a switch that is on.
    pub fn is_on(&self, name: &str) -> bool {
        self.switch(name) == Some(SwitchState::On)
    }

    /// The name of the first switch member that is on.
    pub fn selected(&self) -> Option<&str> {
        self.members
            .iter()
            .find(|member| member.value == MemberValue::Switch(SwitchState::On))
            .map(|member| member.name.as_str())
    }
}

/// Properties by device and name, with a signal for every change.
#[derive(Debug)]
pub(crate) struct Store {
    properties: Mutex<BTreeMap<(String, String), Property>>,
    /// Counts changes. Waiters watch it.
    changed: watch::Sender<u64>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            properties: Mutex::new(BTreeMap::new()),
            changed: watch::channel(0).0,
        }
    }
}

impl Store {
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<(String, String), Property>> {
        self.properties
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn touch(&self) {
        self.changed
            .send_modify(|count| *count = count.wrapping_add(1));
    }

    /// A receiver that is notified of every change from now on.
    pub(crate) fn watch(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    pub(crate) fn get(&self, device: &str, name: &str) -> Option<Property> {
        self.lock()
            .get(&(device.to_string(), name.to_string()))
            .cloned()
    }

    pub(crate) fn of_device(&self, device: &str) -> Vec<Property> {
        self.lock()
            .iter()
            .filter(|((owner, _), _)| owner == device)
            .map(|(_, property)| property.clone())
            .collect()
    }

    pub(crate) fn devices(&self) -> Vec<String> {
        let mut devices: Vec<String> = self
            .lock()
            .keys()
            .map(|(device, _)| device.clone())
            .collect();
        devices.dedup();
        devices
    }

    /// Forget everything: the connection is gone, and a new one starts from
    /// new definitions.
    pub(crate) fn clear(&self) {
        self.lock().clear();
        self.touch();
    }

    /// Take a message in. Returns false for an update to a property that
    /// was never defined, which changes nothing.
    pub(crate) fn apply(&self, message: &IndiMessage) -> bool {
        let applied = match message {
            IndiMessage::Define(definition) => {
                self.lock().insert(
                    (definition.device.clone(), definition.name.clone()),
                    definition.clone(),
                );
                true
            }
            IndiMessage::Update(update) => self.update(update),
            IndiMessage::Delete { device, name } => {
                let mut properties = self.lock();
                match name {
                    Some(name) => {
                        properties.remove(&(device.clone(), name.clone()));
                    }
                    None => properties.retain(|(owner, _), _| owner != device),
                }
                true
            }
            _ => return true,
        };
        if applied {
            self.touch();
        }
        applied
    }

    fn update(&self, update: &PropertyUpdate) -> bool {
        let mut properties = self.lock();
        let Some(property) = properties.get_mut(&(update.device.clone(), update.name.clone()))
        else {
            return false;
        };
        if let Some(state) = update.state {
            property.state = state;
        }
        if update.timeout.is_some() {
            property.timeout = update.timeout;
        }
        if update.timestamp.is_some() {
            property.timestamp.clone_from(&update.timestamp);
        }
        if update.message.is_some() {
            property.message.clone_from(&update.message);
        }
        for (name, value) in &update.members {
            if let Some(member) = property
                .members
                .iter_mut()
                .find(|member| &member.name == name)
            {
                member.value = value.clone();
            }
        }
        // Keep the kind the property was defined with.
        debug_assert!(property.kind == update.kind || property.kind == VectorKind::Blob);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::parse::parse_element;

    fn message(xml: &str) -> IndiMessage {
        parse_element(xml.as_bytes()).unwrap()
    }

    const COORDINATES: &str = r#"<defNumberVector device="Scope" name="EQUATORIAL_EOD_COORD" state="Idle" perm="rw"><defNumber name="RA" format="%g" min="0" max="24" step="0">1</defNumber><defNumber name="DEC" format="%g" min="-90" max="90" step="0">2</defNumber></defNumberVector>"#;

    #[test]
    fn a_definition_then_an_update_gives_the_latest_values() {
        let store = Store::default();
        let mut changes = store.watch();
        assert!(store.apply(&message(COORDINATES)));
        assert!(changes.has_changed().unwrap());
        changes.mark_unchanged();

        // One member only, a new state and a message.
        assert!(store.apply(&message(r#"<setNumberVector device="Scope" name="EQUATORIAL_EOD_COORD" state="Busy" message="slewing"><oneNumber name="RA">5.5</oneNumber></setNumberVector>"#)));
        assert!(changes.has_changed().unwrap());
        let property = store.get("Scope", "EQUATORIAL_EOD_COORD").unwrap();
        assert_eq!(property.number("RA"), Some(5.5));
        assert_eq!(property.number("DEC"), Some(2.0));
        assert_eq!(property.state, PropertyState::Busy);
        assert_eq!(property.message.as_deref(), Some("slewing"));
        // An update that says nothing of the state leaves it.
        store.apply(&message(r#"<setNumberVector device="Scope" name="EQUATORIAL_EOD_COORD"><oneNumber name="DEC">3</oneNumber></setNumberVector>"#));
        let property = store.get("Scope", "EQUATORIAL_EOD_COORD").unwrap();
        assert_eq!(
            (property.number("DEC"), property.state),
            (Some(3.0), PropertyState::Busy)
        );
        assert_eq!(property.number("NOPE"), None);
        assert_eq!(property.switch("RA"), None);
    }

    #[test]
    fn an_update_for_what_was_never_defined_changes_nothing() {
        let store = Store::default();
        let changes = store.watch();
        assert!(!store.apply(&message(r#"<setNumberVector device="Scope" name="X" state="Ok"><oneNumber name="A">1</oneNumber></setNumberVector>"#)));
        assert!(!changes.has_changed().unwrap());
        assert!(store.devices().is_empty());
    }

    #[test]
    fn deleting_removes_a_property_or_a_whole_device() {
        let store = Store::default();
        store.apply(&message(COORDINATES));
        store.apply(&message(r#"<defSwitchVector device="Scope" name="CONNECTION" state="Ok" perm="rw" rule="OneOfMany"><defSwitch name="CONNECT">On</defSwitch><defSwitch name="DISCONNECT">Off</defSwitch></defSwitchVector>"#));
        store.apply(&message(r#"<defTextVector device="Camera" name="DRIVER_INFO" state="Idle" perm="ro"><defText name="DRIVER_INTERFACE">2</defText></defTextVector>"#));
        assert_eq!(store.devices(), ["Camera", "Scope"]);
        assert_eq!(store.of_device("Scope").len(), 2);
        let connection = store.get("Scope", "CONNECTION").unwrap();
        assert!(connection.is_on("CONNECT") && !connection.is_on("DISCONNECT"));
        assert_eq!(connection.selected(), Some("CONNECT"));
        assert_eq!(
            store
                .get("Camera", "DRIVER_INFO")
                .unwrap()
                .text("DRIVER_INTERFACE"),
            Some("2")
        );

        store.apply(&message(
            r#"<delProperty device="Scope" name="CONNECTION"/>"#,
        ));
        assert!(store.get("Scope", "CONNECTION").is_none());
        assert_eq!(store.of_device("Scope").len(), 1);
        store.apply(&message(r#"<delProperty device="Scope"/>"#));
        assert_eq!(store.devices(), ["Camera"]);
        store.clear();
        assert!(store.devices().is_empty());
    }
}
