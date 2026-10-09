//! Block formatting context layout.

use smallvec::SmallVec;

use crate::box_tree::{BoxId, BoxKind, BoxTree, LayoutCx};
use crate::fragment::{FragId, Fragment, FragmentBuilder, FragmentKind};
use crate::inline::layout_inline;
use crate::space::{AvailableSize, BlockResult, ConstraintSpace, MarginStrut, NoFloats};
use crate::types::Rect;
use crate::{block_sizes, list, quirks, relative};

/// Lays out the block box `id` within `space`, pushing its fragment (and its
/// descendants') into `out`.
///
/// Children are laid out recursively and the box's inline formatting context,
/// when it has one, goes through [`layout_inline`]. The step order here is the
/// seam to the sizing, list, quirks and relative-position rules, which live in
/// their own modules.
pub fn layout_block(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    id: BoxId,
    space: &ConstraintSpace,
    out: &mut FragmentBuilder,
) -> BlockResult {
    let b = tree.get(id);
    let width = match space.inline_size {
        AvailableSize::Definite(w) => w,
        AvailableSize::MinContent | AvailableSize::MaxContent => 0.0,
    };
    let mut kids: SmallVec<[FragId; 4]> = SmallVec::new();
    if let BoxKind::ListItem {
        marker: Some(marker),
    } = &b.kind
    {
        kids.push(list::layout_marker(cx, tree, *marker, out));
    }
    let mut height = 0.0;
    for &child in &b.children {
        let result = layout_block(cx, tree, child, space, out);
        let frag = out.get_mut(result.frag);
        frag.rect.y = height;
        height += frag.rect.h;
        kids.push(result.frag);
    }
    if let Some(ifc) = &b.ifc {
        let mut floats = NoFloats;
        let inline = layout_inline(cx, tree, ifc, space, &mut floats, out);
        height += inline.block_size;
        kids.extend(inline.lines);
    }
    let height = quirks::fill_viewport_height(cx, tree, id, height);
    let height = block_sizes::constrain_block_size(cx, tree, id, space, height);
    let (dx, dy) = relative::relative_offset(tree, id, (width, height));

    let frag = out.push(Fragment::new(
        b.node,
        FragmentKind::Box,
        b.paint.clone(),
        Rect::new(dx, dy, width, height),
    ));
    out.set_children(frag, kids);
    BlockResult {
        frag,
        margin_before: MarginStrut::default(),
        margin_after: MarginStrut::default(),
        first_baseline: None,
        last_baseline: None,
        floats_out: Vec::new(),
        exclusions: Vec::new(),
        self_collapsing: false,
    }
}
