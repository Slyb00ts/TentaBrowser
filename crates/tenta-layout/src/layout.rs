//! The layout driver: initial containing block and root sizing.

use crate::box_tree::{BoxTree, LayoutCx};
use crate::fragment::FragmentTree;

/// Lays out the whole box tree against the viewport in `cx` and returns the
/// fragment tree. Deterministic: equal inputs give equal fragments.
pub fn layout(cx: &mut LayoutCx, tree: &BoxTree) -> FragmentTree {
    let _ = (cx, tree);
    todo!("L2a")
}
