//! RTSP live video client for Seestar telescopes.
//!
//! Requires the `rtsp` feature flag. Connects to the Seestar's RTSP stream
//! at `rtsp://{host}:{4554 + camera_id}/stream`.
//!
//! This module provides raw H.264 packet access. Full frame decoding
//! (H.264 -> RGB) is left to the caller or a future feature flag.

use std::net::Ipv4Addr;

/// Base port for RTSP streams. Camera 0 uses 4554, camera 1 uses 4555, etc.
pub const RTSP_BASE_PORT: u16 = 4554;

/// Build the RTSP URL for a given host and camera ID.
pub fn rtsp_url(host: Ipv4Addr, camera_id: u16) -> String {
    let port = RTSP_BASE_PORT + camera_id;
    format!("rtsp://{host}:{port}/stream")
}

/// Expected resolution of the Seestar RTSP stream.
pub const RTSP_WIDTH: u32 = 1080;
pub const RTSP_HEIGHT: u32 = 1920;

// TODO: Full RTSP client implementation behind `rtsp` feature flag.
// Will use the `retina` crate for async RTSP/RTP with H.264.
// Design:
// - Background task reads RTSP stream
// - Latest frame stored in Arc<ArcSwap<Option<RtspFrame>>>
// - No buffering — always latest frame
// - Reconnect on stream drop (500ms backoff)
// - Triggered when telescope enters "Streaming" mode (RTSP event)

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtsp_url_camera_0() {
        let url = rtsp_url(Ipv4Addr::new(192, 168, 1, 100), 0);
        assert_eq!(url, "rtsp://192.168.1.100:4554/stream");
    }

    #[test]
    fn rtsp_url_camera_1() {
        let url = rtsp_url(Ipv4Addr::new(10, 0, 0, 1), 1);
        assert_eq!(url, "rtsp://10.0.0.1:4555/stream");
    }
}
