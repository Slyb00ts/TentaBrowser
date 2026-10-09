//! JPEG via `zune-jpeg` (baseline and progressive) with EXIF orientation.

use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

use super::ctx::Ctx;
use super::decode::RawImage;
use super::error::DecodeError;
use super::format::ImageFormat;
use super::orient::Orientation;
use super::pixels::Pixels;

const FORMAT: ImageFormat = ImageFormat::Jpeg;
const ORIENTATION_TAG: u16 = 0x0112;

/// Working bytes per pixel beyond the RGBA output, measured with the counting
/// allocator on 1 Mpx images (`docs/dev/p3-plan.md`, IM1): a baseline decode
/// adds only MCU-row buffers; a progressive one keeps every coefficient as an
/// `i16` until the last scan, so it pays 2 bytes per sample (3 per pixel for
/// 4:2:0, 6 for 4:4:4).
const BASELINE_EXTRA_PER_PIXEL: u64 = 1;
/// Upsampling and colour-conversion planes of a progressive decode.
const PROGRESSIVE_EXTRA_FIXED: u64 = 2;

pub(super) fn decode(bytes: &[u8], ctx: &Ctx) -> Result<RawImage, DecodeError> {
    // JPEG dimensions are 16-bit, so these bounds are the format's own maximum;
    // the pixel budget is enforced from the parsed header below.
    let options = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGBA)
        .set_max_width(usize::from(u16::MAX))
        .set_max_height(usize::from(u16::MAX));
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    decoder
        .decode_headers()
        .map_err(|e| DecodeError::malformed(FORMAT, format!("{e:?}")))?;

    if matches!(
        decoder.input_colorspace(),
        Some(ColorSpace::CMYK | ColorSpace::YCCK)
    ) {
        return Err(DecodeError::Unsupported {
            format: FORMAT,
            feature: "CMYK and YCCK colour",
        });
    }
    let info = decoder
        .info()
        .ok_or_else(|| DecodeError::malformed(FORMAT, "missing frame header"))?;
    let (width, height) = (u32::from(info.width), u32::from(info.height));

    let orientation = decoder
        .exif()
        .and_then(|exif| exif_orientation(exif))
        .unwrap_or_default();

    let plan = ctx.plan(FORMAT, width, height, orientation.swaps_axes())?;
    let extra = match progressive_coefficient_bytes(bytes) {
        Some(coefficients) => coefficients + PROGRESSIVE_EXTRA_FIXED,
        None => BASELINE_EXTRA_PER_PIXEL,
    };
    let oriented = orientation != Orientation::Identity;
    ctx.admit(
        &plan,
        plan.source_bytes() + extra * plan.pixels + plan.finish_bytes(oriented),
    )?;

    let size = decoder
        .output_buffer_size()
        .ok_or(DecodeError::TooLarge { width, height })?;
    let mut rgba = Pixels::zeroed(size)?;
    // zune-jpeg cannot be interrupted, so the pixel limit above is what bounds
    // the time spent inside this call.
    ctx.check()?;
    decoder
        .decode_into(&mut rgba)
        .map_err(|e| DecodeError::malformed(FORMAT, format!("{e:?}")))?;
    Ok(RawImage::straight(width, height, rgba, orientation))
}

/// For a progressive file (SOF2, SOF6, SOF10 or SOF14), the bytes per pixel its
/// coefficient planes occupy, from the sampling factors of the frame header;
/// `None` for any other file.
fn progressive_coefficient_bytes(bytes: &[u8]) -> Option<u64> {
    // Segments follow the two-byte SOI.
    let mut at = 2;
    loop {
        let &[0xFF, marker] = bytes.get(at..at + 2)? else {
            return None;
        };
        match marker {
            // Fill byte.
            0xFF => at += 1,
            // Stuffed zero, TEM and restart markers carry no length.
            0x00 | 0x01 | 0xD0..=0xD7 => at += 2,
            0xC2 | 0xC6 | 0xCA | 0xCE => return sampling_bytes(bytes.get(at + 4..)?),
            // Other frame headers, end of image and start of scan.
            0xC0 | 0xC1 | 0xC3 | 0xC5 | 0xC7 | 0xC9 | 0xCB | 0xCD | 0xCF | 0xD9 | 0xDA => {
                return None;
            }
            _ => {
                let &[hi, lo] = bytes.get(at + 2..at + 4)? else {
                    return None;
                };
                at += 2 + usize::from(u16::from_be_bytes([hi, lo]));
            }
        }
    }
}

/// `2 * samples per pixel`, rounded up, from a frame header body (precision,
/// height, width, component count, then id, sampling and table per component).
/// A damaged header falls back to the 4:4:4 worst case.
fn sampling_bytes(frame: &[u8]) -> Option<u64> {
    let count = usize::from(*frame.get(5)?);
    let (mut samples, mut h_max, mut v_max) = (0u64, 1u64, 1u64);
    for component in frame.get(6..)?.chunks(3).take(count.min(4)) {
        let &[_, sampling, _] = component else {
            return Some(2 * 4);
        };
        let (h, v) = (
            u64::from(sampling >> 4).max(1),
            u64::from(sampling & 15).max(1),
        );
        samples += h * v;
        h_max = h_max.max(h);
        v_max = v_max.max(v);
    }
    Some((2 * samples).div_ceil(h_max * v_max))
}

/// Reads the orientation from an EXIF block that starts at the TIFF header
/// (or at the `Exif\0\0` prefix). Anything unreadable means upright.
fn exif_orientation(exif: &[u8]) -> Option<Orientation> {
    let tiff = exif.strip_prefix(b"Exif\0\0").unwrap_or(exif);
    let little = match tiff.get(..4)? {
        b"II*\0" => true,
        b"MM\0*" => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let b: [u8; 2] = tiff.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(if little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let b: [u8; 4] = tiff.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(if little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    let ifd = usize::try_from(u32_at(4)?).ok()?;
    let count = usize::from(u16_at(ifd)?);
    (0..count).find_map(|i| {
        let entry = i
            .checked_mul(12)?
            .checked_add(2)
            .and_then(|offset| ifd.checked_add(offset))?;
        if u16_at(entry)? != ORIENTATION_TAG {
            return None;
        }
        Some(Orientation::from_exif(u16_at(entry.checked_add(8)?)?))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exif(little: bool, tag: u16, value: u16) -> Vec<u8> {
        let u16b = |v: u16| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let u32b = |v: u32| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let mut out = Vec::new();
        out.extend_from_slice(if little { b"II*\0" } else { b"MM\0*" });
        out.extend_from_slice(&u32b(8));
        out.extend_from_slice(&u16b(1));
        out.extend_from_slice(&u16b(tag));
        out.extend_from_slice(&u16b(3));
        out.extend_from_slice(&u32b(1));
        out.extend_from_slice(&u16b(value));
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&u32b(0));
        out
    }

    #[test]
    fn orientation_is_read_in_both_byte_orders() {
        for little in [true, false] {
            assert_eq!(
                exif_orientation(&exif(little, ORIENTATION_TAG, 6)),
                Some(Orientation::Rotate90)
            );
        }
        let mut prefixed = b"Exif\0\0".to_vec();
        prefixed.extend(exif(true, ORIENTATION_TAG, 3));
        assert_eq!(exif_orientation(&prefixed), Some(Orientation::Rotate180));
    }

    #[test]
    fn other_tags_and_truncation_mean_upright() {
        assert_eq!(exif_orientation(&exif(true, 0x0100, 6)), None);
        let full = exif(true, ORIENTATION_TAG, 6);
        for cut in 0..20 {
            assert_eq!(exif_orientation(&full[..cut]), None, "cut at {cut}");
        }
        assert_eq!(exif_orientation(b"not exif"), None);
    }
}
