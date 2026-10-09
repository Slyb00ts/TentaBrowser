//! Dash and dot placement for dashed and dotted border sides, after Blink's
//! `StyledStrokeData` and `DrawLineWithStyle`. Marks are produced along one
//! side as spans and dot centres, so the caller draws them as rectangles and
//! circles instead of stroking a dashed path.

/// One painted mark, as distances from the start of the side.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) enum Mark {
    /// A filled stretch from the first distance to the second.
    Span(f32, f32),
    /// A round dot of the side's width centred at this distance.
    Dot(f32),
}

/// Emits the marks of a dashed side `length` long and `width` thick. A side too
/// short for two dashes is drawn solid, so dashes always land on both corners.
pub(super) fn dashed_marks(length: f32, width: f32, mut emit: impl FnMut(Mark)) {
    // Thin lines need long dashes and gaps, or they read as dots or as a
    // continuous line.
    let (dash_ratio, gap_ratio) = if width >= 3.0 { (2.0, 1.0) } else { (3.0, 2.0) };
    let (dash, gap) = (width * dash_ratio, width * gap_ratio);
    match fit(length, dash, gap, true) {
        None => emit(Mark::Span(0.0, length)),
        Some((dash, gap)) => emit_dashes(0.0, length, dash, gap, &mut emit),
    }
}

/// Emits the marks of a dotted side. Up to 3 px wide, dots are squares with
/// whole-pixel placement, and the ends are forced to be full dots; wider
/// sides get round dots spread evenly.
pub(super) fn dotted_marks(length: f32, width: f32, mut emit: impl FnMut(Mark)) {
    if width <= 3.0 {
        narrow_dots(length, width, &mut emit);
        return;
    }
    if length < 2.0 * width {
        emit(Mark::Dot(width / 2.0));
        return;
    }
    let gap = best_gap(length, width, width);
    // The epsilon keeps a whole dot at the far end when rounding along the
    // line would otherwise drop it.
    let period = gap + width - 0.01;
    let last = length - width / 2.0;
    if !period.is_finite() || period <= 0.0 {
        emit(Mark::Dot(width / 2.0));
        return;
    }
    let mut centre = width / 2.0;
    while centre <= last + 1e-4 {
        emit(Mark::Dot(centre));
        centre += period;
    }
}

fn narrow_dots(length: f32, width: f32, emit: &mut impl FnMut(Mark)) {
    let ends = enforce_end_dots(length.round() as i32, width.round() as i32);
    if let Some(span) = ends.start {
        emit(Mark::Span(span.0, span.1));
    }
    if let Some(span) = ends.end {
        emit(Mark::Span(span.0, span.1));
    }
    match fit(length, width, width, false) {
        None => emit(Mark::Span(ends.from, ends.to)),
        Some((dash, gap)) => emit_dashes(ends.from, ends.to, dash, gap, emit),
    }
}

fn emit_dashes(from: f32, to: f32, dash: f32, gap: f32, emit: &mut impl FnMut(Mark)) {
    let mut start = from;
    while start + 1e-3 < to {
        emit(Mark::Span(start, (start + dash).min(to)));
        start += dash + gap;
    }
}

/// Dash and gap lengths for a side `length` long: `None` when there is no room
/// for dashes and the side is solid, proportionally squeezed dashes when only
/// two fit, and otherwise the requested dash with the gap adjusted so that a
/// whole number of dashes fills the side when `adjust_gap` is set.
fn fit(length: f32, dash: f32, gap: f32, adjust_gap: bool) -> Option<(f32, f32)> {
    if length <= dash * 2.0 {
        return None;
    }
    let two_dashes_and_gap = 2.0 * dash + gap;
    if length <= two_dashes_and_gap {
        let scale = length / two_dashes_and_gap;
        return Some((dash * scale, gap * scale));
    }
    let gap = if adjust_gap {
        best_gap(length, dash, gap)
    } else {
        gap
    };
    Some((dash, gap))
}

/// The gap, closest to `gap`, that lets a whole number of dashes span `length`
/// with a dash at both ends.
fn best_gap(length: f32, dash: f32, gap: f32) -> f32 {
    let fewer = ((length + gap) / (dash + gap)).floor();
    let more = fewer + 1.0;
    let fewer_gap = (length - fewer * dash) / (fewer - 1.0);
    let more_gap = (length - more * dash) / (more - 1.0);
    if more_gap <= 0.0 || (fewer_gap - gap).abs() < (more_gap - gap).abs() {
        fewer_gap
    } else {
        more_gap
    }
}

/// Where the dashed run of a narrow dotted side starts and stops, and the
/// full dots drawn at its ends by hand.
struct EndDots {
    start: Option<(f32, f32)>,
    end: Option<(f32, f32)>,
    from: f32,
    to: f32,
}

/// Narrow dots need integral sizes and positions so that anti-aliasing does
/// not erase them; a regular pattern ends mid-dot depending on the length
/// modulo the period, so the ends are drawn explicitly and the pattern between
/// them is shifted to fit. The cases follow Blink's `EnforceDotsAtEndpoints`.
fn enforce_end_dots(length: i32, width: i32) -> EndDots {
    let mod_4 = length % 4;
    let mod_6 = length % 6;
    let mut start_dot = false;
    let mut start_growth = 0;
    let mut start_offset = 0;
    let mut end_dot = false;
    let mut end_growth = 0;
    if (width == 1 && length % 2 == 0) || (width == 3 && mod_6 == 0) {
        start_dot = true;
        start_growth = 1;
        start_offset = 1;
    }
    if (width == 2 && (mod_4 == 0 || mod_4 == 1)) || (width == 3 && (mod_6 == 1 || mod_6 == 2)) {
        start_dot = true;
        start_offset = -1;
    }
    if (width == 2 && mod_4 == 0) || (width == 3 && mod_6 == 1) {
        end_dot = true;
    }
    if (width == 2 && mod_4 == 3) || (width == 3 && (mod_6 == 4 || mod_6 == 5)) {
        start_dot = true;
        start_offset = 1;
    }
    if width == 3 && mod_6 == 5 {
        end_dot = true;
    } else if width == 3 && mod_6 == 0 {
        end_dot = true;
        end_growth = 1;
    }

    let mut ends = EndDots {
        start: None,
        end: None,
        from: 0.0,
        to: length as f32,
    };
    if start_dot {
        ends.start = Some((0.0, (width + start_growth) as f32));
        ends.from = (2 * width + start_offset) as f32;
    }
    if end_dot {
        ends.end = Some(((length - width - end_growth) as f32, length as f32));
        ends.to = (length - (width + end_growth + 1)) as f32;
    }
    ends
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dashes(length: f32, width: f32) -> Vec<(f32, f32)> {
        let mut spans = Vec::new();
        dashed_marks(length, width, |mark| {
            if let Mark::Span(a, b) = mark {
                spans.push((a, b));
            }
        });
        spans
    }

    #[test]
    fn dashes_land_on_both_ends_of_the_side() {
        for length in [40.0, 57.0, 100.0, 333.0] {
            let spans = dashes(length, 1.0);
            let first = spans.first().expect("at least one dash");
            let last = spans.last().expect("at least one dash");
            assert_eq!(first.0, 0.0, "length {length}");
            assert!((last.1 - length).abs() < 1e-2, "length {length}: {last:?}");
        }
    }

    #[test]
    fn gaps_are_adjusted_to_fit_a_whole_number_of_dashes() {
        // 1 px dashes are 3 long with a gap of 2. Twenty dashes need gaps of
        // 40 / 19 = 2.1 and twenty-one need 37 / 20 = 1.85; the first is closer.
        let spans = dashes(100.0, 1.0);
        let gaps: Vec<f32> = spans.windows(2).map(|w| w[1].0 - w[0].1).collect();
        assert!(gaps.iter().all(|g| (g - gaps[0]).abs() < 1e-3));
        assert!((gaps[0] - 2.0).abs() < 0.2, "gap {}", gaps[0]);
    }

    #[test]
    fn a_side_too_short_for_two_dashes_is_solid() {
        assert_eq!(dashes(5.0, 2.0), vec![(0.0, 5.0)]);
    }

    #[test]
    fn a_side_with_room_for_exactly_two_dashes_squeezes_them_to_fit() {
        // Width 2: dash 6, gap 4; 15 <= 2 * 6 + 4 scales both by 15/16.
        let spans = dashes(15.0, 2.0);
        assert_eq!(spans.len(), 2);
        assert!((spans[0].1 - spans[0].0 - 6.0 * 15.0 / 16.0).abs() < 1e-4);
        assert!((spans[1].1 - 15.0).abs() < 1e-3);
    }

    #[test]
    fn thick_dots_are_spread_so_the_ends_hold_whole_dots() {
        let mut dots = Vec::new();
        dotted_marks(100.0, 6.0, |mark| {
            if let Mark::Dot(c) = mark {
                dots.push(c);
            }
        });
        assert_eq!(dots.first().copied(), Some(3.0));
        let last = *dots.last().expect("dots");
        assert!((last - 97.0).abs() < 0.5, "last dot at {last}");
        let steps: Vec<f32> = dots.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(steps.iter().all(|s| (s - steps[0]).abs() < 1e-3));
    }

    #[test]
    fn narrow_dots_draw_full_dots_at_both_ends() {
        let mut spans = Vec::new();
        dotted_marks(20.0, 2.0, |mark| {
            if let Mark::Span(a, b) = mark {
                spans.push((a, b));
            }
        });
        // 20 % 4 == 0 for 2 px dots: a start dot, and an end dot ending at 20.
        assert!(spans.contains(&(0.0, 2.0)));
        assert!(spans.contains(&(18.0, 20.0)));
    }
}
