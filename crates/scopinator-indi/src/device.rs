//! One named device on an INDI server.

use std::sync::Arc;
use std::time::Duration;

use crate::client::IndiClient;
use crate::error::IndiError;
use crate::protocol::message::IndiMessage;
use crate::protocol::property::{PropertyState, SwitchState};
use crate::standard::general;
use crate::store::Property;

/// A handle to one device: what it is, connecting it, and reading its
/// properties. Cheap to clone.
#[derive(Debug, Clone)]
pub struct IndiDevice {
    client: Arc<IndiClient>,
    name: String,
}

impl IndiDevice {
    pub fn new(client: Arc<IndiClient>, name: impl Into<String>) -> Self {
        Self {
            client,
            name: name.into(),
        }
    }

    /// The device's name on the server.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn client(&self) -> &Arc<IndiClient> {
        &self.client
    }

    /// One of this device's properties as it is now.
    pub fn property(&self, property: &str) -> Option<Property> {
        self.client.property(&self.name, property)
    }

    /// True if the driver has defined `property`.
    pub fn has(&self, property: &str) -> bool {
        self.property(property).is_some()
    }

    /// The `DRIVER_INTERFACE` bit mask, once the driver has said what it
    /// is. See [`interface`](crate::protocol::property::interface).
    pub fn interfaces(&self) -> Option<u32> {
        self.text(general::DRIVER_INFO, general::DRIVER_INTERFACE)?
            .trim()
            .parse()
            .ok()
    }

    /// True if the driver says it is (among other things) a device of this
    /// kind.
    pub fn has_interface(&self, flag: u32) -> bool {
        self.interfaces().is_some_and(|mask| mask & flag != 0)
    }

    /// The driver's name and version, as far as it gives them.
    pub fn driver(&self) -> Option<String> {
        let name = self.text(general::DRIVER_INFO, general::DRIVER_NAME)?;
        Some(
            match self.text(general::DRIVER_INFO, general::DRIVER_VERSION) {
                Some(version) if !version.is_empty() => format!("{name} {version}"),
                _ => name,
            },
        )
    }

    /// True if the driver is connected to its hardware.
    pub fn is_connected(&self) -> bool {
        self.switch(general::CONNECTION, general::CONNECT) == Some(SwitchState::On)
    }

    /// Connect the driver to its hardware and wait until it says it is
    /// connected. If the driver answers with `Alert`, that is the error,
    /// with the driver's message, without waiting out the timeout.
    pub async fn connect(&self, timeout: Duration) -> Result<(), IndiError> {
        self.set_connection(true, timeout).await
    }

    pub async fn disconnect(&self, timeout: Duration) -> Result<(), IndiError> {
        self.set_connection(false, timeout).await
    }

    async fn set_connection(&self, connect: bool, timeout: Duration) -> Result<(), IndiError> {
        let deadline = tokio::time::Instant::now() + timeout;
        let wanted = if connect {
            general::CONNECT
        } else {
            general::DISCONNECT
        };
        // The driver may not have defined itself yet.
        let now = self
            .client
            .wait_for(&self.name, general::CONNECTION, timeout, |_| true)
            .await?;
        if now.is_on(wanted) && now.state != PropertyState::Busy {
            return Ok(());
        }

        // Listen before asking, so that the answer cannot be missed. An
        // `Alert` left over from an earlier attempt is not an answer.
        let mut messages = self.client.subscribe();
        let state = |on| {
            if on {
                SwitchState::On
            } else {
                SwitchState::Off
            }
        };
        self.client
            .set_switches(
                &self.name,
                general::CONNECTION,
                &[
                    (general::CONNECT, state(connect)),
                    (general::DISCONNECT, state(!connect)),
                ],
            )
            .await?;
        loop {
            let answered = match tokio::time::timeout_at(deadline, messages.recv()).await {
                Err(_) => return Err(IndiError::Timeout),
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                    return Err(IndiError::NotConnected);
                }
                // Something was missed: look at where things stand.
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => true,
                Ok(Ok(IndiMessage::Update(update))) => {
                    update.device == self.name && update.name == general::CONNECTION
                }
                Ok(Ok(_)) => false,
            };
            if !answered {
                continue;
            }
            let Some(property) = self.property(general::CONNECTION) else {
                return Err(IndiError::DeviceNotFound(self.name.clone()));
            };
            match property.state {
                PropertyState::Alert => {
                    return Err(IndiError::PropertyState(property.message.unwrap_or_else(
                        || {
                            format!(
                                "{} refused to {}",
                                self.name,
                                if connect { "connect" } else { "disconnect" }
                            )
                        },
                    )));
                }
                PropertyState::Busy => {}
                _ if property.is_on(wanted) => return Ok(()),
                _ => {}
            }
        }
    }

    /// The value of one number member, or `None` if the property or the
    /// member is not defined.
    pub fn number(&self, property: &str, member: &str) -> Option<f64> {
        self.property(property)?.number(member)
    }

    pub fn switch(&self, property: &str, member: &str) -> Option<SwitchState> {
        self.property(property)?.switch(member)
    }

    pub fn text(&self, property: &str, member: &str) -> Option<String> {
        self.property(property)?.text(member).map(str::to_string)
    }

    pub fn state(&self, property: &str) -> Option<PropertyState> {
        Some(self.property(property)?.state)
    }
}
