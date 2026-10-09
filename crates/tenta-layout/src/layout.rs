//! The layout driver: initial containing block and root sizing.

use crate::block::layout_block;
use crate::box_tree::{BoxTree, LayoutCx};
use crate::constraint::own_direction;
use crate::fragment::{FragFlags, FragId, FragmentBuilder, FragmentKind, FragmentTree};
use crate::space::{AvailableSize, ConstraintSpace};
use crate::types::Rect;

/// Lays out the whole box tree against the viewport in `cx` and returns the
/// fragment tree. Deterministic: equal inputs give equal fragments.
///
/// The root box is laid out inside the initial containing block, which has the
/// viewport's size and the root's own `direction`. The scroll extent is the
/// viewport grown to cover the root's scrollable overflow; overflow towards the
/// top or left cannot be scrolled to and is ignored.
pub fn layout(cx: &mut LayoutCx, tree: &BoxTree) -> FragmentTree {
    let (width, height) = cx.viewport;
    let viewport = Rect::new(0.0, 0.0, width, height);
    let mut space = ConstraintSpace::new(
        AvailableSize::Definite(width),
        AvailableSize::Definite(height),
    );
    space.percentage_base = (Some(width), Some(height));
    space.direction = own_direction(&tree.get(tree.root).style);

    let mut out = FragmentBuilder::new();
    let result = layout_block(cx, tree, tree.root, &space, &mut out);
    let root = result.frag;
    let root_fragment = out.get_mut(root);
    root_fragment.rect.y += result.margin_before.collapsed();
    root_fragment.flags |= FragFlags::STACKING_CONTEXT;
    let extent = root_fragment
        .overflow
        .translated(root_fragment.rect.x, root_fragment.rect.y);

    if let Some(source) = tree.canvas_source(cx.doc) {
        let node = tree.get(source).node;
        let body = (0..out.len() as u32)
            .map(FragId)
            .find(|&id| out.get(id).node == node && out.get(id).kind == FragmentKind::Box);
        if let Some(id) = body {
            out.get_mut(id).flags |= FragFlags::CANVAS_SOURCE;
        }
    }

    let scroll_extent = Rect::new(
        0.0,
        0.0,
        width.max(extent.right()),
        height.max(extent.bottom()),
    );
    out.finish(root, viewport, scroll_extent)
}
