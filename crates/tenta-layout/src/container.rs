//! Shared container entry: resolves a flex or grid container like a block, runs
//! Taffy, and makes the fragments.

use taffy::{AvailableSpace, LayoutInput, Line, RequestedAxis, RunMode, Size, SizingMode};

use crate::block::{child_overflow, fragment_flags, relative_shift};
use crate::block_sizes;
use crate::box_tree::{BoxId, BoxKind, BoxTree, LayoutCx};
use crate::constraint;
use crate::fragment::{Fragment, FragmentBuilder, FragmentKind};
use crate::space::{AvailableSize, BlockResult, ConstraintSpace, MarginStrut};
use crate::taffy_tree::TreeView;
use crate::types::Rect;

/// Lays out the flex or grid container `id` inside `space` and pushes its fragment.
///
/// Same contract as [`crate::layout_block`]: the fragment's `rect.x` is the
/// container's left margin plus any `position: relative` offset, and `rect.y`
/// only the relative offset.
pub(crate) fn layout_container(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    id: BoxId,
    space: &ConstraintSpace,
    out: &mut FragmentBuilder,
) -> BlockResult {
    let b = tree.get(id);
    let m = constraint::resolve_box(cx, tree, id, space);
    let definite_height = match m.content_height {
        Some(h) if constraint::has_block_size_constraints(&b.style) => {
            Some(block_sizes::constrain_block_size(cx, tree, id, space, h))
        }
        other => other,
    };
    let border_width = m.border_box_width();
    let known = Size {
        width: Some(border_width),
        height: definite_height.map(|h| m.border_box_height(h)),
    };
    let definite_inline = match space.inline_size {
        AvailableSize::Definite(w) => Some(w),
        _ => None,
    };
    let inputs = LayoutInput {
        run_mode: RunMode::PerformLayout,
        sizing_mode: SizingMode::ContentSize,
        axis: RequestedAxis::Both,
        known_dimensions: known,
        known_dimensions_are_definite: Size {
            width: true,
            height: true,
        },
        parent_size: Size {
            width: space.percentage_base.0.or(definite_inline),
            height: space.percentage_base.1,
        },
        available_space: Size {
            width: AvailableSpace::Definite(border_width),
            height: match space.block_size {
                AvailableSize::Definite(h) => AvailableSpace::Definite(h),
                AvailableSize::MinContent => AvailableSpace::MinContent,
                AvailableSize::MaxContent => AvailableSpace::MaxContent,
            },
        },
        vertical_margins_are_collapsible: Line::FALSE,
    };

    let mut view = TreeView::new(cx, tree, id);
    let output = view.solve(id, inputs);
    let border_box = Rect::new(0.0, 0.0, output.size.width, output.size.height);
    let percent_base = Size {
        width: Some((border_box.w - m.border.horizontal() - m.padding.horizontal()).max(0.0)),
        height: definite_height,
    };
    let kids = view.emit_items(percent_base, out);
    let baseline = if matches!(b.kind, BoxKind::Grid) {
        view.grid_first_baseline()
    } else {
        output.baselines.first
    };

    let (dx, dy) = relative_shift(tree, id, space);
    let rect = Rect::new(m.margin.left + dx, dy, border_box.w, border_box.h);
    let mut fragment = Fragment::new(
        b.node,
        FragmentKind::Box,
        tree.paint_style_for(id, border_box),
        rect,
    );
    fragment.border = m.border;
    fragment.padding = m.padding;
    fragment.margin = m.margin;
    fragment.flags = fragment_flags(cx, tree, id);
    fragment.overflow = child_overflow(out, &kids, border_box);
    fragment.children = kids;
    let frag = out.push(fragment);

    let mut margin_before = MarginStrut::default();
    margin_before.append(m.margin.top);
    let mut margin_after = MarginStrut::default();
    margin_after.append(m.margin.bottom);
    // Inline alignment uses the last baseline of a block, but a flex container
    // takes its baseline from the first item whichever one is asked for.
    BlockResult {
        frag,
        margin_before,
        margin_after,
        first_baseline: baseline,
        last_baseline: baseline,
        floats_out: Vec::new(),
        exclusions: Vec::new(),
        self_collapsing: false,
    }
}
