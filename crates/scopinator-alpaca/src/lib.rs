//! An ASCOM Alpaca client: discovery, the raw property API, and typed
//! clients for telescopes (mounts), cameras, focusers, filter wheels and
//! switches.

pub mod camera;
pub mod client;
mod device;
pub mod discovery;
pub mod error;
pub mod filterwheel;
pub mod focuser;
pub mod image;
pub mod switch;
pub mod telescope;

pub use camera::AlpacaCamera;
pub use client::AlpacaClient;
pub use error::AlpacaError;
pub use filterwheel::AlpacaFilterWheel;
pub use focuser::AlpacaFocuser;
pub use switch::AlpacaSwitch;
pub use telescope::AlpacaTelescope;
