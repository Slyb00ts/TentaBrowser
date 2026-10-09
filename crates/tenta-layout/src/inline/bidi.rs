//! Bidirectional text (UAX #9) for lines whose content is not all in one
//! direction.
//!
//! The shaper resolves the algorithm for each chunk and arranges the glyphs of
//! every run in visual order, so line layout only has to decide where whole
//! runs go: it recovers an embedding level for each shaped cluster
//! ([`cluster_levels`]), cuts atoms where the level changes, and reorders the
//! atoms, atomic inlines and tabs of each line by level ([`reorder_line`]).

use crate::types::Direction;

/// The paragraph embedding level of `direction`.
pub(super) fn base_level(direction: Direction) -> u8 {
    match direction {
        Direction::Ltr => 0,
        Direction::Rtl => 1,
    }
}

/// Embedding levels of the clusters of a shaped chunk, given in logical order
/// as `(in a right-to-left run, position in the shaper's visual order)`.
///
/// The shaper resolves the algorithm and exposes only each run's direction and
/// the visual order it produced; the levels follow from those. A right-to-left
/// cluster is at level 1. A left-to-right one is at level 0, or at level 2
/// when it was turned around together with a right-to-left cluster next to it
/// (digits after Hebrew): it then sits on the far side of that cluster from
/// where logical order puts it. With no right-to-left cluster at all it is at
/// the paragraph's level, plus one in a right-to-left paragraph. Explicit
/// embeddings (levels above 2) are not recovered.
pub(super) fn cluster_levels(base: Direction, clusters: &[(bool, u32)]) -> Vec<u8> {
    // The visual position of the nearest right-to-left cluster on each side.
    let mut before = Vec::with_capacity(clusters.len());
    let mut nearest = None;
    for &(rtl, visual) in clusters {
        before.push(nearest);
        if rtl {
            nearest = Some(visual);
        }
    }
    let mut after = vec![None; clusters.len()];
    let mut nearest = None;
    for (i, &(rtl, visual)) in clusters.iter().enumerate().rev() {
        after[i] = nearest;
        if rtl {
            nearest = Some(visual);
        }
    }
    clusters
        .iter()
        .zip(before.into_iter().zip(after))
        .map(
            |(&(rtl, visual), (before, after))| match (rtl, before, after) {
                (true, ..) => 1,
                (false, None, None) => 2 * base_level(base),
                (false, before, after)
                    if before.is_some_and(|o| visual < o) || after.is_some_and(|o| visual > o) =>
                {
                    2
                }
                (false, ..) => 0,
            },
        )
        .collect()
}

/// The level of an atomic inline between neighbours at `before` and `after`
/// (`None` at the ends of the line): it follows them when they agree on a
/// direction and the paragraph otherwise.
pub(super) fn object_level(base: Direction, before: Option<u8>, after: Option<u8>) -> u8 {
    let paragraph = base_level(base);
    let before = before.unwrap_or(paragraph);
    let after = after.unwrap_or(paragraph);
    if before & 1 == after & 1 {
        before.min(after)
    } else {
        paragraph
    }
}

/// Reorders the items of one line from logical to visual order (rule L2):
/// returns, left to right, the logical index of each item. `levels` holds the
/// embedding level of each item in logical order.
pub(super) fn reorder_line(levels: &[u8]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..levels.len()).collect();
    let highest = levels.iter().copied().max().unwrap_or(0);
    let Some(lowest_odd) = levels.iter().copied().filter(|l| l & 1 == 1).min() else {
        return order;
    };
    for level in (lowest_odd..=highest).rev() {
        let mut i = 0;
        while i < order.len() {
            if levels[order[i]] < level {
                i += 1;
                continue;
            }
            let start = i;
            while i < order.len() && levels[order[i]] >= level {
                i += 1;
            }
            order[start..i].reverse();
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Levels for clusters whose runs are `rtl` and whose visual positions
    /// are `visual`, both in logical order.
    fn levels(base: Direction, rtl: &[bool], visual: &[u32]) -> Vec<u8> {
        let clusters: Vec<(bool, u32)> = rtl.iter().copied().zip(visual.iter().copied()).collect();
        cluster_levels(base, &clusters)
    }

    #[test]
    fn left_to_right_text_in_a_left_to_right_paragraph_is_level_zero() {
        assert_eq!(levels(Direction::Ltr, &[false; 3], &[0, 1, 2]), [0, 0, 0]);
    }

    #[test]
    fn a_hebrew_run_between_latin_text_is_level_one() {
        // "a \u{5d0}\u{5d1} b": the Hebrew pair is reversed in place.
        assert_eq!(
            levels(
                Direction::Ltr,
                &[false, false, true, true, false, false],
                &[0, 1, 3, 2, 4, 5]
            ),
            [0, 0, 1, 1, 0, 0]
        );
    }

    #[test]
    fn digits_after_hebrew_are_level_two() {
        // "\u{5d0} 12": the whole line reverses around the digits.
        assert_eq!(
            levels(Direction::Ltr, &[true, true, false, false], &[3, 2, 0, 1]),
            [1, 1, 2, 2]
        );
    }

    #[test]
    fn arabic_digits_between_latin_and_hebrew_do_not_pull_the_latin_around() {
        // "a AN \u{5d0}": the digits turn around with the Hebrew, the Latin stays.
        assert_eq!(
            levels(
                Direction::Ltr,
                &[false, false, false, true, true],
                &[0, 1, 4, 3, 2]
            ),
            [0, 0, 2, 1, 1]
        );
    }

    #[test]
    fn latin_in_a_right_to_left_paragraph_is_level_two() {
        assert_eq!(levels(Direction::Rtl, &[false; 2], &[0, 1]), [2, 2]);
        // "ab." with the full stop moved to the front of the line.
        assert_eq!(
            levels(Direction::Rtl, &[false, false, true], &[1, 2, 0]),
            [2, 2, 1]
        );
    }

    #[test]
    fn reorder_reverses_right_to_left_runs() {
        assert_eq!(reorder_line(&[0, 1, 1, 0]), [0, 2, 1, 3]);
        assert_eq!(reorder_line(&[1, 1, 1]), [2, 1, 0]);
    }

    #[test]
    fn reorder_keeps_numbers_in_reading_order_inside_right_to_left_text() {
        assert_eq!(reorder_line(&[1, 2, 2, 1]), [3, 1, 2, 0]);
    }

    #[test]
    fn reorder_without_odd_levels_changes_nothing() {
        assert_eq!(reorder_line(&[0, 2, 2, 0]), [0, 1, 2, 3]);
    }

    #[test]
    fn objects_follow_agreeing_neighbours() {
        assert_eq!(object_level(Direction::Ltr, Some(1), Some(1)), 1);
        assert_eq!(object_level(Direction::Ltr, Some(1), Some(0)), 0);
        assert_eq!(object_level(Direction::Rtl, None, Some(2)), 1);
    }
}
