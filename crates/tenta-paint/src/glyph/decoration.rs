//! Text-decoration lines.
//!
//! The display list carries the line's box, already sized from the font's
//! underline metrics or an explicit thickness, so this module only chooses
//! how a style fills that box. Dash and dot proportions follow Blink's
//! `StrokeData`; the wave is a smooth approximation rather than Blink's exact
//! curve.

use tenta_layout::{Color, LineStyle, Rect};
use tiny_skia::{Mask, Paint, PathBuilder, Pixmap, Stroke, Transform};

use super::{axis_scale, paint_for};

/// Draws the line; every style is drawable, so nothing is ever left undrawn.
pub(super) fn draw(
    canvas: &mut Pixmap,
    transform: Transform,
    rect: Rect,
    color: Color,
    style: LineStyle,
    mask: Option<&Mask>,
) {
    let Some(paint) = paint_for(color, true) else {
        return;
    };
    // Under an axis-aligned transform the band is snapped to whole device
    // pixels, at least one thick, so thin underlines stay crisp; any other
    // transform keeps exact geometry.
    let (rect, transform, snapped) = match axis_scale(transform) {
        Some(_) => {
            let Some(device) = snap(rect, transform) else {
                return;
            };
            (device, Transform::identity(), true)
        }
        None => (rect, transform, false),
    };
    if rect.w <= 0.0 || rect.h <= 0.0 {
        return;
    }
    let mut pen = Pen {
        canvas,
        transform,
        paint,
        mask,
        snapped,
    };
    match style {
        LineStyle::Solid => pen.bar(rect.x, rect.y, rect.w, rect.h),
        LineStyle::Double => {
            // Two lines of a third of the band each, a third apart; a band
            // too thin for that is one line.
            let third = rect.h / 3.0;
            if snapped && rect.h < 3.0 {
                pen.bar(rect.x, rect.y, rect.w, rect.h);
            } else {
                pen.bar(rect.x, rect.y, rect.w, third);
                pen.bar(rect.x, rect.y + 2.0 * third, rect.w, third);
            }
        }
        LineStyle::Dashed => {
            let (dash, gap) = if rect.h >= 3.0 {
                (2.0 * rect.h, rect.h)
            } else {
                (3.0 * rect.h, 2.0 * rect.h)
            };
            let (count, gap) = fit(rect.w, dash, gap);
            for i in 0..count {
                pen.bar(rect.x + i as f32 * (dash + gap), rect.y, dash, rect.h);
            }
        }
        LineStyle::Dotted => {
            let (count, gap) = fit(rect.w, rect.h, rect.h);
            let radius = rect.h / 2.0;
            let mut builder = PathBuilder::new();
            for i in 0..count {
                let cx = rect.x + radius + i as f32 * (rect.h + gap);
                builder.push_circle(cx, rect.y + radius, radius);
            }
            if let Some(path) = builder.finish() {
                pen.canvas.fill_path(
                    &path,
                    &pen.paint,
                    tiny_skia::FillRule::Winding,
                    pen.transform,
                    pen.mask,
                );
            }
        }
        LineStyle::Wavy => pen.wave(rect),
    }
}

struct Pen<'a> {
    canvas: &'a mut Pixmap,
    transform: Transform,
    paint: Paint<'static>,
    mask: Option<&'a Mask>,
    snapped: bool,
}

impl Pen<'_> {
    fn bar(&mut self, x: f32, y: f32, w: f32, h: f32) {
        // Dash edges snap to columns as well so repeated dashes keep one width.
        let (x, w) = if self.snapped {
            let left = x.round();
            (left, ((x + w).round() - left).max(1.0))
        } else {
            (x, w)
        };
        let Some(rect) = tiny_skia::Rect::from_xywh(x, y, w, h) else {
            return;
        };
        let mut paint = self.paint.clone();
        paint.anti_alias = !self.snapped;
        self.canvas
            .fill_rect(rect, &paint, self.transform, self.mask);
    }

    /// A centre line swinging between the band's top and bottom in whole
    /// half-waves, stroked a third of the band thick.
    fn wave(&mut self, rect: Rect) {
        let thickness = (rect.h / 3.0).max(1.0);
        let top = rect.y + thickness / 2.0;
        let bottom = rect.y + rect.h - thickness / 2.0;
        let halves = (rect.w / rect.h).round().max(1.0);
        let half = rect.w / halves;
        let mut builder = PathBuilder::new();
        builder.move_to(rect.x, bottom);
        let mut low = true;
        for i in 0..halves as u32 {
            let x0 = rect.x + i as f32 * half;
            let (from, to) = if low { (bottom, top) } else { (top, bottom) };
            builder.cubic_to(x0 + half / 2.0, from, x0 + half / 2.0, to, x0 + half, to);
            low = !low;
        }
        let Some(path) = builder.finish() else {
            return;
        };
        let stroke = Stroke {
            width: thickness,
            ..Stroke::default()
        };
        self.canvas
            .stroke_path(&path, &self.paint, &stroke, self.transform, self.mask);
    }
}

/// `rect` in device pixels with its edges rounded and at least one pixel thick.
fn snap(rect: Rect, transform: Transform) -> Option<Rect> {
    let x0 = (rect.x * transform.sx + transform.tx).round();
    let x1 = ((rect.x + rect.w) * transform.sx + transform.tx).round();
    let y0 = (rect.y * transform.sy + transform.ty).round();
    let y1 = ((rect.y + rect.h) * transform.sy + transform.ty)
        .round()
        .max(y0 + 1.0);
    let finite = [x0, x1, y0, y1].iter().all(|v| v.is_finite());
    (finite && x1 > x0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
}

/// How many elements of length `len` fit in `total` with gaps close to `gap`,
/// and the gap that makes the pattern start and end on a whole element.
fn fit(total: f32, len: f32, gap: f32) -> (u32, f32) {
    if len <= 0.0 || total <= len {
        return (1, 0.0);
    }
    let mut count = ((total + gap) / (len + gap)).round().max(1.0);
    if count * len > total {
        count -= 1.0;
    }
    if count < 2.0 {
        return (1, 0.0);
    }
    (count as u32, (total - count * len) / (count - 1.0))
}
