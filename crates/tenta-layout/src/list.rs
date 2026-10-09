//! List-item layout and marker placement.

use crate::box_tree::{BoxId, BoxTree, LayoutCx};
use crate::fragment::{FragId, FragmentBuilder};

/// Lays out the outside marker box `marker` of a list item and returns its
/// fragment, positioned relative to the list item's border box.
pub(crate) fn layout_marker(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    marker: BoxId,
    out: &mut FragmentBuilder,
) -> FragId {
    let _ = (cx, tree, marker, out);
    todo!("L2b")
}
