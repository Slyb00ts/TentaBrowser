//! Borders, outlines and radius resolution.

use tenta_layout::{BorderStyle, Edges, Fragment, LengthPercent, Rect};

use crate::dl::{Item, Radii};

/// Resolves the `border-radius` of `frag` against its border box.
///
/// Horizontal radii resolve against the width and vertical ones against the
/// height. Radii that would overlap on a side shrink together, so the result
/// can be handed to any item without further clamping.
pub(super) fn resolve_radii(radii: &[LengthPercent; 8], border_box: Rect) -> Radii {
    let mut out = [0.0; 8];
    let mut any = false;
    for (i, radius) in radii.iter().enumerate() {
        let basis = if i % 2 == 0 {
            border_box.w
        } else {
            border_box.h
        };
        out[i] = radius.resolve(basis).max(0.0);
        any |= out[i] > 0.0;
    }
    if !any {
        return out;
    }
    let sides = [
        (border_box.w, out[0] + out[2]),
        (border_box.h, out[3] + out[5]),
        (border_box.w, out[4] + out[6]),
        (border_box.h, out[7] + out[1]),
    ];
    let factor = sides
        .iter()
        .filter(|(_, sum)| *sum > 0.0)
        .map(|(len, sum)| len.max(0.0) / sum)
        .fold(1.0_f32, f32::min);
    if factor < 1.0 {
        out.iter_mut().for_each(|r| *r *= factor);
    }
    out
}

/// The radii of an inner edge: each corner loses the widths of the two sides
/// it sits against, never going below zero.
pub(super) fn inset_radii(radii: Radii, by: &Edges) -> Radii {
    let shrink = |r: f32, w: f32| (r - w).max(0.0);
    [
        shrink(radii[0], by.left),
        shrink(radii[1], by.top),
        shrink(radii[2], by.right),
        shrink(radii[3], by.top),
        shrink(radii[4], by.right),
        shrink(radii[5], by.bottom),
        shrink(radii[6], by.left),
        shrink(radii[7], by.bottom),
    ]
}

/// The rectangle left after removing `by` from every side.
pub(super) fn inset_rect(rect: Rect, by: &Edges) -> Rect {
    Rect::new(
        rect.x + by.left,
        rect.y + by.top,
        (rect.w - by.horizontal()).max(0.0),
        (rect.h - by.vertical()).max(0.0),
    )
}

fn draws(style: BorderStyle) -> bool {
    !matches!(style, BorderStyle::None | BorderStyle::Hidden)
}

/// Pushes the border item of `frag`, when any side has something to draw.
pub(super) fn emit(frag: &Fragment, border_box: Rect, items: &mut Vec<Item>) {
    let border = &frag.paint.border;
    let used = [
        frag.border.top,
        frag.border.right,
        frag.border.bottom,
        frag.border.left,
    ];
    let mut widths = [0.0; 4];
    let mut visible = false;
    for side in 0..4 {
        if draws(border.styles[side]) && used[side] > 0.0 && !border.colors[side].is_transparent() {
            widths[side] = used[side];
            visible = true;
        }
    }
    if !visible {
        return;
    }
    items.push(Item::Border {
        rect: border_box,
        widths: Edges {
            top: widths[0],
            right: widths[1],
            bottom: widths[2],
            left: widths[3],
        },
        colors: border.colors,
        styles: border.styles,
        radii: resolve_radii(&border.radii, border_box),
    });
}

/// Pushes the outline item of `frag`. The outline hugs the border edge pushed
/// out by `outline-offset` and follows the rounded corners.
pub(super) fn emit_outline(frag: &Fragment, border_box: Rect, items: &mut Vec<Item>) {
    let Some(outline) = frag.paint.outline else {
        return;
    };
    if outline.width <= 0.0 || !draws(outline.style) || outline.color.is_transparent() {
        return;
    }
    let gap = outline.offset + outline.width;
    let rect = Rect::new(
        border_box.x - gap,
        border_box.y - gap,
        border_box.w + 2.0 * gap,
        border_box.h + 2.0 * gap,
    );
    if rect.w <= 0.0 || rect.h <= 0.0 {
        return;
    }
    let mut radii = resolve_radii(&frag.paint.border.radii, border_box);
    radii
        .iter_mut()
        .filter(|r| **r > 0.0)
        .for_each(|r| *r = (*r + gap).max(0.0));
    items.push(Item::Border {
        rect,
        widths: Edges::all(outline.width),
        colors: [outline.color; 4],
        styles: [outline.style; 4],
        radii,
    });
}
