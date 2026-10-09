//! Grid layout against Chrome: track sizing, auto-repeat, implicit tracks and
//! auto-flow, line and area placement, gaps, alignment, right-to-left grids,
//! intrinsic sizing and nesting.
//!
//! The expectations in `grid/chrome_*.rs` are `getBoundingClientRect` values
//! recorded from Chrome 154 for exactly the HTML next to them.

// `FontSystem::new_embedded` exists only with the bundled fonts.
#![cfg(feature = "embedded-fonts")]

#[path = "grid/chrome_align.rs"]
mod chrome_align;
#[path = "grid/chrome_areas.rs"]
mod chrome_areas;
#[path = "grid/chrome_baseline.rs"]
mod chrome_baseline;
#[path = "grid/chrome_extra.rs"]
mod chrome_extra;
#[path = "grid/chrome_flow.rs"]
mod chrome_flow;
#[path = "grid/chrome_gaps.rs"]
mod chrome_gaps;
#[path = "grid/chrome_intrinsic.rs"]
mod chrome_intrinsic;
#[path = "grid/chrome_placement.rs"]
mod chrome_placement;
#[path = "grid/chrome_repeat.rs"]
mod chrome_repeat;
#[path = "grid/chrome_rtl.rs"]
mod chrome_rtl;
#[path = "grid/chrome_tracks.rs"]
mod chrome_tracks;
mod support;

use support::{Case, check_cases};

#[test]
fn track_sizing_matches_chrome() {
    check_cases(chrome_tracks::CASES);
}

#[test]
fn auto_repeat_matches_chrome() {
    check_cases(chrome_repeat::CASES);
}

#[test]
fn implicit_tracks_and_auto_flow_match_chrome() {
    check_cases(chrome_flow::CASES);
}

#[test]
fn line_and_name_placement_match_chrome() {
    check_cases(chrome_placement::CASES);
}

#[test]
fn template_areas_match_chrome() {
    check_cases(chrome_areas::CASES);
}

#[test]
fn gaps_match_chrome() {
    check_cases(chrome_gaps::CASES);
}

#[test]
fn alignment_matches_chrome() {
    check_cases(chrome_align::CASES);
}

#[test]
fn right_to_left_grids_match_chrome() {
    check_cases(chrome_rtl::CASES);
}

#[test]
fn intrinsic_sizing_and_nesting_match_chrome() {
    check_cases(chrome_intrinsic::CASES);
}

#[test]
fn baselines_match_chrome() {
    check_cases(chrome_baseline::CASES);
}

#[test]
fn aspect_ratio_overflow_and_nested_contexts_match_chrome() {
    check_cases(chrome_extra::CASES);
}
