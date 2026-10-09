//! `position: relative` offsets.

use crate::box_tree::{BoxId, BoxTree};

/// The `(dx, dy)` offset `position: relative` applies to box `id`, resolving
/// percentages against `containing`, the containing block's content size.
/// `(0, 0)` for boxes that are not relatively positioned.
pub(crate) fn relative_offset(tree: &BoxTree, id: BoxId, containing: (f32, f32)) -> (f32, f32) {
    let _ = (tree, id, containing);
    todo!("L2b")
}
