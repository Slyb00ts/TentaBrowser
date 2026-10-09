//! Intrinsic block sizing and size constraints.
//!
//! Every size here is a content-box size unless the name says otherwise:
//! `intrinsic_inline_sizes_of_block` answers for the content box, and
//! [`intrinsic_inline_contribution`] is the border-box figure a parent adds up
//! when it shrinks to fit its children.

use style::properties::ComputedValues;
use style::properties::longhands::box_sizing::computed_value::T as BoxSizing;
use style::values::computed::{Length, LengthPercentage, Margin, MaxSize, Size};
use tenta_core::NodeId;

use crate::box_tree::{BoxFlags, BoxId, BoxKind, BoxTree, LayoutCx};
use crate::inline::intrinsic_inline_content_sizes;
use crate::quirks;
use crate::space::ConstraintSpace;
use crate::types::{Edges, IntrinsicSize, ReplacedKind};

/// A length-percentage in CSS pixels. A percentage needs `basis`; without one
/// the value is unresolvable and `None` is returned.
fn resolve(lp: &LengthPercentage, basis: Option<f32>) -> Option<f32> {
    if lp.has_percentage() {
        basis.map(|b| lp.resolve(Length::new(b)).px())
    } else {
        lp.to_length().map(Length::px)
    }
}

/// A length-percentage that is a plain length.
fn fixed(lp: &LengthPercentage) -> Option<f32> {
    lp.to_length().map(Length::px)
}

fn size_lp(size: &Size) -> Option<&LengthPercentage> {
    match size {
        Size::LengthPercentage(lp) => Some(&lp.0),
        _ => None,
    }
}

fn max_size_lp(size: &MaxSize) -> Option<&LengthPercentage> {
    match size {
        MaxSize::LengthPercentage(lp) => Some(&lp.0),
        _ => None,
    }
}

fn is_border_box(style: &ComputedValues) -> bool {
    matches!(style.get_position().box_sizing, BoxSizing::BorderBox)
}

/// Converts a specified size to a content-box size.
fn to_content(value: f32, border_box: bool, edges: f32) -> f32 {
    if border_box {
        (value - edges).max(0.0)
    } else {
        value
    }
}

/// Margin, padding and border of a box with percentages resolved against
/// `basis`, the width of the containing block. `auto` margins are zero.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) struct Frame {
    pub(crate) margin: Edges,
    pub(crate) padding: Edges,
    pub(crate) border: Edges,
}

pub(crate) fn frame_of(tree: &BoxTree, id: BoxId, basis: f32) -> Frame {
    let b = tree.get(id);
    let padding_of = |lp: &LengthPercentage| resolve(lp, Some(basis)).unwrap_or(0.0).max(0.0);
    let margin_of = |m: &Margin| match m {
        Margin::LengthPercentage(lp) => resolve(lp, Some(basis)).unwrap_or(0.0),
        _ => 0.0,
    };
    let p = b.style.get_padding();
    let m = b.style.get_margin();
    Frame {
        margin: Edges {
            top: margin_of(&m.margin_top),
            right: margin_of(&m.margin_right),
            bottom: margin_of(&m.margin_bottom),
            left: margin_of(&m.margin_left),
        },
        padding: Edges {
            top: padding_of(&p.padding_top.0),
            right: padding_of(&p.padding_right.0),
            bottom: padding_of(&p.padding_bottom.0),
            left: padding_of(&p.padding_left.0),
        },
        border: b.paint.border.widths,
    }
}

/// The `(min-content, max-content)` inline sizes of the content box of the
/// block box `id`, memoised in [`crate::LayoutBox::intrinsic_inline`]. The
/// box's own `width`, padding and border do not take part: those are what the
/// caller is deciding. Boxes with an inline formatting context delegate to
/// [`crate::intrinsic_inline_content_sizes`].
pub fn intrinsic_inline_sizes_of_block(cx: &mut LayoutCx, tree: &BoxTree, id: BoxId) -> (f32, f32) {
    let b = tree.get(id);
    if let Some(sizes) = b.intrinsic_inline.get() {
        return sizes;
    }
    let sizes = match (&b.kind, &b.ifc) {
        (BoxKind::Replaced(kind), _) => {
            let (w, _) = replaced_content_size(cx, b.node, *kind, (None, None), &SizeLimits::NONE);
            (w, w)
        }
        (_, Some(ifc)) => intrinsic_inline_content_sizes(cx, tree, ifc),
        _ => {
            let mut min = 0.0_f32;
            let mut max = 0.0_f32;
            for &child in &b.children {
                // Floats are left out until float layout exists.
                if tree.get(child).flags.contains(BoxFlags::OUT_OF_FLOW) {
                    continue;
                }
                let margin = frame_of(tree, child, 0.0).margin;
                let extra = margin.left + margin.right;
                let (child_min, child_max) = intrinsic_inline_contribution(cx, tree, child);
                min = min.max(child_min + extra);
                max = max.max(child_max + extra);
            }
            (min, max)
        }
    };
    b.intrinsic_inline.set(Some(sizes));
    sizes
}

/// The border-box `(min-content, max-content)` contribution of box `id` to the
/// intrinsic size of its container, margins excluded: its own definite `width`
/// when it has one, otherwise its content sizes, kept inside `min-width` and
/// `max-width`. Percentages that cannot resolve yet count as `auto`, and a
/// replaced box that can shrink (percentage `width` or `max-width`) has a
/// min-content contribution of only its padding and border.
pub(crate) fn intrinsic_inline_contribution(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    id: BoxId,
) -> (f32, f32) {
    let b = tree.get(id);
    let style = &*b.style;
    let pos = style.get_position();
    let frame = frame_of(tree, id, 0.0);
    let border_box = is_border_box(style);
    let edges = frame.padding.horizontal() + frame.border.horizontal();
    let min_limit = size_lp(&pos.min_width)
        .and_then(fixed)
        .map(|v| to_content(v, border_box, edges));
    let max_limit = max_size_lp(&pos.max_width)
        .and_then(fixed)
        .map(|v| to_content(v, border_box, edges));
    let clamp = |v: f32| {
        let v = max_limit.map_or(v, |m| v.min(m));
        min_limit.map_or(v, |m| v.max(m))
    };
    let width = size_lp(&pos.width);
    let specified = width
        .and_then(fixed)
        .map(|v| to_content(v, border_box, edges));

    let (min, max) = if let BoxKind::Replaced(kind) = b.kind {
        let vertical = frame.padding.vertical() + frame.border.vertical();
        let height = size_lp(&pos.height)
            .and_then(fixed)
            .map(|v| to_content(v, border_box, vertical));
        let limits = replaced_limits(style, (None, None), border_box, (edges, vertical));
        let (w, _) = replaced_content_size(cx, b.node, kind, (specified, height), &limits);
        let shrinks = width.is_some_and(|lp| lp.has_percentage())
            || max_size_lp(&pos.max_width).is_some_and(|lp| lp.has_percentage());
        (if shrinks { 0.0 } else { w }, w)
    } else {
        let (min, max) = intrinsic_inline_sizes_of_block(cx, tree, id);
        match (specified, &pos.width) {
            (Some(w), _) => (clamp(w), clamp(w)),
            (None, Size::MinContent) => (clamp(min), clamp(min)),
            (None, Size::MaxContent) => (clamp(max), clamp(max)),
            _ => (clamp(min), clamp(max)),
        }
    };
    (min + edges, max + edges)
}

/// Limits `min-width`/`max-width`/`min-height`/`max-height` put on a content
/// box. A limit that cannot apply is `0` (minimum) or infinite (maximum).
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct SizeLimits {
    pub(crate) min_w: f32,
    pub(crate) max_w: f32,
    pub(crate) min_h: f32,
    pub(crate) max_h: f32,
}

impl SizeLimits {
    pub(crate) const NONE: SizeLimits = SizeLimits {
        min_w: 0.0,
        max_w: f32::INFINITY,
        min_h: 0.0,
        max_h: f32::INFINITY,
    };

    fn clamp_w(&self, v: f32) -> f32 {
        v.min(self.max_w).max(self.min_w)
    }

    fn clamp_h(&self, v: f32) -> f32 {
        v.min(self.max_h).max(self.min_h)
    }
}

/// Reads the four limits of `style` as content-box sizes. `bases` are the
/// widths and heights percentages resolve against; an indefinite one makes a
/// percentage `min-*` zero and a percentage `max-*` unlimited.
/// `edges` is the horizontal and vertical padding plus border.
pub(crate) fn replaced_limits(
    style: &ComputedValues,
    bases: (Option<f32>, Option<f32>),
    border_box: bool,
    edges: (f32, f32),
) -> SizeLimits {
    let pos = style.get_position();
    let min = |size: &Size, basis: Option<f32>, edges: f32| {
        size_lp(size)
            .and_then(|lp| resolve(lp, basis))
            .map_or(0.0, |v| to_content(v.max(0.0), border_box, edges))
    };
    let max = |size: &MaxSize, basis: Option<f32>, edges: f32| {
        max_size_lp(size)
            .and_then(|lp| resolve(lp, basis))
            .map_or(f32::INFINITY, |v| to_content(v.max(0.0), border_box, edges))
    };
    let min_w = min(&pos.min_width, bases.0, edges.0);
    let min_h = min(&pos.min_height, bases.1, edges.1);
    SizeLimits {
        min_w,
        max_w: max(&pos.max_width, bases.0, edges.0).max(min_w),
        min_h,
        max_h: max(&pos.max_height, bases.1, edges.1).max(min_h),
    }
}

/// The size a replaced element has before any content says otherwise: the
/// default object size for canvas, video, SVG and frames, nothing for images
/// and controls.
fn default_natural_size(kind: ReplacedKind) -> IntrinsicSize {
    match kind {
        ReplacedKind::Canvas | ReplacedKind::Video => IntrinsicSize {
            width: Some(300.0),
            height: Some(150.0),
            ratio: Some(2.0),
        },
        ReplacedKind::Svg | ReplacedKind::Iframe => IntrinsicSize {
            width: Some(300.0),
            height: Some(150.0),
            ratio: None,
        },
        ReplacedKind::Image | ReplacedKind::Control(_) => IntrinsicSize::default(),
    }
}

/// Content-box width and height of a block-level replaced box (CSS 2.1 10.3.4,
/// 10.6.2 and the table of 10.4). `specified` holds the content-box sizes the
/// style asks for, `None` for `auto`.
///
/// A box with one specified side derives the other from the intrinsic ratio
/// and then applies that other side's own limits, as Chrome does, so a
/// `width: 300px; max-width: 100px` canvas ends up 100 by 50. Only when both
/// sides are `auto` do the limits of one axis scale the other.
pub(crate) fn replaced_content_size(
    cx: &LayoutCx,
    node: NodeId,
    kind: ReplacedKind,
    specified: (Option<f32>, Option<f32>),
    limits: &SizeLimits,
) -> (f32, f32) {
    let natural = cx
        .images
        .intrinsic(node)
        .unwrap_or_else(|| default_natural_size(kind));
    // Frames have a default size but no intrinsic ratio.
    let ratio = natural
        .ratio
        .or(match (natural.width, natural.height, kind) {
            (_, _, ReplacedKind::Iframe | ReplacedKind::Svg) => None,
            (Some(w), Some(h), _) if h > 0.0 => Some(w / h),
            _ => None,
        })
        .filter(|r| *r > 0.0);
    match specified {
        (Some(w), Some(h)) => (limits.clamp_w(w), limits.clamp_h(h)),
        (Some(w), None) => {
            let w = limits.clamp_w(w);
            let h = ratio.map(|r| w / r).or(natural.height).unwrap_or(0.0);
            (w, limits.clamp_h(h))
        }
        (None, Some(h)) => {
            let h = limits.clamp_h(h);
            let w = ratio.map(|r| h * r).or(natural.width).unwrap_or(0.0);
            (limits.clamp_w(w), h)
        }
        (None, None) => {
            let w = natural
                .width
                .or_else(|| natural.height.zip(ratio).map(|(h, r)| h * r));
            let h = natural
                .height
                .or_else(|| natural.width.zip(ratio).map(|(w, r)| w / r));
            let (w, h) = (w.unwrap_or(0.0), h.unwrap_or(0.0));
            if ratio.is_some() {
                table_10_4(w, h, limits)
            } else {
                (limits.clamp_w(w), limits.clamp_h(h))
            }
        }
    }
}

/// The constraint violation table of CSS 2.1 section 10.4, which keeps the
/// aspect ratio of an element whose width and height are both `auto`.
fn table_10_4(w: f32, h: f32, l: &SizeLimits) -> (f32, f32) {
    if w <= 0.0 || h <= 0.0 {
        return (l.clamp_w(w), l.clamp_h(h));
    }
    let (over_w, under_w) = (w > l.max_w, w < l.min_w);
    let (over_h, under_h) = (h > l.max_h, h < l.min_h);
    let shrink_to_width = || (l.max_w, l.min_h.max(l.max_w * h / w));
    let shrink_to_height = || (l.min_w.max(l.max_h * w / h), l.max_h);
    let grow_to_width = || (l.min_w, l.max_h.min(l.min_w * h / w));
    let grow_to_height = || (l.max_w.min(l.min_h * w / h), l.min_h);
    match (over_w, under_w, over_h, under_h) {
        (true, _, true, _) => {
            if l.max_w / w <= l.max_h / h {
                shrink_to_width()
            } else {
                shrink_to_height()
            }
        }
        (_, true, _, true) => {
            if l.min_w / w <= l.min_h / h {
                grow_to_height()
            } else {
                grow_to_width()
            }
        }
        (_, true, true, _) => (l.min_w, l.max_h),
        (true, _, _, true) => (l.max_w, l.min_h),
        (true, ..) => shrink_to_width(),
        (_, true, ..) => grow_to_width(),
        (_, _, true, _) => shrink_to_height(),
        (_, _, _, true) => grow_to_height(),
        _ => (w, h),
    }
}

/// Applies `min-height`/`max-height` to the used `content_height` of box `id`
/// and returns the content-box height. A minimum beats a maximum. Percentages
/// resolve against the containing block's height, which is indefinite (zero
/// for a minimum, no limit for a maximum) unless the block's height is
/// definite or the quirks-mode percentage rule finds one.
pub(crate) fn constrain_block_size(
    cx: &LayoutCx,
    tree: &BoxTree,
    id: BoxId,
    space: &ConstraintSpace,
    content_height: f32,
) -> f32 {
    let b = tree.get(id);
    let pos = b.style.get_position();
    let border_box = is_border_box(&b.style);
    let base = quirks::percentage_limit_base(cx, tree, id, space);
    let edges = if border_box {
        let width_basis = space.percentage_base.0.or(match space.inline_size {
            crate::space::AvailableSize::Definite(w) => Some(w),
            _ => None,
        });
        let frame = frame_of(tree, id, width_basis.unwrap_or(0.0));
        frame.padding.vertical() + frame.border.vertical()
    } else {
        0.0
    };
    let min = size_lp(&pos.min_height)
        .and_then(|lp| resolve(lp, base))
        .map_or(0.0, |v| to_content(v.max(0.0), border_box, edges));
    let max = max_size_lp(&pos.max_height)
        .and_then(|lp| resolve(lp, base))
        .map_or(f32::INFINITY, |v| to_content(v.max(0.0), border_box, edges));
    content_height.min(max).max(min)
}

#[cfg(all(test, feature = "embedded-fonts"))]
mod tests;

#[cfg(test)]
mod table_tests {
    use super::*;

    fn limits(min_w: f32, max_w: f32, min_h: f32, max_h: f32) -> SizeLimits {
        SizeLimits {
            min_w,
            max_w,
            min_h,
            max_h,
        }
    }

    #[test]
    fn table_scales_the_other_axis_when_one_limit_is_violated() {
        let inf = f32::INFINITY;
        assert_eq!(
            table_10_4(300.0, 150.0, &limits(0.0, 100.0, 0.0, inf)),
            (100.0, 50.0)
        );
        assert_eq!(
            table_10_4(300.0, 150.0, &limits(0.0, inf, 0.0, 50.0)),
            (100.0, 50.0)
        );
        assert_eq!(
            table_10_4(300.0, 150.0, &limits(400.0, inf, 0.0, inf)),
            (400.0, 200.0)
        );
        assert_eq!(
            table_10_4(300.0, 150.0, &limits(0.0, inf, 200.0, inf)),
            (400.0, 200.0)
        );
    }

    #[test]
    fn table_resolves_the_two_limit_cases() {
        let inf = f32::INFINITY;
        // Both maxima violated: the tighter ratio decides.
        assert_eq!(
            table_10_4(300.0, 150.0, &limits(0.0, 100.0, 0.0, 100.0)),
            (100.0, 50.0)
        );
        // Width over, height under.
        assert_eq!(
            table_10_4(300.0, 150.0, &limits(0.0, 100.0, 200.0, inf)),
            (100.0, 200.0)
        );
        // Width under, height over.
        assert_eq!(
            table_10_4(300.0, 150.0, &limits(500.0, inf, 0.0, 50.0)),
            (500.0, 50.0)
        );
        // Raising the width is capped by the maximum height.
        assert_eq!(
            table_10_4(300.0, 150.0, &limits(400.0, inf, 0.0, 100.0)),
            (400.0, 100.0)
        );
        // Minimum width wins over a max-width that undercuts it.
        assert_eq!(
            table_10_4(300.0, 150.0, &limits(0.0, 100.0, 80.0, inf)),
            (100.0, 80.0)
        );
    }

    #[test]
    fn table_leaves_a_size_inside_its_limits_alone() {
        assert_eq!(
            table_10_4(300.0, 150.0, &limits(10.0, 400.0, 10.0, 400.0)),
            (300.0, 150.0)
        );
    }
}
