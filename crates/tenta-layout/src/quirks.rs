//! Quirks-mode layout rules for block flow.

use crate::box_tree::{BoxId, BoxTree, LayoutCx};

/// Applies the quirks-mode rule that `html` and `body` fill the viewport
/// height. Returns `content_height` unchanged outside quirks mode and for
/// other boxes.
pub(crate) fn fill_viewport_height(
    cx: &LayoutCx,
    tree: &BoxTree,
    id: BoxId,
    content_height: f32,
) -> f32 {
    let _ = (cx, tree, id, content_height);
    todo!("L2b")
}
