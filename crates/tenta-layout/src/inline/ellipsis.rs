//! `text-overflow: ellipsis`.
//!
//! A line that overflows its block container loses the content that does not
//! fit next to the ellipsis, measured from the end of the line that text runs
//! towards: whole glyph clusters of the straddling text stay, atomic inlines
//! that straddle go, and the ellipsis follows the last thing kept.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use smallvec::SmallVec;
use tenta_core::NodeId;

use super::atoms::Hyphen;
use crate::font::VerticalMetrics;
use crate::fragment::{FragFlags, FragId, Fragment, FragmentBuilder, FragmentKind, TextFragment};
use crate::types::{Direction, PaintStyle};

/// Slack for widths summed in a different order than the limit was derived.
const EPSILON: f32 = 1.0 / 1024.0;

/// Inline boxes are searched this deep for the one that straddles the limit; a
/// deeper box is dropped as a whole.
const MAX_DEPTH: usize = 256;

/// The ellipsis a block container shows.
pub(super) struct Ellipsis {
    /// The shaped U+2026, or three full stops when the font has no ellipsis.
    pub(super) mark: Rc<Hyphen>,
    /// Direction of the block container: the ellipsis goes to its end side.
    pub(super) direction: Direction,
    /// Vertical metrics of the block's primary font.
    pub(super) metrics: VerticalMetrics,
    /// Paint style of the container's text.
    pub(super) paint: Arc<PaintStyle>,
}

/// A cluster of a text fragment: the glyphs of one cluster are kept or dropped
/// together, so a base letter never loses its combining mark.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Cut {
    /// Where the cluster starts and ends, from the fragment's glyph origin.
    pub(super) start: f32,
    pub(super) end: f32,
    /// How many of the fragment's glyphs the clusters up to this one hold.
    pub(super) glyphs: usize,
}

/// The clusters of the text fragments of a line.
type Cuts = HashMap<FragId, Vec<Cut>>;

/// What survives of a run of sibling fragments.
struct Kept {
    children: SmallVec<[FragId; 8]>,
    /// Distance from the start edge to the end of the last kept fragment.
    end: f32,
}

/// Truncates `line`, whose content `available` is too narrow for, so that
/// the ellipsis fits, with the ellipsis on the line's baseline `baseline`.
pub(super) fn truncate_line(
    ellipsis: &Ellipsis,
    line: FragId,
    baseline: f32,
    available: f32,
    cuts: &Cuts,
    out: &mut FragmentBuilder,
) {
    let rtl = ellipsis.direction == Direction::Rtl;
    let limit = (available - ellipsis.mark.width).max(0.0);
    let children: SmallVec<[FragId; 8]> = out.get(line).children.iter().copied().collect();
    let mut kept = keep_within(out, cuts, &children, available, limit, rtl, 0);

    let x = if rtl {
        available - kept.end - ellipsis.mark.width
    } else {
        kept.end
    };
    let height = ellipsis.metrics.ascent + ellipsis.metrics.descent;
    let rect = crate::types::Rect::new(
        x,
        baseline - ellipsis.metrics.ascent,
        ellipsis.mark.width,
        height,
    );
    let fragment = Fragment::new(
        NodeId::NONE,
        FragmentKind::Text(TextFragment {
            text_range: 0..0,
            runs: SmallVec::from_elem(Arc::new(ellipsis.mark.run.clone()), 1),
            baseline: ellipsis.metrics.ascent,
        }),
        ellipsis.paint.clone(),
        rect,
    );
    let id = out.push(fragment);
    if rtl {
        kept.children.insert(0, id);
    } else {
        kept.children.push(id);
    }
    let kept_children: SmallVec<[FragId; 4]> = kept.children.iter().copied().collect();
    out.set_children(line, kept_children);
}

/// Keeps the fragments of `children` (left to right, in a container `size`
/// wide) that end within `limit` of the start edge, cutting the one that
/// straddles it when it is text or an inline box.
fn keep_within(
    out: &mut FragmentBuilder,
    cuts: &Cuts,
    children: &[FragId],
    size: f32,
    limit: f32,
    rtl: bool,
    depth: usize,
) -> Kept {
    let mut kept = Kept {
        children: SmallVec::new(),
        end: 0.0,
    };
    let ordered: SmallVec<[FragId; 8]> = if rtl {
        children.iter().rev().copied().collect()
    } else {
        children.iter().copied().collect()
    };
    for child in ordered {
        let rect = out.get(child).rect;
        let (start, length) = if rtl {
            (size - rect.x - rect.w, rect.w)
        } else {
            (rect.x, rect.w)
        };
        if start + length <= limit + EPSILON {
            kept.children.push(child);
            kept.end = kept.end.max(start + length);
            continue;
        }
        if start < limit - EPSILON
            && let Some(cut) = cut_straddling(out, cuts, child, limit - start, rtl, depth)
        {
            kept.children.push(child);
            kept.end = kept.end.max(start + cut);
        }
        break;
    }
    if rtl {
        kept.children.reverse();
    }
    kept
}

/// Cuts the fragment `frag` so that at most `room` of it remains at its start
/// side, and returns its new length, or `None` when nothing of it stays.
fn cut_straddling(
    out: &mut FragmentBuilder,
    cuts: &Cuts,
    frag: FragId,
    room: f32,
    rtl: bool,
    depth: usize,
) -> Option<f32> {
    let f = out.get(frag);
    match &f.kind {
        FragmentKind::Text(_) => trim_text(out.get_mut(frag), cuts.get(&frag)?, room, rtl),
        FragmentKind::Box if !f.flags.contains(FragFlags::PAINT_ATOMIC) && depth < MAX_DEPTH => {
            let (size, children) = (f.rect.w, f.children.clone());
            let kept = keep_within(out, cuts, &children, size, room, rtl, depth + 1);
            if kept.children.is_empty() {
                return None;
            }
            let shrink = size - kept.end;
            let fragment = out.get_mut(frag);
            fragment.rect.w = kept.end;
            if rtl {
                fragment.rect.x += shrink;
            }
            let moved: SmallVec<[FragId; 4]> = kept.children.iter().copied().collect();
            out.set_children(frag, moved);
            if rtl {
                for &child in &kept.children {
                    out.get_mut(child).rect.x -= shrink;
                }
            }
            Some(kept.end)
        }
        _ => None,
    }
}

/// Keeps the clusters of a text fragment that fit in `room` from its start
/// side and returns the new width, or `None` when not even one fits.
fn trim_text(frag: &mut Fragment, clusters: &[Cut], room: f32, rtl: bool) -> Option<f32> {
    let FragmentKind::Text(text) = &mut frag.kind else {
        return None;
    };
    let width = clusters.last()?.end;
    let (first, last) = if rtl {
        let count = clusters
            .iter()
            .rev()
            .take_while(|c| width - c.start <= room + EPSILON)
            .count();
        (clusters.len() - count, clusters.len())
    } else {
        let count = clusters
            .iter()
            .take_while(|c| c.end <= room + EPSILON)
            .count();
        (0, count)
    };
    if first == last {
        return None;
    }
    let glyphs = first.checked_sub(1).map_or(0, |i| clusters[i].glyphs)..clusters[last - 1].glyphs;
    let shift = clusters[first].start;
    let new_width = clusters[last - 1].end - shift;

    let mut index = 0;
    let mut runs = SmallVec::new();
    for run in &text.runs {
        let from = index;
        index += run.glyphs.len();
        let kept = glyphs.start.saturating_sub(from).min(run.glyphs.len())
            ..glyphs.end.saturating_sub(from).min(run.glyphs.len());
        if kept.is_empty() {
            continue;
        }
        let mut trimmed = (**run).clone();
        trimmed.glyphs = run.glyphs[kept]
            .iter()
            .map(|g| crate::types::Glyph {
                x: g.x - shift,
                ..*g
            })
            .collect();
        runs.push(Arc::new(trimmed));
    }
    text.runs = runs;
    frag.rect.x += shift;
    frag.rect.w = new_width;
    Some(new_width)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FontBlob, FontRef, Glyph, GlyphRun, Rect};

    /// Clusters ten pixels wide; the glyph counts say how many glyphs each
    /// holds (two for a letter with a combining mark).
    fn clusters(glyph_counts: &[usize]) -> Vec<Cut> {
        let mut total = 0;
        glyph_counts
            .iter()
            .enumerate()
            .map(|(i, n)| {
                total += n;
                Cut {
                    start: i as f32 * 10.0,
                    end: (i + 1) as f32 * 10.0,
                    glyphs: total,
                }
            })
            .collect()
    }

    /// A text fragment at x = 5 whose glyphs sit at the starts of `cuts`
    /// clusters (marks share their base's position).
    fn text(cuts: &[Cut]) -> Fragment {
        let glyphs = cuts
            .iter()
            .enumerate()
            .flat_map(|(i, c)| {
                let before = i.checked_sub(1).map_or(0, |p| cuts[p].glyphs);
                (before..c.glyphs).map(move |_| Glyph {
                    id: 1,
                    x: c.start,
                    y: 0.0,
                })
            })
            .collect();
        let run = GlyphRun {
            font: FontRef {
                data: Arc::new(FontBlob::Static(&[])),
                index: 0,
                coords: Arc::from([]),
            },
            size: 16.0,
            glyphs,
            synthetic_bold: false,
            skew: 0.0,
        };
        Fragment::new(
            NodeId::NONE,
            FragmentKind::Text(TextFragment {
                text_range: 0..cuts.len() as u32,
                runs: SmallVec::from_elem(Arc::new(run), 1),
                baseline: 12.0,
            }),
            Arc::new(PaintStyle::default()),
            Rect::new(5.0, 0.0, cuts.last().map_or(0.0, |c| c.end), 16.0),
        )
    }

    fn glyph_xs(fragment: &Fragment) -> Vec<f32> {
        match &fragment.kind {
            FragmentKind::Text(t) => t
                .runs
                .iter()
                .flat_map(|r| r.glyphs.iter().map(|g| g.x))
                .collect(),
            _ => unreachable!("a text fragment"),
        }
    }

    #[test]
    fn left_to_right_text_keeps_the_clusters_that_fit_whole() {
        let cuts = clusters(&[1, 1, 1, 1]);
        let mut fragment = text(&cuts);
        assert_eq!(trim_text(&mut fragment, &cuts, 25.0, false), Some(20.0));
        assert_eq!(glyph_xs(&fragment), [0.0, 10.0]);
        assert_eq!(fragment.rect, Rect::new(5.0, 0.0, 20.0, 16.0));
    }

    #[test]
    fn a_base_letter_and_its_mark_stay_or_go_together() {
        let cuts = clusters(&[2, 2, 2]);
        let mut fragment = text(&cuts);
        assert_eq!(trim_text(&mut fragment, &cuts, 25.0, false), Some(20.0));
        assert_eq!(glyph_xs(&fragment), [0.0, 0.0, 10.0, 10.0]);
    }

    #[test]
    fn text_that_does_not_fit_one_cluster_is_dropped() {
        let cuts = clusters(&[1, 1]);
        let mut fragment = text(&cuts);
        assert_eq!(trim_text(&mut fragment, &cuts, 9.0, false), None);
    }

    #[test]
    fn right_to_left_text_keeps_the_clusters_at_its_start_side() {
        let cuts = clusters(&[1, 2, 1, 1]);
        let mut fragment = text(&cuts);
        assert_eq!(trim_text(&mut fragment, &cuts, 25.0, true), Some(20.0));
        assert_eq!(glyph_xs(&fragment), [0.0, 10.0]);
        // The fragment shrinks from its far side: the kept glyphs keep their
        // place on the page.
        assert_eq!(fragment.rect, Rect::new(25.0, 0.0, 20.0, 16.0));
    }

    #[test]
    fn text_that_fits_exactly_stays_whole() {
        let cuts = clusters(&[1, 1]);
        let mut fragment = text(&cuts);
        assert_eq!(trim_text(&mut fragment, &cuts, 20.0, false), Some(20.0));
        assert_eq!(glyph_xs(&fragment), [0.0, 10.0]);
    }
}
