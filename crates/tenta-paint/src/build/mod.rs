//! The display-list builder: fragment tree to [`DisplayList`].

mod background;
mod border;
mod clip;
mod effects;
mod marker;
mod shadow;
mod stacking;
mod text;

use tenta_layout::FragmentTree;

use crate::dl::DisplayList;
use crate::image::ImageStore;

/// What the builder reads besides the fragments.
#[derive(Copy, Clone)]
pub struct PaintContext<'a> {
    /// Decoded images, for intrinsic sizes and `Item::Image`.
    pub images: &'a ImageStore,
}

/// Builds the display list for `frags` in CSS 2.1 Appendix E painting order.
///
/// Every `Push*` item in the result is closed by its `Pop*`.
pub fn build(frags: &FragmentTree, cx: &PaintContext) -> DisplayList {
    let mut items = Vec::new();
    stacking::paint_fragment(frags, frags.root, (0.0, 0.0), cx, &mut items);
    DisplayList {
        items,
        bounds: frags.scroll_extent,
    }
}
