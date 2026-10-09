//! Background layers, gradients and canvas propagation.

use tenta_layout::{
    BackgroundContent, BackgroundLayer, BackgroundSize, BoxArea, Fragment, GradientPaint,
    PaintStyle, Rect, RepeatKind,
};

use super::border::{inset_radii, inset_rect, resolve_radii};
use super::replaced;
use crate::PaintContext;
use crate::dl::{Item, Radii};

const NO_RADII: Radii = [0.0; 8];
pub(super) const EPSILON: f32 = 1e-3;

/// Whether the style paints anything as a background. This decides what the
/// canvas takes from the root element.
pub(super) fn paints(style: &PaintStyle) -> bool {
    style.background.iter().any(|layer| match &layer.content {
        BackgroundContent::Color(color) => !color.is_transparent(),
        _ => true,
    })
}

/// Pushes the background items of `frag`, bottom layer first (the colour layer
/// is last in the style, so it comes out first).
pub(super) fn emit(frag: &Fragment, border_box: Rect, cx: &PaintContext, items: &mut Vec<Item>) {
    if border_box.w <= 0.0 || border_box.h <= 0.0 {
        return;
    }
    let outer = resolve_radii(&frag.paint.border.radii, border_box);
    for layer in frag.paint.background.iter().rev() {
        let (clip, radii) = area(frag, border_box, outer, layer.clip);
        emit_layer(layer, (frag, border_box), clip, radii, cx, items);
    }
}

/// Pushes the backgrounds of `source` over the whole `canvas`. Origin, size and
/// position are resolved against the root element (`positioning`), whichever
/// element the layers come from; only the painted area grows and loses its
/// rounded corners.
pub(super) fn emit_canvas(
    source: &Fragment,
    positioning: (&Fragment, Rect),
    canvas: Rect,
    cx: &PaintContext,
    items: &mut Vec<Item>,
) {
    for layer in source.paint.background.iter().rev() {
        emit_layer(layer, positioning, canvas, NO_RADII, cx, items);
    }
}

/// The rectangle and corner radii of one of the three CSS boxes.
fn area(frag: &Fragment, border_box: Rect, outer: Radii, which: BoxArea) -> (Rect, Radii) {
    match which {
        BoxArea::BorderBox => (border_box, outer),
        BoxArea::PaddingBox => (
            inset_rect(border_box, &frag.border),
            inset_radii(outer, &frag.border),
        ),
        BoxArea::ContentBox => {
            let padding_box = inset_rect(border_box, &frag.border);
            let padding_radii = inset_radii(outer, &frag.border);
            (
                inset_rect(padding_box, &frag.padding),
                inset_radii(padding_radii, &frag.padding),
            )
        }
    }
}

/// `positioning` is the fragment and border box that origin, size and position
/// refer to.
fn emit_layer(
    layer: &BackgroundLayer,
    positioning: (&Fragment, Rect),
    clip: Rect,
    clip_radii: Radii,
    cx: &PaintContext,
    items: &mut Vec<Item>,
) {
    if clip.w <= 0.0 || clip.h <= 0.0 {
        return;
    }
    let (frag, border_box) = positioning;
    let origin = || {
        let outer = resolve_radii(&frag.paint.border.radii, border_box);
        area(frag, border_box, outer, layer.origin).0
    };
    match &layer.content {
        BackgroundContent::Color(color) => {
            if color.is_transparent() {
                return;
            }
            items.push(if clip_radii.iter().any(|&r| r > 0.0) {
                Item::FillRRect {
                    rect: clip,
                    radii: clip_radii,
                    color: *color,
                }
            } else {
                Item::FillRect {
                    rect: clip,
                    color: *color,
                }
            });
        }
        BackgroundContent::Gradient(gradient) => {
            emit_gradient(layer, gradient, origin(), clip, clip_radii, items);
        }
        BackgroundContent::Image(key) => {
            replaced::emit_background(layer, key, origin(), clip, clip_radii, cx, items);
        }
    }
}

fn emit_gradient(
    layer: &BackgroundLayer,
    gradient: &GradientPaint,
    origin: Rect,
    clip: Rect,
    clip_radii: Radii,
    items: &mut Vec<Item>,
) {
    // A gradient has no intrinsic size or ratio, so `auto`, `cover` and
    // `contain` all fill the positioning area.
    let (width, height) = match layer.size {
        BackgroundSize::Explicit(w, h) => (
            w.map_or(origin.w, |w| w.resolve(origin.w)),
            h.map_or(origin.h, |h| h.resolve(origin.h)),
        ),
        _ => (origin.w, origin.h),
    };
    if width < 1.0 || height < 1.0 {
        return;
    }
    let start_x = origin.x + layer.position.0.resolve(origin.w - width);
    let start_y = origin.y + layer.position.1.resolve(origin.h - height);
    let columns = tile_spans(
        (origin.x, origin.w),
        (start_x, width),
        layer.repeat.0,
        (clip.x, clip.right()),
    );
    let rows = tile_spans(
        (origin.y, origin.h),
        (start_y, height),
        layer.repeat.1,
        (clip.y, clip.bottom()),
    );
    let tiles: Vec<Rect> = rows
        .iter()
        .flat_map(|&(y, h)| columns.iter().map(move |&(x, w)| Rect::new(x, y, w, h)))
        .collect();
    let Some(first) = tiles.first() else {
        return;
    };
    let rounded = clip_radii.iter().any(|&r| r > 0.0);
    let exact = tiles.len() == 1 && near(first, &clip);
    let contained = tiles.iter().all(|t| contains(&clip, t));
    let clipped = !exact && (rounded || !contained);
    if clipped {
        items.push(Item::PushClip {
            rect: clip,
            radii: clip_radii,
        });
    }
    let radii = if exact { clip_radii } else { NO_RADII };
    for tile in tiles {
        if let Some(item) = gradient_item(gradient, tile, radii) {
            items.push(item);
        }
    }
    if clipped {
        items.push(Item::PopClip);
    }
}

fn gradient_item(gradient: &GradientPaint, tile: Rect, radii: Radii) -> Option<Item> {
    let (sin, cos) = gradient.angle_deg.to_radians().sin_cos();
    let line = (tile.w * sin).abs() + (tile.h * cos).abs();
    if line <= EPSILON || gradient.stops.is_empty() {
        return None;
    }
    Some(Item::LinearGradient {
        rect: tile,
        radii,
        angle: gradient.angle_deg,
        stops: gradient
            .stops
            .iter()
            .map(|stop| (stop.position.resolve(line) / line, stop.color))
            .collect(),
        repeat: gradient.repeating,
    })
}

/// The `(start, length)` of every tile along one axis that can reach
/// `visible`. `area` is the positioning area and `tile` the first tile.
pub(super) fn tile_spans(
    area: (f32, f32),
    tile: (f32, f32),
    repeat: RepeatKind,
    visible: (f32, f32),
) -> Vec<(f32, f32)> {
    let (area_start, area_len) = area;
    let (start, len) = tile;
    match repeat {
        RepeatKind::NoRepeat => {
            if start < visible.1 && start + len > visible.0 {
                vec![(start, len)]
            } else {
                Vec::new()
            }
        }
        RepeatKind::Repeat => repeated(start, len, len, visible),
        RepeatKind::Round => {
            let count = (area_len / len).round().max(1.0);
            let fitted = area_len / count;
            repeated(start, fitted, fitted, visible)
        }
        RepeatKind::Space => {
            let count = (area_len / len).floor();
            if !(len >= MIN_TILE && count <= MAX_SPANS as f32) {
                return Vec::new();
            }
            if count < 2.0 {
                return tile_spans(area, tile, RepeatKind::NoRepeat, visible);
            }
            let step = len + (area_len - count * len) / (count - 1.0);
            (0..count as u32)
                .map(|i| (area_start + i as f32 * step, len))
                .filter(|&(s, l)| s < visible.1 && s + l > visible.0)
                .collect()
        }
    }
}

/// The smallest tile worth drawing, in CSS pixels; below it the pattern is
/// invisible and the tile count explodes.
pub(super) const MIN_TILE: f32 = 0.5;

/// Most tiles along one axis. Past it nothing is drawn: the count would be
/// absurd and the step may be too small for `f32` to advance.
pub(super) const MAX_SPANS: usize = 4096;

fn repeated(start: f32, len: f32, step: f32, visible: (f32, f32)) -> Vec<(f32, f32)> {
    if !(step >= MIN_TILE && len >= MIN_TILE && step.is_finite() && start.is_finite()) {
        return Vec::new();
    }
    // The count is known before the first push, so a huge one never allocates.
    let first = start + ((visible.0 - start) / step).floor() * step;
    let count = (visible.1 - first) / step;
    if count.is_nan() || count > MAX_SPANS as f32 {
        return Vec::new();
    }
    let mut cursor = first;
    let mut spans = Vec::new();
    while cursor < visible.1 && spans.len() < MAX_SPANS {
        if cursor + len > visible.0 {
            spans.push((cursor, len));
        }
        let next = cursor + step;
        // A step below the float spacing at this magnitude does not advance.
        if next <= cursor {
            return Vec::new();
        }
        cursor = next;
    }
    spans
}

pub(super) fn near(a: &Rect, b: &Rect) -> bool {
    (a.x - b.x).abs() < EPSILON
        && (a.y - b.y).abs() < EPSILON
        && (a.w - b.w).abs() < EPSILON
        && (a.h - b.h).abs() < EPSILON
}

pub(super) fn contains(outer: &Rect, inner: &Rect) -> bool {
    inner.x >= outer.x - EPSILON
        && inner.y >= outer.y - EPSILON
        && inner.right() <= outer.right() + EPSILON
        && inner.bottom() <= outer.bottom() + EPSILON
}
