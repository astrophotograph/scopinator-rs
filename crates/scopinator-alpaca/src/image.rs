//! Fetching a camera's image in the Alpaca ImageBytes encoding.
//!
//! The JSON form of `imagearray` is a nested array of numbers, tens of
//! megabytes for one frame. A client that sends
//! `Accept: application/imagebytes` gets a binary reply instead: a 44-byte
//! header of little-endian 32-bit integers followed by the pixels.
//!
//! | Offset | Field |
//! |---|---|
//! | 0 | MetadataVersion (1) |
//! | 4 | ErrorNumber |
//! | 8 | ClientTransactionID |
//! | 12 | ServerTransactionID |
//! | 16 | DataStart: offset of the pixels (or of the error message) |
//! | 20 | ImageElementType: the type the camera works in |
//! | 24 | TransmissionElementType: the type sent, which may be narrower |
//! | 28 | Rank: 2 for one plane, 3 for colour planes |
//! | 32 | Dimension1: width |
//! | 36 | Dimension2: height |
//! | 40 | Dimension3: planes (0 when Rank is 2) |
//!
//! The pixels are an array indexed `[x][y]` (or `[x][y][plane]`) sent with
//! the last index changing fastest. That is column by column, not row by
//! row: the pixel at (x, y) is element `x * height + y`.
//! [`AlpacaImage::to_row_major_u16`] turns it into the usual order.

use crate::camera::AlpacaCamera;
use crate::error::AlpacaError;

/// The media type of the ImageBytes encoding.
pub const IMAGE_BYTES_MIME: &str = "application/imagebytes";

const HEADER_LEN: usize = 44;

/// The type of one array element (`ImageArrayElementTypes`). Only the types
/// a camera image is sent in are supported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementType {
    Int16,
    Int32,
    Byte,
    UInt16,
}

impl ElementType {
    fn from_number(number: i32) -> Option<Self> {
        match number {
            1 => Some(Self::Int16),
            2 => Some(Self::Int32),
            6 => Some(Self::Byte),
            8 => Some(Self::UInt16),
            _ => None,
        }
    }

    /// Bytes one element takes on the wire.
    pub fn size(self) -> usize {
        match self {
            Self::Byte => 1,
            Self::Int16 | Self::UInt16 => 2,
            Self::Int32 => 4,
        }
    }
}

/// A camera image as Alpaca sent it.
#[derive(Debug, Clone, PartialEq)]
pub struct AlpacaImage {
    /// Width in pixels (Dimension1).
    pub width: u32,
    /// Height in pixels (Dimension2).
    pub height: u32,
    /// 1 for a mono or Bayer image, 3 for colour planes.
    pub planes: u32,
    /// The type the elements were sent as.
    pub element_type: ElementType,
    /// The elements, little-endian, in Alpaca's `[x][y][plane]` order.
    pub data: Vec<u8>,
}

impl AlpacaImage {
    /// Parse an ImageBytes reply. A reply that carries an error becomes
    /// that error.
    pub fn parse(bytes: &[u8]) -> Result<Self, AlpacaError> {
        let bad = |what: &str| AlpacaError::Value(format!("ImageBytes: {what}"));
        if bytes.len() < HEADER_LEN {
            return Err(bad(&format!(
                "{} bytes is shorter than the {HEADER_LEN}-byte header",
                bytes.len()
            )));
        }
        let field = |offset: usize| {
            i32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ])
        };
        let version = field(0);
        if version != 1 {
            return Err(bad(&format!("unknown metadata version {version}")));
        }
        let data_start = usize::try_from(field(16))
            .ok()
            .filter(|start| (HEADER_LEN..=bytes.len()).contains(start))
            .ok_or_else(|| bad("data start is outside the reply"))?;
        let error_number = field(4);
        if error_number != 0 {
            let message = String::from_utf8_lossy(&bytes[data_start..]).into_owned();
            return Err(AlpacaError::from_reply(error_number, message));
        }

        let element_type = ElementType::from_number(field(24)).ok_or_else(|| {
            bad(&format!(
                "unsupported transmission element type {}",
                field(24)
            ))
        })?;
        let dimension = |offset: usize, name: &str| {
            u32::try_from(field(offset))
                .ok()
                .filter(|&size| size > 0)
                .ok_or_else(|| bad(&format!("{name} is {}", field(offset))))
        };
        let width = dimension(32, "width")?;
        let height = dimension(36, "height")?;
        let planes = match field(28) {
            2 => 1,
            3 => dimension(40, "plane count")?,
            rank => return Err(bad(&format!("rank {rank}"))),
        };

        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(planes as usize))
            .and_then(|n| n.checked_mul(element_type.size()))
            .ok_or_else(|| bad("dimensions are too large"))?;
        let data = &bytes[data_start..];
        if data.len() != expected {
            return Err(bad(&format!(
                "{width}x{height}x{planes} {element_type:?} needs {expected} bytes of data, got {}",
                data.len()
            )));
        }
        Ok(Self {
            width,
            height,
            planes,
            element_type,
            data: data.to_vec(),
        })
    }

    /// The element at Alpaca index `index`, widened.
    fn element(&self, index: usize) -> i64 {
        let at = index * self.element_type.size();
        let b = &self.data[at..];
        match self.element_type {
            ElementType::Byte => i64::from(b[0]),
            ElementType::Int16 => i64::from(i16::from_le_bytes([b[0], b[1]])),
            ElementType::UInt16 => i64::from(u16::from_le_bytes([b[0], b[1]])),
            ElementType::Int32 => i64::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        }
    }

    /// The pixels as 16-bit samples in the usual order: rows from the top
    /// down, each row left to right, the planes of a colour image
    /// interleaved per pixel.
    ///
    /// Fails if a value does not fit in 16 unsigned bits.
    pub fn to_row_major_u16(&self) -> Result<Vec<u16>, AlpacaError> {
        let (width, height, planes) = (
            self.width as usize,
            self.height as usize,
            self.planes as usize,
        );
        let mut pixels = vec![0u16; width * height * planes];
        for x in 0..width {
            for y in 0..height {
                for plane in 0..planes {
                    let value = self.element((x * height + y) * planes + plane);
                    pixels[(y * width + x) * planes + plane] =
                        u16::try_from(value).map_err(|_| {
                            AlpacaError::Value(format!(
                                "ImageBytes: pixel value {value} at ({x}, {y}) does not fit in 16 bits"
                            ))
                        })?;
                }
            }
        }
        Ok(pixels)
    }
}

impl AlpacaCamera {
    /// Fetch the image of the last exposure (`imagearray`) in the
    /// ImageBytes encoding.
    ///
    /// Fails if the server answers in JSON instead, which a server that
    /// does not know ImageBytes does.
    pub async fn image_bytes(&self) -> Result<AlpacaImage, AlpacaError> {
        let (content_type, body) = self
            .client
            .get_image_bytes("camera", self.device_number(), "imagearray")
            .await?;
        if !content_type
            .to_ascii_lowercase()
            .starts_with(IMAGE_BYTES_MIME)
        {
            return Err(AlpacaError::Value(format!(
                "asked for {IMAGE_BYTES_MIME}, the server sent `{content_type}`"
            )));
        }
        AlpacaImage::parse(&body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An ImageBytes reply built by hand: the header fields, then `data`.
    fn reply(
        error_number: i32,
        element_type: i32,
        transmission_type: i32,
        rank: i32,
        dimensions: [i32; 3],
        data: &[u8],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        for field in [
            1, // metadata version
            error_number,
            7,  // client transaction
            99, // server transaction
            44, // data start
            element_type,
            transmission_type,
            rank,
            dimensions[0],
            dimensions[1],
            dimensions[2],
        ] {
            out.extend_from_slice(&i32::to_le_bytes(field));
        }
        assert_eq!(out.len(), HEADER_LEN);
        out.extend_from_slice(data);
        out
    }

    fn u16s(values: &[u16]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    /// The image
    ///
    /// ```text
    ///   1 2 3
    ///   4 5 6
    /// ```
    ///
    /// is 3 wide and 2 high. Alpaca indexes it `[x][y]` and sends the last
    /// index fastest: column 0 (1, 4), column 1 (2, 5), column 2 (3, 6).
    #[test]
    fn a_3_by_2_image_is_transposed_into_rows() {
        let wire = u16s(&[1, 4, 2, 5, 3, 6]);
        let image = AlpacaImage::parse(&reply(0, 2, 8, 2, [3, 2, 0], &wire)).unwrap();
        assert_eq!((image.width, image.height, image.planes), (3, 2, 1));
        assert_eq!(image.element_type, ElementType::UInt16);
        assert_eq!(image.to_row_major_u16().unwrap(), [1, 2, 3, 4, 5, 6]);
    }

    /// Two pixels side by side, red-ish then blue-ish: 2 wide, 1 high, 3
    /// planes. Alpaca order `[x][y][plane]` happens to be pixel by pixel
    /// here; make it 1 wide and 2 high to see the planes travel with their
    /// pixel through the transposition.
    #[test]
    fn colour_planes_stay_with_their_pixel() {
        // 2 wide, 2 high. Pixel (x, y) has planes (10x + y) * 10 + plane.
        //   (0,0)=0,1,2    (1,0)=100,101,102
        //   (0,1)=10,11,12 (1,1)=110,111,112
        let wire = u16s(&[
            0, 1, 2, // x0 y0
            10, 11, 12, // x0 y1
            100, 101, 102, // x1 y0
            110, 111, 112, // x1 y1
        ]);
        let image = AlpacaImage::parse(&reply(0, 2, 8, 3, [2, 2, 3], &wire)).unwrap();
        assert_eq!(image.planes, 3);
        assert_eq!(
            image.to_row_major_u16().unwrap(),
            [0, 1, 2, 100, 101, 102, 10, 11, 12, 110, 111, 112]
        );
    }

    #[test]
    fn bytes_and_int16_and_int32_elements_are_widened() {
        // 2 wide, 1 high: no transposition to think about.
        let bytes = AlpacaImage::parse(&reply(0, 2, 6, 2, [2, 1, 0], &[7, 255])).unwrap();
        assert_eq!(bytes.to_row_major_u16().unwrap(), [7, 255]);

        let int16: Vec<u8> = [300i16, 32767]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let image = AlpacaImage::parse(&reply(0, 2, 1, 2, [2, 1, 0], &int16)).unwrap();
        assert_eq!(image.to_row_major_u16().unwrap(), [300, 32767]);

        let int32: Vec<u8> = [0i32, 65535].iter().flat_map(|v| v.to_le_bytes()).collect();
        let image = AlpacaImage::parse(&reply(0, 2, 2, 2, [2, 1, 0], &int32)).unwrap();
        assert_eq!(image.to_row_major_u16().unwrap(), [0, 65535]);
    }

    #[test]
    fn a_value_that_does_not_fit_in_16_bits_is_an_error() {
        let too_big: Vec<u8> = [65536i32, 0].iter().flat_map(|v| v.to_le_bytes()).collect();
        let image = AlpacaImage::parse(&reply(0, 2, 2, 2, [2, 1, 0], &too_big)).unwrap();
        assert!(image.to_row_major_u16().is_err());

        let negative: Vec<u8> = [-1i16, 0].iter().flat_map(|v| v.to_le_bytes()).collect();
        let image = AlpacaImage::parse(&reply(0, 2, 1, 2, [2, 1, 0], &negative)).unwrap();
        assert!(image.to_row_major_u16().is_err());
    }

    #[test]
    fn an_error_reply_carries_its_number_and_message() {
        let wire = reply(0x40B, 0, 0, 0, [0, 0, 0], "no image available".as_bytes());
        match AlpacaImage::parse(&wire) {
            Err(AlpacaError::Device { code, message }) => {
                assert_eq!(code, 0x40B);
                assert_eq!(message, "no image available");
            }
            other => panic!("got {other:?}"),
        }
        // Not implemented keeps its own kind.
        let wire = reply(0x400, 0, 0, 0, [0, 0, 0], b"nope");
        assert!(AlpacaImage::parse(&wire).unwrap_err().is_not_implemented());
    }

    #[test]
    fn malformed_replies_are_errors() {
        // Too short for a header.
        assert!(AlpacaImage::parse(&[0; 43]).is_err());
        // Data shorter and longer than the dimensions call for.
        assert!(AlpacaImage::parse(&reply(0, 2, 8, 2, [3, 2, 0], &u16s(&[1, 2, 3]))).is_err());
        assert!(AlpacaImage::parse(&reply(0, 2, 8, 2, [1, 1, 0], &u16s(&[1, 2]))).is_err());
        // Element types and ranks this client does not read.
        assert!(AlpacaImage::parse(&reply(0, 3, 3, 2, [1, 1, 0], &[0; 8])).is_err());
        assert!(AlpacaImage::parse(&reply(0, 2, 8, 1, [2, 0, 0], &u16s(&[1, 2]))).is_err());
        // Zero and negative dimensions.
        assert!(AlpacaImage::parse(&reply(0, 2, 8, 2, [0, 2, 0], &[])).is_err());
        assert!(AlpacaImage::parse(&reply(0, 2, 8, 2, [-1, 2, 0], &[])).is_err());
        // A data start that points outside the reply.
        let mut wire = reply(0, 2, 8, 2, [1, 1, 0], &u16s(&[1]));
        wire[16..20].copy_from_slice(&1000i32.to_le_bytes());
        assert!(AlpacaImage::parse(&wire).is_err());
        // An unknown metadata version.
        let mut wire = reply(0, 2, 8, 2, [1, 1, 0], &u16s(&[1]));
        wire[0..4].copy_from_slice(&2i32.to_le_bytes());
        assert!(AlpacaImage::parse(&wire).is_err());
    }

    #[test]
    fn a_data_start_past_the_header_is_honoured() {
        // Four bytes of padding between the header and the pixels.
        let mut wire = reply(0, 2, 8, 2, [1, 1, 0], &[0xAA, 0xBB, 0xCC, 0xDD]);
        wire.extend_from_slice(&u16s(&[4242]));
        wire[16..20].copy_from_slice(&48i32.to_le_bytes());
        let image = AlpacaImage::parse(&wire).unwrap();
        assert_eq!(image.to_row_major_u16().unwrap(), [4242]);
    }

    proptest::proptest! {
        #[test]
        fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..200)) {
            if let Ok(image) = AlpacaImage::parse(&bytes) {
                let _ = image.to_row_major_u16();
            }
        }

        #[test]
        fn arbitrary_headers_never_panic(
            fields in proptest::collection::vec(-3i32..70_000, 11),
            tail in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..64),
        ) {
            let mut bytes: Vec<u8> = fields.iter().flat_map(|f| f.to_le_bytes()).collect();
            bytes.extend_from_slice(&tail);
            if let Ok(image) = AlpacaImage::parse(&bytes) {
                let _ = image.to_row_major_u16();
            }
        }
    }

    #[tokio::test]
    async fn a_json_reply_to_an_imagebytes_request_is_an_error() {
        use crate::client::tests::serve;
        use std::net::Ipv4Addr;
        use std::sync::Arc;

        let body = r#"{"Type": 2, "Rank": 2, "Value": [[1]], "ErrorNumber": 0}"#;
        let (addr, handle) = serve("200 OK", "application/json", body, 1).await;
        let client = Arc::new(crate::AlpacaClient::new(Ipv4Addr::LOCALHOST, addr.port()));
        let camera = AlpacaCamera::new(client, 0);

        let err = camera.image_bytes().await.unwrap_err();
        assert!(matches!(err, AlpacaError::Value(_)), "{err:?}");

        // The request asked for the binary encoding.
        let request = handle.await.unwrap().pop().unwrap().to_ascii_lowercase();
        assert!(
            request.contains("accept: application/imagebytes"),
            "{request}"
        );
        assert!(
            request.starts_with("get /api/v1/camera/0/imagearray?"),
            "{request}"
        );
    }
}
