// Derived from `stylo_taffy` 0.3.0-beta.2 (`src/convert.rs`), part of Blitz by
// DioxusLabs, https://github.com/DioxusLabs/blitz (`packages/stylo_taffy`).
// The original is licensed MIT OR Apache-2.0 OR MPL-2.0; this derived file is
// used under the MIT licence. The upstream licence text carries no copyright
// line; see THIRD_PARTY_NOTICES.md for the notice. Modified for Stylo 0.21 and
// this crate: eager `Vec` results replace the borrowing iterator wrappers.

//! Conversions of the grid properties.

use style::computed_values::grid_auto_flow::T as GridAutoFlow;
use style::values::computed::LengthPercentage;
use style::values::computed::{GridLine, GridTemplateComponent, ImplicitGridTracks};
use style::values::generics::grid::{RepeatCount, TrackBreadth, TrackListValue, TrackSize};
use style::values::specified::GenericGridTemplateComponent;
use style::values::specified::position::{GridTemplateAreas, NamedArea};

use style::Atom;
use taffy::style_helpers::*;

use super::convert::length_percentage;

pub(super) fn auto_flow(input: GridAutoFlow) -> taffy::GridAutoFlow {
    let is_row = input.contains(GridAutoFlow::ROW);
    let is_dense = input.contains(GridAutoFlow::DENSE);

    match (is_row, is_dense) {
        (true, false) => taffy::GridAutoFlow::Row,
        (true, true) => taffy::GridAutoFlow::RowDense,
        (false, false) => taffy::GridAutoFlow::Column,
        (false, true) => taffy::GridAutoFlow::ColumnDense,
    }
}

pub(super) fn line(input: &GridLine) -> taffy::GridPlacement<Atom> {
    let named = !input.ident.0.is_empty();
    if input.is_auto() {
        taffy::GridPlacement::Auto
    } else if input.is_span {
        let span = u16::try_from(input.line_num).unwrap_or(1);
        if named {
            taffy::GridPlacement::NamedSpan(input.ident.0.clone(), span)
        } else {
            taffy::GridPlacement::Span(span)
        }
    } else if named {
        taffy::GridPlacement::NamedLine(
            input.ident.0.clone(),
            i16::try_from(input.line_num).unwrap_or(i16::MAX),
        )
    } else if input.line_num != 0 {
        taffy::style_helpers::line(i16::try_from(input.line_num).unwrap_or(i16::MAX))
    } else {
        taffy::GridPlacement::Auto
    }
}

pub(super) fn template_tracks(
    input: &GridTemplateComponent,
) -> Vec<taffy::GridTemplateComponent<Atom>> {
    match input {
        GenericGridTemplateComponent::TrackList(list) => list
            .values
            .iter()
            .map(|track| match track {
                TrackListValue::TrackSize(size) => {
                    taffy::GridTemplateComponent::Single(track_size(size))
                }
                TrackListValue::TrackRepeat(repeat) => {
                    taffy::GridTemplateComponent::Repeat(taffy::GridTemplateRepetition {
                        count: track_repeat(repeat.count),
                        tracks: repeat.track_sizes.iter().map(track_size).collect(),
                        line_names: repeat
                            .line_names
                            .iter()
                            .map(|names| names.iter().map(|ident| ident.0.clone()).collect())
                            .collect(),
                    })
                }
            })
            .collect(),
        // Subgrid and masonry are not supported by Taffy: the container gets
        // no explicit tracks.
        GenericGridTemplateComponent::None
        | GenericGridTemplateComponent::Subgrid(_)
        | GenericGridTemplateComponent::Masonry => Vec::new(),
    }
}

pub(super) fn template_line_names(input: &GridTemplateComponent) -> Vec<Vec<Atom>> {
    match input {
        GenericGridTemplateComponent::TrackList(list) => list
            .line_names
            .iter()
            .map(|names| names.iter().map(|ident| ident.0.clone()).collect())
            .collect(),
        GenericGridTemplateComponent::None
        | GenericGridTemplateComponent::Subgrid(_)
        | GenericGridTemplateComponent::Masonry => Vec::new(),
    }
}

fn template_area(input: &NamedArea) -> taffy::GridTemplateArea<Atom> {
    taffy::GridTemplateArea {
        name: input.name.clone(),
        row_start: input.rows.start as u16,
        row_end: input.rows.end as u16,
        column_start: input.columns.start as u16,
        column_end: input.columns.end as u16,
    }
}

pub(super) fn template_areas(input: &GridTemplateAreas) -> Option<taffy::GridTemplateAreas<Atom>> {
    match input {
        GridTemplateAreas::None => None,
        GridTemplateAreas::Areas(areas) => {
            let template = &areas.0;
            Some(taffy::GridTemplateAreas {
                areas: template.areas.iter().map(template_area).collect(),
                row_count: template.strings.len() as u16,
                column_count: template.width as u16,
            })
        }
    }
}

pub(super) fn auto_tracks(input: &ImplicitGridTracks) -> Vec<taffy::TrackSizingFunction> {
    input.0.iter().map(track_size).collect()
}

fn track_repeat(input: RepeatCount<i32>) -> taffy::RepetitionCount {
    match input {
        RepeatCount::Number(val) => {
            taffy::RepetitionCount::Count(val.clamp(1, i32::from(u16::MAX)) as u16)
        }
        RepeatCount::AutoFill => taffy::RepetitionCount::AutoFill,
        RepeatCount::AutoFit => taffy::RepetitionCount::AutoFit,
    }
}

fn track_size(input: &TrackSize<LengthPercentage>) -> taffy::TrackSizingFunction {
    match input {
        TrackSize::Breadth(breadth) => taffy::MinMax {
            min: min_track(breadth),
            max: max_track(breadth),
        },
        TrackSize::Minmax(min, max) => taffy::MinMax {
            min: min_track(min),
            max: max_track(max),
        },
        TrackSize::FitContent(limit) => taffy::MinMax {
            min: taffy::MinTrackSizingFunction::AUTO,
            max: match limit {
                TrackBreadth::Breadth(lp) => {
                    taffy::MaxTrackSizingFunction::fit_content(length_percentage(lp))
                }
                // `fit-content()` takes only a length-percentage.
                TrackBreadth::Flex(_)
                | TrackBreadth::Auto
                | TrackBreadth::MinContent
                | TrackBreadth::MaxContent => taffy::MaxTrackSizingFunction::AUTO,
            },
        },
    }
}

fn min_track(input: &TrackBreadth<LengthPercentage>) -> taffy::MinTrackSizingFunction {
    match input {
        TrackBreadth::Breadth(lp) => taffy::MinTrackSizingFunction::from(length_percentage(lp)),
        TrackBreadth::Flex(_) | TrackBreadth::Auto => taffy::MinTrackSizingFunction::AUTO,
        TrackBreadth::MinContent => taffy::MinTrackSizingFunction::MIN_CONTENT,
        TrackBreadth::MaxContent => taffy::MinTrackSizingFunction::MAX_CONTENT,
    }
}

fn max_track(input: &TrackBreadth<LengthPercentage>) -> taffy::MaxTrackSizingFunction {
    match input {
        TrackBreadth::Breadth(lp) => taffy::MaxTrackSizingFunction::from(length_percentage(lp)),
        TrackBreadth::Flex(val) => taffy::MaxTrackSizingFunction::from_fr(val.0),
        TrackBreadth::Auto => taffy::MaxTrackSizingFunction::AUTO,
        TrackBreadth::MinContent => taffy::MaxTrackSizingFunction::MIN_CONTENT,
        TrackBreadth::MaxContent => taffy::MaxTrackSizingFunction::MAX_CONTENT,
    }
}
