//! `box-shadow` items.

use tenta_layout::{Fragment, Rect};

use super::border::{inset_radii, inset_rect, resolve_radii};
use crate::dl::Item;

/// Pushes the outer `box-shadow` items of `frag`, bottom shadow first, so they
/// land under the background.
pub(super) fn emit_outer(frag: &Fragment, border_box: Rect, items: &mut Vec<Item>) {
    if border_box.w <= 0.0 || border_box.h <= 0.0 {
        return;
    }
    let radii = resolve_radii(&frag.paint.border.radii, border_box);
    for shadow in frag.paint.box_shadow.iter().rev() {
        if shadow.inset || shadow.color.is_transparent() {
            continue;
        }
        items.push(Item::BoxShadow {
            rect: border_box,
            radii,
            offset: shadow.offset,
            blur: shadow.blur,
            spread: shadow.spread,
            color: shadow.color,
            inset: false,
        });
    }
}

/// Pushes the `inset` shadow items of `frag`, bottom shadow first. An inset
/// shadow is drawn inside the padding box, so that box and its inner radii are
/// what the item carries.
pub(super) fn emit_inset(frag: &Fragment, border_box: Rect, items: &mut Vec<Item>) {
    let padding_box = inset_rect(border_box, &frag.border);
    if padding_box.w <= 0.0 || padding_box.h <= 0.0 {
        return;
    }
    let radii = inset_radii(
        resolve_radii(&frag.paint.border.radii, border_box),
        &frag.border,
    );
    for shadow in frag.paint.box_shadow.iter().rev() {
        if !shadow.inset || shadow.color.is_transparent() {
            continue;
        }
        items.push(Item::BoxShadow {
            rect: padding_box,
            radii,
            offset: shadow.offset,
            blur: shadow.blur,
            spread: shadow.spread,
            color: shadow.color,
            inset: true,
        });
    }
}
