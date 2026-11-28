//! What the typed device clients share: one device of one type on a server.

use std::sync::Arc;

use serde::de::DeserializeOwned;

use crate::client::AlpacaClient;
use crate::error::AlpacaError;

/// One device on an Alpaca server: a device type and number.
#[derive(Clone)]
pub(crate) struct Endpoint {
    client: Arc<AlpacaClient>,
    device_type: &'static str,
    device_number: u32,
}

impl Endpoint {
    pub(crate) fn new(client: Arc<AlpacaClient>, device_type: &'static str, number: u32) -> Self {
        Self {
            client,
            device_type,
            device_number: number,
        }
    }

    pub(crate) fn device_number(&self) -> u32 {
        self.device_number
    }

    /// GET a property and read its `Value` as `T`.
    pub(crate) async fn get<T: DeserializeOwned>(&self, property: &str) -> Result<T, AlpacaError> {
        let value = self
            .client
            .get_property(self.device_type, self.device_number, property)
            .await?;
        Ok(serde_json::from_value(value)?)
    }

    /// GET a property that takes parameters and read its `Value` as `T`.
    pub(crate) async fn get_with<T: DeserializeOwned>(
        &self,
        property: &str,
        params: &[(&str, String)],
    ) -> Result<T, AlpacaError> {
        let value = self
            .client
            .get_property_with(self.device_type, self.device_number, property, params)
            .await?;
        Ok(serde_json::from_value(value)?)
    }

    /// PUT a property or call a method.
    pub(crate) async fn put(
        &self,
        property: &str,
        params: &[(&str, String)],
    ) -> Result<(), AlpacaError> {
        self.client
            .set_property(self.device_type, self.device_number, property, params)
            .await?;
        Ok(())
    }
}

/// A boolean as Alpaca form parameters spell it.
pub(crate) fn bool_param(value: bool) -> String {
    if value { "True" } else { "False" }.to_string()
}

/// Async getters for properties: `name: Type = "alpacaproperty"`.
macro_rules! getters {
    ($( $(#[$meta:meta])* $name:ident : $ty:ty = $property:literal; )*) => {
        $(
            $(#[$meta])*
            pub async fn $name(&self) -> Result<$ty, $crate::error::AlpacaError> {
                self.endpoint.get($property).await
            }
        )*
    };
}

/// Async setters for properties: `name(Type) = "alpacaproperty", "FormName"`.
macro_rules! setters {
    ($( $(#[$meta:meta])* $name:ident ( $ty:ty ) = $property:literal, $param:literal; )*) => {
        $(
            $(#[$meta])*
            pub async fn $name(&self, value: $ty) -> Result<(), $crate::error::AlpacaError> {
                self.endpoint
                    .put($property, &[($param, $crate::device::Param::param(&value))])
                    .await
            }
        )*
    };
}

/// Methods that take no parameters: `name = "alpacamethod"`.
macro_rules! actions {
    ($( $(#[$meta:meta])* $name:ident = $method:literal; )*) => {
        $(
            $(#[$meta])*
            pub async fn $name(&self) -> Result<(), $crate::error::AlpacaError> {
                self.endpoint.put($method, &[]).await
            }
        )*
    };
}

/// An enum Alpaca sends as an integer. An unknown number is an error, not a
/// guess.
macro_rules! int_enum {
    ($(#[$meta:meta])* $name:ident { $( $(#[$vmeta:meta])* $variant:ident = $value:literal, )* }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $( $(#[$vmeta])* $variant = $value, )*
        }

        impl $name {
            /// The number Alpaca uses for this value.
            pub fn number(self) -> i32 {
                self as i32
            }

            pub fn from_number(number: i64) -> Option<Self> {
                match number {
                    $( $value => Some(Self::$variant), )*
                    _ => None,
                }
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let number = i64::deserialize(deserializer)?;
                Self::from_number(number).ok_or_else(|| {
                    serde::de::Error::custom(format!(
                        "{number} is not a {}",
                        stringify!($name)
                    ))
                })
            }
        }

        impl $crate::device::Param for $name {
            fn param(&self) -> String {
                self.number().to_string()
            }
        }
    };
}

pub(crate) use {actions, getters, int_enum, setters};

/// A value as an Alpaca form parameter.
pub(crate) trait Param {
    fn param(&self) -> String;
}

impl Param for bool {
    fn param(&self) -> String {
        bool_param(*self)
    }
}

macro_rules! display_param {
    ($($ty:ty),*) => {
        $( impl Param for $ty {
            fn param(&self) -> String {
                self.to_string()
            }
        } )*
    };
}

display_param!(f64, i32, u16, u32);
