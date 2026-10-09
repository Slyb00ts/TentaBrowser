//! Glyph outline rasterisation and the glyph mask cache.
//!
//! Rendering choices, all aimed at matching headless Chrome on Linux without
//! depending on platform libraries:
//!
//! - Outlines are unhinted and positioned at quarter-pixel steps
//!   horizontally; the baseline is rounded to a whole device pixel, as Skia
//!   does for horizontal text.
//! - Anti-aliasing is greyscale coverage, never LCD subpixel.
//! - Faux bold widens the outline by a twenty-fourth of the em, keeping the
//!   left and bottom edges in place (FreeType's `FT_Outline_Embolden`, which
//!   Skia uses on Linux).
//! - Faux italic shears the outline by the run's skew.
//! - Colour glyphs (COLR, bitmaps) are not rendered and are reported as
//!   undrawn.

mod cache;
mod decoration;
mod outline;
#[cfg(test)]
mod tests;

use tenta_layout::{Color, GlyphRun, LineStyle, Rect};
use tiny_skia::{Mask, Paint, Pixmap, Transform};

use cache::{Cached, Request, SUBPIXEL_STEPS};
use outline::Outliner;

/// Glyphs larger than this many device pixels per em are filled directly
/// instead of cached; their masks would dominate the cache.
const MAX_CACHED_SIZE: f32 = 256.0;

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
    if color.is_transparent() {
        return 0;
    }
    let mut outliner = None;
    match axis_scale(transform) {
        Some(scale) if run.size * scale <= MAX_CACHED_SIZE => {
            draw_cached(canvas, transform, origin, run, color, mask, &mut outliner)
        }
        _ => draw_direct(canvas, transform, origin, run, color, mask, &mut outliner),
    }
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
    decoration::draw(canvas, transform, rect, color, style, mask);
    0
}

/// The uniform scale of a transform that only scales and translates, or
/// `None` when it rotates, skews, mirrors or scales the axes differently.
fn axis_scale(transform: Transform) -> Option<f32> {
    let uniform = (transform.sx - transform.sy).abs() <= transform.sx.abs() * 1e-4;
    let upright = transform.kx == 0.0 && transform.ky == 0.0 && transform.sx > 0.0;
    (uniform && upright && transform.sx.is_finite()).then_some(transform.sx)
}

fn paint_for(color: Color, anti_alias: bool) -> Option<Paint<'static>> {
    if color.is_transparent() {
        return None;
    }
    let straight = |channel: f32| (channel / color.a).clamp(0.0, 1.0);
    let mut paint = Paint::default();
    paint.set_color(tiny_skia::Color::from_rgba(
        straight(color.r),
        straight(color.g),
        straight(color.b),
        color.a.clamp(0.0, 1.0),
    )?);
    paint.anti_alias = anti_alias;
    Some(paint)
}

/// Axis-aligned text: cached masks blitted at snapped positions.
fn draw_cached<'a>(
    canvas: &mut Pixmap,
    transform: Transform,
    origin: (f32, f32),
    run: &'a GlyphRun,
    color: Color,
    clip: Option<&Mask>,
    outliner: &mut Option<Option<Outliner<'a>>>,
) -> usize {
    let scale = transform.sx;
    let premultiplied = color.to_premultiplied_rgba8();
    let size = run.size * scale;
    let font_id = cache::font_id(&run.font);
    let mut undrawn = 0;
    for glyph in &run.glyphs {
        let x = (origin.0 + glyph.x) * scale + transform.tx;
        let y = (origin.1 + glyph.y) * scale + transform.ty;
        let steps = (x * SUBPIXEL_STEPS as f32 + 0.5).floor();
        let row = y.round();
        if !(steps.is_finite() && row.is_finite()) || steps.abs() > 1e8 || row.abs() > 1e8 {
            undrawn += 1;
            continue;
        }
        let steps = steps as i32;
        let request = Request {
            font: &run.font,
            font_id,
            glyph: glyph.id,
            size,
            quarter: steps.rem_euclid(SUBPIXEL_STEPS) as u8,
            bold: run.synthetic_bold,
            skew: run.skew,
        };
        match &*cache::lookup(&request, outliner) {
            Cached::Drawn(mask) => blit(
                canvas,
                mask,
                steps.div_euclid(SUBPIXEL_STEPS),
                row as i32,
                premultiplied,
                clip,
            ),
            Cached::Undrawn => undrawn += 1,
        }
    }
    undrawn
}

/// Source-over of `mask` at (`x`, `y`) in `color`, cut by `clip`.
fn blit(
    canvas: &mut Pixmap,
    mask: &cache::Mask,
    x: i32,
    y: i32,
    color: [u8; 4],
    clip: Option<&Mask>,
) {
    let (canvas_w, canvas_h) = (canvas.width() as i32, canvas.height() as i32);
    let (left, top) = (x + mask.left, y + mask.top);
    let columns = left.max(0)..(left + mask.width as i32).min(canvas_w);
    let rows = top.max(0)..(top + mask.height as i32).min(canvas_h);
    if columns.is_empty() || rows.is_empty() {
        return;
    }
    debug_assert!(
        clip.is_none_or(|m| (m.width(), m.height()) == (canvas.width(), canvas.height())),
        "the clip mask must cover the canvas"
    );
    let clip_width = clip.map_or(0, |m| m.width() as usize);
    let pixels = canvas.data_mut();
    for row in rows {
        let mask_row = (row - top) as usize * mask.width as usize;
        for column in columns.clone() {
            let mut coverage = mask.alpha[mask_row + (column - left) as usize];
            if coverage == 0 {
                continue;
            }
            if let Some(clip) = clip {
                let index = row as usize * clip_width + column as usize;
                coverage = mul255(coverage, clip.data()[index]);
                if coverage == 0 {
                    continue;
                }
            }
            let at = (row as usize * canvas_w as usize + column as usize) * 4;
            let dst = &mut pixels[at..at + 4];
            if coverage == 255 && color[3] == 255 {
                dst.copy_from_slice(&color);
                continue;
            }
            let inverse = 255 - mul255(color[3], coverage);
            for (d, s) in dst.iter_mut().zip(color) {
                *d = mul255(s, coverage) + mul255(*d, inverse);
            }
        }
    }
}

/// `a * b / 255`, rounded.
fn mul255(a: u8, b: u8) -> u8 {
    let t = u32::from(a) * u32::from(b) + 128;
    ((t + (t >> 8)) >> 8) as u8
}

/// Rotated, skewed or very large text: each outline is filled under the full
/// transform with exact geometry, uncached.
fn draw_direct<'a>(
    canvas: &mut Pixmap,
    transform: Transform,
    origin: (f32, f32),
    run: &'a GlyphRun,
    color: Color,
    clip: Option<&Mask>,
    outliner: &mut Option<Option<Outliner<'a>>>,
) -> usize {
    let Some(paint) = paint_for(color, true) else {
        return 0;
    };
    let Some(outliner) = outliner.get_or_insert_with(|| Outliner::new(&run.font)) else {
        return run.glyphs.len();
    };
    let grow = cache::embolden_width(run.size);
    let mut undrawn = 0;
    for glyph in &run.glyphs {
        let Ok(path) = outliner.path(glyph.id, run.size, run.skew) else {
            undrawn += 1;
            continue;
        };
        let Some(path) = path else {
            continue;
        };
        let mut placed = transform.pre_translate(origin.0 + glyph.x, origin.1 + glyph.y);
        if run.synthetic_bold {
            placed = placed.pre_translate(grow / 2.0, -grow / 2.0);
            draw_embolden(canvas, &path, placed, grow, color, clip);
        } else {
            canvas.fill_path(&path, &paint, tiny_skia::FillRule::Winding, placed, clip);
        }
    }
    undrawn
}

/// Draws an emboldened outline through the same coverage the cache uses, on a
/// scratch grid covering the glyph's device bounds.
fn draw_embolden(
    canvas: &mut Pixmap,
    path: &tiny_skia::Path,
    placed: Transform,
    grow: f32,
    color: Color,
    clip: Option<&Mask>,
) {
    let Some(device) = path.clone().transform(placed) else {
        return;
    };
    let scale = [placed.sx, placed.kx, placed.ky, placed.sy]
        .iter()
        .fold(1.0_f32, |acc, v| acc.max(v.abs()));
    let pad = grow * scale + 2.0;
    let bounds = device.bounds();
    let left = (bounds.left() - pad).floor().max(0.0);
    let top = (bounds.top() - pad).floor().max(0.0);
    let right = (bounds.right() + pad).ceil().min(canvas.width() as f32);
    let bottom = (bounds.bottom() + pad).ceil().min(canvas.height() as f32);
    if !(right > left && bottom > top) {
        return;
    }
    let (width, height) = ((right - left) as u32, (bottom - top) as u32);
    let moved = placed.post_translate(-left, -top);
    let Some(alpha) = cache::coverage(path, moved, (width, height), Some(grow)) else {
        return;
    };
    let mask = cache::Mask {
        left: left as i32,
        top: top as i32,
        width,
        height,
        alpha,
    };
    blit(canvas, &mask, 0, 0, color.to_premultiplied_rgba8(), clip);
}
