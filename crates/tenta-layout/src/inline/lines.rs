//! Line breaking and line-box construction.

use smallvec::SmallVec;

use super::{atomic, bidi, ellipsis, valign};
use crate::box_tree::{BoxTree, InlineFormattingContext, InlineItem, LayoutCx};
use crate::fragment::{FragId, Fragment, FragmentBuilder, FragmentKind};
use crate::space::{AvailableSize, ConstraintSpace, FloatContext, InlineResult};
use crate::types::Rect;
use tenta_core::NodeId;

/// Breaks `ifc` into lines and builds their fragments. The calls to the
/// atomic-inline, vertical-alignment, bidi and ellipsis steps mark where those
/// rules plug into line construction.
pub(super) fn build_lines(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    ifc: &InlineFormattingContext,
    space: &ConstraintSpace,
    _floats: &mut dyn FloatContext,
    out: &mut FragmentBuilder,
) -> InlineResult {
    let width = match space.inline_size {
        AvailableSize::Definite(w) => w,
        AvailableSize::MinContent | AvailableSize::MaxContent => 0.0,
    };
    let mut children: SmallVec<[FragId; 4]> = SmallVec::new();
    for item in &ifc.items {
        if let InlineItem::Atomic { box_id } = item {
            let (frag, _baseline) = atomic::layout_atomic(cx, tree, *box_id, space, out);
            children.push(frag);
        }
    }
    bidi::reorder_line(space.direction, &mut children);
    let (ascent, descent) = valign::align_line(tree, &children, out);
    let height = ascent + descent;
    let line = out.push(Fragment::new(
        NodeId::NONE,
        FragmentKind::Line,
        tree.get(ifc.root).paint.clone(),
        Rect::new(0.0, 0.0, width, height),
    ));
    out.set_children(line, children);
    ellipsis::truncate_line(tree, ifc.root, line, width, out);
    InlineResult {
        lines: SmallVec::from_slice(&[line]),
        block_size: height,
        first_baseline: Some(ascent),
        last_baseline: Some(ascent),
        overflow: Rect::new(0.0, 0.0, width, height),
        inline_extent: width,
    }
}
