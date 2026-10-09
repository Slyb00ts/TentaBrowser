// Derived from `stylo_taffy` 0.3.0-beta.2 (`src/convert.rs`), part of Blitz by
// DioxusLabs, https://github.com/DioxusLabs/blitz (`packages/stylo_taffy`).
// The original is licensed MIT OR Apache-2.0 OR MPL-2.0; this derived file is
// used under the MIT licence. The upstream licence text carries no copyright
// line; see THIRD_PARTY_NOTICES.md for the notice. Modified for Stylo 0.21 and
// this crate: no wrapper type, no block/float/text-align conversion, and
// `display` split into the algorithm that owns a box and the Taffy value.

//! Conversions of the non-grid properties and the eager [`to_taffy_style`].

use taffy::style_helpers::*;

use super::calc;
use super::grid;

/// Stylo types under short names.
pub(super) mod stylo {
    pub(super) use style::Atom;
    pub(super) use style::computed_values::{
        flex_direction::T as FlexDirection, flex_wrap::T as FlexWrap,
    };
    pub(super) use style::properties::ComputedValues;
    pub(super) use style::properties::generated::longhands::box_sizing::computed_value::T as BoxSizing;
    pub(super) use style::properties::generated::longhands::direction::computed_value::T as Direction;
    pub(super) use style::properties::longhands::aspect_ratio::computed_value::T as AspectRatio;
    pub(super) use style::properties::longhands::position::computed_value::T as Position;
    pub(super) use style::values::computed::length_percentage::Unpacked as UnpackedLengthPercentage;
    pub(super) use style::values::computed::{
        BorderSideWidth, Contain, LengthPercentage, Percentage,
    };
    pub(super) use style::values::generics::NonNegative;
    pub(super) use style::values::generics::flex::GenericFlexBasis;
    pub(super) use style::values::generics::length::{
        GenericLengthPercentageOrNormal, GenericMargin, GenericMaxSize, GenericSize,
    };
    pub(super) use style::values::generics::position::{Inset as GenericInset, PreferredRatio};
    pub(super) use style::values::specified::align::{AlignFlags, ContentDistribution};
    pub(super) use style::values::specified::border::BorderStyle;
    pub(super) use style::values::specified::box_::{
        Display, DisplayInside, DisplayOutside, Overflow,
    };

    pub(super) type MarginVal = GenericMargin<LengthPercentage>;
    pub(super) type InsetVal = GenericInset<Percentage, LengthPercentage>;
    pub(super) type Size = GenericSize<NonNegative<LengthPercentage>>;
    pub(super) type MaxSize = GenericMaxSize<NonNegative<LengthPercentage>>;
    pub(super) type Gap = GenericLengthPercentageOrNormal<NonNegative<LengthPercentage>>;
    pub(super) type FlexBasis = GenericFlexBasis<Size>;
}

/// Which layout algorithm lays out a box's children.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ContainerKind {
    /// Taffy's flexbox algorithm.
    Flex,
    /// Taffy's grid algorithm.
    Grid,
    /// This crate's block, inline, float and table algorithms.
    Native,
    /// `display: none`: no box.
    None,
    /// `display: contents`: no box of its own; the children join the parent.
    Contents,
}

pub(crate) fn container_kind(input: stylo::Display) -> ContainerKind {
    // `display: contents` also has a `none` outside, so the inside decides first.
    match input.inside() {
        stylo::DisplayInside::Contents => ContainerKind::Contents,
        _ if input.outside() == stylo::DisplayOutside::None => ContainerKind::None,
        stylo::DisplayInside::None => ContainerKind::None,
        stylo::DisplayInside::Flex => ContainerKind::Flex,
        stylo::DisplayInside::Grid => ContainerKind::Grid,
        _ => ContainerKind::Native,
    }
}

/// The Taffy `display`. Boxes that Taffy does not lay out (see [`container_kind`])
/// get the default, because Taffy is never asked to run an algorithm on them.
pub(super) fn display(input: stylo::Display) -> taffy::Display {
    match container_kind(input) {
        ContainerKind::Flex => taffy::Display::Flex,
        ContainerKind::Grid => taffy::Display::Grid,
        ContainerKind::None => taffy::Display::None,
        ContainerKind::Native | ContainerKind::Contents => taffy::Display::DEFAULT,
    }
}

pub(super) fn length_percentage(val: &stylo::LengthPercentage) -> taffy::LengthPercentage {
    match val.unpack() {
        stylo::UnpackedLengthPercentage::Calc(calc_ref) => calc::length_percentage(calc_ref),
        stylo::UnpackedLengthPercentage::Length(len) => length(len.px()),
        stylo::UnpackedLengthPercentage::Percentage(percentage) => percent(percentage.0),
    }
}

pub(super) fn dimension(val: &stylo::Size) -> taffy::Dimension {
    match val {
        stylo::Size::LengthPercentage(val) => length_percentage(&val.0).into(),
        stylo::Size::Auto => taffy::Dimension::AUTO,
        stylo::Size::MaxContent => taffy::Dimension::max_content(),
        stylo::Size::MinContent => taffy::Dimension::min_content(),
        stylo::Size::FitContent => taffy::Dimension::fit_content(),
        stylo::Size::FitContentFunction(val) => match val.0.unpack() {
            stylo::UnpackedLengthPercentage::Length(len) => {
                taffy::Dimension::fit_content_px(len.px())
            }
            stylo::UnpackedLengthPercentage::Percentage(percentage) => {
                taffy::Dimension::fit_content_percent(percentage.0)
            }
            // Taffy's fit-content limit has no calc form.
            stylo::UnpackedLengthPercentage::Calc(_) => taffy::Dimension::AUTO,
        },
        stylo::Size::Stretch | stylo::Size::WebkitFillAvailable => taffy::Dimension::stretch(),
        stylo::Size::AnchorSizeFunction(_) | stylo::Size::AnchorContainingCalcFunction(_) => {
            taffy::Dimension::AUTO
        }
    }
}

/// Taffy's min/max sizes take no keywords: they resolve like `auto`, so an
/// item with `min-width: min-content` loses that constraint until the
/// integration measures it.
pub(super) fn min_size(val: &stylo::Size) -> taffy::LengthPercentageAuto {
    match val {
        stylo::Size::LengthPercentage(val) => length_percentage(&val.0).into(),
        _ => taffy::LengthPercentageAuto::AUTO,
    }
}

pub(super) fn max_size(val: &stylo::MaxSize) -> taffy::LengthPercentageAuto {
    match val {
        stylo::MaxSize::LengthPercentage(val) => length_percentage(&val.0).into(),
        _ => taffy::LengthPercentageAuto::AUTO,
    }
}

pub(super) fn margin(val: &stylo::MarginVal) -> taffy::LengthPercentageAuto {
    match val {
        stylo::MarginVal::LengthPercentage(val) => length_percentage(val).into(),
        _ => taffy::LengthPercentageAuto::AUTO,
    }
}

pub(super) fn border(
    width: &stylo::BorderSideWidth,
    style: stylo::BorderStyle,
) -> taffy::LengthPercentage {
    if style.none_or_hidden() {
        return taffy::style_helpers::zero();
    }
    length(width.0.to_f32_px())
}

pub(super) fn inset(val: &stylo::InsetVal) -> taffy::LengthPercentageAuto {
    match val {
        stylo::InsetVal::LengthPercentage(val) => length_percentage(val).into(),
        _ => taffy::LengthPercentageAuto::AUTO,
    }
}

pub(super) fn box_sizing(input: stylo::BoxSizing) -> taffy::BoxSizing {
    match input {
        stylo::BoxSizing::BorderBox => taffy::BoxSizing::BorderBox,
        stylo::BoxSizing::ContentBox => taffy::BoxSizing::ContentBox,
    }
}

/// Taffy knows only the two positions that change how an item is placed in a
/// flex or grid container. `static` and `sticky` items are in flow; `fixed` is
/// out of flow like `absolute` (its containing block is the caller's concern).
pub(super) fn position(input: stylo::Position) -> taffy::Position {
    match input {
        stylo::Position::Static | stylo::Position::Relative | stylo::Position::Sticky => {
            taffy::Position::Relative
        }
        stylo::Position::Absolute | stylo::Position::Fixed => taffy::Position::Absolute,
    }
}

/// Taffy has no `auto`; `overflow: auto` and `scroll` both make the automatic
/// minimum size zero, and the scrollbar width is zero in this engine.
pub(super) fn overflow(input: stylo::Overflow) -> taffy::Overflow {
    match input {
        stylo::Overflow::Visible => taffy::Overflow::Visible,
        stylo::Overflow::Clip => taffy::Overflow::Clip,
        stylo::Overflow::Hidden => taffy::Overflow::Hidden,
        stylo::Overflow::Scroll | stylo::Overflow::Auto => taffy::Overflow::Scroll,
    }
}

pub(super) fn contain(input: stylo::Contain, display: stylo::Display) -> taffy::Contain {
    // Layout and paint containment do not apply to non-atomic inline-level boxes
    // (https://drafts.csswg.org/css-contain-1/#containment-layout)
    if display.outside() == stylo::DisplayOutside::Inline
        && display.inside() == stylo::DisplayInside::Flow
    {
        return taffy::Contain::NONE;
    }
    let mut result = taffy::Contain::NONE;
    if input.contains(stylo::Contain::LAYOUT) {
        result |= taffy::Contain::LAYOUT;
    }
    if input.contains(stylo::Contain::PAINT) {
        result |= taffy::Contain::PAINT;
    }
    result
}

pub(super) fn direction(input: stylo::Direction) -> taffy::Direction {
    match input {
        stylo::Direction::Ltr => taffy::Direction::Ltr,
        stylo::Direction::Rtl => taffy::Direction::Rtl,
    }
}

pub(super) fn aspect_ratio(input: stylo::AspectRatio) -> Option<f32> {
    match input.ratio {
        stylo::PreferredRatio::None => None,
        stylo::PreferredRatio::Ratio(val) => Some(val.0.0 / val.1.0),
    }
}

/// Convert `align-content`/`justify-content` for a container with the given `display`.
///
/// In a block container the whole in-flow content is a single alignment subject, and positional
/// keywords default to `safe` overflow alignment, only opted out of by an explicit `unsafe`
/// (<https://github.com/w3c/csswg-drafts/issues/10154>).
pub(super) fn content_alignment(
    input: stylo::ContentDistribution,
    display: stylo::Display,
) -> Option<taffy::AlignContent> {
    let primary = input.primary();
    let mut align = match primary.value() {
        stylo::AlignFlags::START | stylo::AlignFlags::LEFT => Some(taffy::AlignContent::START),
        stylo::AlignFlags::END | stylo::AlignFlags::RIGHT => Some(taffy::AlignContent::END),
        stylo::AlignFlags::FLEX_START => Some(taffy::AlignContent::FLEX_START),
        stylo::AlignFlags::STRETCH => Some(taffy::AlignContent::STRETCH),
        stylo::AlignFlags::FLEX_END => Some(taffy::AlignContent::FLEX_END),
        stylo::AlignFlags::CENTER => Some(taffy::AlignContent::CENTER),
        stylo::AlignFlags::SPACE_BETWEEN => Some(taffy::AlignContent::SPACE_BETWEEN),
        stylo::AlignFlags::SPACE_AROUND => Some(taffy::AlignContent::SPACE_AROUND),
        stylo::AlignFlags::SPACE_EVENLY => Some(taffy::AlignContent::SPACE_EVENLY),
        // Baseline content-alignment is not supported: it falls back to start/end
        // (<https://www.w3.org/TR/css-align-3/#baseline-align-self>)
        stylo::AlignFlags::BASELINE => Some(taffy::AlignContent::START),
        stylo::AlignFlags::LAST_BASELINE => Some(taffy::AlignContent::END),
        // `normal`, `auto` and anything the parser cannot produce here.
        _ => None,
    }?;
    let is_block_container = matches!(
        display.inside(),
        stylo::DisplayInside::Flow | stylo::DisplayInside::FlowRoot
    );
    let safe = primary.flags().contains(stylo::AlignFlags::SAFE)
        || (is_block_container && !primary.flags().contains(stylo::AlignFlags::UNSAFE));
    if safe {
        align.safety = taffy::AlignmentSafety::Safe;
    }
    Some(align)
}

/// Convert `justify-content`, resolving the physical `left`/`right` keywords against the
/// container's flex main axis and text direction. `left`/`right` behave as `start` when the
/// main axis is not the inline axis (<https://www.w3.org/TR/css-align-3/#positional-values>).
pub(super) fn justify_content(
    input: stylo::ContentDistribution,
    flex_direction: stylo::FlexDirection,
    direction: stylo::Direction,
    display: stylo::Display,
) -> Option<taffy::AlignContent> {
    let is_row = matches!(
        flex_direction,
        stylo::FlexDirection::Row | stylo::FlexDirection::RowReverse
    );
    let is_rtl = matches!(direction, stylo::Direction::Rtl);
    let primary = input.primary();
    let is_right = match primary.value() {
        stylo::AlignFlags::LEFT => false,
        stylo::AlignFlags::RIGHT => true,
        _ => return content_alignment(input, display),
    };
    let mut align = if is_row && is_right != is_rtl {
        taffy::AlignContent::END
    } else {
        taffy::AlignContent::START
    };
    if primary.flags().contains(stylo::AlignFlags::SAFE) {
        align.safety = taffy::AlignmentSafety::Safe;
    }
    Some(align)
}

/// Convert item alignment values (`align-items`/`align-self`/`justify-items`/`justify-self`),
/// resolving the physical `left`/`right` keywords against `is_horiz_rtl`: whether the axis being
/// aligned is a horizontal axis with right-to-left text direction. Pass `false` for the vertical
/// axis (<https://www.w3.org/TR/css-align-3/#positional-values>).
pub(super) fn item_alignment(
    input: stylo::AlignFlags,
    is_horiz_rtl: bool,
) -> Option<taffy::AlignItems> {
    let mut align = match input.value() {
        stylo::AlignFlags::NORMAL | stylo::AlignFlags::STRETCH => Some(taffy::AlignItems::STRETCH),
        stylo::AlignFlags::FLEX_START => Some(taffy::AlignItems::FLEX_START),
        stylo::AlignFlags::FLEX_END => Some(taffy::AlignItems::FLEX_END),
        stylo::AlignFlags::SELF_START => Some(taffy::AlignItems::SELF_START),
        stylo::AlignFlags::SELF_END => Some(taffy::AlignItems::SELF_END),
        stylo::AlignFlags::START => Some(taffy::AlignItems::START),
        stylo::AlignFlags::END => Some(taffy::AlignItems::END),
        stylo::AlignFlags::LEFT if is_horiz_rtl => Some(taffy::AlignItems::END),
        stylo::AlignFlags::LEFT => Some(taffy::AlignItems::START),
        stylo::AlignFlags::RIGHT if is_horiz_rtl => Some(taffy::AlignItems::START),
        stylo::AlignFlags::RIGHT => Some(taffy::AlignItems::END),
        stylo::AlignFlags::CENTER => Some(taffy::AlignItems::CENTER),
        stylo::AlignFlags::BASELINE => Some(taffy::AlignItems::BASELINE),
        // Taffy does not support last-baseline alignment, so map it to its
        // fallback alignment (https://www.w3.org/TR/css-align-3/#baseline-values)
        stylo::AlignFlags::LAST_BASELINE => Some(taffy::AlignItems::END),
        // `auto` defers to the container's `align-items`.
        _ => None,
    }?;
    if input.flags().contains(stylo::AlignFlags::SAFE) {
        align.safety = taffy::AlignmentSafety::Safe;
    }
    Some(align)
}

pub(super) fn gap(input: &stylo::Gap) -> taffy::LengthPercentage {
    match input {
        // For flex and grid containers `normal` is 0px.
        stylo::Gap::Normal => taffy::LengthPercentage::ZERO,
        stylo::Gap::LengthPercentage(val) => length_percentage(&val.0),
    }
}

pub(super) fn flex_basis(input: &stylo::FlexBasis) -> taffy::Dimension {
    match input {
        stylo::FlexBasis::Content => taffy::Dimension::content(),
        stylo::FlexBasis::Size(size) => dimension(size),
    }
}

pub(super) fn flex_direction(input: stylo::FlexDirection) -> taffy::FlexDirection {
    match input {
        stylo::FlexDirection::Row => taffy::FlexDirection::Row,
        stylo::FlexDirection::RowReverse => taffy::FlexDirection::RowReverse,
        stylo::FlexDirection::Column => taffy::FlexDirection::Column,
        stylo::FlexDirection::ColumnReverse => taffy::FlexDirection::ColumnReverse,
    }
}

pub(super) fn flex_wrap(input: stylo::FlexWrap) -> taffy::FlexWrap {
    match input {
        stylo::FlexWrap::Wrap => taffy::FlexWrap::Wrap,
        stylo::FlexWrap::WrapReverse => taffy::FlexWrap::WrapReverse,
        stylo::FlexWrap::Nowrap => taffy::FlexWrap::NoWrap,
    }
}

/// A converted style, bound to the computed values its `calc()` handles point into.
pub(crate) struct ConvertedStyle<'a> {
    style: taffy::Style<stylo::Atom>,
    _computed: core::marker::PhantomData<&'a stylo::ComputedValues>,
}

impl core::ops::Deref for ConvertedStyle<'_> {
    type Target = taffy::Style<stylo::Atom>;

    fn deref(&self) -> &Self::Target {
        &self.style
    }
}

impl ConvertedStyle<'_> {
    /// The Taffy style. It still holds `calc()` handles, so it must not be
    /// used after the computed values it was converted from are gone.
    pub(crate) fn style(&self) -> &taffy::Style<stylo::Atom> {
        &self.style
    }
}

/// Eagerly convert an entire [`stylo::ComputedValues`] into a [`taffy::Style`].
///
/// `item_is_replaced` is left `false`: whether a box is a replaced element is
/// a box-tree fact, so the tree integration must set it on the converted style.
/// The same integration's `resolve_calc_value` must keep an `Arc` clone of every
/// converted box's computed values alive for the whole layout pass.
pub(crate) fn to_taffy_style(style: &stylo::ComputedValues) -> ConvertedStyle<'_> {
    ConvertedStyle {
        style: convert_style(style),
        _computed: core::marker::PhantomData,
    }
}

fn convert_style(style: &stylo::ComputedValues) -> taffy::Style<stylo::Atom> {
    let display = style.clone_display();
    let pos = style.get_position();
    let margin = style.get_margin();
    let padding = style.get_padding();
    let border = style.get_border();
    let direction = style.clone_direction();

    taffy::Style {
        dummy: core::marker::PhantomData,
        display: self::display(display),
        box_sizing: self::box_sizing(style.clone_box_sizing()),
        item_is_table: display.inside() == stylo::DisplayInside::Table,
        item_is_replaced: false,
        position: self::position(style.clone_position()),
        overflow: taffy::Point {
            x: self::overflow(style.clone_overflow_x()),
            y: self::overflow(style.clone_overflow_y()),
        },
        direction: self::direction(direction),
        scrollbar_width: 0.0,
        contain: self::contain(style.clone_contain(), display),

        size: taffy::Size {
            width: self::dimension(&pos.width),
            height: self::dimension(&pos.height),
        },
        min_size: taffy::Size {
            width: self::min_size(&pos.min_width),
            height: self::min_size(&pos.min_height),
        },
        max_size: taffy::Size {
            width: self::max_size(&pos.max_width),
            height: self::max_size(&pos.max_height),
        },
        aspect_ratio: self::aspect_ratio(pos.aspect_ratio),

        inset: taffy::Rect {
            left: self::inset(&pos.left),
            right: self::inset(&pos.right),
            top: self::inset(&pos.top),
            bottom: self::inset(&pos.bottom),
        },
        margin: taffy::Rect {
            left: self::margin(&margin.margin_left),
            right: self::margin(&margin.margin_right),
            top: self::margin(&margin.margin_top),
            bottom: self::margin(&margin.margin_bottom),
        },
        padding: taffy::Rect {
            left: self::length_percentage(&padding.padding_left.0),
            right: self::length_percentage(&padding.padding_right.0),
            top: self::length_percentage(&padding.padding_top.0),
            bottom: self::length_percentage(&padding.padding_bottom.0),
        },
        border: taffy::Rect {
            left: self::border(&border.border_left_width, border.border_left_style),
            right: self::border(&border.border_right_width, border.border_right_style),
            top: self::border(&border.border_top_width, border.border_top_style),
            bottom: self::border(&border.border_bottom_width, border.border_bottom_style),
        },

        gap: taffy::Size {
            width: self::gap(&pos.column_gap),
            height: self::gap(&pos.row_gap),
        },

        align_content: self::content_alignment(pos.align_content, display),
        justify_content: self::justify_content(
            pos.justify_content,
            pos.flex_direction,
            direction,
            display,
        ),
        align_items: self::item_alignment(pos.align_items.0, false),
        align_self: self::item_alignment(pos.align_self.0, false),
        justify_items: self::item_alignment(
            pos.justify_items.computed.0.0,
            direction == stylo::Direction::Rtl,
        ),
        justify_self: self::item_alignment(pos.justify_self.0, direction == stylo::Direction::Rtl),

        flex_direction: self::flex_direction(pos.flex_direction),
        flex_wrap: self::flex_wrap(pos.flex_wrap),
        flex_grow: pos.flex_grow.0,
        flex_shrink: pos.flex_shrink.0,
        flex_basis: self::flex_basis(&pos.flex_basis),

        grid_auto_flow: grid::auto_flow(pos.grid_auto_flow),
        grid_template_rows: grid::template_tracks(&pos.grid_template_rows),
        grid_template_columns: grid::template_tracks(&pos.grid_template_columns),
        grid_template_row_names: grid::template_line_names(&pos.grid_template_rows),
        grid_template_column_names: grid::template_line_names(&pos.grid_template_columns),
        grid_template_areas: grid::template_areas(&pos.grid_template_areas),
        grid_auto_rows: grid::auto_tracks(&pos.grid_auto_rows),
        grid_auto_columns: grid::auto_tracks(&pos.grid_auto_columns),
        grid_row: taffy::Line {
            start: grid::line(&pos.grid_row_start),
            end: grid::line(&pos.grid_row_end),
        },
        grid_column: taffy::Line {
            start: grid::line(&pos.grid_column_start),
            end: grid::line(&pos.grid_column_end),
        },
    }
}
