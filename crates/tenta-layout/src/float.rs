//! Float placement and the real `FloatContext` (CSS 2.1 sections 9.5 and 9.5.1).
//!
//! Every block formatting context owns one [`Floats`] set: the margin boxes of
//! its floats in the coordinates of the context root's content box. The set of
//! the context being laid out is the thread's *current* one; a box that
//! establishes a formatting context swaps in an empty set for its subtree and
//! swaps the previous one back ([`enter_root`], [`RootScope::leave`]), so no function
//! on the layout path needs an extra parameter.
//!
//! Floats are placed in source order, and a float's top is never above an
//! earlier float's top (rule 5), so each side keeps its rectangles sorted by
//! top. Together with a running maximum of the bottoms that lets a lookup stop
//! walking back as soon as nothing older can reach the queried range.

use std::cell::RefCell;

use style::computed_values::clear::T as StyleClear;
use style::computed_values::float::T as StyleFloat;
use style::properties::ComputedValues;

use style::values::computed::Size;

use crate::block_sizes::frame_of;
use crate::box_tree::{BoxFlags, BoxId, BoxKind, BoxTree, LayoutCx};
use crate::constraint::resolve_box;
use crate::fragment::{FragFlags, FragId, FragmentBuilder};
use crate::inline::atomic::layout_atomic;
use crate::space::{AvailableSize, Band, ConstraintSpace, FloatContext};
use crate::types::{ClearSide, Direction, Edges, FloatSide, Rect, TableKind};

/// Slack for comparing offsets that were summed in different orders; Blink
/// lays out in 1/64 px units.
pub(crate) const EPSILON: f32 = 1.0 / 64.0;

/// The floats on one side, sorted by top.
#[derive(Clone, Debug, Default)]
struct Side {
    rects: Vec<Rect>,
    /// `lowest[i]` is the largest bottom among `rects[..=i]`.
    lowest: Vec<f32>,
}

impl Side {
    fn push(&mut self, rect: Rect) {
        let bottom = rect.bottom();
        let lowest = self.lowest.last().map_or(bottom, |&l| l.max(bottom));
        self.rects.push(rect);
        self.lowest.push(lowest);
    }

    fn truncate(&mut self, len: usize) {
        self.rects.truncate(len);
        self.lowest.truncate(len);
    }

    fn bottom(&self) -> Option<f32> {
        self.lowest.last().copied()
    }

    /// Rebuilds the running maximum from `from` on after rectangles moved.
    fn refresh(&mut self, from: usize) {
        for i in from..self.rects.len() {
            let bottom = self.rects[i].bottom();
            self.lowest[i] = if i == 0 {
                bottom
            } else {
                self.lowest[i - 1].max(bottom)
            };
        }
    }

    /// Calls `f` for every rectangle that intersects the block range
    /// `[y, y + h)`, newest first.
    fn for_each_in(&self, y: f32, h: f32, mut f: impl FnMut(&Rect)) {
        let end = self.rects.partition_point(|r| r.y < y + h);
        for i in (0..end).rev() {
            if self.lowest[i] <= y {
                break;
            }
            if self.rects[i].bottom() > y {
                f(&self.rects[i]);
            }
        }
    }
}

/// A moment in the life of a [`Floats`] set, to discard what came after it.
#[derive(Copy, Clone, Debug)]
pub(crate) struct Mark {
    left: usize,
    right: usize,
    last_top: Option<f32>,
}

/// The floats of one block formatting context.
#[derive(Clone, Debug, Default)]
pub(crate) struct Floats {
    left: Side,
    right: Side,
    /// Top of the lowest-placed float so far (rule 5).
    last_top: Option<f32>,
    /// Counts the questions asked of a non-empty set and the floats placed, so
    /// a caller can tell whether a subtree depended on its position.
    activity: u64,
}

impl Floats {
    pub(crate) fn is_empty(&self) -> bool {
        self.left.rects.is_empty() && self.right.rects.is_empty()
    }

    pub(crate) fn activity(&self) -> u64 {
        self.activity
    }

    pub(crate) fn mark(&self) -> Mark {
        Mark {
            left: self.left.rects.len(),
            right: self.right.rects.len(),
            last_top: self.last_top,
        }
    }

    /// Forgets the floats placed after `mark`.
    pub(crate) fn truncate(&mut self, mark: &Mark) {
        self.left.truncate(mark.left);
        self.right.truncate(mark.right);
        self.last_top = mark.last_top;
    }

    /// Moves the floats placed after `mark` down by `dy`.
    pub(crate) fn shift_since(&mut self, mark: &Mark, dy: f32) {
        for (side, from) in [(&mut self.left, mark.left), (&mut self.right, mark.right)] {
            for rect in &mut side.rects[from..] {
                rect.y += dy;
            }
            side.refresh(from);
        }
        if self.last_top.is_some() {
            let moved = self.left.rects[mark.left..]
                .iter()
                .chain(&self.right.rects[mark.right..])
                .map(|r| r.y)
                .fold(f32::NEG_INFINITY, f32::max);
            self.last_top = Some(match mark.last_top {
                Some(before) => before.max(moved),
                None => moved,
            });
        }
    }

    /// Bottom margin edge of the lowest float.
    pub(crate) fn max_bottom(&self) -> Option<f32> {
        match (self.left.bottom(), self.right.bottom()) {
            (Some(l), Some(r)) => Some(l.max(r)),
            (l, r) => l.or(r),
        }
    }

    /// The block offset below the floats `clear` names.
    pub(crate) fn clear_y(&mut self, clear: ClearSide) -> Option<f32> {
        self.activity += 1;
        match clear {
            ClearSide::None => None,
            ClearSide::Left => self.left.bottom(),
            ClearSide::Right => self.right.bottom(),
            ClearSide::Both => self.max_bottom(),
        }
    }

    /// The part of `[origin_x, origin_x + width)` the floats occupy at the
    /// block range `[y, y + h)`.
    pub(crate) fn band(&mut self, origin_x: f32, width: f32, y: f32, h: f32) -> Band {
        if !self.is_empty() {
            self.activity += 1;
        }
        self.band_of(origin_x, width, y, h)
    }

    fn band_of(&self, origin_x: f32, width: f32, y: f32, h: f32) -> Band {
        let mut band = Band::default();
        self.left.for_each_in(y, h, |r| {
            band.left = band.left.max(r.right() - origin_x);
        });
        self.right.for_each_in(y, h, |r| {
            band.right = band.right.max(origin_x + width - r.x);
        });
        band
    }

    /// The nearest bottom edge below `y` among the floats that touch the range
    /// `[y, y + h)` inside `[origin_x, origin_x + width)`.
    pub(crate) fn next_edge(&self, origin_x: f32, width: f32, y: f32, h: f32) -> Option<f32> {
        let mut edge: Option<f32> = None;
        let mut note = |bottom: f32| {
            if bottom > y {
                edge = Some(edge.map_or(bottom, |e| e.min(bottom)));
            }
        };
        self.left.for_each_in(y, h, |r| {
            if r.right() > origin_x {
                note(r.bottom());
            }
        });
        self.right.for_each_in(y, h, |r| {
            if r.x < origin_x + width {
                note(r.bottom());
            }
        });
        edge
    }

    /// Places a float whose margin box is `size` (width, height) on `side`
    /// inside `[origin_x, origin_x + width)`, no higher than `at_least`, and
    /// returns the margin box's top-left corner (rules 1 to 9).
    pub(crate) fn place(
        &mut self,
        side: FloatSide,
        (w, h): (f32, f32),
        at_least: f32,
        origin_x: f32,
        width: f32,
    ) -> (f32, f32) {
        self.activity += 1;
        let mut y = self.last_top.map_or(at_least, |top| top.max(at_least));
        let query = h.max(EPSILON);
        let x = loop {
            let band = self.band_of(origin_x, width, y, query);
            let free = band.left <= 0.0 && band.right <= 0.0;
            if free || w <= width - band.left - band.right + EPSILON {
                break match side {
                    FloatSide::Left => origin_x + band.left,
                    FloatSide::Right => origin_x + width - band.right - w,
                };
            }
            match self.next_edge(origin_x, width, y, query) {
                Some(next) => y = next,
                None => {
                    break match side {
                        FloatSide::Left => origin_x + band.left,
                        FloatSide::Right => origin_x + width - band.right - w,
                    };
                }
            }
        };
        self.last_top = Some(y);
        // A float with no width still reaches down: containment and `clear`
        // see it, while it takes no room beside it.
        if h > 0.0 {
            let rect = Rect::new(x, y, w, h);
            match side {
                FloatSide::Left => self.left.push(rect),
                FloatSide::Right => self.right.push(rect),
            }
        }
        (x, y)
    }
}

thread_local! {
    static CURRENT: RefCell<Floats> = RefCell::new(Floats::default());
}

/// Runs `f` on the current formatting context's floats.
pub(crate) fn with<R>(f: impl FnOnce(&mut Floats) -> R) -> R {
    CURRENT.with(|cell| f(&mut cell.borrow_mut()))
}

/// A block formatting context in progress: while it lives, the current float
/// set is its own. Dropping it, also when a panic unwinds through it, puts the
/// outer set back.
pub(crate) struct RootScope {
    outer: Option<Floats>,
}

/// Starts a block formatting context: the current set is replaced by an empty
/// one that the returned scope owns until it ends.
pub(crate) fn enter_root() -> RootScope {
    let outer = CURRENT.with(|cell| std::mem::take(&mut *cell.borrow_mut()));
    RootScope { outer: Some(outer) }
}

impl RootScope {
    /// Ends the context, restoring the outer set and returning the floats of
    /// the context that ended.
    pub(crate) fn leave(mut self) -> Floats {
        let outer = self.outer.take().unwrap_or_default();
        CURRENT.with(|cell| std::mem::replace(&mut *cell.borrow_mut(), outer))
    }
}

impl Drop for RootScope {
    fn drop(&mut self) {
        if let Some(outer) = self.outer.take() {
            CURRENT.with(|cell| *cell.borrow_mut() = outer);
        }
    }
}

thread_local! {
    /// Subtree layouts repeated because a child's position was mispredicted.
    static RELAYOUTS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Most repeated subtree layouts one `layout()` allows. A page that needs more
/// is adversarial; its floats are then shifted instead of laid out again.
const RELAYOUT_BUDGET: u32 = 4096;

/// Takes one repeated layout out of the budget; false when it is spent.
pub(crate) fn spend_relayout() -> bool {
    RELAYOUTS.with(|n| {
        let used = n.get();
        n.set(used.saturating_add(1));
        used < RELAYOUT_BUDGET
    })
}

/// Repeated subtree layouts since the last [`reset`].
#[cfg(test)]
pub(crate) fn relayouts() -> u32 {
    RELAYOUTS.with(std::cell::Cell::get)
}

/// Forgets all floats; a layout begins with no state from an earlier one.
pub(crate) fn reset() {
    reset_floats();
    RELAYOUTS.with(|n| n.set(0));
}

/// Forgets all floats, keeping the count of repeated layouts.
pub(crate) fn reset_floats() {
    with(|f| *f = Floats::default());
}

/// A content box seen from inside its formatting context: the
/// [`FloatContext`] that inline layout and block layout use. Offsets are
/// relative to the content box's top-left corner.
#[derive(Copy, Clone, Debug)]
pub(crate) struct FloatView {
    origin: (f32, f32),
    width: f32,
}

impl FloatView {
    /// The view of a content box `width` wide at `origin` in its context.
    pub(crate) fn new(origin: (f32, f32), width: f32) -> Self {
        Self { origin, width }
    }
}

impl FloatContext for FloatView {
    fn band(&self, block_offset: f32, block_size: f32) -> Band {
        let (ox, oy) = self.origin;
        with(|f| f.band(ox, self.width, oy + block_offset, block_size.max(EPSILON)))
    }

    fn place_float(&mut self, side: FloatSide, size: (f32, f32), at_least: f32) -> (f32, f32) {
        let (ox, oy) = self.origin;
        let (x, y) = with(|f| f.place(side, size, oy + at_least, ox, self.width));
        (x - ox, y - oy)
    }

    fn clear_offset(&self, clear: ClearSide) -> f32 {
        let oy = self.origin.1;
        with(|f| f.clear_y(clear)).map_or(0.0, |y| y - oy)
    }

    fn next_edge(&self, block_offset: f32, block_size: f32) -> Option<f32> {
        let (ox, oy) = self.origin;
        with(|f| f.next_edge(ox, self.width, oy + block_offset, block_size.max(EPSILON)))
            .map(|y| y - oy)
    }

    fn is_empty(&self) -> bool {
        with(|f| f.is_empty())
    }
}

/// The side a `float` value puts a box on inside a containing block of
/// `direction`, or `None` when it does not float.
pub(crate) fn float_side(style: &ComputedValues, direction: Direction) -> Option<FloatSide> {
    let start = match direction {
        Direction::Ltr => FloatSide::Left,
        Direction::Rtl => FloatSide::Right,
    };
    let end = match start {
        FloatSide::Left => FloatSide::Right,
        FloatSide::Right => FloatSide::Left,
    };
    match style.get_box().float {
        StyleFloat::None => None,
        StyleFloat::Left => Some(FloatSide::Left),
        StyleFloat::Right => Some(FloatSide::Right),
        StyleFloat::InlineStart => Some(start),
        StyleFloat::InlineEnd => Some(end),
    }
}

/// The floats a `clear` value skips below, in a containing block of
/// `direction`.
pub(crate) fn clear_side(style: &ComputedValues, direction: Direction) -> ClearSide {
    let (start, end) = match direction {
        Direction::Ltr => (ClearSide::Left, ClearSide::Right),
        Direction::Rtl => (ClearSide::Right, ClearSide::Left),
    };
    match style.get_box().clear {
        StyleClear::None => ClearSide::None,
        StyleClear::Left => ClearSide::Left,
        StyleClear::Right => ClearSide::Right,
        StyleClear::Both => ClearSide::Both,
        StyleClear::InlineStart => start,
        StyleClear::InlineEnd => end,
    }
}

/// True when box `id` really floats: `float` does not apply to the children
/// of flex and grid containers, which keep the flag for the box tree's sake.
pub(crate) fn is_float(tree: &BoxTree, id: BoxId) -> bool {
    let b = tree.get(id);
    b.flags.contains(BoxFlags::FLOAT)
        && (b.parent == BoxId::NONE
            || !matches!(tree.get(b.parent).kind, BoxKind::Flex | BoxKind::Grid))
}

/// A float that has been laid out but not yet placed.
#[derive(Copy, Clone, Debug)]
pub(crate) struct LaidFloat {
    pub(crate) frag: FragId,
    /// Margin-box width and height.
    pub(crate) size: (f32, f32),
    margin: Edges,
}

/// Lays out the float `id` in the containing block described by `space`.
/// Its fragment stays at its `position: relative` offset until
/// [`position_float`] places it.
pub(crate) fn lay_out_float(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    id: BoxId,
    space: &ConstraintSpace,
    out: &mut FragmentBuilder,
) -> LaidFloat {
    let (frag, _) = layout_atomic(cx, tree, id, space, out);
    let f = out.get(frag);
    LaidFloat {
        frag,
        size: (
            f.rect.w + f.margin.horizontal(),
            f.rect.h + f.margin.vertical(),
        ),
        margin: f.margin,
    }
}

/// Places a laid-out float no higher than `at_least` (and below the floats its
/// `clear` names) and moves its fragment there. Offsets are relative to the
/// content box `view` describes, and so is the fragment afterwards.
pub(crate) fn position_float(
    out: &mut FragmentBuilder,
    tree: &BoxTree,
    id: BoxId,
    laid: &LaidFloat,
    view: &mut dyn FloatContext,
    (side, direction): (FloatSide, Direction),
    at_least: f32,
) {
    let clear = clear_side(&tree.get(id).style, direction);
    let at_least = if clear == ClearSide::None {
        at_least
    } else {
        at_least.max(view.clear_offset(clear))
    };
    let (x, y) = view.place_float(side, laid.size, at_least);
    let f = out.get_mut(laid.frag);
    f.rect.x += x + laid.margin.left;
    f.rect.y += y + laid.margin.top;
    f.flags |= FragFlags::FLOAT | FragFlags::OUT_OF_FLOW;
}

/// Lays out the float `id` in the containing block described by `space` and
/// places it in the current formatting context.
///
/// `view` is the containing block's content box; `at_least` is the highest
/// block offset the float may take, relative to that box. The returned
/// fragment's rectangle is relative to the same content box: the caller adds
/// the content box's offset inside its own fragment.
#[allow(clippy::too_many_arguments)]
pub(crate) fn layout_float(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    id: BoxId,
    space: &ConstraintSpace,
    view: &mut FloatView,
    side: FloatSide,
    at_least: f32,
    out: &mut FragmentBuilder,
) -> FragId {
    let laid = lay_out_float(cx, tree, id, space, out);
    position_float(
        out,
        tree,
        id,
        &laid,
        view,
        (side, space.direction),
        at_least,
    );
    laid.frag
}

/// The margin-box width a formatting context root insists on before it moves
/// below the floats: what its own `width` (or replaced size) asks for. A box
/// with an automatic width takes whatever is left, so it never insists.
fn required_width(cx: &mut LayoutCx, tree: &BoxTree, id: BoxId, space: &ConstraintSpace) -> f32 {
    let b = tree.get(id);
    let definite = match space.inline_size {
        AvailableSize::Definite(w) => Some(w),
        _ => None,
    };
    let basis = space.percentage_base.0.or(definite);
    let margin = frame_of(tree, id, basis.unwrap_or(0.0)).margin;
    let sized = matches!(
        &b.style.get_position().width,
        Size::LengthPercentage(_)
            | Size::MaxContent
            | Size::MinContent
            | Size::FitContent
            | Size::FitContentFunction(_)
    );
    if b.kind == BoxKind::Table(TableKind::Table) {
        // A table cannot shrink below its min-content width.
        let (min, _) = crate::table::border_box_sizes(cx, tree, id);
        let sized_width = if sized {
            let roomy = ConstraintSpace {
                inline_size: AvailableSize::Definite(1.0e6),
                percentage_base: (basis, space.percentage_base.1),
                ..*space
            };
            resolve_box(cx, tree, id, &roomy).border_box_width()
        } else {
            0.0
        };
        return min.max(sized_width) + margin.left + margin.right;
    }
    if !sized && !matches!(b.kind, BoxKind::Replaced(_)) {
        return 0.0;
    }
    // Room enough that only the box's own request decides the width.
    let roomy = ConstraintSpace {
        inline_size: AvailableSize::Definite(1.0e6),
        percentage_base: (basis, space.percentage_base.1),
        ..*space
    };
    resolve_box(cx, tree, id, &roomy).border_box_width() + margin.left + margin.right
}

/// Where a formatting context root can sit beside the floats of the current
/// context, starting at block offset `y` (from the parent's content-box top,
/// which is `top` in the context).
pub(crate) struct Opening {
    /// Block offset of the box's top.
    pub(crate) y: f32,
    /// Offset of the margin box's left edge from the content box's left edge.
    pub(crate) shift: f32,
    /// Width available to the margin box there.
    pub(crate) available: f32,
}

/// Finds the [`Opening`] for the formatting context root `id`. The border box
/// must clear the floats but the margins may overlap them, so the room grows
/// by the margins on the sides that have a float. A box whose width does not
/// fit moves below the nearest float, repeatedly.
pub(crate) fn avoid(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    id: BoxId,
    space: &ConstraintSpace,
    (origin_x, width): (f32, f32),
    top: f32,
    mut y: f32,
) -> Opening {
    // The box's height is not known yet, so the room is what remains over the
    // whole rest of the context, like Blink's layout opportunity.
    const REST: f32 = 1.0e9;
    let needed = required_width(cx, tree, id, space);
    let basis = space.percentage_base.0.unwrap_or(width);
    let margin = frame_of(tree, id, basis).margin;
    loop {
        let band = with(|f| f.band(origin_x, width, top + y, REST));
        let overlap_left = margin.left.max(0.0).min(band.left);
        let overlap_right = margin.right.max(0.0).min(band.right);
        let available = (width - band.left - band.right + overlap_left + overlap_right).max(0.0);
        let opening = Opening {
            y,
            shift: band.left - overlap_left,
            available,
        };
        if (band.left <= 0.0 && band.right <= 0.0) || needed <= available + EPSILON {
            return opening;
        }
        match with(|f| f.next_edge(origin_x, width, top + y, REST)) {
            Some(next) => y = next - top,
            None => return opening,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f32 = 300.0;

    fn place(f: &mut Floats, side: FloatSide, w: f32, h: f32, at: f32) -> (f32, f32) {
        f.place(side, (w, h), at, 0.0, W)
    }

    #[test]
    fn left_floats_sit_side_by_side_then_wrap_below_the_shortest_blocker() {
        let mut f = Floats::default();
        assert_eq!(place(&mut f, FloatSide::Left, 100.0, 50.0, 0.0), (0.0, 0.0));
        assert_eq!(
            place(&mut f, FloatSide::Left, 100.0, 80.0, 0.0),
            (100.0, 0.0)
        );
        // No room on the first row, nor below the shorter float while the
        // taller one remains: it goes down until it fits, as far left as it can.
        assert_eq!(
            place(&mut f, FloatSide::Left, 150.0, 10.0, 0.0),
            (0.0, 80.0)
        );
        assert_eq!(
            place(&mut f, FloatSide::Left, 90.0, 10.0, 0.0),
            (150.0, 80.0)
        );
    }

    #[test]
    fn right_floats_hug_the_right_edge_and_opposing_floats_share_a_row() {
        let mut f = Floats::default();
        assert_eq!(
            place(&mut f, FloatSide::Right, 100.0, 50.0, 0.0),
            (200.0, 0.0)
        );
        assert_eq!(place(&mut f, FloatSide::Left, 120.0, 20.0, 0.0), (0.0, 0.0));
        // 300 - 100 - 120 = 80 left; 90 does not fit beside both, so it waits for
        // the left float to end and then hugs the right float.
        assert_eq!(
            place(&mut f, FloatSide::Right, 90.0, 10.0, 0.0),
            (110.0, 20.0)
        );
    }

    #[test]
    fn a_float_is_never_higher_than_an_earlier_one() {
        let mut f = Floats::default();
        place(&mut f, FloatSide::Left, 280.0, 30.0, 0.0);
        let (_, y) = place(&mut f, FloatSide::Right, 100.0, 10.0, 0.0);
        assert_eq!(y, 30.0);
        // Free space beside the first row is not reused by a later float.
        let (_, y) = place(&mut f, FloatSide::Left, 10.0, 10.0, 0.0);
        assert!(y >= 30.0);
    }

    #[test]
    fn a_float_wider_than_the_context_is_placed_at_its_edge() {
        let mut f = Floats::default();
        assert_eq!(place(&mut f, FloatSide::Left, 500.0, 10.0, 0.0), (0.0, 0.0));
        assert_eq!(
            place(&mut f, FloatSide::Right, 20.0, 10.0, 0.0),
            (280.0, 10.0)
        );
    }

    #[test]
    fn band_reports_the_occupied_edges_for_a_block_range() {
        let mut f = Floats::default();
        place(&mut f, FloatSide::Left, 100.0, 50.0, 0.0);
        place(&mut f, FloatSide::Right, 60.0, 20.0, 0.0);
        assert_eq!(
            f.band(0.0, W, 0.0, 10.0),
            Band {
                left: 100.0,
                right: 60.0
            }
        );
        assert_eq!(
            f.band(0.0, W, 30.0, 10.0),
            Band {
                left: 100.0,
                right: 0.0
            }
        );
        assert_eq!(f.band(0.0, W, 50.0, 10.0), Band::default());
        // A range that only touches the bottom edge is free.
        assert_eq!(f.band(0.0, W, 20.0, 5.0).right, 0.0);
        // Seen from a narrower, indented content box.
        assert_eq!(
            f.band(40.0, 200.0, 0.0, 10.0),
            Band {
                left: 60.0,
                right: 0.0
            }
        );
    }

    #[test]
    fn next_edge_is_the_nearest_bottom_below() {
        let mut f = Floats::default();
        place(&mut f, FloatSide::Left, 100.0, 50.0, 0.0);
        place(&mut f, FloatSide::Right, 100.0, 20.0, 0.0);
        assert_eq!(f.next_edge(0.0, W, 0.0, 10.0), Some(20.0));
        assert_eq!(f.next_edge(0.0, W, 20.0, 10.0), Some(50.0));
        assert_eq!(f.next_edge(0.0, W, 50.0, 10.0), None);
    }

    #[test]
    fn clear_names_the_lowest_bottom_of_the_cleared_sides() {
        let mut f = Floats::default();
        assert_eq!(f.clear_y(ClearSide::Both), None);
        place(&mut f, FloatSide::Left, 10.0, 40.0, 0.0);
        place(&mut f, FloatSide::Right, 10.0, 70.0, 0.0);
        assert_eq!(f.clear_y(ClearSide::Left), Some(40.0));
        assert_eq!(f.clear_y(ClearSide::Right), Some(70.0));
        assert_eq!(f.clear_y(ClearSide::Both), Some(70.0));
        assert_eq!(f.clear_y(ClearSide::None), None);
    }

    #[test]
    fn truncate_and_shift_undo_and_move_only_what_came_after_the_mark() {
        let mut f = Floats::default();
        place(&mut f, FloatSide::Left, 100.0, 20.0, 0.0);
        let mark = f.mark();
        place(&mut f, FloatSide::Left, 100.0, 20.0, 30.0);
        f.shift_since(&mark, 5.0);
        assert_eq!(f.band(0.0, W, 36.0, 1.0).left, 100.0);
        assert_eq!(f.band(0.0, W, 20.0, 1.0).left, 0.0);
        assert_eq!(f.max_bottom(), Some(55.0));
        f.truncate(&mark);
        assert_eq!(f.max_bottom(), Some(20.0));
        assert_eq!(
            place(&mut f, FloatSide::Left, 100.0, 10.0, 0.0),
            (100.0, 0.0)
        );
    }

    #[test]
    fn five_hundred_floats_in_a_column_stay_cheap_to_query() {
        let mut f = Floats::default();
        for i in 0..500 {
            let (x, y) = place(&mut f, FloatSide::Left, 280.0, 10.0, 0.0);
            assert_eq!((x, y), (0.0, 10.0 * i as f32));
        }
        // Only the newest floats intersect a range near the end.
        assert_eq!(f.band(0.0, W, 4990.0, 5.0).left, 280.0);
        assert_eq!(f.band(0.0, W, 5000.0, 5.0).left, 0.0);
        assert_eq!(f.next_edge(0.0, W, 4985.0, 1.0), Some(4990.0));
    }

    #[test]
    fn a_zero_sized_float_takes_no_room_but_still_orders_later_floats() {
        let mut f = Floats::default();
        place(&mut f, FloatSide::Left, 100.0, 0.0, 40.0);
        assert!(f.is_empty());
        let (_, y) = place(&mut f, FloatSide::Left, 10.0, 10.0, 0.0);
        assert_eq!(y, 40.0);
    }

    /// The page that made a prediction miss at every level: a tall float, then
    /// nested blocks whose first child is a margined formatting context with a
    /// float inside.
    fn adversarial_page(depth: usize) -> String {
        let mut html = String::from(
            "<!doctype html><style>body{margin:0}.f{float:left;width:10px;height:10px}</style><body>\
             <div style='float:left;width:50px;height:400px'></div><div style='height:1px'></div>",
        );
        for _ in 0..depth {
            html.push_str("<div><div style='overflow:hidden;margin-top:6px'><div class=f></div>");
        }
        html.push_str("<p>text</p>");
        for _ in 0..depth {
            html.push_str("</div></div>");
        }
        html
    }

    #[test]
    fn nested_mispredictions_do_not_multiply_layouts() {
        use crate::{BoxTree, FontSystem, LayoutCx, NoImages, layout};
        use tenta_css::cascade::StyleEnv;
        use tenta_css::cascade::test_util::style_html;

        let depth = 24;
        let (doc, engine) = style_html(&adversarial_page(depth), StyleEnv::default());
        let mut fonts = FontSystem::new_embedded();
        let mut cx = LayoutCx {
            doc: &doc,
            styles: &engine,
            fonts: &mut fonts,
            images: &NoImages,
            viewport: (800.0, 600.0),
            quirks: false,
        };
        let tree = BoxTree::build(&cx);
        let start = std::time::Instant::now();
        let frags = layout(&mut cx, &tree);
        assert!(!frags.is_empty());
        let elapsed = start.elapsed();
        assert!(elapsed.as_secs() < 3, "layout took {elapsed:?}");
        assert!(super::relayouts() <= depth as u32, "{}", super::relayouts());
    }
}
