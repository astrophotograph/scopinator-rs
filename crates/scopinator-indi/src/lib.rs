//! The INDI protocol: reading and writing its XML, and a client that keeps
//! a live copy of everything an INDI server has defined.

pub mod client;
pub mod error;
pub mod protocol;
pub mod store;

pub use client::{IndiClient, IndiClientConfig};
pub use error::IndiError;
pub use store::Property;
