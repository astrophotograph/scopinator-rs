//! Reading the FITS files INDI cameras send.
//!
//! Only what a camera driver writes: one image in the primary HDU, 8 or 16
//! bits a pixel, one plane or three. Not a general FITS reader.

use crate::error::IndiError;

const BLOCK: usize = 2880;
const CARD: usize = 80;
/// More header than any camera writes; past this the file is not one.
const MAX_CARDS: usize = 4096;

/// An image out of a FITS file.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct FitsImage {
    pub width: u32,
    pub height: u32,
    /// 8 or 16.
    pub bits_per_pixel: u8,
    /// 1 for a mono or Bayer image, 3 for colour. The planes follow one
    /// another: all of the first, then all of the second.
    pub planes: u8,
    /// `BAYERPAT`, for example `RGGB`.
    pub bayer: Option<String>,
    /// The samples in file order: bytes for 8 bits, unsigned little-endian
    /// pairs for 16.
    pub pixels: Vec<u8>,
    /// `EXPTIME`, in seconds.
    pub exposure_s: Option<f64>,
    /// `GAIN`.
    pub gain: Option<f64>,
    /// `CCD-TEMP`, in degrees Celsius.
    pub temperature_c: Option<f64>,
}

fn bad(what: impl Into<String>) -> IndiError {
    IndiError::Blob(format!("FITS: {}", what.into()))
}

/// The header as keyword and value text, and where the data starts.
fn header(bytes: &[u8]) -> Result<(Vec<(String, String)>, usize), IndiError> {
    let mut cards = Vec::new();
    for (index, card) in bytes.as_chunks::<CARD>().0.iter().enumerate() {
        if index >= MAX_CARDS {
            return Err(bad("the header does not end"));
        }
        let keyword = String::from_utf8_lossy(&card[..8]).trim_end().to_string();
        if keyword == "END" {
            let header_len = (index + 1) * CARD;
            return Ok((cards, header_len.div_ceil(BLOCK) * BLOCK));
        }
        if &card[8..10] != b"= " {
            // A comment, HISTORY or a blank card.
            continue;
        }
        let text = String::from_utf8_lossy(&card[10..]);
        let text = text.trim_start();
        let value = if let Some(quoted) = text.strip_prefix('\'') {
            // A string: up to the closing quote, where '' is a quote.
            let mut value = String::new();
            let mut chars = quoted.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '\'' {
                    if chars.peek() == Some(&'\'') {
                        chars.next();
                        value.push('\'');
                    } else {
                        break;
                    }
                } else {
                    value.push(c);
                }
            }
            value.trim_end().to_string()
        } else {
            text.split('/').next().unwrap_or("").trim().to_string()
        };
        cards.push((keyword, value));
    }
    Err(bad("no END card"))
}

/// Read the image in a FITS file.
pub fn decode(bytes: &[u8]) -> Result<FitsImage, IndiError> {
    let (cards, data_start) = header(bytes)?;
    let text = |keyword: &str| {
        cards
            .iter()
            .find(|(name, _)| name == keyword)
            .map(|(_, value)| value.as_str())
    };
    let integer = |keyword: &str| -> Result<i64, IndiError> {
        let value = text(keyword).ok_or_else(|| bad(format!("no {keyword}")))?;
        value
            .parse::<i64>()
            .map_err(|_| bad(format!("{keyword} is {value:?}")))
    };
    let number = |keyword: &str| text(keyword).and_then(|value| value.parse::<f64>().ok());

    if text("SIMPLE") != Some("T") {
        return Err(bad("not a standard file (SIMPLE is not T)"));
    }
    let bitpix = integer("BITPIX")?;
    let bits_per_pixel: u8 = match bitpix {
        8 => 8,
        16 => 16,
        other => return Err(bad(format!("BITPIX {other} is not 8 or 16"))),
    };
    let axes = integer("NAXIS")?;
    let dimension = |keyword: &str| -> Result<u32, IndiError> {
        let value = integer(keyword)?;
        u32::try_from(value)
            .ok()
            .filter(|size| *size > 0)
            .ok_or_else(|| bad(format!("{keyword} is {value}")))
    };
    let planes: u8 = match axes {
        2 => 1,
        3 => match integer("NAXIS3")? {
            3 => 3,
            // A cube one plane deep is an image.
            1 => 1,
            other => return Err(bad(format!("NAXIS3 is {other}, not 3"))),
        },
        other => return Err(bad(format!("NAXIS {other} is not 2 or 3"))),
    };
    let (width, height) = (dimension("NAXIS1")?, dimension("NAXIS2")?);

    let bzero = number("BZERO").unwrap_or(0.0);
    let bscale = number("BSCALE").unwrap_or(1.0);
    let scaling_ok = match bits_per_pixel {
        8 => bzero == 0.0 && bscale == 1.0,
        // Unsigned 16-bit data, written as signed with an offset.
        _ => bzero == 32768.0 && bscale == 1.0,
    };
    if !scaling_ok {
        return Err(bad(format!(
            "BZERO {bzero} and BSCALE {bscale} with BITPIX {bitpix}"
        )));
    }

    let length = (width as usize)
        .checked_mul(height as usize)
        .and_then(|samples| samples.checked_mul(planes as usize))
        .and_then(|samples| samples.checked_mul(usize::from(bits_per_pixel / 8)))
        .ok_or_else(|| bad("the image is too large to address"))?;
    let end = data_start
        .checked_add(length)
        .ok_or_else(|| bad("the image is too large to address"))?;
    let data = bytes.get(data_start..end).ok_or_else(|| {
        bad(format!(
            "{width}x{height}x{planes} at {bits_per_pixel} bits needs {length} bytes of data, the file has {}",
            bytes.len().saturating_sub(data_start)
        ))
    })?;

    let pixels = if bits_per_pixel == 8 {
        data.to_vec()
    } else {
        // Big-endian signed plus 32768 is the unsigned value: flip the top
        // bit, and swap to little-endian.
        let mut pixels = Vec::with_capacity(length);
        for [high, low] in data.as_chunks::<2>().0 {
            pixels.push(*low);
            pixels.push(high ^ 0x80);
        }
        pixels
    };

    Ok(FitsImage {
        width,
        height,
        bits_per_pixel,
        planes,
        bayer: text("BAYERPAT")
            .map(|pattern| pattern.trim().to_string())
            .filter(|pattern| !pattern.is_empty()),
        pixels,
        exposure_s: number("EXPTIME"),
        gain: number("GAIN"),
        temperature_c: number("CCD-TEMP"),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A FITS file with these header cards and this data, padded as the
    /// standard says.
    pub(crate) fn fits(cards: &[(&str, &str)], data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for (keyword, value) in cards {
            let card = format!("{keyword:<8}= {value:>20}");
            bytes.extend_from_slice(format!("{card:<80}").as_bytes());
        }
        bytes.extend_from_slice(format!("{:<80}", "END").as_bytes());
        bytes.resize(bytes.len().div_ceil(BLOCK) * BLOCK, b' ');
        bytes.extend_from_slice(data);
        bytes.resize(bytes.len().div_ceil(BLOCK) * BLOCK, 0);
        bytes
    }

    fn mono16(values: &[u16], width: u32, height: u32, extra: &[(&str, &str)]) -> Vec<u8> {
        let (w, h) = (width.to_string(), height.to_string());
        let mut cards = vec![
            ("SIMPLE", "T"),
            ("BITPIX", "16"),
            ("NAXIS", "2"),
            ("NAXIS1", w.as_str()),
            ("NAXIS2", h.as_str()),
            ("BZERO", "32768"),
            ("BSCALE", "1"),
        ];
        cards.extend_from_slice(extra);
        let data: Vec<u8> = values
            .iter()
            .flat_map(|value| ((i32::from(*value) - 32768) as i16).to_be_bytes())
            .collect();
        fits(&cards, &data)
    }

    #[test]
    fn an_eight_bit_image_is_its_bytes() {
        let data: Vec<u8> = (0..12).collect();
        let file = fits(
            &[
                ("SIMPLE", "T"),
                ("BITPIX", "8"),
                ("NAXIS", "2"),
                ("NAXIS1", "4"),
                ("NAXIS2", "3"),
            ],
            &data,
        );
        assert_eq!(file.len() % BLOCK, 0);
        let image = decode(&file).unwrap();
        assert_eq!((image.width, image.height), (4, 3));
        assert_eq!((image.bits_per_pixel, image.planes), (8, 1));
        assert_eq!(image.pixels, data);
        assert_eq!(image.bayer, None);
        assert_eq!(image.exposure_s, None);
    }

    #[test]
    fn sixteen_bit_values_come_out_unsigned_and_little_endian() {
        let values = [0u16, 1, 255, 256, 32767, 32768, 40000, 65535, 2, 3, 4, 5];
        let image = decode(&mono16(&values, 4, 3, &[])).unwrap();
        assert_eq!(
            (image.width, image.height, image.bits_per_pixel),
            (4, 3, 16)
        );
        let got: Vec<u16> = image
            .pixels
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        assert_eq!(got, values);
    }

    #[test]
    fn the_keywords_a_camera_writes_are_read() {
        let file = mono16(
            &[1, 2, 3, 4],
            2,
            2,
            &[
                ("BAYERPAT", "'RGGB    '"),
                ("EXPTIME", "2.5 / seconds"),
                ("GAIN", "120"),
                ("CCD-TEMP", "-9.75"),
                ("OBJECT", "'O''Neill''s star'"),
            ],
        );
        let image = decode(&file).unwrap();
        assert_eq!(image.bayer.as_deref(), Some("RGGB"));
        assert_eq!(image.exposure_s, Some(2.5));
        assert_eq!(image.gain, Some(120.0));
        assert_eq!(image.temperature_c, Some(-9.75));
    }

    #[test]
    fn a_colour_image_has_three_planes_one_after_another() {
        let data: Vec<u8> = (0..24).collect();
        let file = fits(
            &[
                ("SIMPLE", "T"),
                ("BITPIX", "8"),
                ("NAXIS", "3"),
                ("NAXIS1", "4"),
                ("NAXIS2", "2"),
                ("NAXIS3", "3"),
            ],
            &data,
        );
        let image = decode(&file).unwrap();
        assert_eq!((image.width, image.height, image.planes), (4, 2, 3));
        assert_eq!(image.pixels, data);
    }

    #[test]
    fn what_is_wrong_with_a_file_is_said() {
        let base = [
            ("SIMPLE", "T"),
            ("BITPIX", "8"),
            ("NAXIS", "2"),
            ("NAXIS1", "4"),
            ("NAXIS2", "3"),
        ];
        let with = |keyword: &str, value: &'static str| {
            let mut cards = base.to_vec();
            match cards.iter_mut().find(|(name, _)| *name == keyword) {
                Some(card) => card.1 = value,
                None => cards.push((
                    match keyword {
                        "BZERO" => "BZERO",
                        "NAXIS3" => "NAXIS3",
                        _ => "OTHER",
                    },
                    value,
                )),
            }
            fits(&cards, &[0u8; 96])
        };
        let failure = |file: &[u8]| decode(file).unwrap_err().to_string();

        assert!(failure(&with("BITPIX", "32")).contains("BITPIX 32"));
        assert!(failure(&with("BITPIX", "-32")).contains("BITPIX -32"));
        assert!(failure(&with("NAXIS", "1")).contains("NAXIS 1"));
        assert!(failure(&with("NAXIS1", "0")).contains("NAXIS1 is 0"));
        assert!(failure(&with("NAXIS2", "-3")).contains("NAXIS2 is -3"));
        assert!(failure(&with("SIMPLE", "F")).contains("SIMPLE"));
        assert!(failure(&with("BZERO", "32768")).contains("BZERO 32768"));
        assert!(failure(&with("NAXIS1", "4000")).contains("needs"));

        // Signed sixteen-bit data is not what a camera sends.
        let signed = fits(
            &[
                ("SIMPLE", "T"),
                ("BITPIX", "16"),
                ("NAXIS", "2"),
                ("NAXIS1", "2"),
                ("NAXIS2", "2"),
            ],
            &[0u8; 8],
        );
        assert!(failure(&signed).contains("BZERO 0"));

        // Cut short in the data, and in the header.
        let whole = with("BITPIX", "8");
        assert!(failure(&whole[..BLOCK + 5]).contains("needs 12 bytes"));
        assert!(failure(&whole[..200]).contains("no END card"));
        assert!(failure(b"").contains("no END card"));
        assert!(failure(b"not a FITS file at all").contains("no END card"));
        // A missing keyword.
        let missing = fits(&[("SIMPLE", "T"), ("BITPIX", "8"), ("NAXIS", "2")], &[]);
        assert!(failure(&missing).contains("no NAXIS1"));
    }

    #[test]
    fn dimensions_too_large_to_hold_are_an_error_not_an_overflow() {
        let file = fits(
            &[
                ("SIMPLE", "T"),
                ("BITPIX", "16"),
                ("NAXIS", "3"),
                ("NAXIS1", "4294967295"),
                ("NAXIS2", "4294967295"),
                ("NAXIS3", "3"),
                ("BZERO", "32768"),
            ],
            &[],
        );
        assert!(decode(&file).is_err());
    }

    mod prop {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..6000)) {
                let _ = decode(&bytes);
            }

            #[test]
            fn a_plausible_header_with_arbitrary_values_never_panics(
                bitpix in "-?[0-9]{1,3}", naxis in "[0-9]", w in "-?[0-9]{1,10}", h in "-?[0-9]{1,10}",
                planes in "[0-9]{1,2}", bzero in "-?[0-9]{1,6}", len in 0usize..4000,
            ) {
                let file = fits(
                    &[("SIMPLE", "T"), ("BITPIX", &bitpix), ("NAXIS", &naxis), ("NAXIS1", &w),
                      ("NAXIS2", &h), ("NAXIS3", &planes), ("BZERO", &bzero)],
                    &vec![7u8; len],
                );
                let _ = decode(&file);
            }

            #[test]
            fn sixteen_bit_images_round_trip(values in proptest::collection::vec(any::<u16>(), 12)) {
                let image = decode(&mono16(&values, 3, 4, &[])).unwrap();
                let got: Vec<u16> = image.pixels.as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair)).collect();
                prop_assert_eq!(got, values);
            }
        }
    }
}
