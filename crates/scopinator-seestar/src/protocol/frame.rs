/// 80-byte binary frame header (big-endian).
///
/// Format: `>HHHIHHBBHH` (first 20 bytes) + 60 bytes padding.
///
/// Key fields:
/// - `size` (offset 6-9): payload length in bytes
/// - `code` (offset 14): control code
/// - `id` (offset 15): frame type
/// - `width`/`height` (offsets 16-19): image dimensions
pub const HEADER_SIZE: usize = 80;

/// Maximum payload size we'll accept (50 MB).
pub const MAX_PAYLOAD_SIZE: u32 = 50 * 1024 * 1024;

/// Frame type IDs.
pub mod frame_id {
    /// Handshake frame ("server connected!").
    pub const HANDSHAKE: u8 = 2;
    /// View / live preview frame.
    pub const VIEW: u8 = 20;
    /// Streaming preview frame.
    pub const PREVIEW: u8 = 21;
    /// Stacked image (ZIP archive containing "raw_data").
    pub const STACK: u8 = 23;
}

/// Parsed binary frame header from the imaging port (4800).
#[derive(Debug, Clone)]
pub struct FrameHeader {
    /// Payload size in bytes.
    pub size: u32,
    /// Control code.
    pub code: u8,
    /// Frame type ID (see [`frame_id`]).
    pub id: u8,
    /// Image width in pixels.
    pub width: u16,
    /// Image height in pixels.
    pub height: u16,
}

impl FrameHeader {
    /// Parse a frame header from exactly 80 bytes.
    pub fn parse(buf: &[u8; HEADER_SIZE]) -> Self {
        Self {
            size: u32::from_be_bytes([buf[6], buf[7], buf[8], buf[9]]),
            code: buf[14],
            id: buf[15],
            width: u16::from_be_bytes([buf[16], buf[17]]),
            height: u16::from_be_bytes([buf[18], buf[19]]),
        }
    }

    /// Returns true if this looks like a real image frame (not a handshake).
    pub fn is_image(&self) -> bool {
        self.width > 0 && self.height > 0 && self.size > 1000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_handshake_header() {
        let mut buf = [0u8; HEADER_SIZE];
        buf[6..10].copy_from_slice(&17u32.to_be_bytes());
        buf[15] = frame_id::HANDSHAKE;

        let header = FrameHeader::parse(&buf);
        assert_eq!(header.size, 17);
        assert_eq!(header.id, frame_id::HANDSHAKE);
        assert_eq!(header.width, 0);
        assert_eq!(header.height, 0);
        assert!(!header.is_image());
    }

    #[test]
    fn parse_preview_header() {
        let mut buf = [0u8; HEADER_SIZE];
        buf[6..10].copy_from_slice(&1_833_419u32.to_be_bytes());
        buf[14] = 3;
        buf[15] = frame_id::VIEW;
        buf[16..18].copy_from_slice(&1080u16.to_be_bytes());
        buf[18..20].copy_from_slice(&1920u16.to_be_bytes());

        let header = FrameHeader::parse(&buf);
        assert_eq!(header.size, 1_833_419);
        assert_eq!(header.id, frame_id::VIEW);
        assert_eq!(header.width, 1080);
        assert_eq!(header.height, 1920);
        assert!(header.is_image());
    }

    #[test]
    fn parse_stacked_image_header() {
        let mut buf = [0u8; HEADER_SIZE];
        buf[6..10].copy_from_slice(&5_000_000u32.to_be_bytes());
        buf[15] = frame_id::STACK;
        buf[16..18].copy_from_slice(&4056u16.to_be_bytes());
        buf[18..20].copy_from_slice(&3040u16.to_be_bytes());

        let header = FrameHeader::parse(&buf);
        assert_eq!(header.id, frame_id::STACK);
        assert!(header.is_image());
    }

    #[test]
    fn is_image_requires_nonzero_dimensions() {
        let mut buf = [0u8; HEADER_SIZE];
        buf[6..10].copy_from_slice(&2_000_000u32.to_be_bytes());

        // zero width
        buf[16..18].copy_from_slice(&0u16.to_be_bytes());
        buf[18..20].copy_from_slice(&1080u16.to_be_bytes());
        assert!(!FrameHeader::parse(&buf).is_image());

        // zero height
        buf[16..18].copy_from_slice(&1920u16.to_be_bytes());
        buf[18..20].copy_from_slice(&0u16.to_be_bytes());
        assert!(!FrameHeader::parse(&buf).is_image());
    }

    #[test]
    fn is_image_size_boundary() {
        let mut buf = [0u8; HEADER_SIZE];
        buf[16..18].copy_from_slice(&100u16.to_be_bytes());
        buf[18..20].copy_from_slice(&100u16.to_be_bytes());

        buf[6..10].copy_from_slice(&1000u32.to_be_bytes());
        assert!(!FrameHeader::parse(&buf).is_image());

        buf[6..10].copy_from_slice(&1001u32.to_be_bytes());
        assert!(FrameHeader::parse(&buf).is_image());
    }
}
