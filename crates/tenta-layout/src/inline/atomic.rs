//! Atomic inlines: inline-block and replaced baselines.

use style::properties::longhands::box_sizing::computed_value::T as BoxSizing;
use style::values::computed::{Length, LengthPercentage, MaxSize, Size};
use style::values::specified::box_::Contain;
use tenta_core::NodeId;

use crate::block::layout_block;
use crate::block_sizes::{frame_of, intrinsic_inline_contribution};
use crate::box_tree::{BoxId, BoxKind, BoxTree, LayoutBox, LayoutCx};
use crate::constraint::resolve_box;
use crate::fragment::{FragId, FragmentBuilder, FragmentKind};
use crate::space::{AvailableSize, ConstraintSpace};
use crate::types::OverflowKind;

/// Room offered to a box whose width does not depend on it. Large enough that
/// no auto margin or over-constrained equation is decided by it, small enough
/// to keep `f32` arithmetic exact to a fraction of a pixel.
const UNCONSTRAINED: f32 = 1.0e6;

/// Lays out the atomic inline `box_id` as a unit and returns its fragment and
/// its baseline from the fragment's top, when it has one.
///
/// The fragment's `rect.x` and `rect.y` are only the `position: relative`
/// offset: the line adds the position of the margin box. An inline-block's
/// baseline is that of its last line box; a scroll container, a box that has
/// no line box, is replaced or contains its layout sits on the baseline with its
/// bottom margin edge, which is the `None` answer.
pub(crate) fn layout_atomic(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    box_id: BoxId,
    space: &ConstraintSpace,
    out: &mut FragmentBuilder,
) -> (FragId, Option<f32>) {
    let b = tree.get(box_id);
    let width = border_box_width(cx, tree, box_id, space);
    let own = ConstraintSpace {
        inline_size: AvailableSize::Definite(width),
        fixed_inline: true,
        ..*space
    };
    let result = layout_block(cx, tree, box_id, &own, out);
    let fragment = out.get_mut(result.frag);
    // Block layout offsets a box by its left margin; the line places the
    // margin box and adds that margin itself.
    fragment.rect.x -= fragment.margin.left;
    if let BoxKind::Marker(kind) = b.kind {
        // A marker shares its list item's node, and a geometry query for the
        // item must find the item's own box first.
        fragment.node = NodeId::NONE;
        fragment.kind = FragmentKind::Marker(kind);
    }

    let baseline = if suppresses_baseline(b) {
        None
    } else {
        result.last_baseline
    };
    (result.frag, baseline)
}

/// Whether the box sits on the baseline with its bottom margin edge: it is
/// replaced, a scroll container (`overflow: clip` is not one), or its layout is
/// contained.
fn suppresses_baseline(b: &LayoutBox) -> bool {
    let scrolls = |o: OverflowKind| {
        matches!(
            o,
            OverflowKind::Hidden | OverflowKind::Scroll | OverflowKind::Auto
        )
    };
    let contain = b.style.get_box().clone_contain();
    matches!(b.kind, BoxKind::Replaced(_))
        || scrolls(b.paint.overflow_x)
        || scrolls(b.paint.overflow_y)
        || contain.intersects(Contain::LAYOUT)
}

/// The border-box width an atomic inline gets in `space`: the width the style
/// asks for, else (CSS 2.1 section 10.3.9) the preferred width limited by the
/// available width but not below the minimum content width.
fn border_box_width(cx: &mut LayoutCx, tree: &BoxTree, id: BoxId, space: &ConstraintSpace) -> f32 {
    let b = tree.get(id);
    let definite = match space.inline_size {
        AvailableSize::Definite(w) => Some(w),
        _ => None,
    };
    // A percentage width needs a width to resolve against; without one it
    // behaves as `auto`.
    let relative_width = (definite.is_some() || space.percentage_base.0.is_some())
        && matches!(&b.style.get_position().width, Size::LengthPercentage(lp) if lp.0.has_percentage());
    if matches!(b.kind, BoxKind::Replaced(_)) || relative_width {
        let roomy = ConstraintSpace {
            inline_size: AvailableSize::Definite(UNCONSTRAINED),
            percentage_base: (
                space.percentage_base.0.or(definite),
                space.percentage_base.1,
            ),
            ..*space
        };
        return resolve_box(cx, tree, id, &roomy).border_box_width();
    }
    let (min, max) = intrinsic_inline_contribution(cx, tree, id);
    let basis = space.percentage_base.0.or(definite).unwrap_or(0.0);
    let margin = frame_of(tree, id, basis).margin;
    let available = match space.inline_size {
        AvailableSize::Definite(w) => w,
        AvailableSize::MinContent => 0.0,
        AvailableSize::MaxContent => f32::INFINITY,
    };
    let fitted = (available - margin.left - margin.right).max(min).min(max);
    clamp_to_percentages(b, fitted, space.percentage_base.0.or(definite), basis)
}

/// Applies a percentage `min-width` and `max-width` to the border-box width
/// `width`. They need the containing block's width, so the intrinsic sizes that
/// the width was fitted from leave them out; without a width they are `auto`.
fn clamp_to_percentages(b: &LayoutBox, width: f32, base: Option<f32>, padding_basis: f32) -> f32 {
    let Some(base) = base else { return width };
    let pos = b.style.get_position();
    let percent = |lp: &LengthPercentage| {
        lp.has_percentage()
            .then(|| lp.resolve(Length::new(base)).px())
    };
    let mut frame_width = b.paint.border.widths.horizontal();
    let padding = b.style.get_padding();
    for side in [&padding.padding_left.0, &padding.padding_right.0] {
        frame_width += side.resolve(Length::new(padding_basis)).px().max(0.0);
    }
    let to_border = |content_or_border: f32| {
        if matches!(pos.box_sizing, BoxSizing::BorderBox) {
            content_or_border
        } else {
            content_or_border + frame_width
        }
    };
    let mut width = width;
    if let MaxSize::LengthPercentage(lp) = &pos.max_width
        && let Some(max) = percent(&lp.0)
    {
        width = width.min(to_border(max));
    }
    if let Size::LengthPercentage(lp) = &pos.min_width
        && let Some(min) = percent(&lp.0)
    {
        width = width.max(to_border(min));
    }
    width.max(frame_width)
}
