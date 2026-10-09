//! Atomic inlines: inline-block and replaced baselines.

use crate::box_tree::{BoxId, BoxTree, LayoutCx};
use crate::fragment::{FragId, FragmentBuilder};
use crate::space::ConstraintSpace;

/// Lays out the atomic inline `box_id` as a unit and returns its fragment and
/// its baseline from the fragment's top, when it has one.
pub(super) fn layout_atomic(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    box_id: BoxId,
    space: &ConstraintSpace,
    out: &mut FragmentBuilder,
) -> (FragId, Option<f32>) {
    let _ = (cx, tree, box_id, space, out);
    todo!("L3b")
}
