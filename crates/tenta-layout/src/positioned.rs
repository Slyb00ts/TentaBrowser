//! Absolutely and fixed positioned layout (CSS Positioned Layout 3).
//!
//! An out-of-flow box cannot be laid out where the flow meets it: its
//! containing block is the padding box of the nearest positioned ancestor, whose
//! size is known only once that ancestor is done. So block, inline and
//! flex/grid layout skip these boxes and record a [`Candidate`] on the fragment
//! of their *static parent*: the position the box would have had in flow. After
//! the whole tree is laid out, [`place_all`] walks the fragment tree once, finds
//! each candidate's containing block on the way down, solves the width and
//! height equations of CSS 2.1 10.3.7 and 10.6.4, lays the box out at that size
//! through [`layout_block`] and hangs its fragment under the containing block's
//! fragment, at the place paint order (tree order among positioned boxes) wants.
//!
//! Candidates are processed in tree order, so an out-of-flow box inside another
//! one finds its containing block already laid out. The scrollable overflow of
//! every fragment on the way grows by what was attached to it; a fixed box
//! contributes nothing because the viewport cannot scroll it.

use std::collections::{HashMap, HashSet};

use style::computed_values::position::T as StylePosition;
use style::properties::ComputedValues;
use style::values::specified::align::AlignFlags;

use crate::block::layout_block;
use crate::box_tree::{BoxId, BoxKind, BoxTree, LayoutCx};
use crate::constraint::{self, own_direction};
use crate::fragment::{FragFlags, FragId, FragmentBuilder, FragmentKind};
use crate::space::{AvailableSize, ConstraintSpace};
use crate::types::{Direction, PositionKind, Rect, TableKind};
use crate::{block_sizes, relative};

/// Where a box sits along one axis of its static-position rectangle.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum Anchor {
    /// Margin box starts at the rectangle's left (top) edge.
    Start,
    /// Margin box is centred in the rectangle.
    Center,
    /// Margin box ends at the rectangle's right (bottom) edge.
    End,
}

/// An out-of-flow box waiting for its containing block.
#[derive(Copy, Clone, Debug)]
pub(crate) struct Candidate {
    pub(crate) box_id: BoxId,
    /// The static-position rectangle in the static parent's border-box
    /// coordinates. In flow it is a zero-height strip at the box's would-be
    /// position; in flex and grid containers it is the content box.
    pub(crate) rect: Rect,
    /// Horizontal and vertical alignment inside `rect`.
    pub(crate) anchors: (Anchor, Anchor),
    /// Index among the static parent's children where the box belongs in tree
    /// order.
    pub(crate) at: usize,
    /// The innermost positioned inline box the box sits in, which is then its
    /// containing block.
    pub(crate) inline_cb: Option<BoxId>,
}

/// The facts of a style that make it a containing block.
///
/// Returns `(absolute, fixed)`: whether the box is the containing block of
/// absolutely positioned and of fixed positioned descendants. A positioned box
/// contains absolute descendants only; transforms, filters, perspective,
/// `contain: layout | paint` and `will-change` naming them contain both.
pub(crate) fn containing_block_flags(style: &ComputedValues) -> FragFlags {
    use style::values::generics::transform::{GenericRotate, GenericScale, GenericTranslate};
    use style::values::specified::box_::{Contain, WillChangeBits};
    let b = style.get_box();
    let transformed = !b.transform.0.is_empty()
        || !matches!(b.translate, GenericTranslate::None)
        || !matches!(b.rotate, GenericRotate::None)
        || !matches!(b.scale, GenericScale::None)
        || !matches!(
            b.perspective,
            style::values::generics::box_::GenericPerspective::None
        );
    let effects = style.get_effects();
    let filtered = !effects.filter.0.is_empty() || !effects.backdrop_filter.0.is_empty();
    let contained = b.contain.intersects(Contain::LAYOUT | Contain::PAINT);
    let will_change = b.will_change.bits.intersects(
        WillChangeBits::TRANSFORM
            | WillChangeBits::PERSPECTIVE
            | WillChangeBits::CONTAIN
            | WillChangeBits::FIXPOS_CB_NON_SVG,
    );
    let mut flags = FragFlags::empty();
    if transformed || filtered || contained || will_change {
        flags |= FragFlags::ABSPOS_CB | FragFlags::FIXED_CB;
    }
    if b.position != StylePosition::Static {
        flags |= FragFlags::ABSPOS_CB;
    }
    flags
}

/// The candidate for an absolutely positioned child of a block container
/// whose in-flow content ends at `block_offset` (content-box coordinates
/// shifted by `origin`) and whose content box is `content_width` wide.
pub(crate) fn flow_candidate(
    box_id: BoxId,
    origin: (f32, f32),
    block_offset: f32,
    content_width: f32,
    direction: Direction,
    at: usize,
) -> Candidate {
    Candidate {
        box_id,
        rect: Rect::new(origin.0, origin.1 + block_offset, content_width, 0.0),
        anchors: (inline_start(direction), Anchor::Start),
        at,
        inline_cb: None,
    }
}

/// The candidate for an absolutely positioned box whose placeholder inline
/// layout found in a line: the strip at the top of that line, at the
/// placeholder's inline offset.
pub(crate) fn inline_candidate(
    mark: &crate::fragment::InlineStatic,
    origin: (f32, f32),
    at: usize,
    inline_cb: Option<BoxId>,
) -> Candidate {
    Candidate {
        box_id: mark.box_id,
        rect: Rect::new(origin.0 + mark.x, origin.1 + mark.y, 0.0, 0.0),
        anchors: (
            inline_start(if mark.rtl {
                Direction::Rtl
            } else {
                Direction::Ltr
            }),
            Anchor::Start,
        ),
        at,
        inline_cb,
    }
}

/// For each out-of-flow placeholder of `ifc`, the innermost positioned inline
/// box around it.
pub(crate) fn inline_containing_blocks(
    tree: &BoxTree,
    ifc: &crate::box_tree::InlineFormattingContext,
) -> HashMap<BoxId, BoxId> {
    use crate::box_tree::InlineItem;
    let mut open: Vec<BoxId> = Vec::new();
    let mut found = HashMap::new();
    for item in &ifc.items {
        match item {
            InlineItem::OpenInline { box_id, .. } => open.push(*box_id),
            InlineItem::CloseInline { box_id, .. } => {
                if let Some(index) = open.iter().rposition(|b| b == box_id) {
                    open.truncate(index);
                }
            }
            InlineItem::AbsPos { box_id } => {
                let positioned = open
                    .iter()
                    .rev()
                    .find(|&&b| tree.get(b).paint.position != PositionKind::Static);
                if let Some(&cb) = positioned {
                    found.insert(*box_id, cb);
                }
            }
            _ => {}
        }
    }
    found
}

fn inline_start(direction: Direction) -> Anchor {
    match direction {
        Direction::Ltr => Anchor::Start,
        Direction::Rtl => Anchor::End,
    }
}

/// The candidate for an absolutely positioned child of a flex or grid
/// container: the child is aligned inside the container's content box as if it
/// were the only item (CSS Flexbox 4.1, CSS Grid 9).
pub(crate) fn container_candidate(
    tree: &BoxTree,
    container: BoxId,
    box_id: BoxId,
    content_box: Rect,
    at: usize,
) -> Candidate {
    let c = tree.get(container);
    let child = tree.get(box_id);
    let rtl = own_direction(&c.style) == Direction::Rtl;
    let anchors = if matches!(c.kind, BoxKind::Flex) {
        flex_anchors(&c.style, &child.style, rtl)
    } else {
        grid_anchors(&c.style, &child.style, rtl)
    };
    Candidate {
        box_id,
        rect: content_box,
        anchors,
        at,
        inline_cb: None,
    }
}

/// Item alignment of an axis as an anchor. `far_start` says the axis' start
/// edge is the right or bottom one; `flex_far_start` the same for the flex
/// start side.
fn item_anchor(
    value: AlignFlags,
    far_start: bool,
    flex_far_start: bool,
    horizontal: bool,
) -> Anchor {
    let start = if far_start {
        Anchor::End
    } else {
        Anchor::Start
    };
    let end = if far_start {
        Anchor::Start
    } else {
        Anchor::End
    };
    let flex_start = if flex_far_start {
        Anchor::End
    } else {
        Anchor::Start
    };
    let flex_end = if flex_far_start {
        Anchor::Start
    } else {
        Anchor::End
    };
    match value {
        AlignFlags::CENTER => Anchor::Center,
        AlignFlags::START | AlignFlags::SELF_START => start,
        AlignFlags::END | AlignFlags::SELF_END => end,
        AlignFlags::FLEX_START => flex_start,
        AlignFlags::FLEX_END => flex_end,
        AlignFlags::LEFT if horizontal => Anchor::Start,
        AlignFlags::RIGHT if horizontal => Anchor::End,
        AlignFlags::LAST_BASELINE => flex_end,
        // `normal`, `stretch` and the baselines start the box.
        _ => flex_start,
    }
}

/// The value `align-self` has once `auto` is replaced by the container's
/// `align-items`.
fn self_alignment(container: &ComputedValues, child: &ComputedValues) -> AlignFlags {
    let own = child.get_position().align_self.0.value();
    if own == AlignFlags::AUTO {
        container.get_position().align_items.0.value()
    } else {
        own
    }
}

fn flex_anchors(container: &ComputedValues, child: &ComputedValues, rtl: bool) -> (Anchor, Anchor) {
    use style::computed_values::flex_direction::T as Dir;
    use style::computed_values::flex_wrap::T as Wrap;
    let pos = container.get_position();
    let row = matches!(pos.flex_direction, Dir::Row | Dir::RowReverse);
    let main_reversed = matches!(pos.flex_direction, Dir::RowReverse | Dir::ColumnReverse);
    let cross_reversed = matches!(pos.flex_wrap, Wrap::WrapReverse);
    // The flex start side of the main axis is the axis' start edge unless the
    // direction is reversed; row direction follows the inline direction.
    let main_far = if row {
        rtl != main_reversed
    } else {
        main_reversed
    };
    let cross_far = if row {
        cross_reversed
    } else {
        rtl != cross_reversed
    };
    let main_value = {
        let v = pos.justify_content.primary().value();
        match v {
            AlignFlags::SPACE_AROUND | AlignFlags::SPACE_EVENLY => AlignFlags::CENTER,
            AlignFlags::SPACE_BETWEEN | AlignFlags::STRETCH | AlignFlags::NORMAL => {
                AlignFlags::FLEX_START
            }
            other => other,
        }
    };
    let cross_value = self_alignment(container, child);
    // The writing-mode start edge of each axis.
    let inline_far = rtl;
    let (inline, block) = if row {
        (
            item_anchor(main_value, inline_far, main_far, true),
            item_anchor(cross_value, false, cross_far, false),
        )
    } else {
        (
            item_anchor(cross_value, inline_far, cross_far, true),
            item_anchor(main_value, false, main_far, false),
        )
    };
    (inline, block)
}

fn grid_anchors(container: &ComputedValues, child: &ComputedValues, rtl: bool) -> (Anchor, Anchor) {
    let pos = container.get_position();
    let own_justify = child.get_position().justify_self.0.value();
    let justify = if own_justify == AlignFlags::AUTO {
        pos.justify_items.computed.0.0.value()
    } else {
        own_justify
    };
    let align = self_alignment(container, child);
    (
        item_anchor(justify, rtl, rtl, true),
        item_anchor(align, false, false, false),
    )
}

/// Everything one axis of the position equations needs.
struct Axis {
    /// Length of the containing block's padding box.
    cb: f32,
    start: Option<f32>,
    end: Option<f32>,
    margin_start: Option<f32>,
    margin_end: Option<f32>,
    /// Static-position rectangle `(start, length)` in the padding box.
    stat: (f32, f32),
    anchor: Anchor,
    /// The containing block's inline direction is right to left, so the start
    /// inset gives way when the equation is over-constrained.
    rtl: bool,
    /// Auto margins share a negative free space equally (the vertical axis);
    /// otherwise the margin at the end of the direction takes all of it.
    split_negative: bool,
}

/// Used border-box start, and the margins, along one axis.
struct Placed {
    pos: f32,
    margin_start: f32,
    margin_end: f32,
}

impl Axis {
    /// Solves the equation `start + margin + size + margin + end = cb` for the
    /// box of border-box `size` (CSS 2.1 10.3.7 and 10.6.4).
    fn place(&self, size: f32) -> Placed {
        let ms0 = self.margin_start.unwrap_or(0.0);
        let me0 = self.margin_end.unwrap_or(0.0);
        match (self.start, self.end) {
            (Some(s), Some(e)) => {
                let free = self.cb - s - e - size - ms0 - me0;
                let (ms, me) = match (self.margin_start, self.margin_end) {
                    (None, None) if free >= 0.0 || self.split_negative => (free / 2.0, free / 2.0),
                    (None, None) if self.rtl => (free, 0.0),
                    (None, None) => (0.0, free),
                    (None, Some(me)) => (free + ms0, me),
                    (Some(ms), None) => (ms, free + me0),
                    (Some(ms), Some(me)) => (ms, me),
                };
                let over_constrained = self.margin_start.is_some() && self.margin_end.is_some();
                let pos = if over_constrained && self.rtl {
                    self.cb - e - me - size
                } else {
                    s + ms
                };
                Placed {
                    pos,
                    margin_start: ms,
                    margin_end: me,
                }
            }
            (Some(s), None) => Placed {
                pos: s + ms0,
                margin_start: ms0,
                margin_end: me0,
            },
            (None, Some(e)) => Placed {
                pos: self.cb - e - me0 - size,
                margin_start: ms0,
                margin_end: me0,
            },
            (None, None) => {
                let (start, length) = self.stat;
                let pos = match self.anchor {
                    Anchor::Start => start + ms0,
                    Anchor::End => start + length - me0 - size,
                    Anchor::Center => start + (length - (ms0 + size + me0)) / 2.0 + ms0,
                };
                Placed {
                    pos,
                    margin_start: ms0,
                    margin_end: me0,
                }
            }
        }
    }

    /// Space a shrink-to-fit box may take: the containing block less the
    /// insets that are set, or less the side of the static position the box
    /// grows away from, and less the margins.
    fn available(&self) -> f32 {
        let ms0 = self.margin_start.unwrap_or(0.0);
        let me0 = self.margin_end.unwrap_or(0.0);
        let room = match (self.start, self.end) {
            (Some(s), Some(e)) => self.cb - s - e,
            (Some(s), None) => self.cb - s,
            (None, Some(e)) => self.cb - e,
            (None, None) => match self.anchor {
                Anchor::Start => self.cb - self.stat.0,
                Anchor::End => self.stat.0 + self.stat.1,
                Anchor::Center => self.cb,
            },
        };
        (room - ms0 - me0).max(0.0)
    }
}

/// A containing block found for a candidate.
struct Cb {
    /// Padding box in document coordinates.
    padding_box: Rect,
    /// The fragment that is the containing block (the root for the initial
    /// one) and the document origin of its border box.
    fragment: FragId,
    origin: (f32, f32),
    direction: Direction,
}

struct Frame {
    frag: FragId,
    /// Document coordinates of the fragment's border-box origin.
    origin: (f32, f32),
}

/// An out-of-flow fragment and the offset of its border box from its
/// containing block's.
type Contained = (FragId, (f32, f32));

struct Pass<'a, 'c, 't> {
    cx: &'a mut LayoutCx<'c>,
    tree: &'t BoxTree,
    out: &'a mut FragmentBuilder,
    viewport: (f32, f32),
    pending: HashMap<FragId, Vec<Candidate>>,
    parents: Vec<u32>,
    marked: Vec<bool>,
    /// Fragments whose overflow must be recomputed.
    dirty: HashSet<FragId>,
    stack: Vec<Frame>,
    abs: Vec<usize>,
    fixed: Vec<usize>,
    /// Fragments below this index have their children recorded in `parents`.
    scanned: usize,
    /// Out-of-flow fragments by containing block, with the offset of their
    /// border box from the block's border box.
    contained: HashMap<FragId, Vec<Contained>>,
    /// Out-of-flow fragment to its containing block.
    block_of: HashMap<FragId, FragId>,
}

const NO_PARENT: u32 = u32::MAX;

/// Lays out every recorded out-of-flow box and attaches it to its containing
/// block's fragment. `viewport` is the initial containing block.
pub(crate) fn place_all(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    out: &mut FragmentBuilder,
    root: FragId,
    viewport: (f32, f32),
) {
    let first = out.take_candidates();
    if first.is_empty() {
        return;
    }
    let mut pass = Pass {
        cx,
        tree,
        out,
        viewport,
        pending: HashMap::new(),
        parents: Vec::new(),
        marked: Vec::new(),
        dirty: HashSet::new(),
        stack: Vec::new(),
        abs: Vec::new(),
        fixed: Vec::new(),
        scanned: 0,
        contained: HashMap::new(),
        block_of: HashMap::new(),
    };
    pass.absorb(first);
    let origin = pass.out.get(root).rect;
    pass.visit(root, (origin.x, origin.y));
}

impl Pass<'_, '_, '_> {
    /// Indexes the fragments pushed since the last call and files the
    /// candidates in `found`.
    fn absorb(&mut self, found: Vec<(FragId, Candidate)>) {
        self.index_new();
        self.file(found);
    }

    /// Records the parents of the fragments pushed since the last call.
    fn index_new(&mut self) {
        let len = self.out.len();
        self.parents.resize(len, NO_PARENT);
        self.marked.resize(len, false);
        for index in self.scanned..len {
            for &child in &self.out.get(FragId(index as u32)).children {
                self.parents[child.index()] = index as u32;
            }
        }
        self.scanned = len;
    }

    /// Files candidates under their static parents and marks the way to them.
    fn file(&mut self, found: Vec<(FragId, Candidate)>) {
        for (frag, candidate) in found {
            self.mark(frag);
            self.pending.entry(frag).or_default().push(candidate);
        }
    }

    /// Marks `frag` and its ancestors as needing a visit.
    fn mark(&mut self, frag: FragId) {
        let mut cursor = frag.index();
        loop {
            if self.marked[cursor] {
                return;
            }
            self.marked[cursor] = true;
            match self.parents[cursor] {
                NO_PARENT => return,
                parent => cursor = parent as usize,
            }
        }
    }

    /// Visits `frag`, whose border box starts at `origin`, and returns whether
    /// its scrollable overflow grew.
    fn visit(&mut self, frag: FragId, origin: (f32, f32)) -> bool {
        let depth = self.stack.len();
        let flags = self.out.get(frag).flags;
        self.stack.push(Frame { frag, origin });
        if flags.contains(FragFlags::ABSPOS_CB) {
            self.abs.push(depth);
        }
        if flags.contains(FragFlags::FIXED_CB) {
            self.fixed.push(depth);
        }
        if let Some(candidates) = self.pending.remove(&frag) {
            let mut placed_here = 0;
            for candidate in candidates {
                self.place(depth, candidate, &mut placed_here);
            }
        }
        let mut index = 0;
        while index < self.out.get(frag).children.len() {
            let child = self.out.get(frag).children[index];
            if self.marked[child.index()] {
                let rect = self.out.get(child).rect;
                if self.visit(child, (origin.0 + rect.x, origin.1 + rect.y)) {
                    self.dirty.insert(frag);
                    if let Some(&block) = self.block_of.get(&child) {
                        self.dirty.insert(block);
                    }
                }
            }
            index += 1;
        }
        if flags.contains(FragFlags::FIXED_CB) {
            self.fixed.pop();
        }
        if flags.contains(FragFlags::ABSPOS_CB) {
            self.abs.pop();
        }
        self.stack.pop();
        self.grow_overflow(frag)
    }

    /// Recomputes the overflow of `frag` from its children when something
    /// below it changed; true when it grew.
    fn grow_overflow(&mut self, frag: FragId) -> bool {
        if !self.dirty.remove(&frag) {
            return false;
        }
        let f = self.out.get(frag);
        let before = f.overflow;
        let holds_fixed = f.flags.contains(FragFlags::FIXED_CB);
        let mut overflow = before;
        // Out-of-flow boxes count towards their containing block, not towards
        // the fragments they hang below.
        let contribution = |k: &crate::fragment::Fragment, at: (f32, f32)| {
            if k.flags.contains(FragFlags::CLIPS_OVERFLOW) {
                k.rect.translated(at.0 - k.rect.x, at.1 - k.rect.y)
            } else {
                k.overflow.translated(at.0, at.1)
            }
        };
        for &kid in &f.children {
            if self.block_of.contains_key(&kid) {
                continue;
            }
            let k = self.out.get(kid);
            overflow = overflow.union(&contribution(k, (k.rect.x, k.rect.y)));
        }
        for &(kid, at) in self.contained.get(&frag).into_iter().flatten() {
            let k = self.out.get(kid);
            if k.paint.position == PositionKind::Fixed && !holds_fixed {
                continue;
            }
            overflow = overflow.union(&contribution(k, at));
        }
        self.out.get_mut(frag).overflow = overflow;
        overflow != before
    }

    /// The containing block of `candidate` for a walk currently at `depth`.
    fn containing_block(&mut self, depth: usize, candidate: &Candidate, fixed: bool) -> Cb {
        if let Some(cb) = self.inline_containing_block(depth, candidate) {
            return cb;
        }
        let chain = if fixed { &self.fixed } else { &self.abs };
        match chain.last() {
            Some(&index) => {
                let frame = &self.stack[index];
                let f = self.out.get(frame.frag);
                let padding_box = Rect::new(
                    frame.origin.0 + f.border.left,
                    frame.origin.1 + f.border.top,
                    (f.rect.w - f.border.horizontal()).max(0.0),
                    (f.rect.h - f.border.vertical()).max(0.0),
                );
                let direction = self
                    .tree
                    .boxes_of(f.node)
                    .first()
                    .map_or(Direction::Ltr, |&b| own_direction(&self.tree.get(b).style));
                Cb {
                    padding_box,
                    fragment: frame.frag,
                    origin: frame.origin,
                    direction,
                }
            }
            None => Cb {
                padding_box: Rect::new(0.0, 0.0, self.viewport.0, self.viewport.1),
                fragment: self.stack[0].frag,
                origin: self.stack[0].origin,
                direction: own_direction(&self.tree.get(self.tree.root).style),
            },
        }
    }

    /// The containing block when the box sits in a positioned inline box: from
    /// the first fragment's start corner to the last fragment's end corner.
    fn inline_containing_block(&self, depth: usize, candidate: &Candidate) -> Option<Cb> {
        let inline = candidate.inline_cb?;
        if self.tree.get(candidate.box_id).paint.position == PositionKind::Fixed {
            return None;
        }
        let node = self.tree.get(inline).node;
        let frame = &self.stack[depth];
        let mut pieces = Vec::new();
        self.find_pieces(frame.frag, frame.origin, node, &mut pieces);
        // Bidirectional reordering can leave empty pieces of the box behind.
        if pieces
            .iter()
            .any(|&(piece, _)| self.out.get(piece).rect.w > 0.0)
        {
            pieces.retain(|&(piece, _)| self.out.get(piece).rect.w > 0.0);
        }
        let (first, first_origin) = *pieces.first()?;
        let (last, last_origin) = *pieces.last()?;
        let direction = own_direction(&self.tree.get(inline).style);
        let padding_box = |piece: FragId, origin: (f32, f32)| {
            let f = self.out.get(piece);
            Rect::new(
                origin.0 + f.border.left,
                origin.1 + f.border.top,
                (f.rect.w - f.border.horizontal()).max(0.0),
                (f.rect.h - f.border.vertical()).max(0.0),
            )
        };
        // CSS 2.1 10.1.4.1: from the start corner of the first fragment's
        // padding box to the end corner of the last one's.
        let (first_box, last_box) = (
            padding_box(first, first_origin),
            padding_box(last, last_origin),
        );
        let (left, right) = match direction {
            Direction::Ltr => (first_box.x, last_box.right()),
            Direction::Rtl => (last_box.x, first_box.right()),
        };
        let bounds = Rect::new(
            left,
            first_box.y,
            (right - left).max(0.0),
            (last_box.bottom() - first_box.y).max(0.0),
        );
        Some(Cb {
            padding_box: bounds,
            fragment: first,
            origin: first_origin,
            direction,
        })
    }

    /// Collects the box fragments of `node` below `frag` in document order.
    fn find_pieces(
        &self,
        frag: FragId,
        origin: (f32, f32),
        node: tenta_core::NodeId,
        found: &mut Vec<(FragId, (f32, f32))>,
    ) {
        for &child in &self.out.get(frag).children {
            let c = self.out.get(child);
            let child_origin = (origin.0 + c.rect.x, origin.1 + c.rect.y);
            if c.node == node && c.kind == FragmentKind::Box {
                found.push((child, child_origin));
            }
            if matches!(c.kind, FragmentKind::Line | FragmentKind::Box)
                && !c.flags.contains(FragFlags::PAINT_ATOMIC)
            {
                self.find_pieces(child, child_origin, node, found);
            }
        }
    }

    /// Lays out one candidate and attaches its fragment.
    fn place(&mut self, depth: usize, candidate: Candidate, placed_here: &mut usize) {
        let tree = self.tree;
        let id = candidate.box_id;
        let b = tree.get(id);
        let fixed = b.paint.position == PositionKind::Fixed;
        let cb = self.containing_block(depth, &candidate, fixed);
        let (cb_w, cb_h) = (cb.padding_box.w, cb.padding_box.h);
        let inputs = constraint::abspos_inputs(self.cx, tree, id, (cb_w, cb_h));
        let pos = b.style.get_position();

        // The static-position rectangle in the containing block's coordinates.
        let origin = self.stack[depth].origin;
        let stat_x = origin.0 + candidate.rect.x - cb.padding_box.x;
        let stat_y = origin.1 + candidate.rect.y - cb.padding_box.y;
        let rtl = cb.direction == Direction::Rtl;
        let horizontal = Axis {
            cb: cb_w,
            start: relative::resolve(&pos.left, Some(cb_w)),
            end: relative::resolve(&pos.right, Some(cb_w)),
            margin_start: inputs.margin_left,
            margin_end: inputs.margin_right,
            stat: (stat_x, candidate.rect.w),
            anchor: candidate.anchors.0,
            rtl,
            split_negative: false,
        };
        let vertical = Axis {
            cb: cb_h,
            start: relative::resolve(&pos.top, Some(cb_h)),
            end: relative::resolve(&pos.bottom, Some(cb_h)),
            margin_start: inputs.margin_top,
            margin_end: inputs.margin_bottom,
            stat: (stat_y, candidate.rect.h),
            anchor: candidate.anchors.1,
            rtl: false,
            split_negative: true,
        };

        let edges = inputs.padding.horizontal() + inputs.border.horizontal();
        let vertical_edges = inputs.padding.vertical() + inputs.border.vertical();
        let both_vertical = vertical.start.is_some() && vertical.end.is_some();
        // A preferred aspect ratio gives the automatic axis the other one's
        // size, when the box is not stretched along it.
        let ratio_width = inputs
            .aspect_ratio
            .zip(inputs.height)
            .filter(|_| {
                inputs.width.is_none() && !(horizontal.start.is_some() && horizontal.end.is_some())
            })
            .map(|(ratio, h)| {
                if inputs.border_box_sizing {
                    (h + vertical_edges) * ratio - edges
                } else {
                    h * ratio
                }
            });
        let content_width = match inputs.width.or(ratio_width) {
            Some(w) => w,
            // Like a replaced box, a table with automatic width shrinks to fit
            // even between two insets.
            None if horizontal.start.is_some()
                && horizontal.end.is_some()
                && tree.get(id).kind != BoxKind::Table(TableKind::Table) =>
            {
                (horizontal.available() - edges).max(0.0)
            }
            None => {
                let (min, max) = block_sizes::intrinsic_inline_sizes_of_block(self.cx, tree, id);
                max.min(min.max((horizontal.available() - edges).max(0.0)))
            }
        };
        let content_width = content_width.min(inputs.max_width).max(inputs.min_width);
        let border_width = content_width + edges;

        let ratio_height = inputs
            .aspect_ratio
            .filter(|_| inputs.height.is_none() && !both_vertical)
            .map(|ratio| {
                if inputs.border_box_sizing {
                    border_width / ratio
                } else {
                    content_width / ratio + vertical_edges
                }
            });
        let fixed_height = ratio_height.or_else(|| {
            (inputs.height.is_none() && both_vertical)
                .then(|| (vertical.available() - vertical_edges).max(0.0) + vertical_edges)
        });

        let mut space = ConstraintSpace::new(
            AvailableSize::Definite(border_width),
            fixed_height.map_or(AvailableSize::MaxContent, AvailableSize::Definite),
        );
        space.percentage_base = (Some(cb_w), Some(cb_h));
        space.fixed_inline = true;
        space.fixed_block = fixed_height.is_some();
        space.direction = cb.direction;
        let result = layout_block(self.cx, tree, id, &space, self.out);
        let frag = result.frag;
        let height = self.out.get(frag).rect.h;

        let x = horizontal.place(border_width);
        let y = vertical.place(height);

        // The fragment hangs below its static parent, so paint finds it in the
        // stacking contexts and effect groups of its tree ancestors; only
        // geometry, overflow and clipping follow the containing block.
        let (parent, parent_origin) = (self.stack[depth].frag, self.stack[depth].origin);
        let (left, top) = (cb.padding_box.x + x.pos, cb.padding_box.y + y.pos);
        {
            let f = self.out.get_mut(frag);
            f.rect.x = left - parent_origin.0;
            f.rect.y = top - parent_origin.1;
            f.margin.left = x.margin_start;
            f.margin.right = x.margin_end;
            f.margin.top = y.margin_start;
            f.margin.bottom = y.margin_end;
            f.flags |= FragFlags::OUT_OF_FLOW;
        }
        let siblings = self.out.get(parent).children.len();
        let index = (candidate.at + *placed_here).min(siblings);
        *placed_here += 1;
        self.out.get_mut(parent).children.insert(index, frag);
        self.out.set_containing_block(frag, cb.fragment);
        self.block_of.insert(frag, cb.fragment);
        self.contained
            .entry(cb.fragment)
            .or_default()
            .push((frag, (left - cb.origin.0, top - cb.origin.1)));
        self.dirty.insert(cb.fragment);
        self.mark(cb.fragment);

        self.index_new();
        self.parents[frag.index()] = parent.index() as u32;
        let found = self.out.take_candidates();
        self.file(found);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axis(start: Option<f32>, end: Option<f32>, margins: (Option<f32>, Option<f32>)) -> Axis {
        Axis {
            cb: 100.0,
            start,
            end,
            margin_start: margins.0,
            margin_end: margins.1,
            stat: (10.0, 50.0),
            anchor: Anchor::Start,
            rtl: false,
            split_negative: false,
        }
    }

    fn solve(a: &Axis, size: f32) -> (f32, f32, f32) {
        let placed = a.place(size);
        (placed.pos, placed.margin_start, placed.margin_end)
    }

    #[test]
    fn auto_margins_share_the_free_space() {
        let a = axis(Some(0.0), Some(0.0), (None, None));
        assert_eq!(solve(&a, 40.0), (30.0, 30.0, 30.0));
    }

    #[test]
    fn negative_free_space_goes_to_the_end_margin_horizontally_and_splits_vertically() {
        let mut a = axis(Some(0.0), Some(0.0), (None, None));
        assert_eq!(solve(&a, 130.0), (0.0, 0.0, -30.0));
        a.rtl = true;
        assert_eq!(solve(&a, 130.0), (-30.0, -30.0, 0.0));
        a.rtl = false;
        a.split_negative = true;
        assert_eq!(solve(&a, 130.0), (-15.0, -15.0, -15.0));
    }

    #[test]
    fn one_auto_margin_absorbs_what_is_left() {
        let a = axis(Some(10.0), Some(20.0), (None, Some(5.0)));
        assert_eq!(solve(&a, 40.0), (35.0, 25.0, 5.0));
        let a = axis(Some(10.0), Some(20.0), (Some(5.0), None));
        assert_eq!(solve(&a, 40.0), (15.0, 5.0, 25.0));
    }

    #[test]
    fn an_over_constrained_axis_ignores_the_end_inset_unless_rtl() {
        let mut a = axis(Some(10.0), Some(10.0), (Some(0.0), Some(0.0)));
        assert_eq!(solve(&a, 30.0).0, 10.0);
        a.rtl = true;
        assert_eq!(solve(&a, 30.0).0, 60.0);
    }

    #[test]
    fn a_single_inset_pins_its_own_edge() {
        let from_start = axis(Some(7.0), None, (Some(3.0), None));
        assert_eq!(solve(&from_start, 20.0).0, 10.0);
        let from_end = axis(None, Some(7.0), (None, Some(3.0)));
        assert_eq!(solve(&from_end, 20.0).0, 70.0);
    }

    #[test]
    fn with_no_inset_the_static_rectangle_and_anchor_decide() {
        let mut a = axis(None, None, (Some(2.0), Some(4.0)));
        assert_eq!(solve(&a, 20.0).0, 12.0);
        a.anchor = Anchor::End;
        assert_eq!(solve(&a, 20.0).0, 36.0);
        a.anchor = Anchor::Center;
        assert_eq!(solve(&a, 20.0).0, 10.0 + (50.0 - 26.0) / 2.0 + 2.0);
    }

    #[test]
    fn shrink_to_fit_room_depends_on_the_side_that_is_pinned() {
        assert_eq!(axis(Some(30.0), None, (None, None)).available(), 70.0);
        assert_eq!(axis(None, Some(30.0), (Some(5.0), None)).available(), 65.0);
        assert_eq!(axis(None, None, (None, None)).available(), 90.0);
        let mut end = axis(None, None, (None, Some(10.0)));
        end.anchor = Anchor::End;
        assert_eq!(end.available(), 50.0);
        assert_eq!(axis(Some(80.0), Some(80.0), (None, None)).available(), 0.0);
    }
}
