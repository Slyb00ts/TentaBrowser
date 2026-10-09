//! `vertical-align` and its effect on line-box height.

use crate::box_tree::BoxTree;
use crate::fragment::{FragId, FragmentBuilder};

/// Aligns the fragments of one line against the line's baseline, moving them
/// vertically, and returns the line box's `(ascent, descent)`.
pub(super) fn align_line(
    tree: &BoxTree,
    children: &[FragId],
    out: &mut FragmentBuilder,
) -> (f32, f32) {
    let _ = (tree, children, out);
    todo!("L3b")
}
