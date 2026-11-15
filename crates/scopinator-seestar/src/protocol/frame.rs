//! The 80-byte header that precedes every reply on the image sockets (4800,
//! 4804) and the file socket (4801). All multi-byte fields are big-endian.
//!
//! | offset | size | field |
//! |---:|---:|---|
//! | 0 | 2 | magic `0x03C3` |
//! | 2 | 2 | version (2) |
//! | 4 | 2 | header length (80) |
//! | 6 | 4 | payload length |
//! | 13 | 1 | imgType: pixel format (see [`FrameHeader::img_type`]) |
//! | 14 | 1 | dataType: text ok / text error / binary / zipped binary |
//! | 15 | 1 | low byte of the `id` of the request this answers |
//! | 16, 18 | 2, 2 | width, height in pixels |
//! | 20, 22 | 2, 2 | hfdX, hfdY |
//! | 24 | 2 | hfd × 100 |
//! | 26 | 2 | canDebayer flag |
//! | 28 | 2 | image id (signed; -1 when unset) |
//!
//! Layout from zmerlynn/seestar-spec (`spec/media/image-sockets.md`, "Reply
//! framing"). The magic, version, header length and the meaning of byte 15
//! were confirmed on an S50, firmware 6.70: a `test_connection` with id 77
//! came back with byte 15 = 77, and id 300 with 44.
//!
//! Byte 15 is **not** a frame type. What a frame contains is given by
//! `dataType` and `imgType` ([`FrameHeader::payload_kind`]); which request it
//! answers is given by byte 15.

/// Size of the frame header in bytes.
pub const HEADER_SIZE: usize = 80;

/// Maximum payload size we'll accept (50 MB).
pub const MAX_PAYLOAD_SIZE: u32 = 50 * 1024 * 1024;

/// The magic number that opens every frame header.
pub const MAGIC: u16 = 0x03C3;

/// `imgType` value for a JPEG payload.
pub const IMG_TYPE_JPEG: u8 = 4;

/// What the payload of a frame is, from the header's `dataType` and `imgType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PayloadKind {
    /// A successful text reply, e.g. `server connected!` or `done`.
    Text,
    /// An error message, e.g. `there is no image now`. It is the reply to a
    /// request that failed, not image data.
    ErrorText,
    /// Uncompressed pixel data; the layout is given by `imgType`.
    Raw,
    /// A zip archive holding the pixel data, to be inflated before use.
    Zip,
    /// A JPEG image.
    Jpeg,
    /// A `dataType` this crate does not know.
    Unknown,
}

impl PayloadKind {
    /// True for payloads that carry an image (raw, zipped or JPEG).
    pub fn is_image(self) -> bool {
        matches!(self, Self::Raw | Self::Zip | Self::Jpeg)
    }
}

/// Parsed frame header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameHeader {
    /// Magic number; [`MAGIC`] on a well-formed frame.
    pub magic: u16,
    /// Header format version (2).
    pub version: u16,
    /// Header length as stated by the device (80).
    pub header_len: u16,
    /// Payload size in bytes.
    pub size: u32,
    /// Pixel format: 0 = 1 byte/pixel, 1 = 2 bytes/pixel, 2 and 3 = 3
    /// bytes/pixel, 4 = JPEG, 5 = 6 bytes/pixel, 6 = binary file. Text
    /// replies use 0.
    pub img_type: u8,
    /// 0 = text, 1 = error text, 2 = binary, 3 = zip-compressed binary.
    pub data_type: u8,
    /// Low byte of the `id` of the request this frame answers. Pushed stream
    /// frames repeat the id of the `begin_streaming` request.
    pub request_id: u8,
    /// Image width in pixels.
    pub width: u16,
    /// Image height in pixels.
    pub height: u16,
    /// Named hfdX by the app; meaning not established.
    pub hfd_x: u16,
    /// Named hfdY by the app; meaning not established.
    pub hfd_y: u16,
    /// Star half-flux diameter × 100 where the device fills it in (default 1000).
    pub hfd: u16,
    /// Debayer flag: non-zero when the pixel data is a Bayer mosaic.
    pub can_debayer: u16,
    /// Image id; -1 when not set.
    pub image_id: i16,
}

impl FrameHeader {
    /// Parse a frame header from exactly 80 bytes.
    ///
    /// This never fails; check [`has_valid_magic`](Self::has_valid_magic)
    /// before trusting `size`.
    pub fn parse(buf: &[u8; HEADER_SIZE]) -> Self {
        let u16_at = |i: usize| u16::from_be_bytes([buf[i], buf[i + 1]]);
        Self {
            magic: u16_at(0),
            version: u16_at(2),
            header_len: u16_at(4),
            size: u32::from_be_bytes([buf[6], buf[7], buf[8], buf[9]]),
            img_type: buf[13],
            data_type: buf[14],
            request_id: buf[15],
            width: u16_at(16),
            height: u16_at(18),
            hfd_x: u16_at(20),
            hfd_y: u16_at(22),
            hfd: u16_at(24),
            can_debayer: u16_at(26),
            image_id: i16::from_be_bytes([buf[28], buf[29]]),
        }
    }

    /// True if the header starts with the frame magic. A header without it
    /// means the stream is out of step and the connection must be dropped.
    pub fn has_valid_magic(&self) -> bool {
        self.magic == MAGIC
    }

    /// What the payload is.
    pub fn payload_kind(&self) -> PayloadKind {
        match self.data_type {
            0 => PayloadKind::Text,
            1 => PayloadKind::ErrorText,
            2 | 3 if self.img_type == IMG_TYPE_JPEG => PayloadKind::Jpeg,
            2 => PayloadKind::Raw,
            3 => PayloadKind::Zip,
            _ => PayloadKind::Unknown,
        }
    }

    /// Bytes per pixel of raw pixel data, from `imgType`. `None` for JPEG,
    /// binary files and unknown types.
    pub fn bytes_per_pixel(&self) -> Option<u8> {
        match self.img_type {
            0 => Some(1),
            1 => Some(2),
            2 | 3 => Some(3),
            5 => Some(6),
            _ => None,
        }
    }

    /// Returns true if this frame carries an image (not a text reply).
    pub fn is_image(&self) -> bool {
        self.payload_kind().is_image() && self.width > 0 && self.height > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(f: impl FnOnce(&mut [u8; HEADER_SIZE])) -> FrameHeader {
        let mut buf = [0u8; HEADER_SIZE];
        buf[0..2].copy_from_slice(&MAGIC.to_be_bytes());
        buf[2..4].copy_from_slice(&2u16.to_be_bytes());
        buf[4..6].copy_from_slice(&80u16.to_be_bytes());
        f(&mut buf);
        FrameHeader::parse(&buf)
    }

    // The header an S50 on firmware 6.70 sent for `test_connection` with
    // request id 300 (2026-11-14): byte 15 is the low byte of the id.
    #[test]
    fn parse_text_reply_observed_on_hardware() {
        let h = header(|b| {
            b[6..10].copy_from_slice(&17u32.to_be_bytes());
            b[15] = (300u32 & 0xff) as u8;
        });
        assert!(h.has_valid_magic());
        assert_eq!((h.version, h.header_len), (2, 80));
        assert_eq!(h.size, 17);
        assert_eq!(h.request_id, 44);
        assert_eq!(h.payload_kind(), PayloadKind::Text);
        assert!(!h.is_image());
    }

    #[test]
    fn payload_kind_comes_from_data_type_and_img_type_only() {
        let kind = |data_type: u8, img_type: u8, id: u8| {
            header(|b| {
                b[13] = img_type;
                b[14] = data_type;
                b[15] = id;
            })
            .payload_kind()
        };
        // The request id byte never influences the kind.
        for id in [0, 2, 20, 21, 23, 99, 255] {
            assert_eq!(kind(0, 0, id), PayloadKind::Text);
            assert_eq!(kind(1, 0, id), PayloadKind::ErrorText);
            assert_eq!(kind(2, 1, id), PayloadKind::Raw);
            assert_eq!(kind(3, 1, id), PayloadKind::Zip);
            assert_eq!(kind(2, IMG_TYPE_JPEG, id), PayloadKind::Jpeg);
            assert_eq!(kind(7, 1, id), PayloadKind::Unknown);
        }
    }

    #[test]
    fn is_image_needs_an_image_payload_and_dimensions() {
        let h = |data_type: u8, w: u16, hgt: u16| {
            header(|b| {
                b[13] = 1;
                b[14] = data_type;
                b[16..18].copy_from_slice(&w.to_be_bytes());
                b[18..20].copy_from_slice(&hgt.to_be_bytes());
            })
        };
        assert!(h(2, 1080, 1920).is_image());
        assert!(h(3, 1080, 1920).is_image());
        assert!(!h(2, 0, 1920).is_image());
        assert!(!h(2, 1080, 0).is_image());
        // An error text is never an image, whatever the dimensions say.
        assert!(!h(1, 1080, 1920).is_image());
        assert!(!h(0, 1080, 1920).is_image());
    }

    #[test]
    fn bytes_per_pixel_follows_img_type() {
        let bpp = |img_type: u8| header(|b| b[13] = img_type).bytes_per_pixel();
        assert_eq!(bpp(0), Some(1));
        assert_eq!(bpp(1), Some(2));
        assert_eq!(bpp(2), Some(3));
        assert_eq!(bpp(3), Some(3));
        assert_eq!(bpp(IMG_TYPE_JPEG), None);
        assert_eq!(bpp(5), Some(6));
        assert_eq!(bpp(6), None);
    }

    #[test]
    fn trailing_fields_are_parsed() {
        let h = header(|b| {
            b[20..22].copy_from_slice(&11u16.to_be_bytes());
            b[22..24].copy_from_slice(&22u16.to_be_bytes());
            b[24..26].copy_from_slice(&1000u16.to_be_bytes());
            b[26..28].copy_from_slice(&1u16.to_be_bytes());
            b[28..30].copy_from_slice(&(-1i16).to_be_bytes());
        });
        assert_eq!((h.hfd_x, h.hfd_y, h.hfd), (11, 22, 1000));
        assert_eq!(h.can_debayer, 1);
        assert_eq!(h.image_id, -1);
    }

    #[test]
    fn missing_magic_is_detected() {
        assert!(!FrameHeader::parse(&[0u8; HEADER_SIZE]).has_valid_magic());
        assert!(!FrameHeader::parse(&[0xffu8; HEADER_SIZE]).has_valid_magic());
    }

    /// Shared vectors in `conformance/vectors/frame_headers.json`; the Python
    /// check (`conformance/parity/check_frame_vectors.py`) reads the same file
    /// with `struct.unpack`.
    #[test]
    fn shared_frame_header_vectors() {
        let raw = include_str!("../../../../conformance/vectors/frame_headers.json");
        let doc: serde_json::Value = serde_json::from_str(raw).unwrap();
        let vectors = doc["vectors"].as_array().unwrap();
        assert!(!vectors.is_empty());
        let mut kinds = std::collections::BTreeSet::new();
        for v in vectors {
            let name = v["name"].as_str().unwrap();
            let hex = v["header_hex"].as_str().unwrap();
            assert_eq!(hex.len(), HEADER_SIZE * 2, "{name}: bad hex length");
            let mut buf = [0u8; HEADER_SIZE];
            for (i, b) in buf.iter_mut().enumerate() {
                *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
            }
            let h = FrameHeader::parse(&buf);
            let kind = match h.payload_kind() {
                PayloadKind::Text => "text",
                PayloadKind::ErrorText => "error_text",
                PayloadKind::Raw => "raw",
                PayloadKind::Zip => "zip",
                PayloadKind::Jpeg => "jpeg",
                PayloadKind::Unknown => "unknown",
            };
            kinds.insert(kind);
            let got = serde_json::json!({
                "magic": h.magic,
                "version": h.version,
                "header_len": h.header_len,
                "size": h.size,
                "img_type": h.img_type,
                "data_type": h.data_type,
                "request_id": h.request_id,
                "width": h.width,
                "height": h.height,
                "hfd_x": h.hfd_x,
                "hfd_y": h.hfd_y,
                "hfd": h.hfd,
                "can_debayer": h.can_debayer,
                "image_id": h.image_id,
                "valid_magic": h.has_valid_magic(),
                "payload_kind": kind,
            });
            assert_eq!(got, v["expected"], "{name}");
        }
        for kind in ["text", "error_text", "raw", "zip", "jpeg"] {
            assert!(kinds.contains(kind), "no vector with a {kind} payload");
        }
    }

    proptest::proptest! {
        #[test]
        fn parse_never_panics(buf in proptest::collection::vec(proptest::prelude::any::<u8>(), HEADER_SIZE..=HEADER_SIZE)) {
            let mut arr = [0u8; HEADER_SIZE];
            arr.copy_from_slice(&buf);
            let h = FrameHeader::parse(&arr);
            // Every accessor must be total.
            let _ = (h.is_image(), h.payload_kind(), h.bytes_per_pixel(), h.has_valid_magic());
        }

        #[test]
        fn fields_match_byte_offsets(
            size in proptest::prelude::any::<u32>(),
            img_type in proptest::prelude::any::<u8>(),
            data_type in proptest::prelude::any::<u8>(),
            request_id in proptest::prelude::any::<u8>(),
            width in proptest::prelude::any::<u16>(),
            height in proptest::prelude::any::<u16>(),
            image_id in proptest::prelude::any::<i16>(),
            noise in proptest::collection::vec(proptest::prelude::any::<u8>(), HEADER_SIZE..=HEADER_SIZE),
        ) {
            // Start with fuzz noise to ensure unread bytes don't bleed into fields.
            let mut buf = [0u8; HEADER_SIZE];
            buf.copy_from_slice(&noise);
            buf[6..10].copy_from_slice(&size.to_be_bytes());
            buf[13] = img_type;
            buf[14] = data_type;
            buf[15] = request_id;
            buf[16..18].copy_from_slice(&width.to_be_bytes());
            buf[18..20].copy_from_slice(&height.to_be_bytes());
            buf[28..30].copy_from_slice(&image_id.to_be_bytes());

            let h = FrameHeader::parse(&buf);
            proptest::prop_assert_eq!(h.size, size);
            proptest::prop_assert_eq!(h.img_type, img_type);
            proptest::prop_assert_eq!(h.data_type, data_type);
            proptest::prop_assert_eq!(h.request_id, request_id);
            proptest::prop_assert_eq!(h.width, width);
            proptest::prop_assert_eq!(h.height, height);
            proptest::prop_assert_eq!(h.image_id, image_id);
        }
    }
}
