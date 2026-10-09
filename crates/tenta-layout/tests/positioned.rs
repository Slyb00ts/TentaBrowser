//! Absolutely and fixed positioned layout against Chrome: containing blocks,
//! the width and height equations, static positions in block, inline, flex and
//! grid parents, and scrollable overflow.
//!
//! The expectations in `positioned/chrome_*.rs` are `getBoundingClientRect`
//! values recorded from Chrome 154 for exactly the HTML next to them (text cases
//! load the bundled Liberation Sans through `@font-face` in Chrome).

// `FontSystem::new_embedded` exists only with the bundled fonts.
#![cfg(feature = "embedded-fonts")]

#[path = "positioned/chrome_containing.rs"]
mod chrome_containing;
#[path = "positioned/chrome_equations.rs"]
mod chrome_equations;
#[path = "positioned/chrome_flex_grid.rs"]
mod chrome_flex_grid;
#[path = "positioned/chrome_inline.rs"]
mod chrome_inline;
#[path = "positioned/chrome_misc.rs"]
mod chrome_misc;
#[path = "positioned/chrome_scroll.rs"]
mod chrome_scroll;
#[path = "positioned/chrome_static_flow.rs"]
mod chrome_static_flow;
mod support;

use support::{Case, check_cases};

#[test]
fn containing_blocks_match_chrome() {
    check_cases(chrome_containing::CASES);
}

#[test]
fn size_and_inset_equations_match_chrome() {
    check_cases(chrome_equations::CASES);
}

#[test]
fn static_positions_in_flow_match_chrome() {
    check_cases(chrome_static_flow::CASES);
}

#[test]
fn scrollable_overflow_matches_chrome() {
    check_cases(chrome_scroll::CASES);
}

#[test]
fn flex_and_grid_parents_match_chrome() {
    check_cases(chrome_flex_grid::CASES);
}

#[test]
fn inline_parents_and_containing_blocks_match_chrome() {
    check_cases(chrome_inline::CASES);
}

#[test]
fn miscellaneous_positioned_boxes_match_chrome() {
    check_cases(chrome_misc::CASES);
}
