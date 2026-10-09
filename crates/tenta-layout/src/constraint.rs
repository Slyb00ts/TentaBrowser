//! Box-model resolution for block boxes: padding, border, margins, the used
//! inline size (CSS 2.1 section 10.3.3 with the min/max loop of 10.4) and the
//! definite block size.
//!
//! Everything here reads the containing block's [`ConstraintSpace`] and the
//! box's computed style; nothing depends on where the box ends up, so the
//! functions are pure apart from the intrinsic-size queries.

use style::properties::ComputedValues;
use style::properties::longhands::box_sizing::computed_value::T as BoxSizing;
use style::properties::longhands::direction::computed_value::T as StyleDirection;
use style::values::computed::{Length, LengthPercentage, Margin, MaxSize, Size};

use crate::block_sizes::{
    ReplacedHints, SizeLimits, intrinsic_inline_sizes_of_block, replaced_content_size,
    replaced_limits,
};
use crate::box_tree::{BoxId, BoxKind, BoxTree, LayoutCx};
use crate::quirks;
use crate::space::{AvailableSize, ConstraintSpace};
use crate::types::{Direction, Edges, ReplacedKind};

/// The used box model of a block box inside its containing block.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct BoxModel {
    pub(crate) border: Edges,
    pub(crate) padding: Edges,
    /// Used margins. Horizontal values are final; vertical values are the box's
    /// own margins before any collapsing, with `auto` as zero.
    pub(crate) margin: Edges,
    /// Used content-box width.
    pub(crate) content_width: f32,
    /// Definite content-box height before `min-height`/`max-height`, or `None`
    /// when the height is `auto` or a percentage with an indefinite base.
    pub(crate) content_height: Option<f32>,
}

impl BoxModel {
    /// Border-box width.
    pub(crate) fn border_box_width(&self) -> f32 {
        self.content_width + self.padding.horizontal() + self.border.horizontal()
    }

    /// Border-box height for a content-box height of `content`.
    pub(crate) fn border_box_height(&self, content: f32) -> f32 {
        content + self.padding.vertical() + self.border.vertical()
    }
}

/// A requested size along one axis, before it meets the available space.
#[derive(Copy, Clone, Debug, PartialEq)]
enum Extent {
    /// `auto`, `none`, `stretch` and anything that cannot resolve (a
    /// percentage of an indefinite base).
    Auto,
    /// A length in CSS pixels, still subject to `box-sizing`.
    Length(f32),
    /// A content-box size that `box-sizing` has already been applied to.
    Content(f32),
    MinContent,
    MaxContent,
    /// `fit-content`, optionally with an explicit limit: `min(max-content,
    /// max(min-content, limit))`, the limit defaulting to the available space.
    FitContent(Option<f32>),
}

fn length_percentage(lp: &LengthPercentage, basis: Option<f32>) -> Option<f32> {
    if lp.has_percentage() {
        basis.map(|b| lp.resolve(Length::new(b)).px())
    } else {
        lp.to_length().map(Length::px)
    }
}

fn extent_of_size(size: &Size, basis: Option<f32>) -> Extent {
    match size {
        Size::LengthPercentage(lp) => {
            length_percentage(&lp.0, basis).map_or(Extent::Auto, |v| Extent::Length(v.max(0.0)))
        }
        Size::MaxContent => Extent::MaxContent,
        Size::MinContent => Extent::MinContent,
        Size::FitContent => Extent::FitContent(None),
        Size::FitContentFunction(lp) => length_percentage(&lp.0, basis)
            .map_or(Extent::FitContent(None), |v| Extent::FitContent(Some(v))),
        _ => Extent::Auto,
    }
}

fn extent_of_max(size: &MaxSize, basis: Option<f32>) -> Extent {
    match size {
        MaxSize::LengthPercentage(lp) => {
            length_percentage(&lp.0, basis).map_or(Extent::Auto, |v| Extent::Length(v.max(0.0)))
        }
        MaxSize::MaxContent => Extent::MaxContent,
        MaxSize::MinContent => Extent::MinContent,
        MaxSize::FitContent => Extent::FitContent(None),
        MaxSize::FitContentFunction(lp) => length_percentage(&lp.0, basis)
            .map_or(Extent::FitContent(None), |v| Extent::FitContent(Some(v))),
        _ => Extent::Auto,
    }
}

/// The inline direction a box establishes for its own content.
pub(crate) fn own_direction(style: &ComputedValues) -> Direction {
    match style.get_inherited_box().direction {
        StyleDirection::Ltr => Direction::Ltr,
        StyleDirection::Rtl => Direction::Rtl,
    }
}

/// True when `min-height` or `max-height` can change the used height, so the
/// block-size constraint step has work to do.
pub(crate) fn has_block_size_constraints(style: &ComputedValues) -> bool {
    let pos = style.get_position();
    let min_is_zero = match &pos.min_height {
        Size::Auto => true,
        Size::LengthPercentage(lp) => lp.0.to_length().is_some_and(|l| l.px() == 0.0),
        _ => false,
    };
    !min_is_zero || !matches!(pos.max_height, MaxSize::None)
}

/// True when `min-height` is `auto` or zero, the precondition of CSS 2.1 8.3.1
/// for a box's bottom margin to collapse with its last child's. A percentage
/// is zero when its base is indefinite; `base` is asked for only then.
pub(crate) fn min_height_is_zero(
    style: &ComputedValues,
    base: impl FnOnce() -> Option<f32>,
) -> bool {
    match &style.get_position().min_height {
        Size::Auto => true,
        Size::LengthPercentage(lp) if lp.0.has_percentage() => {
            base().is_none_or(|b| lp.0.resolve(Length::new(b)).px() == 0.0)
        }
        Size::LengthPercentage(lp) => lp.0.to_length().is_some_and(|l| l.px() == 0.0),
        Size::MaxContent | Size::MinContent | Size::FitContent => false,
        _ => true,
    }
}

/// True when `height`, `min-height` or `max-height` holds a percentage.
fn has_percentage_height(style: &ComputedValues) -> bool {
    let pos = style.get_position();
    let size = |s: &Size| matches!(s, Size::LengthPercentage(lp) if lp.0.has_percentage());
    let max = |s: &MaxSize| matches!(s, MaxSize::LengthPercentage(lp) if lp.0.has_percentage());
    size(&pos.height) || size(&pos.min_height) || max(&pos.max_height)
}

/// Inputs of the inline-size equation of CSS 2.1 section 10.3.3.
#[derive(Copy, Clone, Debug)]
pub(crate) struct InlineEquation {
    /// Width of the containing block's content box.
    pub(crate) available: f32,
    /// Horizontal padding plus border of the box.
    pub(crate) edges: f32,
    /// Specified margins, `None` for `auto`.
    pub(crate) margin_left: Option<f32>,
    pub(crate) margin_right: Option<f32>,
    /// Direction of the containing block, which decides what absorbs an
    /// over-constrained equation.
    pub(crate) direction: Direction,
}

impl InlineEquation {
    /// Solves for `(content width, margin-left, margin-right)`; `width` is the
    /// content-box width, or `None` for `auto`.
    pub(crate) fn solve(&self, width: Option<f32>) -> (f32, f32, f32) {
        let left = self.margin_left.unwrap_or(0.0);
        let right = self.margin_right.unwrap_or(0.0);
        let Some(width) = width else {
            return (
                (self.available - left - right - self.edges).max(0.0),
                left,
                right,
            );
        };
        let rest = self.available - self.edges - width;
        let free = rest - left - right;
        match (self.margin_left, self.margin_right) {
            (None, None) if free >= 0.0 => return (width, free / 2.0, free / 2.0),
            (None, Some(_)) if free >= 0.0 => return (width, free, right),
            (Some(_), None) if free >= 0.0 => return (width, left, free),
            _ => {}
        }
        // Over-constrained, or auto margins with no room left (they count as
        // zero): the margin at the end of the containing block's direction
        // takes the difference.
        match self.direction {
            Direction::Ltr => (width, left, rest - left),
            Direction::Rtl => (width, rest - right, right),
        }
    }

    /// The width algorithm with the `min-width`/`max-width` loop of CSS 2.1
    /// 10.4: apply the maximum first, then the minimum, each time solving the
    /// equation again with the clamp as the specified width.
    pub(crate) fn solve_constrained(
        &self,
        width: Option<f32>,
        min: f32,
        max: f32,
    ) -> (f32, f32, f32) {
        let mut solved = self.solve(width);
        if solved.0 > max {
            solved = self.solve(Some(max));
        }
        if solved.0 < min {
            solved = self.solve(Some(min));
        }
        solved
    }
}

fn margin_value(margin: &Margin, basis: f32) -> Option<f32> {
    match margin {
        Margin::Auto => None,
        Margin::LengthPercentage(lp) => Some(length_percentage(lp, Some(basis)).unwrap_or(0.0)),
        _ => Some(0.0),
    }
}

/// Converts a `width`/`height`-style length to a content-box size.
fn to_content_box(value: f32, border_box_sizing: bool, edges: f32) -> f32 {
    if border_box_sizing {
        (value - edges).max(0.0)
    } else {
        value
    }
}

/// Resolves the box model of `id` inside the containing block described by
/// `space`.
pub(crate) fn resolve_box(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    id: BoxId,
    space: &ConstraintSpace,
) -> BoxModel {
    let b = tree.get(id);
    let style = &*b.style;
    let definite = match space.inline_size {
        AvailableSize::Definite(w) => Some(w),
        _ => None,
    };
    let width_basis = space.percentage_base.0.or(definite);
    let basis = width_basis.unwrap_or(0.0);

    let padding_style = style.get_padding();
    let padding_of =
        |lp: &LengthPercentage| length_percentage(lp, Some(basis)).unwrap_or(0.0).max(0.0);
    let padding = Edges {
        top: padding_of(&padding_style.padding_top.0),
        right: padding_of(&padding_style.padding_right.0),
        bottom: padding_of(&padding_style.padding_bottom.0),
        left: padding_of(&padding_style.padding_left.0),
    };
    let border = b.paint.border.widths;
    let margin_style = style.get_margin();
    let margin_left = margin_value(&margin_style.margin_left, basis);
    let margin_right = margin_value(&margin_style.margin_right, basis);
    let margin_top = margin_value(&margin_style.margin_top, basis).unwrap_or(0.0);
    let margin_bottom = margin_value(&margin_style.margin_bottom, basis).unwrap_or(0.0);

    let horizontal_edges = padding.horizontal() + border.horizontal();
    let vertical_edges = padding.vertical() + border.vertical();
    let pos = style.get_position();
    let border_box_sizing = matches!(pos.box_sizing, BoxSizing::BorderBox);

    let replaced = match b.kind {
        BoxKind::Replaced(kind) => Some(kind),
        _ => None,
    };
    let mut width = extent_of_size(&pos.width, width_basis);
    // Quirks mode lets a percentage height reach past auto-height ancestors.
    let height_basis = if has_percentage_height(style) {
        quirks::percentage_height_base(cx, tree, id, space)
    } else {
        space.percentage_base.1
    };
    let limit_basis = if has_percentage_height(style) {
        quirks::percentage_limit_base(cx, tree, id, space)
    } else {
        space.percentage_base.1
    };
    let mut height = extent_of_size(&pos.height, height_basis);
    if let Some(kind) = replaced {
        let limits = replaced_limits(
            style,
            (width_basis, limit_basis),
            border_box_sizing,
            (horizontal_edges, vertical_edges),
        );
        // A ratio-only image fills the containing block's width, as a
        // block-level non-replaced box would.
        let stretch = space.percentage_base.0.or(definite).map(|cb| {
            cb - horizontal_edges - margin_left.unwrap_or(0.0) - margin_right.unwrap_or(0.0)
        });
        let (w, h) = replaced_size(
            cx,
            b.node,
            kind,
            (width, height),
            border_box_sizing,
            (horizontal_edges, vertical_edges),
            &limits,
            ReplacedHints::of(style, stretch),
        );
        width = Extent::Content(w);
        height = Extent::Content(h);
    }

    let equation = InlineEquation {
        available: definite.unwrap_or(0.0),
        edges: horizontal_edges,
        margin_left,
        margin_right,
        direction: space.direction,
    };

    if space.fixed_inline {
        let border_box = definite.unwrap_or(0.0);
        let content_width = (border_box - horizontal_edges).max(0.0);
        return BoxModel {
            border,
            padding,
            margin: Edges {
                top: margin_top,
                right: margin_right.unwrap_or(0.0),
                bottom: margin_bottom,
                left: margin_left.unwrap_or(0.0),
            },
            content_width,
            content_height: block_size(space, height, border_box_sizing, vertical_edges),
        };
    }

    let fit_limit = (equation.available
        - horizontal_edges
        - margin_left.unwrap_or(0.0)
        - margin_right.unwrap_or(0.0))
    .max(0.0);
    let mut sizes = InlineSizes {
        tree,
        id,
        border_box_sizing,
        edges: horizontal_edges,
        fit_limit,
        intrinsic: None,
    };
    let mut specified = sizes.content_size(cx, width);
    if specified.is_none() {
        specified = match space.inline_size {
            AvailableSize::MinContent => sizes.content_size(cx, Extent::MinContent),
            AvailableSize::MaxContent => sizes.content_size(cx, Extent::MaxContent),
            AvailableSize::Definite(_) => None,
        };
    }
    let min = sizes
        .content_size(cx, extent_of_size(&pos.min_width, width_basis))
        .unwrap_or(0.0);
    let max = sizes
        .content_size(cx, extent_of_max(&pos.max_width, width_basis))
        .unwrap_or(f32::INFINITY);
    let (content_width, left, right) = equation.solve_constrained(specified, min, max);

    BoxModel {
        border,
        padding,
        margin: Edges {
            top: margin_top,
            right,
            bottom: margin_bottom,
            left,
        },
        content_width,
        content_height: block_size(space, height, border_box_sizing, vertical_edges),
    }
}

/// What an absolutely positioned box asks for along both axes, resolved against
/// its containing block (CSS Positioned Layout 3, 5.2 and CSS 2.1 10.3.7).
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct AbsInputs {
    pub(crate) border: Edges,
    pub(crate) padding: Edges,
    /// Specified margins; `None` is `auto`.
    pub(crate) margin_left: Option<f32>,
    pub(crate) margin_right: Option<f32>,
    pub(crate) margin_top: Option<f32>,
    pub(crate) margin_bottom: Option<f32>,
    /// Content-box width from `width` (or the replaced size); `None` is `auto`.
    pub(crate) width: Option<f32>,
    /// `min-width` and `max-width` as content-box widths.
    pub(crate) min_width: f32,
    pub(crate) max_width: f32,
    /// Content-box height from `height` (or the replaced size); `None` when
    /// the used height comes from the content (`height: auto` and friends).
    pub(crate) height: Option<f32>,
    /// `aspect-ratio` of a non-replaced box, width over height.
    pub(crate) aspect_ratio: Option<f32>,
    /// `box-sizing: border-box`.
    pub(crate) border_box_sizing: bool,
}

/// Resolves the box model inputs of the absolutely positioned box `id` whose
/// containing block has the padding box `(cb_width, cb_height)`.
pub(crate) fn abspos_inputs(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    id: BoxId,
    (cb_width, cb_height): (f32, f32),
) -> AbsInputs {
    let b = tree.get(id);
    let style = &*b.style;
    let padding_style = style.get_padding();
    let padding_of = |lp: &LengthPercentage| {
        length_percentage(lp, Some(cb_width))
            .unwrap_or(0.0)
            .max(0.0)
    };
    let padding = Edges {
        top: padding_of(&padding_style.padding_top.0),
        right: padding_of(&padding_style.padding_right.0),
        bottom: padding_of(&padding_style.padding_bottom.0),
        left: padding_of(&padding_style.padding_left.0),
    };
    let border = b.paint.border.widths;
    let margin_style = style.get_margin();
    let margin = |m: &Margin| margin_value(m, cb_width);
    let (margin_left, margin_right) = (
        margin(&margin_style.margin_left),
        margin(&margin_style.margin_right),
    );
    let (margin_top, margin_bottom) = (
        margin(&margin_style.margin_top),
        margin(&margin_style.margin_bottom),
    );
    let horizontal_edges = padding.horizontal() + border.horizontal();
    let vertical_edges = padding.vertical() + border.vertical();
    let pos = style.get_position();
    let border_box_sizing = matches!(pos.box_sizing, BoxSizing::BorderBox);

    let mut width = extent_of_size(&pos.width, Some(cb_width));
    let mut height = extent_of_size(&pos.height, Some(cb_height));
    if let BoxKind::Replaced(kind) = b.kind {
        let limits = replaced_limits(
            style,
            (Some(cb_width), Some(cb_height)),
            border_box_sizing,
            (horizontal_edges, vertical_edges),
        );
        let stretch =
            cb_width - horizontal_edges - margin_left.unwrap_or(0.0) - margin_right.unwrap_or(0.0);
        let (w, h) = replaced_size(
            cx,
            b.node,
            kind,
            (width, height),
            border_box_sizing,
            (horizontal_edges, vertical_edges),
            &limits,
            ReplacedHints::of(style, Some(stretch)),
        );
        width = Extent::Content(w);
        height = Extent::Content(h);
    }
    let fit_limit =
        (cb_width - horizontal_edges - margin_left.unwrap_or(0.0) - margin_right.unwrap_or(0.0))
            .max(0.0);
    let mut sizes = InlineSizes {
        tree,
        id,
        border_box_sizing,
        edges: horizontal_edges,
        fit_limit,
        intrinsic: None,
    };
    let specified = sizes.content_size(cx, width);
    let min_width = sizes
        .content_size(cx, extent_of_size(&pos.min_width, Some(cb_width)))
        .unwrap_or(0.0);
    let max_width = sizes
        .content_size(cx, extent_of_max(&pos.max_width, Some(cb_width)))
        .unwrap_or(f32::INFINITY);
    AbsInputs {
        border,
        padding,
        margin_left,
        margin_right,
        margin_top,
        margin_bottom,
        width: specified,
        min_width,
        max_width,
        height: match height {
            Extent::Length(v) => Some(to_content_box(v, border_box_sizing, vertical_edges)),
            Extent::Content(v) => Some(v),
            _ => None,
        },
        aspect_ratio: crate::style_taffy::preferred_aspect_ratio(style)
            .filter(|ratio| *ratio > 0.0 && !matches!(b.kind, BoxKind::Replaced(_))),
        border_box_sizing,
    }
}

/// Turns requested inline extents into content-box widths, asking for the box's
/// intrinsic sizes at most once.
struct InlineSizes<'a> {
    tree: &'a BoxTree,
    id: BoxId,
    border_box_sizing: bool,
    edges: f32,
    /// Space `fit-content` may grow into: the available width less margins,
    /// padding and border.
    fit_limit: f32,
    intrinsic: Option<(f32, f32)>,
}

impl InlineSizes<'_> {
    fn content_size(&mut self, cx: &mut LayoutCx, extent: Extent) -> Option<f32> {
        match extent {
            Extent::Length(v) => {
                return Some(to_content_box(v, self.border_box_sizing, self.edges));
            }
            Extent::Content(v) => return Some(v),
            Extent::Auto => return None,
            Extent::MinContent | Extent::MaxContent | Extent::FitContent(_) => {}
        }
        let (min, max) = *self
            .intrinsic
            .get_or_insert_with(|| intrinsic_inline_sizes_of_block(cx, self.tree, self.id));
        Some(match extent {
            Extent::FitContent(limit) => max.min(min.max(limit.unwrap_or(self.fit_limit))),
            Extent::MinContent => min,
            _ => max,
        })
    }
}

fn block_size(
    space: &ConstraintSpace,
    height: Extent,
    border_box_sizing: bool,
    vertical_edges: f32,
) -> Option<f32> {
    if let (true, AvailableSize::Definite(h)) = (space.fixed_block, space.block_size) {
        return Some((h - vertical_edges).max(0.0));
    }
    if space.content_block_size {
        return None;
    }
    match height {
        Extent::Length(v) => Some(to_content_box(v, border_box_sizing, vertical_edges)),
        Extent::Content(v) => Some(v),
        _ => None,
    }
}

/// Content-box width and height of a block-level replaced box: specified
/// sizes win, then the ratio, then the natural size, all inside the
/// `min-`/`max-` limits (CSS 2.1 10.3.4, 10.6.2 and 10.4).
#[allow(clippy::too_many_arguments)]
fn replaced_size(
    cx: &LayoutCx,
    node: tenta_core::NodeId,
    kind: ReplacedKind,
    (width, height): (Extent, Extent),
    border_box_sizing: bool,
    (horizontal_edges, vertical_edges): (f32, f32),
    limits: &SizeLimits,
    hints: ReplacedHints,
) -> (f32, f32) {
    let specified_w = match width {
        Extent::Length(v) => Some(to_content_box(v, border_box_sizing, horizontal_edges)),
        _ => None,
    };
    let specified_h = match height {
        Extent::Length(v) => Some(to_content_box(v, border_box_sizing, vertical_edges)),
        _ => None,
    };
    replaced_content_size(cx, node, kind, (specified_w, specified_h), limits, hints)
}

/// The constraint space a box with the given content box offers its
/// block-level children and inline formatting context.
pub(crate) fn child_space(
    parent: &ConstraintSpace,
    content_width: f32,
    content_height: Option<f32>,
    direction: Direction,
    bfc_offset: (f32, f32),
) -> ConstraintSpace {
    ConstraintSpace {
        inline_size: AvailableSize::Definite(content_width),
        block_size: content_height.map_or(AvailableSize::MaxContent, AvailableSize::Definite),
        percentage_base: (Some(content_width), content_height),
        fixed_inline: false,
        fixed_block: false,
        content_block_size: false,
        bfc_offset,
        direction,
        writing_mode: parent.writing_mode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eq(
        available: f32,
        edges: f32,
        l: Option<f32>,
        r: Option<f32>,
        d: Direction,
    ) -> InlineEquation {
        InlineEquation {
            available,
            edges,
            margin_left: l,
            margin_right: r,
            direction: d,
        }
    }

    #[test]
    fn auto_width_fills_between_fixed_margins() {
        let e = eq(900.0, 10.0, Some(20.0), Some(30.0), Direction::Ltr);
        assert_eq!(e.solve(None), (840.0, 20.0, 30.0));
    }

    #[test]
    fn auto_width_never_goes_negative_and_keeps_margins() {
        let e = eq(50.0, 0.0, Some(40.0), Some(40.0), Direction::Ltr);
        assert_eq!(e.solve(None), (0.0, 40.0, 40.0));
    }

    #[test]
    fn two_auto_margins_split_the_free_space() {
        let e = eq(900.0, 0.0, None, None, Direction::Ltr);
        assert_eq!(e.solve(Some(300.0)), (300.0, 300.0, 300.0));
    }

    #[test]
    fn one_auto_margin_absorbs_the_rest() {
        let start = eq(900.0, 0.0, None, Some(50.0), Direction::Ltr);
        assert_eq!(start.solve(Some(200.0)), (200.0, 650.0, 50.0));
        let end = eq(900.0, 0.0, Some(50.0), None, Direction::Ltr);
        assert_eq!(end.solve(Some(200.0)), (200.0, 50.0, 650.0));
    }

    #[test]
    fn over_constrained_ltr_ignores_margin_right() {
        let e = eq(500.0, 0.0, Some(10.0), Some(10.0), Direction::Ltr);
        assert_eq!(e.solve(Some(300.0)), (300.0, 10.0, 190.0));
    }

    #[test]
    fn over_constrained_rtl_ignores_margin_left() {
        let e = eq(500.0, 0.0, Some(10.0), Some(10.0), Direction::Rtl);
        assert_eq!(e.solve(Some(300.0)), (300.0, 190.0, 10.0));
    }

    #[test]
    fn auto_margins_become_zero_when_the_box_overflows() {
        let ltr = eq(100.0, 0.0, None, None, Direction::Ltr);
        assert_eq!(ltr.solve(Some(300.0)), (300.0, 0.0, -200.0));
        let rtl = eq(100.0, 0.0, None, None, Direction::Rtl);
        assert_eq!(rtl.solve(Some(300.0)), (300.0, -200.0, 0.0));
        let one = eq(100.0, 0.0, None, Some(10.0), Direction::Ltr);
        assert_eq!(one.solve(Some(300.0)), (300.0, 0.0, -200.0));
    }

    #[test]
    fn max_width_reruns_the_equation_with_the_cap() {
        let e = eq(900.0, 0.0, None, None, Direction::Ltr);
        assert_eq!(
            e.solve_constrained(Some(630.0), 0.0, 500.0),
            (500.0, 200.0, 200.0)
        );
    }

    #[test]
    fn min_width_wins_over_max_width() {
        let e = eq(900.0, 0.0, Some(0.0), Some(0.0), Direction::Ltr);
        assert_eq!(e.solve_constrained(None, 600.0, 400.0).0, 600.0);
    }
}
