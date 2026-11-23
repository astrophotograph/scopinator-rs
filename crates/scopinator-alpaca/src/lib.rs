//! An ASCOM Alpaca client: discovery, the raw property API, and typed
//! clients for telescopes (mounts), cameras, focusers and filter wheels.

pub mod camera;
pub mod client;
mod device;
pub mod discovery;
pub mod error;
pub mod filterwheel;
pub mod focuser;
pub mod image;
pub mod telescope;

pub use camera::AlpacaCamera;
pub use client::AlpacaClient;
pub use error::AlpacaError;
pub use filterwheel::AlpacaFilterWheel;
pub use focuser::AlpacaFocuser;
pub use telescope::AlpacaTelescope;
