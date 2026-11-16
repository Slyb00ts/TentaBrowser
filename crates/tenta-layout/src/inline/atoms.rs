//! Shaped content cut into the units that line breaking places.
//!
//! Each chunk is shaped once as one unbroken line together with its wrap
//! opportunities. The pieces between opportunities become [`Atom`]s, with the
//! box edges and atomic inlines between them, and their widths are read off the
//! shaped line. Line breaking over atoms can then account for everything on a
//! line, which the shaper alone cannot: padding, atomic inlines and floats.
//! Nothing here copies glyphs; a line's glyph runs are built when its
//! fragments are.

use std::ops::Range;
use std::rc::Rc;

use super::atomic;
use super::bidi;
use super::style::{BoxInfo, Infos};
use super::whitespace::{Chunk, Entry, Event, Prepared};
use crate::block_sizes::intrinsic_inline_contribution;
use crate::box_tree::{BoxFlags, BoxId, BoxTree, LayoutCx};
use crate::font::FontSystem;
use crate::fragment::{FragFlags, FragId, FragmentBuilder};
use crate::space::ConstraintSpace;
use crate::text::chunk::ShapedChunk;
use crate::text::shape;
use crate::text::style::{OverflowWrap, Paragraph, TextSpan, TextStyle, WhiteSpace};
use crate::types::{Direction, Edges, Glyph, GlyphRun};
use tenta_core::NodeId;

/// Whether a line may end before an atom.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Opp {
    /// No.
    None,
    /// At a wrap opportunity.
    Soft,
    /// Inside a word, where `overflow-wrap: anywhere` allows it (and counts
    /// it for min-content). Taken only when the word overflows the line.
    Anywhere,
}

/// A short generated mark: the glyph shown where a line breaks at a soft
/// hyphen, or the ellipsis of `text-overflow`.
#[derive(Debug)]
pub(super) struct Hyphen {
    pub(super) width: f32,
    /// The glyph run, positioned from its own origin.
    pub(super) run: GlyphRun,
}

/// A stretch of one span's shaped glyphs.
#[derive(Clone, Debug)]
pub(super) struct TextAtom {
    pub(super) chunk: usize,
    /// Clusters of the shaped chunk.
    pub(super) clusters: Range<usize>,
    /// Runs of the shaped chunk that the clusters belong to.
    pub(super) runs: Range<usize>,
    /// Index into the chunk's spans.
    pub(super) span: usize,
    /// All runs are right to left.
    pub(super) rtl: bool,
    /// Some runs are right to left and some are not.
    pub(super) mixed: bool,
    /// Resolved bidirectional embedding level; zero where the chunk needs no
    /// resolution.
    pub(super) level: u8,
    /// The trailing white space that hangs is at the left of the glyphs.
    pub(super) hang_left: bool,
    /// A word of this atom may be split between clusters when it overflows
    /// (`overflow-wrap: break-word`).
    pub(super) splittable: bool,
    /// Byte range of the chunk text.
    pub(super) range: Range<usize>,
    /// Shown when a line ends after this atom, which ends in a soft hyphen.
    pub(super) hyphen: Option<Rc<Hyphen>>,
}

/// An atomic inline, laid out before it is placed.
#[derive(Clone, Debug)]
pub(super) struct AtomicAtom {
    pub(super) box_id: BoxId,
    pub(super) frag: Option<FragId>,
    /// Baseline from the border-box top; `None` puts the bottom margin edge on
    /// the line's baseline.
    pub(super) baseline: Option<f32>,
    pub(super) size: (f32, f32),
    pub(super) margin: Edges,
    /// Width of the margin box under a min-content constraint.
    pub(super) min_width: f32,
}

#[derive(Clone, Debug)]
pub(super) enum AtomKind {
    Text(TextAtom),
    Open {
        box_id: BoxId,
        resumes: bool,
    },
    Close {
        interrupted: bool,
    },
    Atomic(AtomicAtom),
    /// A preserved tab. Its width depends on where on the line it falls, so
    /// `Atom::width` holds the distance between tab stops and line layout
    /// resolves the rest.
    Tab {
        box_id: BoxId,
        node: NodeId,
        src: Range<u32>,
    },
    Forced,
    /// The place of an absolutely positioned box: no width, no effect on
    /// breaking.
    Static {
        box_id: BoxId,
    },
}

/// One unit of line breaking, in logical order.
#[derive(Clone, Debug)]
pub(super) struct Atom {
    pub(super) kind: AtomKind,
    /// Inline size including any hanging part.
    pub(super) width: f32,
    /// Trailing white space that hangs when the line ends after this atom.
    pub(super) hang: f32,
    /// Whether a line may end before this atom.
    pub(super) opp: Opp,
    /// Width of the edges that close boxes right after trailing white space
    /// that hangs. They cannot start a line, so they stay on this one and are
    /// not weighed against its width.
    pub(super) slack: f32,
}

/// What the atoms of a context are built for.
pub(super) enum Mode<'a> {
    Layout {
        space: &'a ConstraintSpace,
        out: &'a mut FragmentBuilder,
    },
    Intrinsic,
}

/// The atoms of a context and the shaped text they refer to.
pub(super) struct Atoms {
    pub(super) atoms: Vec<Atom>,
    pub(super) shaped: Vec<Rc<ShapedChunk>>,
}

/// Rounds a text width up to 1/64 px as Blink's snapped widths do. Lines are
/// broken on exact widths, which fit exactly when Blink's snapped total does.
pub(super) fn snap(width: f32) -> f32 {
    (width * 64.0).ceil() / 64.0
}

struct Sink {
    atoms: Vec<Atom>,
    pending: Opp,
}

impl Sink {
    fn push(&mut self, kind: AtomKind, width: f32, hang: f32) {
        let opp = if matches!(
            kind,
            AtomKind::Close { .. } | AtomKind::Forced | AtomKind::Static { .. }
        ) {
            Opp::None
        } else {
            std::mem::replace(&mut self.pending, Opp::None)
        };
        if matches!(kind, AtomKind::Forced) {
            self.pending = Opp::None;
        }
        self.atoms.push(Atom {
            kind,
            width,
            hang,
            opp,
            slack: 0.0,
        });
    }
}

/// The inline-start and inline-end edge sizes of an inline box.
pub(super) fn edge_sizes(info: &BoxInfo) -> (f32, f32) {
    let left = info.margin.left + info.border.left + info.padding.left;
    let right = info.margin.right + info.border.right + info.padding.right;
    match info.direction {
        Direction::Ltr => (left, right),
        Direction::Rtl => (right, left),
    }
}

fn push_event(sink: &mut Sink, infos: &Infos, event: Event) {
    match event {
        Event::Open { box_id, resumes } => {
            let width = if resumes {
                0.0
            } else {
                edge_sizes(infos.get(box_id)).0
            };
            sink.push(AtomKind::Open { box_id, resumes }, width, 0.0);
        }
        Event::Close {
            box_id,
            interrupted,
        } => {
            let width = if interrupted {
                0.0
            } else {
                edge_sizes(infos.get(box_id)).1
            };
            sink.push(AtomKind::Close { interrupted }, width, 0.0);
        }
        Event::Static { box_id } => sink.push(AtomKind::Static { box_id }, 0.0, 0.0),
    }
}

/// Shapes the chunk as a paragraph of the block's direction `base`.
fn shape_chunk(
    cx: &mut LayoutCx,
    infos: &Infos,
    chunk: &Chunk,
    base: Direction,
) -> Rc<ShapedChunk> {
    let lone: Vec<TextStyle> = chunk
        .spans
        .iter()
        .filter(|s| s.lone_space)
        .map(|s| TextStyle {
            white_space: WhiteSpace::Pre,
            ..infos.get(s.box_id).text.clone()
        })
        .collect();
    let mut lone_iter = lone.iter();
    let spans: Vec<TextSpan> = chunk
        .spans
        .iter()
        .map(|s| TextSpan {
            range: s.range.clone(),
            style: if s.lone_space {
                lone_iter.next().expect("one lone style per lone span")
            } else {
                &infos.get(s.box_id).text
            },
        })
        .collect();
    let paragraph = Paragraph {
        text: &chunk.text,
        spans: &spans,
        direction: base,
    };
    shape::shape_chunk(cx.fonts, &paragraph)
}

/// The first of `candidates` that `style`'s fonts can draw, shaped.
fn shape_mark(
    fonts: &mut FontSystem,
    style: &TextStyle,
    candidates: &[&str],
) -> Option<Rc<Hyphen>> {
    for &text in candidates {
        let spans = [TextSpan {
            range: 0..text.len(),
            style,
        }];
        let paragraph = Paragraph {
            text,
            spans: &spans,
            direction: Direction::Ltr,
        };
        let shaped = shape::shape_chunk(fonts, &paragraph);
        let [run] = shaped.runs.as_slice() else {
            continue;
        };
        if shaped.glyphs.is_empty() || shaped.glyphs.iter().any(|g| g.id == 0) {
            continue;
        }
        return Some(Rc::new(Hyphen {
            width: shaped.width,
            run: GlyphRun {
                font: run.face.font.clone(),
                size: run.face.size,
                glyphs: shaped
                    .glyphs
                    .iter()
                    .map(|g| Glyph {
                        id: g.id,
                        x: g.x,
                        y: g.y,
                    })
                    .collect(),
                synthetic_bold: run.face.synthetic_bold,
                skew: run.face.skew,
            },
        }));
    }
    None
}

/// The hyphen glyph of `style`: U+2010 when the font has it, else U+002D.
fn make_hyphen(fonts: &mut FontSystem, style: &TextStyle) -> Option<Rc<Hyphen>> {
    shape_mark(fonts, style, &["\u{2010}", "-"])
}

/// The ellipsis of `style`: U+2026 when the font has it, else three full stops.
pub(super) fn make_ellipsis(fonts: &mut FontSystem, style: &TextStyle) -> Option<Rc<Hyphen>> {
    shape_mark(fonts, style, &["\u{2026}", "..."])
}

/// Hyphen glyphs by the box whose style they use.
struct Hyphens(Vec<(BoxId, Option<Rc<Hyphen>>)>);

impl Hyphens {
    fn of(&mut self, fonts: &mut FontSystem, infos: &Infos, box_id: BoxId) -> Option<Rc<Hyphen>> {
        if let Some((_, found)) = self.0.iter().find(|(id, _)| *id == box_id) {
            return found.clone();
        }
        let made = make_hyphen(fonts, &infos.get(box_id).text);
        self.0.push((box_id, made.clone()));
        made
    }
}

/// The width of the spaces at the logical end of the cluster range, which hang
/// off a line that ends there.
fn trailing_spaces(shaped: &ShapedChunk, text: &str, clusters: &Range<usize>) -> f32 {
    if clusters.is_empty() || (shaped.any_rtl && !shaped.all_rtl) {
        return 0.0;
    }
    let is_space = |i: usize| {
        let c = &shaped.clusters[i];
        c.text.len() == 1 && text.as_bytes()[c.text.start as usize] == b' '
    };
    let width = |i: usize| shaped.x_at(i + 1) - shaped.x_at(i);
    if shaped.all_rtl {
        clusters
            .clone()
            .take_while(|&i| is_space(i))
            .map(width)
            .sum()
    } else {
        clusters
            .clone()
            .rev()
            .take_while(|&i| is_space(i))
            .map(width)
            .sum()
    }
}

/// Bidirectional facts of a chunk that needs resolution.
struct ChunkLevels {
    /// The embedding level of every shaped cluster, indexed like
    /// [`ShapedChunk::clusters`].
    levels: Vec<u8>,
    /// The shaped clusters in logical order.
    logical: Vec<u32>,
    base: Direction,
}

impl ChunkLevels {
    fn new(shaped: &ShapedChunk, base: Direction) -> Self {
        let mut logical: Vec<u32> = (0..shaped.clusters.len() as u32).collect();
        if shaped.any_rtl {
            logical.sort_by_key(|&i| shaped.clusters[i as usize].text.start);
        }
        let input: Vec<(bool, u32)> = logical
            .iter()
            .map(|&i| (shaped.runs[shaped.clusters[i as usize].run as usize].rtl, i))
            .collect();
        let mut levels = vec![0; logical.len()];
        for (&i, level) in logical.iter().zip(bidi::cluster_levels(base, &input)) {
            levels[i as usize] = level;
        }
        Self {
            levels,
            logical,
            base,
        }
    }

    /// The clusters of the byte range `start..end`, as the shaped chunk's
    /// `cluster_range` has them, found without scanning every cluster.
    fn clusters(&self, shaped: &ShapedChunk, start: usize, end: usize) -> Range<usize> {
        let visual = self.slice(shaped, start, end).iter().map(|&i| i as usize);
        match (visual.clone().min(), visual.max()) {
            (Some(first), Some(last)) => first..last + 1,
            _ => 0..0,
        }
    }

    /// The logical-order indices of the clusters that start in `start..end`.
    fn slice(&self, shaped: &ShapedChunk, start: usize, end: usize) -> &[u32] {
        let key = |i: &u32| shaped.clusters[*i as usize].text.start as usize;
        let from = self.logical.partition_point(|i| key(i) < start);
        let to = self.logical.partition_point(|i| key(i) < end);
        &self.logical[from..to]
    }

    /// The sub-ranges of `at..end` that lie at one embedding level each, with
    /// their levels. The spaces that end the piece (from `trailing`) are apart:
    /// they hang when the line ends there and then belong to no run.
    fn runs(
        &self,
        shaped: &ShapedChunk,
        at: usize,
        end: usize,
        trailing: usize,
    ) -> Vec<(Range<usize>, u8)> {
        let slice = self.slice(shaped, at, end);
        let Some(&first) = slice.first() else {
            return vec![(at..end, bidi::base_level(self.base))];
        };
        let mut runs = Vec::new();
        let mut start = at;
        let mut level = self.levels[first as usize];
        for &i in &slice[1..] {
            let here = shaped.clusters[i as usize].text.start as usize;
            if self.levels[i as usize] != level || here == trailing {
                runs.push((start..here, level));
                start = here;
                level = self.levels[i as usize];
            }
        }
        runs.push((start..end, level));
        runs
    }
}

#[allow(clippy::too_many_arguments)]
fn push_chunk(
    sink: &mut Sink,
    infos: &Infos,
    chunk: &Chunk,
    index: usize,
    shaped: &ShapedChunk,
    bidi: Option<&ChunkLevels>,
    fonts: &mut FontSystem,
    hyphens: &mut Hyphens,
) {
    let mut events = chunk.events.iter().peekable();
    let flush_events = |sink: &mut Sink,
                        events: &mut std::iter::Peekable<std::slice::Iter<'_, (usize, Event)>>,
                        upto: usize| {
        while let Some((_, event)) = events.next_if(|e| e.0 <= upto) {
            push_event(sink, infos, *event);
        }
    };
    let text_end = chunk.text.len();
    let mut span_index = 0;
    let mut start = 0;
    for piece in 0..=shaped.breaks.len() {
        let (end, boundary) = match shaped.breaks.get(piece) {
            Some(b) => (b.end as usize, Some(*b)),
            None => (text_end, None),
        };
        if end <= start {
            continue;
        }
        let first_atom = sink.atoms.len();
        let mut piece_clusters: Option<Range<usize>> = None;
        let trailing = start + chunk.text[start..end].trim_end_matches(' ').len();
        let mut at = start;
        while at < end {
            while chunk.spans[span_index].range.end <= at {
                span_index += 1;
            }
            let span = &chunk.spans[span_index];
            let group_end = end.min(span.range.end);
            // Atoms stay at one embedding level, so that each is a contiguous
            // stretch of the shaped line and line layout can move it whole.
            let parts = match bidi {
                Some(bidi) => bidi.runs(shaped, at, group_end, trailing),
                None => vec![(at..group_end, 0)],
            };
            for (part, part_level) in parts {
                let clusters = match bidi {
                    Some(bidi) => bidi.clusters(shaped, part.start, part.end),
                    None => shaped.cluster_range(part.start as u32, part.end as u32),
                };
                if clusters.is_empty() {
                    continue;
                }
                flush_events(sink, &mut events, part.start);
                piece_clusters = Some(match piece_clusters {
                    Some(seen) => seen.start.min(clusters.start)..seen.end.max(clusters.end),
                    None => clusters.clone(),
                });
                let style = &infos.get(span.box_id).text;
                let ends_in_hyphen = part.end == end
                    && boundary.is_some()
                    && chunk.text[part.clone()].ends_with('\u{ad}');
                let hyphen = if ends_in_hyphen {
                    hyphens.of(fonts, infos, span.box_id)
                } else {
                    None
                };
                // Spaces that hang off the end of a line sit on the side the
                // paragraph's direction ends at.
                let (rtl, mixed, level, hang_left) = match bidi {
                    Some(bidi) => {
                        let level = part_level;
                        let spaces = part.start >= trailing;
                        let hang_left = if spaces {
                            bidi.base == Direction::Rtl
                        } else {
                            level & 1 == 1
                        };
                        (level & 1 == 1, false, level, hang_left)
                    }
                    None => (
                        shaped.all_rtl,
                        shaped.any_rtl && !shaped.all_rtl,
                        0,
                        shaped.all_rtl,
                    ),
                };
                let atom = TextAtom {
                    chunk: index,
                    runs: shaped.runs_of(&clusters),
                    span: span_index,
                    rtl,
                    mixed,
                    level,
                    hang_left,
                    splittable: style.overflow_wrap == OverflowWrap::BreakWord
                        && style.white_space.wraps()
                        && !shaped.any_rtl,
                    range: part.clone(),
                    hyphen,
                    clusters: clusters.clone(),
                };
                let width = shaped.width_of(&clusters);
                sink.push(AtomKind::Text(atom), width, 0.0);
            }
            at = group_end;
        }
        // Spaces at the end of the piece hang off a line that ends there, from
        // the last atoms backwards.
        if let Some(piece_clusters) = piece_clusters
            && sink.atoms.len() > first_atom
        {
            let last_span = match &sink.atoms.last().map(|a| &a.kind) {
                Some(AtomKind::Text(t)) => Some(t.span),
                _ => None,
            };
            let hangs = last_span.is_some_and(|s| {
                let span = &chunk.spans[s];
                !span.lone_space && infos.get(span.box_id).text.white_space.wraps()
            });
            if hangs {
                let mut remaining = match bidi {
                    Some(bidi) if shaped.any_rtl && !shaped.all_rtl => bidi
                        .slice(shaped, trailing, end)
                        .iter()
                        .map(|&i| shaped.clusters[i as usize].advance)
                        .sum(),
                    _ => trailing_spaces(shaped, &chunk.text, &piece_clusters),
                };
                for atom in sink.atoms[first_atom..].iter_mut().rev() {
                    if remaining <= f32::EPSILON {
                        break;
                    }
                    atom.hang = remaining.min(atom.width);
                    remaining -= atom.hang;
                }
            }
        }
        sink.pending = match boundary {
            Some(b) if b.inside_word => Opp::Anywhere,
            Some(_) => Opp::Soft,
            None => Opp::None,
        };
        start = end;
    }
    flush_events(sink, &mut events, usize::MAX);
}

/// Builds the atoms for `prepared`, shaping its chunks.
pub(super) fn build(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    prepared: &Prepared,
    infos: &Infos,
    base: Direction,
    mut mode: Mode,
) -> Atoms {
    let mut sink = Sink {
        atoms: Vec::with_capacity(prepared.entries.len() * 4),
        pending: Opp::None,
    };
    let mut hyphens = Hyphens(Vec::new());
    let mut shaped_chunks: Vec<Rc<ShapedChunk>> = Vec::with_capacity(prepared.chunks.len());
    for entry in &prepared.entries {
        match entry.clone() {
            Entry::Chunk(index) => {
                let chunk = &prepared.chunks[index];
                let shaped = shape_chunk(cx, infos, chunk, base);
                // Left-to-right text in a left-to-right block needs no resolution.
                let bidi = (base == Direction::Rtl || shaped.any_rtl)
                    .then(|| ChunkLevels::new(&shaped, base));
                push_chunk(
                    &mut sink,
                    infos,
                    chunk,
                    index,
                    &shaped,
                    bidi.as_ref(),
                    cx.fonts,
                    &mut hyphens,
                );
                debug_assert_eq!(shaped_chunks.len(), index);
                shaped_chunks.push(shaped);
            }
            Entry::Event(event) => push_event(&mut sink, infos, event),
            Entry::Atomic(box_id) => {
                let wraps = infos.get(box_id).wraps;
                if wraps {
                    sink.pending = Opp::Soft;
                }
                let atom = match &mut mode {
                    Mode::Layout { space, out } => {
                        let (frag, baseline) = atomic::layout_atomic(cx, tree, box_id, space, out);
                        let placed = out.get_mut(frag);
                        if tree.is_paint_atomic(box_id) {
                            placed.flags |= FragFlags::PAINT_ATOMIC;
                        }
                        if tree.get(box_id).flags.contains(BoxFlags::ESTABLISHES_BFC) {
                            placed.flags |= FragFlags::ESTABLISHES_BFC;
                        }
                        let size = (placed.rect.w, placed.rect.h);
                        let margin = placed.margin;
                        AtomicAtom {
                            box_id,
                            frag: Some(frag),
                            baseline,
                            size,
                            margin,
                            min_width: size.0 + margin.left + margin.right,
                        }
                    }
                    Mode::Intrinsic => {
                        let (min, max) = intrinsic_inline_contribution(cx, tree, box_id);
                        let margin = infos.get(box_id).margin;
                        let extra = margin.left + margin.right;
                        AtomicAtom {
                            box_id,
                            frag: None,
                            baseline: None,
                            size: (max, 0.0),
                            margin,
                            min_width: min + extra,
                        }
                    }
                };
                let width = atom.size.0 + atom.margin.left + atom.margin.right;
                sink.push(AtomKind::Atomic(atom), width, 0.0);
                if wraps {
                    sink.pending = Opp::Soft;
                }
            }
            Entry::Tab(box_id, node, src) => {
                let info = infos.get(box_id);
                sink.push(
                    AtomKind::Tab {
                        box_id,
                        node,
                        src: src.clone(),
                    },
                    info.tab_stop,
                    0.0,
                );
                if info.wraps {
                    sink.pending = Opp::Soft;
                }
            }
            Entry::Forced => sink.push(AtomKind::Forced, 0.0, 0.0),
            Entry::Soft => sink.pending = Opp::Soft,
        }
    }
    let atoms = &mut sink.atoms;
    for i in 0..atoms.len() {
        if atoms[i].hang > 0.0 {
            let closing: f32 = atoms[i + 1..]
                .iter()
                .take_while(|a| matches!(a.kind, AtomKind::Close { .. }))
                .map(|a| a.width)
                .sum();
            atoms[i].slack = closing;
        }
    }
    Atoms {
        atoms: sink.atoms,
        shaped: shaped_chunks,
    }
}
