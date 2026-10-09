//! Glyph outline rasterisation and the glyph path cache.

use tenta_layout::{Color, GlyphRun, LineStyle, Rect};
use tiny_skia::Mask;

/// Draws `run` with its glyph coordinates origin at `origin`, in `color`,
/// through `mask` when the active clip needs one. Returns how many glyphs it
/// could not draw, such as colour glyphs.
pub(crate) fn draw_text(
    canvas: &mut tiny_skia::Pixmap,
    transform: tiny_skia::Transform,
    origin: (f32, f32),
    run: &GlyphRun,
    color: Color,
    mask: Option<&Mask>,
) -> usize {
    let _ = (canvas, transform, origin, run, color, mask);
    todo!("P1g")
}

/// Draws one text-decoration line covering `rect`, through `mask` when the
/// active clip needs one. Returns how many lines it could not draw.
pub(crate) fn draw_decoration(
    canvas: &mut tiny_skia::Pixmap,
    transform: tiny_skia::Transform,
    rect: Rect,
    color: Color,
    style: LineStyle,
    mask: Option<&Mask>,
) -> usize {
    let _ = (canvas, transform, rect, color, style, mask);
    todo!("P1g")
}
