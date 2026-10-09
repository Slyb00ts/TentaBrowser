//! Block layout completion against Chrome: `min-`/`max-` limits, replaced
//! sizing, relative offsets, shrink-to-fit widths and the quirks-mode height
//! rules.
//!
//! The expectations in `block_complete/*_cases.rs` are `getBoundingClientRect`
//! values recorded from Chrome 153 for exactly the HTML next to them. Cases
//! that name `font-family: LS` were recorded with that name bound to Liberation
//! Serif, which is what this crate resolves unknown families to.

// `FontSystem::new_embedded` exists only with the bundled fonts.
#![cfg(feature = "embedded-fonts")]

#[path = "block_complete/fit_cases.rs"]
mod fit_cases;
#[path = "block_complete/list_cases.rs"]
mod list_cases;
#[path = "block_complete/quirks_cases.rs"]
mod quirks_cases;
#[path = "block_complete/relative_cases.rs"]
mod relative_cases;
#[path = "block_complete/relative_percent_cases.rs"]
mod relative_percent_cases;
#[path = "block_complete/replaced_cases.rs"]
mod replaced_cases;
#[path = "block_complete/size_cases.rs"]
mod size_cases;
mod support;

use support::{Case, check_cases};

#[test]
fn min_and_max_limits_match_chrome() {
    check_cases(size_cases::CASES);
}

#[test]
fn replaced_limits_match_chrome() {
    check_cases(replaced_cases::CASES);
}

#[test]
fn relative_offsets_match_chrome() {
    check_cases(relative_cases::CASES);
}

#[test]
fn relative_percentage_offsets_match_chrome() {
    check_cases(relative_percent_cases::CASES);
}

#[test]
fn shrink_to_fit_widths_match_chrome() {
    check_cases(fit_cases::CASES);
}

#[test]
fn quirks_mode_heights_match_chrome() {
    check_cases(quirks_cases::CASES);
}

#[test]
fn empty_list_items_reserve_a_line_like_chrome() {
    check_cases(list_cases::CASES);
}
