//! Line breaking and line-box construction.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use smallvec::SmallVec;
use tenta_core::NodeId;

use super::atoms::{self, Atom, AtomKind, Atoms, Mode, Opp, TextAtom};
use super::ellipsis::{self, Cut, Ellipsis};
use super::style::{Align, BlockInfo, BoxInfo, Handling, Infos, LineHeight, leading};
use super::valign::{self, LineBox, Member};
use super::whitespace::{self, Prepared};
use super::{align, bidi};
use crate::box_tree::{BoxId, BoxTree, InlineFormattingContext, InlineItem, LayoutCx};
use crate::float;
use crate::font::VerticalMetrics;
use crate::fragment::{
    FragId, Fragment, FragmentBuilder, FragmentKind, InlineStatic, TextFragment,
};
use crate::relative;
use crate::space::{AvailableSize, ConstraintSpace, FloatContext, InlineResult};
use crate::text::chunk::ShapedChunk;
use crate::types::{
    ClearSide, Direction, Edges, FloatSide, Glyph, GlyphRun, OverflowKind, PaintStyle,
    PositionKind, Rect,
};

/// Slack for the comparison of widths that were summed in a different order
/// than the available size was derived.
const EPSILON: f32 = 1.0 / 1024.0;

/// Where a line ends and how wide its content is.
struct Fit {
    end: usize,
    /// The line ends in a forced break.
    forced: bool,
    /// The line ends at a soft hyphen, which is shown.
    hyphen: bool,
    /// The first unbreakable piece is wider than the room it was given.
    wide: bool,
}

/// The distance from `x` to the next tab stop, `stop` apart. A tab at a stop
/// advances to the following one.
fn tab_advance(stop: f32, x: f32) -> f32 {
    if stop <= 0.0 {
        return 0.0;
    }
    let rest = x.rem_euclid(stop);
    if stop - rest < EPSILON {
        stop
    } else {
        stop - rest
    }
}

/// The inline size of `atom` when it starts `x` from the start edge of the
/// line.
fn width_at(atom: &Atom, x: f32) -> f32 {
    match atom.kind {
        AtomKind::Tab { .. } => tab_advance(atom.width, x),
        _ => atom.width,
    }
}

/// Splits the text atom `index` before its cluster `at`. The first part keeps
/// the atom's place; the second becomes a new atom that a line may start with.
fn split_atom(atoms: &mut Vec<Atom>, shaped: &[Rc<ShapedChunk>], index: usize, at: usize) {
    let AtomKind::Text(text) = &atoms[index].kind else {
        return;
    };
    let sh = &shaped[text.chunk];
    let boundary = sh.clusters[at].text.start as usize;
    let mut left = text.clone();
    left.clusters = text.clusters.start..at;
    left.runs = sh.runs_of(&left.clusters);
    left.range = text.range.start..boundary;
    left.hyphen = None;
    let mut right = text.clone();
    right.clusters = at..text.clusters.end;
    right.runs = sh.runs_of(&right.clusters);
    right.range = boundary..text.range.end;
    let hang = atoms[index].hang;
    let slack = atoms[index].slack;
    let (left_width, right_width) = (sh.width_of(&left.clusters), sh.width_of(&right.clusters));
    atoms[index] = Atom {
        kind: AtomKind::Text(left),
        width: left_width,
        hang: 0.0,
        opp: atoms[index].opp,
        slack: 0.0,
    };
    atoms.insert(
        index + 1,
        Atom {
            kind: AtomKind::Text(right),
            width: right_width,
            hang,
            opp: Opp::Anywhere,
            slack,
        },
    );
}

/// The hyphen that a line ending before `end` shows: the last text atom of the
/// line ends in a soft hyphen.
fn hyphen_at(atoms: &[Atom], start: usize, end: usize) -> Option<&Rc<atoms::Hyphen>> {
    if end >= atoms.len() {
        return None;
    }
    atoms[start..end]
        .iter()
        .rev()
        .find(|a| !matches!(a.kind, AtomKind::Close { .. }))
        .and_then(|a| match &a.kind {
            AtomKind::Text(t) => t.hyphen.as_ref(),
            _ => None,
        })
}

/// The first cluster of the splittable atom `index` that does not fit in
/// `room`, or the one after the first when none does and `at_least_one`.
fn cluster_cut(
    atoms: &[Atom],
    shaped: &[Rc<ShapedChunk>],
    index: usize,
    room: f32,
    at_least_one: bool,
) -> Option<usize> {
    let AtomKind::Text(text) = &atoms[index].kind else {
        return None;
    };
    if !text.splittable {
        return None;
    }
    let sh = &shaped[text.chunk];
    let origin = sh.x_at(text.clusters.start);
    let count = sh.clusters[text.clusters.clone()]
        .partition_point(|c| c.x + c.advance - origin <= room + EPSILON);
    let mut cut = text.clusters.start + count;
    if cut == text.clusters.start && at_least_one {
        cut += 1;
    }
    (cut > text.clusters.start && cut < text.clusters.end).then_some(cut)
}

/// Greedy line breaking: takes chunks (runs of atoms with no wrap
/// opportunity inside) while they fit. `indent` is the space the line starts
/// with, which tab stops count from the start edge.
///
/// A word that overflows an otherwise empty line is cut where
/// `overflow-wrap` allows, splitting its atom at a cluster when needed; the
/// scan stops at the first atom that does not fit, so a long unbroken string
/// costs work in proportion to its length overall.
fn fit_line(
    atoms: &mut Vec<Atom>,
    shaped: &[Rc<ShapedChunk>],
    start: usize,
    avail: f32,
    indent: f32,
) -> Fit {
    let mut used = 0.0_f32;
    let mut committed = start;
    let mut previous = start;
    let mut used_before_last = 0.0_f32;
    let mut last_hang = 0.0_f32;
    let mut pending = 0.0_f32;
    let mut pending_hang = 0.0_f32;
    let mut cut: Option<usize> = None;
    let mut overflowing = false;
    let mut wide = false;
    let mut k = start;
    while k < atoms.len() {
        if matches!(atoms[k].kind, AtomKind::Forced) {
            let mut end = k + 1;
            while end < atoms.len() && matches!(atoms[end].kind, AtomKind::Close { .. }) {
                end += 1;
            }
            return Fit {
                end,
                forced: true,
                hyphen: false,
                wide,
            };
        }
        if k > start && atoms[k].opp == Opp::Soft {
            previous = committed;
            used_before_last = used;
            last_hang = pending_hang;
            used += pending;
            committed = k;
            pending = 0.0;
            pending_hang = 0.0;
            cut = None;
            overflowing = false;
        }
        let width = width_at(&atoms[k], indent + used + pending);
        let hang = atoms[k].hang + atoms[k].slack;
        if k > start
            && atoms[k].opp == Opp::Anywhere
            && used + pending - pending_hang <= avail + EPSILON
        {
            cut = Some(k);
        }
        let reaches = used + pending + width - (pending_hang + hang);
        if reaches > avail + EPSILON && !overflowing {
            if committed > start {
                let mut end = committed;
                let mut hyphen = hyphen_at(atoms, start, end).cloned();
                if let Some(h) = &hyphen
                    && used - last_hang + h.width > avail + EPSILON
                    && previous > start
                {
                    end = previous;
                    let _ = used_before_last;
                    hyphen = hyphen_at(atoms, start, end).cloned();
                }
                return Fit {
                    end,
                    forced: false,
                    hyphen: hyphen.is_some(),
                    wide,
                };
            }
            let room = avail - pending;
            if let Some(at) = cluster_cut(atoms, shaped, k, room, k == start) {
                split_atom(atoms, shaped, k, at);
                return Fit {
                    end: k + 1,
                    forced: false,
                    hyphen: false,
                    wide,
                };
            }
            let breakable = matches!(&atoms[k].kind, AtomKind::Text(t) if t.splittable);
            let mut before = if breakable && k > start { Some(k) } else { cut };
            while let Some(b) = before
                && b > start + 1
                && matches!(atoms[b - 1].kind, AtomKind::Open { .. })
            {
                before = Some(b - 1);
            }
            if let Some(b) = before.filter(|&b| b > start) {
                return Fit {
                    end: b,
                    forced: false,
                    hyphen: false,
                    wide,
                };
            }
            overflowing = true;
            wide = true;
        }
        pending += width;
        pending_hang += hang;
        k += 1;
    }
    Fit {
        end: atoms.len(),
        forced: false,
        hyphen: false,
        wide,
    }
}

/// An inline box piece on the line being built.
#[derive(Clone)]
struct Frame {
    box_id: BoxId,
    has_start: bool,
    has_end: bool,
    has_text: bool,
    /// Metrics of fallback fonts whose glyphs sit under this box, which
    /// `line-height: normal` accounts for.
    fallback: SmallVec<[VerticalMetrics; 1]>,
}

#[derive(Copy, Clone, Debug)]
enum Tok {
    Open(usize),
    Close(usize),
    Text(usize),
    Atomic(usize),
    Tab(usize),
    Static(usize),
}

/// What the line is made of, left to right.
enum Vis {
    Open(usize),
    Close(usize),
    /// A run of text atoms of one node, as indices into the order list.
    Text(Range<usize>),
    Atomic(usize),
    Tab(usize),
    Static(usize),
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum LineDir {
    Ltr,
    Rtl,
    Mixed,
}

/// Everything line assembly reads.
struct Ctx<'a> {
    tree: &'a BoxTree,
    infos: &'a Infos,
    prepared: &'a Prepared,
    atoms: &'a Atoms,
    block: &'a BlockInfo,
    root: BoxId,
    quirks: bool,
    line_paint: Arc<PaintStyle>,
    ellipsis: Option<&'a Ellipsis>,
    /// Content height of the block container when it is definite: percentage
    /// insets of relatively positioned inline boxes resolve against it.
    containing_height: Option<f32>,
}

/// The geometry a line is built in.
struct LineIn {
    range: Range<usize>,
    y: f32,
    /// Left edge of the line, after floats.
    left: f32,
    avail: f32,
    indent: f32,
    align: Align,
    /// The line ends at a soft hyphen, which is shown.
    hyphen: bool,
}

/// A built line.
struct Line {
    frag: FragId,
    height: f32,
    ascent: f32,
    content: f32,
    /// Bounds of everything on the line, in the line's coordinates.
    extent: Rect,
    /// Absolutely positioned boxes whose placeholders sit on the line, with
    /// the inline offset of each from the line's left edge and whether content
    /// precedes it on the line.
    statics: Vec<(BoxId, f32, bool, bool)>,
}

/// The right edges, from the atom's left edge, of the separators of `text`
/// that justification widens.
fn collect_separators(ctx: &Ctx, text: &TextAtom, hang: f32) -> Vec<f32> {
    let shaped = &ctx.atoms.shaped[text.chunk];
    let chunk = &ctx.prepared.chunks[text.chunk];
    let origin = shaped.x_at(text.clusters.start);
    let total = shaped.width_of(&text.clusters);
    let mut edges = Vec::new();
    for cluster in &shaped.clusters[text.clusters.clone()] {
        let spaces = chunk
            .text
            .get(cluster.text.start as usize..cluster.text.end as usize)
            .is_some_and(|t| !t.is_empty() && t.chars().all(|c| c == ' ' || c == '\u{a0}'));
        if !spaces {
            continue;
        }
        let from = cluster.x - origin;
        let hanging = hang > 0.0
            && if text.hang_left {
                from + cluster.advance <= hang + EPSILON
            } else {
                from + EPSILON >= total - hang
            };
        if !hanging {
            edges.push(from + cluster.advance);
        }
    }
    edges
}

/// The glyph runs of the atoms of one leaf, positioned from the leaf's left
/// edge. `edges` holds, per atom, the right edges of the separators that
/// justification widens by `gap` each; `hyphen` is shown after the last atom.
fn leaf_runs(
    ctx: &Ctx,
    leaf: &[usize],
    edges: &[&[f32]],
    gap: f32,
    hyphen: Option<&atoms::Hyphen>,
) -> SmallVec<[Arc<GlyphRun>; 1]> {
    let mut built: Vec<GlyphRun> = Vec::new();
    let mut origin = 0.0_f32;
    let extend = |built: &mut Vec<GlyphRun>,
                  face: &crate::text::chunk::Face,
                  glyphs: &mut dyn Iterator<Item = Glyph>| {
        match built.last_mut() {
            Some(last)
                if last.font == face.font
                    && last.size == face.size
                    && last.synthetic_bold == face.synthetic_bold
                    && last.skew == face.skew =>
            {
                last.glyphs.extend(glyphs);
            }
            _ => built.push(GlyphRun {
                font: face.font.clone(),
                size: face.size,
                glyphs: glyphs.collect(),
                synthetic_bold: face.synthetic_bold,
                skew: face.skew,
            }),
        }
    };
    for (position, &index) in leaf.iter().enumerate() {
        let atom = &ctx.atoms.atoms[index];
        let AtomKind::Text(text) = &atom.kind else {
            continue;
        };
        let shaped = &ctx.atoms.shaped[text.chunk];
        let atom_edges = edges.get(position).copied().unwrap_or(&[]);
        let x0 = shaped.x_at(text.clusters.start);
        let mut at = text.clusters.start;
        while at < text.clusters.end {
            let run = shaped.clusters[at].run;
            let mut end = at + 1;
            while end < text.clusters.end && shaped.clusters[end].run == run {
                end += 1;
            }
            let glyphs = shaped.clusters[at].glyphs.start as usize
                ..shaped.clusters[end - 1].glyphs.end as usize;
            let mut shifted = shaped.glyphs[glyphs].iter().map(|g| {
                let x = g.x - x0;
                let passed = atom_edges.partition_point(|e| *e <= x + EPSILON);
                Glyph {
                    id: g.id,
                    x: origin + x + passed as f32 * gap,
                    y: g.y,
                }
            });
            extend(&mut built, &shaped.runs[run as usize].face, &mut shifted);
            at = end;
        }
        origin += atom.width + gap * atom_edges.len() as f32;
    }
    if let Some(h) = hyphen {
        let mut glyphs = h.run.glyphs.iter().map(|g| Glyph {
            id: g.id,
            x: origin + g.x,
            y: g.y,
        });
        let face = crate::text::chunk::Face {
            font: h.run.font.clone(),
            size: h.run.size,
            synthetic_bold: h.run.synthetic_bold,
            skew: h.run.skew,
        };
        extend(&mut built, &face, &mut glyphs);
    }
    built.into_iter().map(Arc::new).collect()
}

/// Where the clusters of a leaf end, from the leaf's left edge, and how many of
/// its glyphs they hold: the places `text-overflow` may cut it.
///
/// A glyph that stands for several characters (a ligature, a letter with its
/// combining mark) comes with one cluster per character, of which only one
/// holds the glyph. Those clusters are one cut: a character never loses the
/// rest of its glyph.
fn leaf_cuts(ctx: &Ctx, leaf: &[usize]) -> Vec<Cut> {
    let mut cuts: Vec<Cut> = Vec::new();
    let mut origin = 0.0_f32;
    let mut glyphs = 0;
    // Where the cut that the next glyph-holding cluster ends begins, when the
    // clusters that belong to it came first (right-to-left text).
    let mut joined_start: Option<f32> = None;
    for &index in leaf {
        let atom = &ctx.atoms.atoms[index];
        let AtomKind::Text(text) = &atom.kind else {
            continue;
        };
        let shaped = &ctx.atoms.shaped[text.chunk];
        let x0 = shaped.x_at(text.clusters.start);
        let mut previous_end = None;
        for position in text.clusters.clone() {
            let cluster = &shaped.clusters[position];
            // The shaper splits the advance of a multi-character glyph evenly
            // between its clusters, so each ends where the next one starts.
            let start = origin + cluster.x - x0;
            let end = origin + shaped.x_at(position + 1) - x0;
            if cluster.glyphs.is_empty() {
                match cuts.last_mut() {
                    Some(last) if previous_end == Some(cluster.text.start) => last.end = end,
                    _ => joined_start = joined_start.or(Some(start)),
                }
            } else {
                glyphs += cluster.glyphs.len();
                cuts.push(Cut {
                    start: joined_start.take().unwrap_or(start),
                    end,
                    glyphs,
                });
            }
            previous_end = Some(cluster.text.end);
        }
        origin += atom.width;
    }
    cuts
}

/// The source bytes the atoms of a leaf cover.
fn source_range(ctx: &Ctx, leaf: &[usize]) -> Range<u32> {
    let mut start = u32::MAX;
    let mut end = 0;
    for &index in leaf {
        let AtomKind::Text(text) = &ctx.atoms.atoms[index].kind else {
            continue;
        };
        let span = &ctx.prepared.chunks[text.chunk].spans[text.span];
        let local = text.range.start - span.range.start..text.range.end - span.range.start;
        start = start.min(span.map.start(local.start));
        end = end.max(span.map.end(local.end));
    }
    start..end.max(start)
}

/// Physical padding, border and margin of one piece of an inline box: the
/// sides that carry the box's start and end edge only on the first and last
/// piece.
fn piece_edges(info: &BoxInfo, frame: &Frame) -> (Edges, Edges, Edges) {
    let (left_present, right_present) = match info.direction {
        Direction::Ltr => (frame.has_start, frame.has_end),
        Direction::Rtl => (frame.has_end, frame.has_start),
    };
    let side = |e: &Edges| Edges {
        top: e.top,
        right: if right_present { e.right } else { 0.0 },
        bottom: e.bottom,
        left: if left_present { e.left } else { 0.0 },
    };
    let mut margin = side(&info.margin);
    margin.top = 0.0;
    margin.bottom = 0.0;
    (side(&info.padding), side(&info.border), margin)
}

/// The levels of the text just before and just after the atoms `line` of the
/// paragraph, which decide where an atomic inline at the edge of a line goes:
/// it belongs to the paragraph, not to its line.
fn outside_levels(atoms: &[Atom], line: &Range<usize>) -> (Option<u8>, Option<u8>) {
    let level_of = |atom: &Atom| match &atom.kind {
        AtomKind::Text(t) => Some(Some(t.level)),
        AtomKind::Forced | AtomKind::Tab { .. } => Some(None),
        _ => None,
    };
    let before = atoms[..line.start]
        .iter()
        .rev()
        .find_map(level_of)
        .flatten();
    let after = atoms[line.end..].iter().find_map(level_of).flatten();
    (before, after)
}

/// What a line is made of, apart from the boxes around it.
struct Leaf {
    /// `None` for an inline box with nothing inside, which still has edges.
    token: Option<Tok>,
    /// The innermost box around the leaf.
    inner: Option<usize>,
    level: u8,
}

/// Puts the tokens of a line, given in logical order, in visual order (UAX #9
/// rule L2). Boxes whose content ends up in several places are split into one
/// piece per place; the start edge goes to the piece that holds the first
/// content in logical order and the end edge to the one that holds the last.
fn visual_tokens(
    ctx: &Ctx,
    frames: &mut Vec<Frame>,
    tokens: &[Tok],
    line: &Range<usize>,
) -> Vec<Tok> {
    let atoms = &ctx.atoms.atoms;
    let mut parent: Vec<Option<usize>> = vec![None; frames.len()];
    let mut first = vec![0_usize; frames.len()];
    let mut last = vec![0_usize; frames.len()];
    let mut leaves: Vec<Leaf> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for &token in tokens {
        match token {
            Tok::Open(f) => {
                parent[f] = stack.last().copied();
                first[f] = leaves.len();
                stack.push(f);
            }
            Tok::Close(f) => {
                stack.pop();
                if leaves.len() == first[f] {
                    leaves.push(Leaf {
                        token: None,
                        inner: Some(f),
                        level: 0,
                    });
                }
                last[f] = leaves.len() - 1;
            }
            content => {
                let level = match content {
                    Tok::Text(i) => match &atoms[i].kind {
                        AtomKind::Text(t) => t.level,
                        _ => 0,
                    },
                    _ => 0,
                };
                leaves.push(Leaf {
                    token: Some(content),
                    inner: stack.last().copied(),
                    level,
                });
            }
        }
    }

    // Objects take their level from the text around them.
    let is_text = |leaf: &Leaf| matches!(leaf.token, Some(Tok::Text(_)));
    let (outside_before, outside_after) = outside_levels(&ctx.atoms.atoms, line);
    let mut before: Vec<Option<u8>> = Vec::with_capacity(leaves.len());
    let mut seen = outside_before;
    for leaf in &leaves {
        before.push(seen);
        if is_text(leaf) {
            seen = Some(leaf.level);
        }
    }
    let mut seen = outside_after;
    for (i, leaf) in leaves.iter_mut().enumerate().rev() {
        if is_text(leaf) {
            seen = Some(leaf.level);
        } else if matches!(leaf.token, Some(Tok::Tab(_))) {
            leaf.level = bidi::base_level(ctx.block.direction);
        } else {
            leaf.level = bidi::object_level(ctx.block.direction, before[i], seen);
        }
    }

    // Spaces that hang off the end of the line take the paragraph's level
    // (rule L1).
    for leaf in leaves.iter_mut().rev() {
        match leaf.token {
            Some(Tok::Text(i))
                if atoms[i].hang > 0.0 && atoms[i].hang + EPSILON >= atoms[i].width =>
            {
                leaf.level = bidi::base_level(ctx.block.direction);
            }
            None => {}
            Some(_) => break,
        }
    }

    let levels: Vec<u8> = leaves.iter().map(|leaf| leaf.level).collect();
    let mut visual: Vec<Tok> = Vec::with_capacity(tokens.len());
    let mut open: Vec<(usize, usize)> = Vec::new();
    let mut chain: Vec<usize> = Vec::new();
    for index in bidi::reorder_line(&levels) {
        let leaf = &leaves[index];
        chain.clear();
        let mut around = leaf.inner;
        while let Some(f) = around {
            chain.push(f);
            around = parent[f];
        }
        chain.reverse();
        let common = open
            .iter()
            .zip(&chain)
            .take_while(|((f, _), c)| f == *c)
            .count();
        while open.len() > common {
            if let Some((_, piece)) = open.pop() {
                visual.push(Tok::Close(piece));
            }
        }
        for &f in &chain[common..] {
            let mut piece = frames[f].clone();
            piece.has_start = false;
            piece.has_end = false;
            frames.push(piece);
            visual.push(Tok::Open(frames.len() - 1));
            open.push((f, frames.len() - 1));
        }
        for &(f, piece) in &open {
            if first[f] == index {
                frames[piece].has_start = frames[f].has_start;
            }
            if last[f] == index {
                frames[piece].has_end = frames[f].has_end;
            }
        }
        if let Some(token) = leaf.token {
            visual.push(token);
        }
    }
    while let Some((_, piece)) = open.pop() {
        visual.push(Tok::Close(piece));
    }
    visual
}

struct Open {
    border_x: f32,
    children: SmallVec<[FragId; 4]>,
}

fn assemble(ctx: &Ctx, out: &mut FragmentBuilder, carry: &mut Vec<BoxId>, input: &LineIn) -> Line {
    let atoms = &ctx.atoms.atoms;
    let infos = ctx.infos;

    // Tokens in logical order, with the inline box pieces they open and close.
    let mut frames: Vec<Frame> = Vec::new();
    let mut tokens: Vec<Tok> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for &box_id in carry.iter() {
        frames.push(Frame {
            box_id,
            has_start: false,
            has_end: false,
            has_text: false,
            fallback: SmallVec::new(),
        });
        tokens.push(Tok::Open(frames.len() - 1));
        stack.push(frames.len() - 1);
    }
    let mut root_fallback: SmallVec<[VerticalMetrics; 1]> = SmallVec::new();
    let mut any_rtl = false;
    let mut any_ltr = false;
    let mut any_mixed = false;
    let mut has_object = false;
    let mut has_atomic = false;
    // White space that hangs off the end of the line is not content.
    let hanging = atoms[input.range.clone()]
        .iter()
        .rev()
        .take_while(|a| match a.kind {
            AtomKind::Open { .. } | AtomKind::Close { .. } | AtomKind::Static { .. } => true,
            AtomKind::Text(_) => a.hang > 0.0 && a.width - a.hang <= EPSILON,
            _ => false,
        })
        .count();
    let mut has_text = false;
    let mut needs_valign = carry.iter().any(|&b| !infos.get(b).baseline_aligned);
    // Placeholders that follow content in logical order, whatever the order
    // the line ends up displayed in.
    let mut after_content: HashSet<usize> = HashSet::new();
    let mut content_before = false;
    for index in input.range.clone() {
        content_before |= match &atoms[index].kind {
            AtomKind::Text(_) | AtomKind::Atomic(_) | AtomKind::Tab { .. } => true,
            AtomKind::Open { .. } | AtomKind::Close { .. } => atoms[index].width > 0.0,
            AtomKind::Static { .. } => {
                if content_before {
                    after_content.insert(index);
                }
                false
            }
            AtomKind::Forced => false,
        };
        match &atoms[index].kind {
            AtomKind::Open { box_id, resumes } => {
                frames.push(Frame {
                    box_id: *box_id,
                    has_start: !*resumes,
                    has_end: false,
                    has_text: false,
                    fallback: SmallVec::new(),
                });
                needs_valign |= !infos.get(*box_id).baseline_aligned;
                tokens.push(Tok::Open(frames.len() - 1));
                stack.push(frames.len() - 1);
            }
            AtomKind::Close { interrupted } => {
                if let Some(frame) = stack.pop() {
                    frames[frame].has_end = !*interrupted;
                    tokens.push(Tok::Close(frame));
                }
            }
            AtomKind::Text(text) => {
                has_text |= index < input.range.end - hanging;
                any_rtl |= text.rtl;
                any_ltr |= !text.rtl && !text.mixed;
                any_mixed |= text.mixed;
                let chunk = &ctx.prepared.chunks[text.chunk];
                let owner = chunk.spans[text.span].box_id;
                let info = infos.get(owner);
                let sink = match stack.last() {
                    Some(&frame) => {
                        frames[frame].has_text = true;
                        &mut frames[frame].fallback
                    }
                    None => &mut root_fallback,
                };
                if info.line_height == LineHeight::Normal {
                    for run in &ctx.atoms.shaped[text.chunk].runs[text.runs.clone()] {
                        if run.metrics != info.metrics && !sink.contains(&run.metrics) {
                            sink.push(run.metrics);
                        }
                    }
                }
                tokens.push(Tok::Text(index));
            }
            AtomKind::Atomic(atomic) => {
                has_object = true;
                has_atomic = true;
                needs_valign |= !infos.get(atomic.box_id).baseline_aligned;
                tokens.push(Tok::Atomic(index));
            }
            AtomKind::Tab { .. } => {
                has_object = true;
                has_text = true;
                let owner = stack.last().copied();
                if let Some(frame) = owner {
                    frames[frame].has_text = true;
                }
                tokens.push(Tok::Tab(index));
            }
            AtomKind::Static { .. } => tokens.push(Tok::Static(index)),
            AtomKind::Forced => {}
        }
    }
    carry.clear();
    for &frame in stack.iter().rev() {
        tokens.push(Tok::Close(frame));
    }
    carry.extend(stack.iter().map(|&frame| frames[frame].box_id));

    // Line box height: the strut and every inline box that counts.
    // In quirks mode a line of atomic inlines and nothing that has text or
    // edges has no strut of its own.
    let root = infos.get(ctx.root);
    let bare = ctx.quirks
        && has_atomic
        && !has_text
        && !frames.iter().any(|f| infos.get(f.box_id).has_edges());
    let strut = (!bare).then(|| valign::strut(root, &root_fallback));
    let (mut ascent, mut descent) = strut.unwrap_or((0.0, 0.0));
    for frame in &frames {
        let info = infos.get(frame.box_id);
        if ctx.quirks && !frame.has_text && !info.has_edges() {
            continue;
        }
        let (a, d) = valign::strut(info, &frame.fallback);
        ascent = ascent.max(a);
        descent = descent.max(d);
    }
    for token in &tokens {
        if let Tok::Atomic(index) = token
            && let AtomKind::Atomic(atomic) = &atoms[*index].kind
        {
            let (above, below) = atomic_extent(atomic);
            ascent = ascent.max(above);
            descent = descent.max(below);
        }
    }

    let block_rtl = ctx.block.direction == Direction::Rtl;
    let line_dir = if any_mixed || (any_rtl && (any_ltr || has_object)) || (block_rtl && any_ltr) {
        LineDir::Mixed
    } else if any_rtl {
        LineDir::Rtl
    } else if any_ltr || !block_rtl {
        LineDir::Ltr
    } else {
        LineDir::Rtl
    };

    // Visual order: a right-to-left line reads from the other end, with the
    // roles of opening and closing a box exchanged. Text of one node that
    // continues across atoms becomes a single leaf, as the browser has it.
    if line_dir == LineDir::Rtl {
        tokens.reverse();
        for token in &mut tokens {
            *token = match *token {
                Tok::Open(f) => Tok::Close(f),
                Tok::Close(f) => Tok::Open(f),
                other => other,
            };
        }
    } else if line_dir == LineDir::Mixed {
        tokens = visual_tokens(ctx, &mut frames, &tokens, &input.range);
    }
    let owner_of = |index: usize| match &atoms[index].kind {
        AtomKind::Text(t) => {
            let span = &ctx.prepared.chunks[t.chunk].spans[t.span];
            (span.box_id, span.node)
        }
        _ => unreachable!("text tokens refer to text atoms"),
    };
    let mut visual: Vec<Vis> = Vec::with_capacity(tokens.len());
    let mut order: Vec<usize> = Vec::new();
    for token in &tokens {
        match *token {
            Tok::Open(f) => visual.push(Vis::Open(f)),
            Tok::Close(f) => visual.push(Vis::Close(f)),
            Tok::Atomic(i) => visual.push(Vis::Atomic(i)),
            Tok::Tab(i) => visual.push(Vis::Tab(i)),
            Tok::Static(i) => visual.push(Vis::Static(i)),
            Tok::Text(i) => match visual.last_mut() {
                Some(Vis::Text(range))
                    if order.get(range.end - 1).map(|&p| owner_of(p)) == Some(owner_of(i)) =>
                {
                    range.end += 1;
                    order.push(i);
                }
                _ => {
                    visual.push(Vis::Text(order.len()..order.len() + 1));
                    order.push(i);
                }
            },
        }
    }

    // Content width. White space that hangs off the end of the line is not
    // part of it.
    let mut widths = Vec::with_capacity(input.range.len());
    let mut at = input.indent;
    for index in input.range.clone() {
        let width = width_at(&atoms[index], at);
        widths.push(width);
        at += width;
    }
    // The hyphen shown at a break after a soft hyphen takes room.
    let hyphen_atom = if input.hyphen {
        atoms[input.range.clone()]
            .iter()
            .rposition(|a| !matches!(a.kind, AtomKind::Close { .. }))
            .map(|i| i + input.range.start)
            .and_then(|i| match &atoms[i].kind {
                AtomKind::Text(t) => t.hyphen.clone().map(|h| (i, h)),
                _ => None,
            })
    } else {
        None
    };
    if let Some((index, hyphen)) = &hyphen_atom {
        widths[index - input.range.start] += hyphen.width;
    }
    let width_of = |index: usize| widths[index - input.range.start];
    let mut hangs = vec![0.0_f32; input.range.len()];
    for index in input.range.clone().rev() {
        match &atoms[index].kind {
            AtomKind::Text(_) => {
                hangs[index - input.range.start] = atoms[index].hang;
                if width_of(index) - atoms[index].hang > EPSILON {
                    break;
                }
            }
            AtomKind::Atomic(_) | AtomKind::Tab { .. } => break,
            _ => {}
        }
    }
    let hang_of = |index: usize| hangs[index - input.range.start];
    let leaf_width = |range: &Range<usize>, extra: f32| {
        let raw: f32 = order[range.clone()]
            .iter()
            .map(|&i| width_of(i) - hang_of(i))
            .sum();
        atoms::snap(raw + extra)
    };
    let mut content = 0.0;
    for v in &visual {
        content += match v {
            Vis::Open(f) => {
                let (p, b, m) = piece_edges(infos.get(frames[*f].box_id), &frames[*f]);
                m.left + b.left + p.left
            }
            Vis::Close(f) => {
                let (p, b, m) = piece_edges(infos.get(frames[*f].box_id), &frames[*f]);
                m.right + b.right + p.right
            }
            Vis::Text(range) => leaf_width(range, 0.0),
            Vis::Atomic(i) | Vis::Tab(i) => width_of(*i),
            Vis::Static(_) => 0.0,
        };
    }

    // Alignment.
    let free = input.avail - input.indent - content;
    let justify = input.align == Align::Justify && free > EPSILON;
    let x0 = if justify {
        match ctx.block.direction {
            Direction::Ltr => input.indent,
            Direction::Rtl => 0.0,
        }
    } else {
        align::offset(
            input.align,
            ctx.block.direction,
            input.avail,
            input.indent,
            content,
        )
    };
    let mut separators: Vec<Vec<f32>> = Vec::new();
    let mut gap = 0.0;
    if justify {
        let mut count = 0;
        for &index in &order {
            if let AtomKind::Text(text) = &atoms[index].kind {
                let edges = collect_separators(ctx, text, hang_of(index));
                count += edges.len();
                separators.push(edges);
            }
        }
        if count > 0 {
            gap = free / count as f32;
        } else {
            separators.clear();
        }
    }

    // Emission.
    let baseline = ascent;
    let mut cursor = x0;
    let mut open: Vec<Open> = Vec::new();
    let mut top: SmallVec<[FragId; 8]> = SmallVec::new();
    let mut extent = Rect::default();
    let mut members: Vec<(FragId, Member)> = Vec::new();
    let truncating = ctx.ellipsis.is_some() && content > input.avail - input.indent + EPSILON;
    let mut cuts: HashMap<FragId, Vec<Cut>> = HashMap::new();
    let mut statics: Vec<(BoxId, f32, bool, bool)> = Vec::new();
    for v in &visual {
        match v {
            Vis::Open(f) => {
                let frame = &frames[*f];
                let (padding, border, margin) = piece_edges(infos.get(frame.box_id), frame);
                cursor += margin.left;
                let border_x = cursor;
                cursor += border.left + padding.left;
                open.push(Open {
                    border_x,
                    children: SmallVec::new(),
                });
            }
            Vis::Close(f) => {
                let Some(piece) = open.pop() else { continue };
                let frame = &frames[*f];
                let info = infos.get(frame.box_id);
                let (padding, border, margin) = piece_edges(info, frame);
                cursor += padding.right + border.right;
                let rect = Rect::new(
                    piece.border_x,
                    baseline - info.metrics.ascent - padding.top - border.top,
                    cursor - piece.border_x,
                    info.metrics.ascent
                        + info.metrics.descent
                        + padding.vertical()
                        + border.vertical(),
                );
                cursor += margin.right;
                extent = extent.union(&rect);
                let b = ctx.tree.get(frame.box_id);
                let mut shifted = rect;
                if b.paint.position == PositionKind::Relative {
                    let (dx, dy) = relative::relative_offset_in(
                        ctx.tree,
                        frame.box_id,
                        (input.avail, ctx.containing_height),
                    );
                    shifted = rect.translated(dx, dy);
                }
                let mut fragment = Fragment::new(
                    b.node,
                    FragmentKind::Box,
                    ctx.tree.paint_style_for(frame.box_id, shifted),
                    shifted,
                );
                fragment.padding = padding;
                fragment.border = border;
                fragment.margin = margin;
                fragment.flags |= crate::positioned::containing_block_flags(&b.style);
                let id = out.push(fragment);
                if needs_valign {
                    members.push((
                        id,
                        Member::Inline {
                            box_id: frame.box_id,
                            counts: !(ctx.quirks && !frame.has_text && !info.has_edges()),
                            fallback: frame.fallback.clone(),
                        },
                    ));
                }
                for &child in &piece.children {
                    let c = out.get_mut(child);
                    c.rect.x -= rect.x;
                    c.rect.y -= rect.y;
                }
                out.set_children(id, piece.children);
                push_child(&mut open, &mut top, id);
            }
            Vis::Text(range) => {
                let leaf: Vec<usize> = order[range.clone()].to_vec();
                let edges: Vec<&[f32]> = if separators.is_empty() {
                    Vec::new()
                } else {
                    separators[range.clone()]
                        .iter()
                        .map(Vec::as_slice)
                        .collect()
                };
                let extra: f32 = edges.iter().map(|e| gap * e.len() as f32).sum();
                let width = leaf_width(range, extra);
                let AtomKind::Text(first) = &atoms[leaf[0]].kind else {
                    continue;
                };
                let lead = if first.hang_left {
                    hang_of(leaf[0])
                } else {
                    0.0
                };
                let span = &ctx.prepared.chunks[first.chunk].spans[first.span];
                let info = infos.get(span.box_id);
                // Preserved spaces that hang are still part of the text.
                let shown: f32 = leaf
                    .iter()
                    .filter(|&&i| {
                        matches!(&atoms[i].kind, AtomKind::Text(t)
                            if infos.get(ctx.prepared.chunks[t.chunk].spans[t.span].box_id).collapse
                                == Handling::Preserve)
                    })
                    .map(|&i| hang_of(i))
                    .sum();
                let rect = Rect::new(
                    cursor - lead,
                    baseline - info.metrics.ascent,
                    width + lead + shown,
                    info.metrics.ascent + info.metrics.descent,
                );
                if rect.w <= 0.0 {
                    // White space that hangs off the end of the line.
                    cursor += width;
                    continue;
                }
                let fragment = Fragment::new(
                    span.node,
                    FragmentKind::Text(TextFragment {
                        text_range: source_range(ctx, &leaf),
                        runs: leaf_runs(
                            ctx,
                            &leaf,
                            &edges,
                            gap,
                            hyphen_atom
                                .as_ref()
                                .filter(|(i, _)| leaf.contains(i))
                                .map(|(_, h)| &**h),
                        ),
                        baseline: info.metrics.ascent,
                    }),
                    info.text_paint.clone(),
                    rect,
                );
                cursor += width;
                extent = extent.union(&rect);
                let id = out.push(fragment);
                if truncating {
                    cuts.insert(id, leaf_cuts(ctx, &leaf));
                }
                push_child(&mut open, &mut top, id);
            }
            Vis::Static(index) => {
                if let AtomKind::Static { box_id } = &atoms[*index].kind {
                    // The placeholder reads in the direction its bidi level gives,
                    // which decides the edge the box is aligned by.
                    let level_of = |a: &Atom| match &a.kind {
                        AtomKind::Text(t) => Some(t.level),
                        _ => None,
                    };
                    let before = atoms[input.range.start..*index]
                        .iter()
                        .rev()
                        .find_map(level_of);
                    let after = atoms[*index + 1..input.range.end].iter().find_map(level_of);
                    let level = bidi::object_level(ctx.block.direction, before, after);
                    statics.push((
                        *box_id,
                        cursor,
                        after_content.contains(index),
                        level & 1 == 1,
                    ));
                }
            }
            Vis::Tab(index) => {
                let AtomKind::Tab { box_id, node, src } = &atoms[*index].kind else {
                    continue;
                };
                let info = infos.get(*box_id);
                let width = width_of(*index);
                let rect = Rect::new(
                    cursor,
                    baseline - info.metrics.ascent,
                    width,
                    info.metrics.ascent + info.metrics.descent,
                );
                let fragment = Fragment::new(
                    *node,
                    FragmentKind::Text(TextFragment {
                        text_range: src.clone(),
                        runs: SmallVec::new(),
                        baseline: info.metrics.ascent,
                    }),
                    info.text_paint.clone(),
                    rect,
                );
                cursor += width;
                extent = extent.union(&rect);
                let id = out.push(fragment);
                push_child(&mut open, &mut top, id);
            }
            Vis::Atomic(index) => {
                let AtomKind::Atomic(atomic) = &atoms[*index].kind else {
                    continue;
                };
                let Some(frag) = atomic.frag else { continue };
                let (above, below) = atomic_extent(atomic);
                if needs_valign {
                    members.push((
                        frag,
                        Member::Atomic {
                            box_id: atomic.box_id,
                            above,
                            below,
                        },
                    ));
                }
                let margin = atomic.margin;
                let x = cursor + margin.left;
                let y = baseline - above + margin.top;
                let placed = out.get_mut(frag);
                placed.rect.x += x;
                placed.rect.y += y;
                let rect = placed.rect;
                cursor += margin.left + atomic.size.0 + margin.right;
                extent = extent.union(&rect);
                push_child(&mut open, &mut top, frag);
            }
        }
    }
    debug_assert!(open.is_empty(), "every opened piece is closed");

    let line_rect = Rect::new(input.left, input.y, input.avail, ascent + descent);
    let mut line = Fragment::new(
        NodeId::NONE,
        FragmentKind::Line,
        ctx.line_paint.clone(),
        line_rect,
    );
    let line_box = Rect::new(0.0, 0.0, input.avail, ascent + descent);
    line.overflow = line_box.union(&extent);
    let line_id = out.push(line);

    out.set_children(line_id, top.iter().copied().collect());
    if needs_valign {
        let aligned = valign::align_line(
            &LineBox {
                tree: ctx.tree,
                infos,
                root: ctx.root,
                strut,
                baseline: ascent,
                members,
            },
            &top,
            out,
        );
        (ascent, descent) = (aligned.ascent, aligned.descent);
        let line = out.get_mut(line_id);
        line.rect.h = ascent + descent;
        line.overflow = Rect::new(0.0, 0.0, input.avail, ascent + descent).union(&aligned.extent);
    }
    if let Some(ellipsis) = ctx.ellipsis
        && truncating
    {
        ellipsis::truncate_line(ellipsis, line_id, ascent, input.avail, &cuts, out);
    }
    Line {
        frag: line_id,
        height: ascent + descent,
        ascent,
        content: content + input.indent,
        extent: out.get(line_id).overflow,
        statics,
    }
}

fn push_child(open: &mut [Open], top: &mut SmallVec<[FragId; 8]>, id: FragId) {
    match open.last_mut() {
        Some(piece) => piece.children.push(id),
        None => top.push(id),
    }
}

/// How far an atomic inline's margin box reaches above and below the baseline
/// when it sits on it.
fn atomic_extent(atomic: &atoms::AtomicAtom) -> (f32, f32) {
    let (w, h) = atomic.size;
    let _ = w;
    let m = atomic.margin;
    let total = h + m.top + m.bottom;
    let above = m.top + atomic.baseline.unwrap_or(h + m.bottom);
    (above, total - above)
}

/// Places the floats that did not fit beside the content of their line, now
/// that the line's bottom is known.
#[allow(clippy::too_many_arguments)]
fn place_deferred(
    deferred: &[BoxId],
    tree: &BoxTree,
    laid: &HashMap<BoxId, float::LaidFloat>,
    floats: &mut dyn FloatContext,
    out: &mut FragmentBuilder,
    root: BoxId,
    direction: Direction,
    at_least: f32,
) {
    for &box_id in deferred {
        let (Some(entry), Some(side)) = (
            laid.get(&box_id),
            float::float_side(&tree.get(box_id).style, direction),
        ) else {
            continue;
        };
        float::position_float(
            out,
            tree,
            box_id,
            entry,
            floats,
            (side, direction),
            at_least,
        );
        out.add_inline_float(root, entry.frag);
    }
}

/// Breaks `ifc` into lines and builds their fragments.
/// Whether the out-of-flow box was block-level before it was blockified, which
/// puts its static position at the inline start of the line below the content
/// before it.
fn is_block_level(tree: &BoxTree, id: BoxId) -> bool {
    use style::values::specified::box_::DisplayOutside;
    tree.get(id).style.get_box().original_display.outside() != DisplayOutside::Inline
}

pub(super) fn build_lines(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    ifc: &InlineFormattingContext,
    space: &ConstraintSpace,
    floats: &mut dyn FloatContext,
    out: &mut FragmentBuilder,
) -> InlineResult {
    let width = match space.inline_size {
        AvailableSize::Definite(w) => w,
        AvailableSize::MinContent => 0.0,
        AvailableSize::MaxContent => f32::INFINITY,
    };
    let base = space
        .percentage_base
        .0
        .unwrap_or(if width.is_finite() { width } else { 0.0 });
    let mut infos = Infos::new(base);
    whitespace::read_infos(cx, tree, ifc, &mut infos);
    let block = super::style::read_block(tree, ifc.root, base);
    let prepared = whitespace::prepare(cx.doc, &infos, ifc);
    let mut built = atoms::build(
        cx,
        tree,
        &prepared,
        &infos,
        block.direction,
        Mode::Layout { space, out },
    );
    let root_info = infos.get(ifc.root);
    let ellipsis = if block.ellipsis && tree.get(ifc.root).paint.overflow_x != OverflowKind::Visible
    {
        atoms::make_ellipsis(cx.fonts, &root_info.text).map(|mark| Ellipsis {
            mark,
            direction: block.direction,
            metrics: root_info.metrics,
            paint: root_info.text_paint.clone(),
        })
    } else {
        None
    };
    let line_paint = super::style::line_paint(&tree.get(ifc.root).paint);

    let strut = leading(infos.get(ifc.root).line_height, infos.get(ifc.root).metrics);
    let strut_height = strut.0 + strut.1;
    let has_floats = ifc
        .items
        .iter()
        .any(|item| matches!(item, InlineItem::Float { box_id } if float::is_float(tree, *box_id)));
    // Floats met so far: laid out when first seen, placed at the line that
    // holds them, or below it when they do not fit beside its content.
    let mut handled: HashSet<BoxId> = HashSet::new();
    // `<br clear>`: the clearance owed after the forced break with that
    // ordinal among the context's forced breaks.
    let br_clear: HashMap<usize, ClearSide> = ifc
        .items
        .iter()
        .filter_map(|item| match item {
            InlineItem::ForcedBreak { box_id } => Some(*box_id),
            _ => None,
        })
        .zip(&prepared.br_forced)
        .filter_map(|(box_id, &ordinal)| {
            let clear = float::clear_side(&tree.get(box_id).style, block.direction);
            (clear != ClearSide::None).then_some((ordinal, clear))
        })
        .collect();
    let mut forced_seen = 0_usize;
    let mut laid: HashMap<BoxId, float::LaidFloat> = HashMap::new();
    let mut lines: SmallVec<[FragId; 8]> = SmallVec::new();
    let mut y = 0.0_f32;
    let mut start = 0;
    let mut carry: Vec<BoxId> = Vec::new();
    let mut first = true;
    let mut after_forced = false;
    let mut result = InlineResult {
        lines: SmallVec::new(),
        block_size: 0.0,
        first_baseline: None,
        last_baseline: None,
        overflow: Rect::default(),
        inline_extent: 0.0,
    };
    'lines: while start < built.atoms.len() {
        // Floats that did not fit beside the content before them wait for the
        // end of the line.
        let mut deferred: Vec<BoxId> = Vec::new();
        // The block extent the available width is measured over: the strut at
        // first, the line's own height once it is known.
        let mut query = strut_height;
        let mut rechecks = 0;
        let (line, band, fit) = 'position: loop {
            let band = floats.band(y, query);
            let avail = (width - band.left - band.right).max(0.0);
            let indent = if align::is_indented(&block, first, after_forced) {
                block.indent
            } else {
                0.0
            };
            let fit = fit_line(
                &mut built.atoms,
                &built.shaped,
                start,
                avail - indent,
                indent,
            );
            if has_floats {
                let mut placed_one = false;
                for index in start..fit.end {
                    let AtomKind::Static { box_id } = built.atoms[index].kind else {
                        continue;
                    };
                    if !float::is_float(tree, box_id) || !handled.insert(box_id) {
                        continue;
                    }
                    let Some(side) = float::float_side(&tree.get(box_id).style, block.direction)
                    else {
                        continue;
                    };
                    let entry = *laid
                        .entry(box_id)
                        .or_insert_with(|| float::lay_out_float(cx, tree, box_id, space, out));
                    let before: f32 = built.atoms[start..index].iter().map(|a| a.width).sum();
                    if before <= EPSILON || before + entry.size.0 <= avail + EPSILON {
                        let at = (side, block.direction);
                        float::position_float(out, tree, box_id, &entry, floats, at, y);
                        out.add_inline_float(ifc.root, entry.frag);
                        placed_one = true;
                    } else {
                        deferred.push(box_id);
                    }
                }
                if placed_one {
                    // The line is narrower now.
                    continue 'position;
                }
            }
            if fit.wide
                && infos.get(ifc.root).wraps
                && (band.left > 0.0 || band.right > 0.0)
                && let Some(next) = floats.next_edge(y, query)
            {
                // Its first unbreakable piece does not fit beside the floats,
                // so the line goes below the nearest one.
                y = next;
                query = strut_height;
                continue 'position;
            }
            let empty = carry.is_empty()
                && built.atoms[start..fit.end].iter().all(|a| {
                    matches!(
                        a.kind,
                        AtomKind::Open { .. } | AtomKind::Close { .. } | AtomKind::Static { .. }
                    ) && a.width == 0.0
                });
            if empty {
                // A line with nothing in it takes no room, but an absolutely
                // positioned box in it still has its place.
                for atom in &built.atoms[start..fit.end] {
                    if let AtomKind::Static { box_id } = atom.kind
                        && !float::is_float(tree, box_id)
                    {
                        // A block-level box ignores floats: it would sit at the
                        // start edge of the content box.
                        let x = match (block.direction, is_block_level(tree, box_id)) {
                            (Direction::Ltr, true) => 0.0,
                            (Direction::Rtl, true) => width,
                            (Direction::Ltr, false) => band.left,
                            (Direction::Rtl, false) => band.left + avail,
                        };
                        out.add_inline_static(InlineStatic {
                            root: ifc.root,
                            box_id,
                            x,
                            y,
                            line: lines.len(),
                            rtl: block.direction == Direction::Rtl,
                        });
                    }
                }
                place_deferred(
                    &deferred,
                    tree,
                    &laid,
                    floats,
                    out,
                    ifc.root,
                    block.direction,
                    y,
                );
                start = fit.end;
                continue 'lines;
            }
            let last = fit.end >= built.atoms.len();
            let indent = if align::is_indented(&block, first, after_forced) {
                block.indent
            } else {
                0.0
            };
            let input = LineIn {
                range: start..fit.end,
                y,
                left: band.left,
                avail,
                indent,
                align: align::line_align(&block, last || fit.forced),
                hyphen: fit.hyphen,
            };
            let ctx = Ctx {
                tree,
                infos: &infos,
                prepared: &prepared,
                atoms: &built,
                block: &block,
                root: ifc.root,
                quirks: cx.quirks,
                line_paint: line_paint.clone(),
                ellipsis: ellipsis.as_ref(),
                containing_height: space.percentage_base.1,
            };
            let saved = carry.clone();
            let mark = out.checkpoint();
            let line = assemble(&ctx, out, &mut carry, &input);
            if rechecks < 2 {
                // A line taller than the strut may reach floats the strut did
                // not; it is built again for the room it really has.
                let taller = query.max(line.height);
                if floats.band(y, taller) != band {
                    out.truncate(mark);
                    carry = saved;
                    query = taller;
                    rechecks += 1;
                    continue 'position;
                }
            }
            break (line, band, fit);
        };
        lines.push(line.frag);
        for &(box_id, x, after_content, rtl) in &line.statics {
            if float::is_float(tree, box_id) {
                continue;
            }
            // A block-level box sits at the inline start, below the content
            // that precedes it on the line.
            let block_level = is_block_level(tree, box_id);
            let rtl = if block_level {
                block.direction == Direction::Rtl
            } else {
                rtl
            };
            let (x, y) = if block_level {
                let start = match block.direction {
                    Direction::Ltr => 0.0,
                    Direction::Rtl => width,
                };
                (start, if after_content { y + line.height } else { y })
            } else {
                (band.left + x, y)
            };
            out.add_inline_static(InlineStatic {
                root: ifc.root,
                box_id,
                x,
                y,
                line: lines.len() - 1,
                rtl,
            });
        }
        if first {
            result.first_baseline = Some(y + line.ascent);
        }
        result.last_baseline = Some(y + line.ascent);
        result.inline_extent = result.inline_extent.max(line.content);
        result.overflow = result.overflow.union(&line.extent.translated(band.left, y));
        place_deferred(
            &deferred,
            tree,
            &laid,
            floats,
            out,
            ifc.root,
            block.direction,
            y + line.height,
        );
        y += line.height;
        if fit.forced {
            if let Some(&clear) = br_clear.get(&forced_seen) {
                y = y.max(floats.clear_offset(clear));
            }
            forced_seen += 1;
        }
        start = fit.end;
        first = false;
        after_forced = fit.forced;
    }
    result.lines = lines;
    result.block_size = y;
    result
}

/// The `(min-content, max-content)` widths of the content of `ifc`.
pub(super) fn intrinsic_sizes(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    ifc: &InlineFormattingContext,
) -> (f32, f32) {
    let mut infos = Infos::new(0.0);
    whitespace::read_infos(cx, tree, ifc, &mut infos);
    let block = super::style::read_block(tree, ifc.root, 0.0);
    let prepared = whitespace::prepare(cx.doc, &infos, ifc);
    let built = atoms::build(
        cx,
        tree,
        &prepared,
        &infos,
        block.direction,
        Mode::Intrinsic,
    );
    let atoms = &built.atoms;

    let mut max = 0.0_f32;
    let mut line = block.indent.max(0.0);
    // Floats side by side, until a `clear` ends the row.
    let (mut floats_left, mut floats_right) = (0.0_f32, 0.0_f32);
    let mut hang = 0.0_f32;
    let mut min = 0.0_f32;
    let mut piece = 0.0_f32;
    let mut piece_hang = 0.0_f32;
    for atom in atoms {
        if matches!(atom.kind, AtomKind::Forced) {
            max = max.max(line - hang + floats_left + floats_right);
            line = 0.0;
            hang = 0.0;
            min = min.max(piece - piece_hang);
            piece = 0.0;
            piece_hang = 0.0;
            continue;
        }
        if let AtomKind::Static { box_id } = atom.kind
            && float::is_float(tree, box_id)
        {
            // A float shares the line with the text around it.
            let margin = crate::block_sizes::frame_of(tree, box_id, 0.0).margin;
            let extra = margin.left + margin.right;
            let (float_min, float_max) =
                crate::block_sizes::intrinsic_inline_contribution(cx, tree, box_id);
            min = min.max(float_min + extra);
            let clear = float::clear_side(&tree.get(box_id).style, block.direction);
            if clear != ClearSide::None {
                max = max.max(floats_left + floats_right);
                if matches!(clear, ClearSide::Left | ClearSide::Both) {
                    floats_left = 0.0;
                }
                if matches!(clear, ClearSide::Right | ClearSide::Both) {
                    floats_right = 0.0;
                }
            }
            match float::float_side(&tree.get(box_id).style, block.direction) {
                Some(FloatSide::Right) => floats_right += float_max + extra,
                _ => floats_left += float_max + extra,
            }
            continue;
        }
        let breaks = matches!(atom.opp, Opp::Soft | Opp::Anywhere);
        if breaks {
            min = min.max(piece - piece_hang);
            piece = 0.0;
            piece_hang = 0.0;
        }
        let minimal = match &atom.kind {
            AtomKind::Atomic(atomic) => atomic.min_width,
            _ => atom.width,
        };
        line += width_at(atom, line);
        piece += minimal;
        if matches!(
            atom.kind,
            AtomKind::Text(_) | AtomKind::Atomic(_) | AtomKind::Tab { .. }
        ) {
            hang = atom.hang;
            piece_hang = atom.hang;
        }
    }
    max = max.max(line - hang + floats_left + floats_right);
    min = min.max(piece - piece_hang);
    (min, max)
}
