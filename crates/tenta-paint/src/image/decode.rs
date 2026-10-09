//! The decode pipeline shared by every format: decode, bound, orient, shrink,
//! premultiply.

use std::panic::{AssertUnwindSafe, catch_unwind};

use super::ctx::{Ctx, DecodeControl};
use super::error::DecodeError;
use super::format::{ImageFormat, sniff_format};
use super::limits::{DecodeLimits, DecodeParams};
use super::orient::Orientation;
use super::pixels::Pixels;
use super::{DecodedImage, bmp, gif, ico, jpeg, png, resize, webp};

/// A decoder's output: RGBA8 as stored in the file, or already shrunk when the
/// decoder shrinks while it decodes.
pub(super) struct RawImage {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) rgba: Pixels,
    pub(super) orientation: Orientation,
    /// True when the colour channels are already multiplied by alpha.
    pub(super) premultiplied: bool,
    /// The size stored in the file when `width x height` was shrunk on the way.
    pub(super) stored: (u32, u32),
}

impl RawImage {
    /// A decoded image at its stored size with straight alpha.
    pub(super) fn straight(
        width: u32,
        height: u32,
        rgba: Pixels,
        orientation: Orientation,
    ) -> Self {
        Self {
            width,
            height,
            rgba,
            orientation,
            premultiplied: false,
            stored: (width, height),
        }
    }
}

/// Decodes `bytes`, choosing the decoder by content.
pub fn decode(
    bytes: &[u8],
    params: &DecodeParams,
    limits: &DecodeLimits,
) -> Result<DecodedImage, DecodeError> {
    decode_with_control(bytes, params, limits, &DecodeControl::new())
}

/// [`decode`] under a deadline and cancel flag.
pub fn decode_with_control(
    bytes: &[u8],
    params: &DecodeParams,
    limits: &DecodeLimits,
    control: &DecodeControl,
) -> Result<DecodedImage, DecodeError> {
    let format = sniff_format(bytes).ok_or(DecodeError::UnknownFormat)?;
    decode_format_with_control(format, bytes, params, limits, control)
}

/// Decodes `bytes` as `format`, which skips sniffing. Never panics: a panic
/// inside a decoder becomes [`DecodeError::Panicked`].
pub fn decode_format(
    format: ImageFormat,
    bytes: &[u8],
    params: &DecodeParams,
    limits: &DecodeLimits,
) -> Result<DecodedImage, DecodeError> {
    decode_format_with_control(format, bytes, params, limits, &DecodeControl::new())
}

/// [`decode_format`] under a deadline and cancel flag.
pub fn decode_format_with_control(
    format: ImageFormat,
    bytes: &[u8],
    params: &DecodeParams,
    limits: &DecodeLimits,
    control: &DecodeControl,
) -> Result<DecodedImage, DecodeError> {
    catch_unwind(AssertUnwindSafe(|| {
        let ctx = Ctx::new(limits, params, control);
        ctx.check()?;
        let raw = decode_raw(format, bytes, &ctx)?;
        ctx.check()?;
        finish(raw, &ctx)
    }))
    .map_err(|_| DecodeError::Panicked(format))?
}

fn decode_raw(format: ImageFormat, bytes: &[u8], ctx: &Ctx) -> Result<RawImage, DecodeError> {
    match format {
        ImageFormat::Png => png::decode(bytes, ctx),
        ImageFormat::Jpeg => jpeg::decode(bytes, ctx),
        ImageFormat::WebP => webp::decode(bytes, ctx),
        ImageFormat::Gif => gif::decode(bytes, ctx),
        ImageFormat::Bmp => bmp::decode(bytes, ctx),
        ImageFormat::Ico => ico::decode(bytes, ctx),
        ImageFormat::Svg => Err(DecodeError::Unsupported {
            format,
            feature: "SVG is rendered by decode_svg, which needs a scale",
        }),
    }
}

/// Orientation is applied last, after the shrink, so the transform runs over
/// the smaller buffer; the target size is therefore computed in stored axes.
/// The full-size buffer is dropped as soon as the shrunk copy exists.
fn finish(raw: RawImage, ctx: &Ctx) -> Result<DecodedImage, DecodeError> {
    let RawImage {
        width,
        height,
        mut rgba,
        orientation,
        premultiplied,
        stored,
    } = raw;
    let (source_width, source_height) = if orientation.swaps_axes() {
        (stored.1, stored.0)
    } else {
        stored
    };
    let (target_w, target_h) = ctx.target(stored.0, stored.1, orientation.swaps_axes());
    debug_assert!(width >= target_w && height >= target_h);

    if !premultiplied {
        premultiply(&mut rgba, width as usize * 4, ctx)?;
    }
    let pixels = if (target_w, target_h) == (width, height) {
        rgba
    } else {
        let scaled = resize::downscale(&rgba, width, height, target_w, target_h, ctx)?;
        drop(rgba);
        scaled
    };
    let (pixels, out_w, out_h) = if orientation == Orientation::Identity {
        (pixels, target_w, target_h)
    } else {
        orientation.apply(&pixels, target_w, target_h, ctx)?
    };
    Ok(DecodedImage {
        width: out_w,
        height: out_h,
        rgba: pixels.into_shared(),
        source_width,
        source_height,
    })
}

/// Multiplies the colour channels by alpha, checking the control between rows.
fn premultiply(rgba: &mut [u8], row_len: usize, ctx: &Ctx) -> Result<(), DecodeError> {
    for (y, row) in rgba.chunks_mut(row_len.max(4)).enumerate() {
        ctx.checkpoint(y, row_len)?;
        premultiply_row(row);
    }
    Ok(())
}

pub(super) fn premultiply_row(rgba: &mut [u8]) {
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = u32::from(px[3]);
        if a == 255 {
            continue;
        }
        for c in &mut px[..3] {
            let t = u32::from(*c) * a + 128;
            *c = ((t + (t >> 8)) >> 8) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiply_rounds_to_nearest() {
        let mut px = [255, 128, 0, 128, 200, 100, 50, 0, 9, 9, 9, 255];
        premultiply_row(&mut px);
        assert_eq!(px, [128, 64, 0, 128, 0, 0, 0, 0, 9, 9, 9, 255]);
    }
}
