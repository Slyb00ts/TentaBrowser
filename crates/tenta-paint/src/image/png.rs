//! PNG via the `png` crate: every colour type and depth, interlaced or not.
//! Gamma, ICC and sRGB chunks are ignored on purpose.
//!
//! Rows are pulled one at a time and converted to RGBA8 as they arrive. A
//! plain (non-interlaced) image is premultiplied and, when it will be drawn
//! smaller, shrunk on the fly, so the full-size image never exists; that makes
//! the peak the size of the result plus a few rows. An interlaced image needs
//! the whole picture before its last pass, so it is assembled at full size.

use std::io::Cursor;

use png::{BitDepth, ColorType, InterlaceInfo};

use super::ctx::Ctx;
use super::decode::{RawImage, premultiply_row};
use super::error::DecodeError;
use super::format::ImageFormat;
use super::orient::Orientation;
use super::pixels::{Pixels, filled};
use super::resize::Scaler;

const FORMAT: ImageFormat = ImageFormat::Png;
const MIB: usize = 1024 * 1024;

/// Bytes of working memory per pixel of image *width*: the decoder's current
/// and previous rows (up to 8 bytes a pixel at 16-bit RGBA), the converted row,
/// and the shrink's `f32` accumulator and finished row.
const ROW_SCRATCH_PER_PIXEL: u64 = 48;

pub(super) fn decode(bytes: &[u8], ctx: &Ctx) -> Result<RawImage, DecodeError> {
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(bytes),
        png::Limits {
            // Bounds ancillary chunks (text, ICC); pixel memory is admitted below.
            bytes: ctx.limits.max_decoded_bytes.clamp(MIB / 16, 8 * MIB),
        },
    );
    // Palette and tRNS expansion and widening of low bit depths; 16-bit stays
    // 16-bit so that the reduction below can round instead of truncate.
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder
        .read_info()
        .map_err(|e| DecodeError::malformed(FORMAT, e))?;
    let (width, height) = reader.info().size();
    let interlaced = reader.info().interlaced;
    let (color, depth) = reader.output_color_type();
    let layout = Layout::new(color, depth)?;

    let plan = ctx.plan(FORMAT, width, height, false)?;
    let stream = ctx.streams && !interlaced;
    let rows = ROW_SCRATCH_PER_PIXEL * u64::from(width);
    let peak = if stream {
        plan.output_bytes() + rows
    } else {
        plan.source_bytes() + plan.finish_bytes(false) + rows
    };
    ctx.admit(&plan, peak)?;

    let stride = width as usize * 4;
    let (target_w, target_h) = plan.target;
    let mut out = Pixels::zeroed(if stream {
        target_w as usize * target_h as usize * 4
    } else {
        stride * height as usize
    })?;
    let mut scaler = if stream && plan.shrinks() {
        Some(Scaler::new(width, height, target_w, target_h)?)
    } else {
        None
    };
    let mut line = if scaler.is_some() || interlaced {
        filled(stride, 0u8)?
    } else {
        Vec::new()
    };

    let mut y = 0usize;
    while let Some(row) = reader
        .next_interlaced_row()
        .map_err(|e| DecodeError::malformed(FORMAT, e))?
    {
        ctx.checkpoint(y, stride)?;
        match row.interlace() {
            InterlaceInfo::Adam7(pass) => {
                let pixels = layout.convert(row.data(), &mut line)?;
                png::expand_interlaced_row(&mut out, stride, &line[..pixels * 4], pass, 32);
            }
            InterlaceInfo::Null(_) => {
                if y >= height as usize {
                    return Err(DecodeError::malformed(FORMAT, "more rows than the height"));
                }
                if let Some(scaler) = &mut scaler {
                    let pixels = layout.convert(row.data(), &mut line)?;
                    premultiply_row(&mut line[..pixels * 4]);
                    if let Some(dy) = scaler.push_row(&line) {
                        let at = dy * target_w as usize * 4;
                        out[at..at + target_w as usize * 4].copy_from_slice(scaler.finished());
                    }
                } else {
                    let dest = &mut out[y * stride..][..stride];
                    let pixels = layout.convert(row.data(), dest)?;
                    if stream {
                        premultiply_row(&mut dest[..pixels * 4]);
                    }
                }
            }
        }
        y += 1;
    }
    if !interlaced && y != height as usize {
        return Err(DecodeError::malformed(FORMAT, "short pixel data"));
    }

    Ok(if stream {
        RawImage {
            width: target_w,
            height: target_h,
            rgba: out,
            orientation: Orientation::Identity,
            premultiplied: true,
            stored: (width, height),
        }
    } else {
        RawImage::straight(width, height, out, Orientation::Identity)
    })
}

/// How to widen one decoded row to RGBA8.
struct Layout {
    channels: usize,
    sixteen: bool,
}

impl Layout {
    fn new(color: ColorType, depth: BitDepth) -> Result<Self, DecodeError> {
        let sixteen = match depth {
            BitDepth::Eight => false,
            BitDepth::Sixteen => true,
            depth => {
                return Err(DecodeError::malformed(
                    FORMAT,
                    format!("unexpanded bit depth {depth:?}"),
                ));
            }
        };
        let channels = match color {
            ColorType::Grayscale => 1,
            ColorType::GrayscaleAlpha => 2,
            ColorType::Rgb => 3,
            ColorType::Rgba => 4,
            ColorType::Indexed => {
                return Err(DecodeError::malformed(FORMAT, "unexpanded palette"));
            }
        };
        Ok(Self { channels, sixteen })
    }

    /// Writes the pixels of `row` to the front of `out`; returns their count.
    fn convert(&self, row: &[u8], out: &mut [u8]) -> Result<usize, DecodeError> {
        let step = self.channels * (1 + usize::from(self.sixteen));
        let pixels = row.len() / step;
        let out = out
            .get_mut(..pixels * 4)
            .ok_or_else(|| DecodeError::malformed(FORMAT, "row wider than the image"))?;
        if self.channels == 4 && !self.sixteen {
            out.copy_from_slice(&row[..pixels * 4]);
            return Ok(pixels);
        }
        let sample = |px: &[u8], c: usize| {
            if self.sixteen {
                narrow(u16::from_be_bytes([px[2 * c], px[2 * c + 1]]))
            } else {
                px[c]
            }
        };
        for (px, o) in row.chunks_exact(step).zip(out.as_chunks_mut::<4>().0) {
            let rgba = match self.channels {
                1 => {
                    let g = sample(px, 0);
                    [g, g, g, 255]
                }
                2 => {
                    let g = sample(px, 0);
                    [g, g, g, sample(px, 1)]
                }
                3 => [sample(px, 0), sample(px, 1), sample(px, 2), 255],
                _ => [sample(px, 0), sample(px, 1), sample(px, 2), sample(px, 3)],
            };
            o.copy_from_slice(&rgba);
        }
        Ok(pixels)
    }
}

/// 16-bit to 8-bit with rounding: `round(v * 255 / 65535)`.
fn narrow(v: u16) -> u8 {
    ((u32::from(v) * 255 + 32895) >> 16) as u8
}
