//! Replaced content and image background layers.
//!
//! No fragment or image key resolves to a decoded image yet, so nothing is
//! drawable and [`images_drawable`] says so. The walk checks it before every
//! call below; the bodies are filled in together with the flag that turns it on,
//! and until then pages and tests never reach them.

use tenta_layout::{BackgroundLayer, Fragment, Rect};

use crate::PaintContext;
use crate::dl::{Item, Radii};

/// Whether the image store can answer for replaced content and image layers.
pub(super) fn images_drawable(cx: &PaintContext) -> bool {
    let _ = cx;
    false
}

/// Pushes the content of a replaced fragment.
pub(crate) fn emit(frag: &Fragment, border_box: Rect, cx: &PaintContext, items: &mut Vec<Item>) {
    let _ = (frag, border_box, cx, items);
    todo!("M2")
}

/// Pushes one image background layer clipped to `clip` with `clip_radii`;
/// `origin` is the positioning area.
pub(super) fn emit_background(
    layer: &BackgroundLayer,
    origin: Rect,
    clip: Rect,
    clip_radii: Radii,
    cx: &PaintContext,
    items: &mut Vec<Item>,
) {
    let _ = (layer, origin, clip, clip_radii, cx, items);
    todo!("M2")
}
