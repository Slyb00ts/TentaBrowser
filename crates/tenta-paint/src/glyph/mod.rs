//! Glyph outline rasterisation and the glyph path cache.

use tenta_layout::{Color, GlyphRun, LineStyle, Rect};

use crate::PaintError;

/// Draws `run` with its glyph coordinates origin at `origin`, in `color`.
pub(crate) fn draw_text(
    canvas: &mut tiny_skia::Pixmap,
    transform: tiny_skia::Transform,
    origin: (f32, f32),
    run: &GlyphRun,
    color: Color,
) -> Result<(), PaintError> {
    let _ = (canvas, transform, origin, run, color);
    todo!("P1g")
}

/// Draws one text-decoration line covering `rect`.
pub(crate) fn draw_decoration(
    canvas: &mut tiny_skia::Pixmap,
    transform: tiny_skia::Transform,
    rect: Rect,
    color: Color,
    style: LineStyle,
) -> Result<(), PaintError> {
    let _ = (canvas, transform, rect, color, style);
    todo!("P1g")
}
