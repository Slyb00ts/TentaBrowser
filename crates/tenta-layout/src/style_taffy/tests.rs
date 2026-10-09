//! Round trip: CSS declarations through the cascade into `taffy::Style`.

use style::Atom;
use taffy::style_helpers::*;
use taffy::{
    AlignContent, AlignItems, AlignmentSafety, CompactLength, Dimension, GridPlacement,
    GridTemplateComponent as Template, LengthPercentage, LengthPercentageAuto,
    MaxTrackSizingFunction, MinMax, MinTrackSizingFunction, RepetitionCount, Style,
    TrackSizingFunction,
};
use tenta_css::cascade::StyleEnv;
use tenta_css::cascade::test_util::style_html;

use super::calc;
use super::convert::{ContainerKind, container_kind, to_taffy_style};

type Check = fn(&Style<Atom>, ContainerKind) -> bool;

struct Case {
    css: &'static str,
    check: Check,
}

const fn case(css: &'static str, check: Check) -> Case {
    Case { css, check }
}

/// Runs `f` on the `#t` element's converted style while its computed values
/// are alive.
fn with_style<R>(css: &str, f: impl FnOnce(&Style<Atom>, ContainerKind) -> R) -> R {
    let html = format!("<!doctype html><style>#t{{ {css} }}</style><div id=t></div>");
    let (doc, engine) = style_html(&html, StyleEnv::default());
    let node = doc.get_element_by_id("t").expect("the element exists");
    let computed = engine.style(node).expect("the element is styled");
    let converted = to_taffy_style(&computed);
    f(converted.style(), container_kind(computed.clone_display()))
}

fn resolve_calc(raw: CompactLength, basis: f32) -> Option<f32> {
    raw.is_calc().then(|| {
        // SAFETY: the handle was made by the conversion and the computed values
        // it points into are alive in the calling test.
        unsafe { calc::resolve(raw.calc_value(), basis) }
    })
}

fn px(value: f32) -> TrackSizingFunction {
    MinMax {
        min: length(value),
        max: length(value),
    }
}

fn fr_track(value: f32) -> TrackSizingFunction {
    MinMax {
        min: MinTrackSizingFunction::AUTO,
        max: fr(value),
    }
}

fn single(track: TrackSizingFunction) -> Template<Atom> {
    Template::Single(track)
}

fn safe(mut align: AlignContent) -> AlignContent {
    align.safety = AlignmentSafety::Safe;
    align
}

fn safe_item(mut align: AlignItems) -> AlignItems {
    align.safety = AlignmentSafety::Safe;
    align
}

fn atom(name: &str) -> Atom {
    Atom::from(name)
}

const CASES: &[Case] = &[
    // display
    case("display: flex", |s, k| {
        k == ContainerKind::Flex && s.display == taffy::Display::Flex
    }),
    case("display: inline-flex", |s, k| {
        k == ContainerKind::Flex && s.display == taffy::Display::Flex
    }),
    case("display: grid", |s, k| {
        k == ContainerKind::Grid && s.display == taffy::Display::Grid
    }),
    case("display: inline-grid", |s, k| {
        k == ContainerKind::Grid && s.display == taffy::Display::Grid
    }),
    case("display: none", |s, k| {
        k == ContainerKind::None && s.display == taffy::Display::None
    }),
    case("display: contents", |s, k| {
        k == ContainerKind::Contents && s.display == taffy::Display::DEFAULT
    }),
    case("display: none; flex-direction: column", |s, k| {
        k == ContainerKind::None && s.display == taffy::Display::None
    }),
    case("display: block", |_, k| k == ContainerKind::Native),
    case("display: flow-root", |_, k| k == ContainerKind::Native),
    case("display: table", |s, k| {
        k == ContainerKind::Native && s.item_is_table
    }),
    // flex container and item properties
    case("flex-direction: column", |s, _| {
        s.flex_direction == taffy::FlexDirection::Column
    }),
    case("flex-direction: row-reverse", |s, _| {
        s.flex_direction == taffy::FlexDirection::RowReverse
    }),
    case("flex-direction: column-reverse", |s, _| {
        s.flex_direction == taffy::FlexDirection::ColumnReverse
    }),
    case("flex-wrap: wrap", |s, _| {
        s.flex_wrap == taffy::FlexWrap::Wrap
    }),
    case("flex-wrap: wrap-reverse", |s, _| {
        s.flex_wrap == taffy::FlexWrap::WrapReverse
    }),
    case("flex-grow: 2.5", |s, _| s.flex_grow == 2.5),
    case("flex-shrink: 0", |s, _| s.flex_shrink == 0.0),
    case("flex-basis: 30px", |s, _| s.flex_basis == length(30.0)),
    case("flex-basis: 25%", |s, _| s.flex_basis == percent(0.25)),
    case("flex-basis: content", |s, _| {
        s.flex_basis == Dimension::content()
    }),
    case("flex-basis: auto", |s, _| s.flex_basis == Dimension::AUTO),
    case("flex: 1", |s, _| {
        s.flex_grow == 1.0 && s.flex_shrink == 1.0 && s.flex_basis == percent(0.0)
    }),
    case("flex: none", |s, _| {
        s.flex_grow == 0.0 && s.flex_shrink == 0.0 && s.flex_basis == Dimension::AUTO
    }),
    // item alignment
    case("align-items: center", |s, _| {
        s.align_items == Some(AlignItems::CENTER)
    }),
    case("align-items: flex-end", |s, _| {
        s.align_items == Some(AlignItems::FLEX_END)
    }),
    case("align-items: baseline", |s, _| {
        s.align_items == Some(AlignItems::BASELINE)
    }),
    case("align-items: normal", |s, _| {
        s.align_items == Some(AlignItems::STRETCH)
    }),
    case("align-items: safe center", |s, _| {
        s.align_items == Some(safe_item(AlignItems::CENTER))
    }),
    case("align-items: unsafe end", |s, _| {
        s.align_items == Some(AlignItems::END)
    }),
    case("align-items: self-start", |s, _| {
        s.align_items == Some(AlignItems::SELF_START)
    }),
    case("align-self: auto", |s, _| s.align_self.is_none()),
    case("align-self: stretch", |s, _| {
        s.align_self == Some(AlignItems::STRETCH)
    }),
    case("align-self: last baseline", |s, _| {
        s.align_self == Some(AlignItems::END)
    }),
    case("justify-items: center", |s, _| {
        s.justify_items == Some(AlignItems::CENTER)
    }),
    case("justify-items: end", |s, _| {
        s.justify_items == Some(AlignItems::END)
    }),
    case("direction: rtl; justify-items: left", |s, _| {
        s.direction == taffy::Direction::Rtl && s.justify_items == Some(AlignItems::END)
    }),
    case("justify-self: right", |s, _| {
        s.justify_self == Some(AlignItems::END)
    }),
    case("justify-self: auto", |s, _| s.justify_self.is_none()),
    case("justify-self: start", |s, _| {
        s.justify_self == Some(AlignItems::START)
    }),
    // content alignment
    case("display: flex; align-content: space-between", |s, _| {
        s.align_content == Some(AlignContent::SPACE_BETWEEN)
    }),
    case("display: flex; align-content: space-around", |s, _| {
        s.align_content == Some(AlignContent::SPACE_AROUND)
    }),
    case("display: flex; align-content: space-evenly", |s, _| {
        s.align_content == Some(AlignContent::SPACE_EVENLY)
    }),
    case("display: flex; align-content: stretch", |s, _| {
        s.align_content == Some(AlignContent::STRETCH)
    }),
    case("align-content: normal", |s, _| s.align_content.is_none()),
    case("display: block; align-content: center", |s, _| {
        s.align_content == Some(safe(AlignContent::CENTER))
    }),
    case("display: block; align-content: unsafe center", |s, _| {
        s.align_content == Some(AlignContent::CENTER)
    }),
    case("display: flex; justify-content: flex-start", |s, _| {
        s.justify_content == Some(AlignContent::FLEX_START)
    }),
    case("display: flex; justify-content: center", |s, _| {
        s.justify_content == Some(AlignContent::CENTER)
    }),
    case("justify-content: left", |s, _| {
        s.justify_content == Some(AlignContent::START)
    }),
    case("justify-content: right", |s, _| {
        s.justify_content == Some(AlignContent::END)
    }),
    case("direction: rtl; justify-content: right", |s, _| {
        s.justify_content == Some(AlignContent::START)
    }),
    case("direction: rtl; justify-content: left", |s, _| {
        s.justify_content == Some(AlignContent::END)
    }),
    case(
        "direction: rtl; flex-direction: row-reverse; justify-content: left",
        |s, _| s.justify_content == Some(AlignContent::END),
    ),
    case(
        "direction: rtl; flex-direction: column; justify-content: left",
        |s, _| s.justify_content == Some(AlignContent::START),
    ),
    case("flex-direction: column; justify-content: right", |s, _| {
        s.justify_content == Some(AlignContent::START)
    }),
    case("justify-content: safe center", |s, _| {
        s.justify_content == Some(safe(AlignContent::CENTER))
    }),
    // gap
    case("gap: 10px 20px", |s, _| {
        s.gap.height == length(10.0) && s.gap.width == length(20.0)
    }),
    case("row-gap: 5%", |s, _| s.gap.height == percent(0.05)),
    case("column-gap: normal", |s, _| {
        s.gap.width == LengthPercentage::ZERO
    }),
    // sizing
    case("width: 100px", |s, _| s.size.width == length(100.0)),
    case("width: 50%", |s, _| s.size.width == percent(0.5)),
    case("width: auto", |s, _| s.size.width == Dimension::AUTO),
    case("width: min-content", |s, _| {
        s.size.width == Dimension::min_content()
    }),
    case("width: max-content", |s, _| {
        s.size.width == Dimension::max_content()
    }),
    case("width: fit-content", |s, _| {
        s.size.width == Dimension::fit_content()
    }),
    case("width: fit-content(40px)", |s, _| {
        s.size.width == Dimension::fit_content_px(40.0)
    }),
    case("width: fit-content(30%)", |s, _| {
        s.size.width == Dimension::fit_content_percent(0.3)
    }),
    case("width: -webkit-fill-available", |s, _| {
        s.size.width == Dimension::stretch()
    }),
    case("width: fit-content(calc(10px + 5%))", |s, _| {
        s.size.width == Dimension::AUTO
    }),
    case("height: 7px", |s, _| s.size.height == length(7.0)),
    case("min-width: 10px", |s, _| s.min_size.width == length(10.0)),
    case("min-height: 5%", |s, _| s.min_size.height == percent(0.05)),
    case("min-width: min-content", |s, _| {
        s.min_size.width == LengthPercentageAuto::AUTO
    }),
    case("max-width: 300px", |s, _| s.max_size.width == length(300.0)),
    case("max-height: none", |s, _| {
        s.max_size.height == LengthPercentageAuto::AUTO
    }),
    case("max-width: max-content", |s, _| {
        s.max_size.width == LengthPercentageAuto::AUTO
    }),
    case("width: calc(100% - 10px)", |s, _| {
        resolve_calc(s.size.width.into_raw(), 200.0) == Some(190.0)
    }),
    case("min-height: calc(2px + 3px)", |s, _| {
        s.min_size.height == length(5.0)
    }),
    case("max-width: calc(50% + 10px)", |s, _| {
        resolve_calc(s.max_size.width.into_raw(), 100.0) == Some(60.0)
    }),
    case("box-sizing: content-box", |s, _| {
        s.box_sizing == taffy::BoxSizing::ContentBox
    }),
    case("box-sizing: border-box", |s, _| {
        s.box_sizing == taffy::BoxSizing::BorderBox
    }),
    case("aspect-ratio: 16 / 9", |s, _| {
        s.aspect_ratio == Some(16.0 / 9.0)
    }),
    case("aspect-ratio: auto", |s, _| s.aspect_ratio.is_none()),
    case("aspect-ratio: auto 4 / 3", |s, _| {
        s.aspect_ratio == Some(4.0 / 3.0)
    }),
    // margin, padding, border
    case("margin: 1px 2px 3px 4px", |s, _| {
        s.margin.top == length(1.0)
            && s.margin.right == length(2.0)
            && s.margin.bottom == length(3.0)
            && s.margin.left == length(4.0)
    }),
    case("margin: auto", |s, _| {
        s.margin.top == LengthPercentageAuto::AUTO
            && s.margin.right == LengthPercentageAuto::AUTO
            && s.margin.bottom == LengthPercentageAuto::AUTO
            && s.margin.left == LengthPercentageAuto::AUTO
    }),
    case("margin-left: 10%", |s, _| s.margin.left == percent(0.1)),
    case("margin-top: calc(10px + 5%)", |s, _| {
        resolve_calc(s.margin.top.into_raw(), 100.0) == Some(15.0)
    }),
    case("padding: 1px 2px 3px 4px", |s, _| {
        s.padding.top == length(1.0)
            && s.padding.right == length(2.0)
            && s.padding.bottom == length(3.0)
            && s.padding.left == length(4.0)
    }),
    case("padding-left: 25%", |s, _| s.padding.left == percent(0.25)),
    case("padding-right: calc(4px * 2)", |s, _| {
        s.padding.right == length(8.0)
    }),
    case("border: 3px solid", |s, _| {
        s.border.top == length(3.0)
            && s.border.right == length(3.0)
            && s.border.bottom == length(3.0)
            && s.border.left == length(3.0)
    }),
    case("border: 3px none", |s, _| {
        s.border.top == LengthPercentage::ZERO
    }),
    case(
        "border-style: solid; border-width: 1px 2px 3px 4px",
        |s, _| {
            s.border.top == length(1.0)
                && s.border.right == length(2.0)
                && s.border.bottom == length(3.0)
                && s.border.left == length(4.0)
        },
    ),
    case("border: 5px hidden", |s, _| {
        s.border.left == LengthPercentage::ZERO
    }),
    // position and inset
    case("position: relative", |s, _| {
        s.position == taffy::Position::Relative
    }),
    case("position: static", |s, _| {
        s.position == taffy::Position::Relative
    }),
    case("position: sticky", |s, _| {
        s.position == taffy::Position::Relative
    }),
    case("position: absolute", |s, _| {
        s.position == taffy::Position::Absolute
    }),
    case("position: fixed", |s, _| {
        s.position == taffy::Position::Absolute
    }),
    case("position: absolute; inset: 1px 2px 3px 4px", |s, _| {
        s.inset.top == length(1.0)
            && s.inset.right == length(2.0)
            && s.inset.bottom == length(3.0)
            && s.inset.left == length(4.0)
    }),
    case("position: relative; left: auto; top: 50%", |s, _| {
        s.inset.left == LengthPercentageAuto::AUTO && s.inset.top == percent(0.5)
    }),
    case("position: relative; right: calc(10px + 10%)", |s, _| {
        resolve_calc(s.inset.right.into_raw(), 50.0) == Some(15.0)
    }),
    // overflow
    case("overflow: visible", |s, _| {
        s.overflow.x == taffy::Overflow::Visible && s.overflow.y == taffy::Overflow::Visible
    }),
    case("overflow: hidden", |s, _| {
        s.overflow.x == taffy::Overflow::Hidden && s.overflow.y == taffy::Overflow::Hidden
    }),
    case("overflow: clip", |s, _| {
        s.overflow.x == taffy::Overflow::Clip
    }),
    case("overflow: scroll", |s, _| {
        s.overflow.x == taffy::Overflow::Scroll && s.scrollbar_width == 0.0
    }),
    case("overflow: auto", |s, _| {
        s.overflow.y == taffy::Overflow::Scroll && s.scrollbar_width == 0.0
    }),
    case("overflow-x: hidden; overflow-y: auto", |s, _| {
        s.overflow.x == taffy::Overflow::Hidden && s.overflow.y == taffy::Overflow::Scroll
    }),
    // grid tracks
    case("grid-template-columns: 100px 1fr", |s, _| {
        s.grid_template_columns == [single(px(100.0)), single(fr_track(1.0))]
    }),
    case("grid-template-rows: minmax(10px, 1fr) auto", |s, _| {
        s.grid_template_rows
            == [
                single(MinMax {
                    min: length(10.0),
                    max: fr(1.0),
                }),
                single(MinMax {
                    min: MinTrackSizingFunction::AUTO,
                    max: MaxTrackSizingFunction::AUTO,
                }),
            ]
    }),
    case("grid-template-columns: repeat(3, 50px)", |s, _| {
        matches!(
            s.grid_template_columns.as_slice(),
            [Template::Repeat(r)] if r.count == RepetitionCount::Count(3) && r.tracks.as_slice() == [px(50.0)]
        )
    }),
    case("grid-template-columns: repeat(auto-fill, 100px)", |s, _| {
        matches!(
            s.grid_template_columns.as_slice(),
            [Template::Repeat(r)] if r.count == RepetitionCount::AutoFill && r.tracks.as_slice() == [px(100.0)]
        )
    }),
    case(
        "grid-template-columns: repeat(auto-fit, minmax(100px, 1fr))",
        |s, _| {
            matches!(
                s.grid_template_columns.as_slice(),
                [Template::Repeat(r)] if r.count == RepetitionCount::AutoFit
                    && r.tracks.as_slice() == [MinMax { min: length(100.0), max: fr(1.0) }]
            )
        },
    ),
    case("grid-template-columns: fit-content(100px) 20px", |s, _| {
        s.grid_template_columns
            == [
                single(MinMax {
                    min: MinTrackSizingFunction::AUTO,
                    max: MaxTrackSizingFunction::fit_content(length(100.0)),
                }),
                single(px(20.0)),
            ]
    }),
    case("grid-template-columns: min-content max-content", |s, _| {
        s.grid_template_columns
            == [
                single(MinMax {
                    min: MinTrackSizingFunction::MIN_CONTENT,
                    max: MaxTrackSizingFunction::MIN_CONTENT,
                }),
                single(MinMax {
                    min: MinTrackSizingFunction::MAX_CONTENT,
                    max: MaxTrackSizingFunction::MAX_CONTENT,
                }),
            ]
    }),
    case("grid-template-columns: none", |s, _| {
        s.grid_template_columns.is_empty() && s.grid_template_column_names.is_empty()
    }),
    case("grid-template-columns: [a] 1fr [b c] 2fr", |s, _| {
        s.grid_template_column_names
            == [
                vec![atom("a")],
                vec![atom("b"), atom("c")],
                Vec::<Atom>::new(),
            ]
    }),
    case("grid-template-rows: 30% calc(10px + 5%)", |s, _| {
        s.grid_template_rows.len() == 2
            && s.grid_template_rows[0]
                == single(MinMax {
                    min: percent(0.3),
                    max: percent(0.3),
                })
            && matches!(&s.grid_template_rows[1],
                Template::Single(t) if resolve_calc(t.min.into_raw(), 100.0) == Some(15.0))
    }),
    case("grid-auto-rows: 50px", |s, _| {
        s.grid_auto_rows == [px(50.0)]
    }),
    case("grid-auto-columns: minmax(10px, auto) 1fr", |s, _| {
        s.grid_auto_columns
            == [
                MinMax {
                    min: length(10.0),
                    max: MaxTrackSizingFunction::AUTO,
                },
                fr_track(1.0),
            ]
    }),
    case("grid-auto-flow: row", |s, _| {
        s.grid_auto_flow == taffy::GridAutoFlow::Row
    }),
    case("grid-auto-flow: column", |s, _| {
        s.grid_auto_flow == taffy::GridAutoFlow::Column
    }),
    case("grid-auto-flow: dense", |s, _| {
        s.grid_auto_flow == taffy::GridAutoFlow::RowDense
    }),
    case("grid-auto-flow: column dense", |s, _| {
        s.grid_auto_flow == taffy::GridAutoFlow::ColumnDense
    }),
    // grid areas and lines
    case(r#"grid-template-areas: "a b" "c d""#, |s, _| {
        s.grid_template_areas.as_ref().is_some_and(|areas| {
            areas.row_count == 2
                && areas.column_count == 2
                && areas.areas.len() == 4
                && areas.areas.iter().any(|a| {
                    a.name == atom("d")
                        && (a.row_start, a.row_end, a.column_start, a.column_end) == (2, 3, 2, 3)
                })
        })
    }),
    case("grid-template-areas: none", |s, _| {
        s.grid_template_areas.is_none()
    }),
    case("grid-row: 2 / span 3", |s, _| {
        s.grid_row.start == line(2) && s.grid_row.end == span(3)
    }),
    case("grid-column: a / b", |s, _| {
        s.grid_column.start == GridPlacement::NamedLine(atom("a"), 0)
            && s.grid_column.end == GridPlacement::NamedLine(atom("b"), 0)
    }),
    case("grid-column: span 2 / auto", |s, _| {
        s.grid_column.start == span(2) && s.grid_column.end == GridPlacement::Auto
    }),
    case("grid-area: 1 / 2 / 3 / 4", |s, _| {
        s.grid_row.start == line(1)
            && s.grid_column.start == line(2)
            && s.grid_row.end == line(3)
            && s.grid_column.end == line(4)
    }),
    case("grid-row-start: span foo 2", |s, _| {
        s.grid_row.start == GridPlacement::NamedSpan(atom("foo"), 2)
    }),
    case("grid-row-start: -1", |s, _| s.grid_row.start == line(-1)),
    case("grid-column-end: auto", |s, _| {
        s.grid_column.end == GridPlacement::Auto
    }),
];

#[test]
fn declarations_convert_to_taffy_style_values() {
    assert!(
        CASES.len() >= 80,
        "the table covers {} declarations",
        CASES.len()
    );
    let failures: Vec<_> = CASES
        .iter()
        .filter_map(|case| {
            with_style(case.css, |style, kind| {
                (!(case.check)(style, kind)).then_some((case.css, kind))
            })
        })
        .collect();
    assert!(
        failures.is_empty(),
        "declarations that did not convert: {failures:#?}"
    );
}

#[test]
fn initial_values_convert_to_taffy_defaults() {
    with_style("", |style, kind| {
        assert_eq!(kind, ContainerKind::Native);
        assert_eq!(style.size.width, Dimension::AUTO);
        assert_eq!(style.min_size.width, LengthPercentageAuto::AUTO);
        assert_eq!(style.max_size.height, LengthPercentageAuto::AUTO);
        assert_eq!(style.flex_shrink, 1.0);
        assert_eq!(style.flex_grow, 0.0);
        assert_eq!(style.flex_basis, Dimension::AUTO);
        assert_eq!(style.position, taffy::Position::Relative);
        assert_eq!(style.box_sizing, taffy::BoxSizing::ContentBox);
        assert_eq!(style.align_items, Some(AlignItems::STRETCH));
        assert_eq!(style.align_content, None);
        assert_eq!(style.aspect_ratio, None);
        assert_eq!(style.scrollbar_width, 0.0);
        assert!(style.grid_template_rows.is_empty());
        assert!(!style.item_is_replaced);
    });
}

#[test]
fn calc_handles_are_aligned_pointers() {
    with_style("width: calc(100% - 10px)", |style, _| {
        let raw = style.size.width.into_raw();
        assert!(raw.is_calc());
        assert_eq!(raw.calc_value() as usize % 8, 0);
    });
}
