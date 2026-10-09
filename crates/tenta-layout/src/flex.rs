//! Flex containers: Taffy's flexbox algorithm over the box tree.
//!
//! The container's own box model is resolved like a block's (so auto margins,
//! `min-width` and friends behave identically), then Taffy distributes the
//! space among the items. Items are measured and finally laid out by this
//! crate's block, inline and replaced algorithms through [`TreeView`].

use taffy::{
    AvailableSpace, LayoutFlexboxContainer, LayoutInput, Line, RequestedAxis, RunMode, Size,
    SizingMode, Style, compute_flexbox_layout,
};

use style::Atom;

use crate::block::{child_overflow, fragment_flags, relative_shift};
use crate::block_sizes;
use crate::box_tree::{BoxId, BoxTree, LayoutCx};
use crate::constraint;
use crate::fragment::{Fragment, FragmentBuilder, FragmentKind};
use crate::space::{AvailableSize, BlockResult, ConstraintSpace, MarginStrut};
use crate::taffy_tree::{TreeView, node_id};
use crate::types::Rect;

impl LayoutFlexboxContainer for TreeView<'_, '_> {
    type FlexboxContainerStyle<'s>
        = &'s Style<Atom>
    where
        Self: 's;
    type FlexboxItemStyle<'s>
        = &'s Style<Atom>
    where
        Self: 's;

    fn get_flexbox_container_style(&self, node: taffy::NodeId) -> Self::FlexboxContainerStyle<'_> {
        self.style_of_node(node)
    }

    fn get_flexbox_child_style(&self, node: taffy::NodeId) -> Self::FlexboxItemStyle<'_> {
        self.style_of_node(node)
    }
}

/// The `(min-content, max-content)` inline sizes of a flex container's content
/// box, as the container would size itself under each constraint.
pub(crate) fn intrinsic_inline_sizes(cx: &mut LayoutCx, tree: &BoxTree, id: BoxId) -> (f32, f32) {
    let frame = block_sizes::frame_of(tree, id, 0.0);
    let edges = frame.padding.horizontal() + frame.border.horizontal();
    let mut view = TreeView::new(cx, tree, id);
    let mut width_under = |space: AvailableSpace| {
        let output = compute_flexbox_layout(
            &mut view,
            node_id(id),
            LayoutInput {
                run_mode: RunMode::ComputeSize,
                sizing_mode: SizingMode::ContentSize,
                axis: RequestedAxis::Horizontal,
                known_dimensions: Size::NONE,
                known_dimensions_are_definite: Size {
                    width: true,
                    height: true,
                },
                parent_size: Size::NONE,
                available_space: Size {
                    width: space,
                    height: AvailableSpace::MaxContent,
                },
                vertical_margins_are_collapsible: Line::FALSE,
            },
        );
        (output.size.width - edges).max(0.0)
    };
    (
        width_under(AvailableSpace::MinContent),
        width_under(AvailableSpace::MaxContent),
    )
}

/// Lays out the flex container `id` inside `space` and pushes its fragment.
///
/// Same contract as [`crate::layout_block`]: the fragment's `rect.x` is the
/// container's left margin plus any `position: relative` offset, and `rect.y`
/// only the relative offset.
pub(crate) fn layout_flex(
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
    let output = compute_flexbox_layout(&mut view, node_id(id), inputs);
    let border_box = Rect::new(0.0, 0.0, output.size.width, output.size.height);
    let percent_base = Size {
        width: Some((border_box.w - m.border.horizontal() - m.padding.horizontal()).max(0.0)),
        height: definite_height,
    };
    let kids = view.emit_items(percent_base, out);

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
    let baseline = output.baselines.first;
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
