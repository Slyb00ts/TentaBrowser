//! Intrinsic block sizing and size constraints.

use crate::box_tree::{BoxId, BoxTree, LayoutCx};
use crate::space::ConstraintSpace;

/// The `(min-content, max-content)` inline sizes of the block box `id`, memoised
/// in [`crate::LayoutBox::intrinsic_inline`]. Boxes with an inline formatting
/// context delegate to [`crate::intrinsic_inline_content_sizes`].
pub fn intrinsic_inline_sizes_of_block(cx: &mut LayoutCx, tree: &BoxTree, id: BoxId) -> (f32, f32) {
    let _ = (cx, tree, id);
    todo!("L2b")
}

/// Applies `min-height`/`max-height` and percentage heights to the used
/// `content_height` of box `id`.
pub(crate) fn constrain_block_size(
    cx: &LayoutCx,
    tree: &BoxTree,
    id: BoxId,
    space: &ConstraintSpace,
    content_height: f32,
) -> f32 {
    let _ = (cx, tree, id, space, content_height);
    todo!("L2b")
}
