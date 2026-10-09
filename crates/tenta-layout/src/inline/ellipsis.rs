//! `text-overflow: ellipsis`.

use crate::box_tree::{BoxId, BoxTree};
use crate::fragment::{FragId, FragmentBuilder};

/// Truncates `line`, a line of the block container `container`, with an
/// ellipsis when `text-overflow: ellipsis` applies and the content exceeds
/// `available`. Returns whether it truncated.
pub(super) fn truncate_line(
    tree: &BoxTree,
    container: BoxId,
    line: FragId,
    available: f32,
    out: &mut FragmentBuilder,
) -> bool {
    let _ = (tree, container, line, available, out);
    todo!("L3b")
}
