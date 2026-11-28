//! The INDI protocol: reading and writing its XML, and a client that keeps
//! a live copy of everything an INDI server has defined.

pub mod client;
pub mod device;
pub mod error;
pub mod fits;
pub mod protocol;
pub mod standard;
pub mod store;

pub use client::{BlobArrival, BlobListener, IndiClient, IndiClientConfig};
pub use device::IndiDevice;
pub use error::IndiError;
pub use store::Property;
