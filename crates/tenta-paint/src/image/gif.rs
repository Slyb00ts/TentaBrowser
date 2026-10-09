//! GIF via the `gif` crate: the first frame, composed on a transparent logical
//! screen. Later frames and disposal methods never matter for a still; see
//! D-035 for why animation is not decoded.

use std::io::Cursor;
use std::num::NonZeroU64;

use gif::{ColorOutput, DecodeOptions, MemoryLimit};

use super::ctx::Ctx;
use super::decode::RawImage;
use super::error::DecodeError;
use super::format::ImageFormat;
use super::orient::Orientation;
use super::pixels::Pixels;

const FORMAT: ImageFormat = ImageFormat::Gif;

/// Bytes per frame pixel the `gif` crate holds while it decodes a frame: the
/// RGBA output plus the indexed row data it expands from.
const FRAME_BYTES_PER_PIXEL: u64 = 5;

pub(super) fn decode(bytes: &[u8], ctx: &Ctx) -> Result<RawImage, DecodeError> {
    let mut options = DecodeOptions::new();
    options.set_color_output(ColorOutput::RGBA);
    if let Some(bytes) = NonZeroU64::new(ctx.limits.max_transient_bytes as u64) {
        options.set_memory_limit(MemoryLimit::Bytes(bytes));
    }
    let mut decoder = options
        .read_info(Cursor::new(bytes))
        .map_err(|e| DecodeError::malformed(FORMAT, e))?;
    let screen = (u32::from(decoder.width()), u32::from(decoder.height()));

    ctx.check()?;
    let frame = decoder
        .next_frame_info()
        .map_err(|e| DecodeError::malformed(FORMAT, e))?
        .ok_or_else(|| DecodeError::malformed(FORMAT, "no image data"))?;
    let (left, top) = (usize::from(frame.left), usize::from(frame.top));
    let (frame_w, frame_h) = (u32::from(frame.width), u32::from(frame.height));
    let frame_plan = ctx.plan(FORMAT, frame_w, frame_h, false)?;

    // Browsers size a still GIF by its logical screen, and fall back to the
    // frame when the screen is declared empty.
    let (width, height) = if screen.0 == 0 || screen.1 == 0 {
        (frame_w, frame_h)
    } else {
        screen
    };
    let plan = ctx.plan(FORMAT, width, height, false)?;
    // A frame that fills the screen decodes straight into the result; any
    // other frame is decoded on its own and copied onto the screen.
    let fills_screen = (left, top, frame_w, frame_h) == (0, 0, width, height);
    let working = if fills_screen {
        FRAME_BYTES_PER_PIXEL * plan.pixels
    } else {
        plan.source_bytes() + FRAME_BYTES_PER_PIXEL * frame_plan.pixels
    };
    ctx.admit(&plan, working + plan.finish_bytes(false))?;

    let (w, h) = (width as usize, height as usize);
    if fills_screen {
        let mut rgba = Pixels::zeroed(decoder.buffer_size())?;
        decoder
            .read_into_buffer(&mut rgba)
            .map_err(|e| DecodeError::malformed(FORMAT, e))?;
        ctx.check()?;
        return Ok(RawImage::straight(
            width,
            height,
            rgba,
            Orientation::Identity,
        ));
    }

    let mut pixels = Pixels::zeroed(decoder.buffer_size())?;
    decoder
        .read_into_buffer(&mut pixels)
        .map_err(|e| DecodeError::malformed(FORMAT, e))?;
    ctx.check()?;

    let mut rgba = Pixels::zeroed(w * h * 4)?;
    // A frame can start outside the screen or overhang it; only the part that
    // lands on the screen is kept.
    let copy_w = (frame_w as usize).min(w.saturating_sub(left));
    if copy_w > 0 {
        for row in 0..(frame_h as usize).min(h.saturating_sub(top)) {
            ctx.checkpoint(row, copy_w * 4)?;
            let from = row * frame_w as usize * 4;
            let to = ((top + row) * w + left) * 4;
            rgba[to..to + copy_w * 4].copy_from_slice(&pixels[from..from + copy_w * 4]);
        }
    }
    Ok(RawImage::straight(
        width,
        height,
        rgba,
        Orientation::Identity,
    ))
}
