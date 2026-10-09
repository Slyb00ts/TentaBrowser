//! Block formatting context layout.
//!
//! A block lays out its in-flow block children top to bottom and its inline
//! formatting context through [`layout_inline`]. Vertical margins collapse
//! while the children are stacked (CSS 2.1 section 8.3.1): each child reports
//! the margins that adjoin its top and bottom edges as [`MarginStrut`]s, and
//! the parent decides where the child goes and what flows out of its own top
//! and bottom edge.

use smallvec::SmallVec;

use crate::box_tree::{BoxFlags, BoxId, BoxKind, BoxTree, LayoutCx};
use crate::constraint;
use crate::fragment::{FragFlags, FragId, Fragment, FragmentBuilder, FragmentKind};
use crate::inline::layout_inline;
use crate::space::{AvailableSize, BlockResult, ConstraintSpace, MarginStrut, NoFloats};
use crate::types::{Edges, OverflowKind, PositionKind, Rect};
use crate::{block_sizes, container, list, positioned, quirks, relative};

/// Lays out the block box `id` within `space`, pushing its fragment (and its
/// descendants') into `out`.
///
/// `space` describes the containing block. The returned fragment's `rect.x` is
/// the box's offset from the containing block's content-box left edge (its
/// used left margin) and `rect.y` is only the `position: relative` offset; the
/// caller adds the in-flow position and the containing block's content origin.
/// Floats are skipped until float layout exists. Absolutely positioned children
/// get no fragment here: their static position is recorded on this box's
/// fragment and [`positioned::place_all`] lays them out against their
/// containing block once the whole tree is known.
pub fn layout_block(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    id: BoxId,
    space: &ConstraintSpace,
    out: &mut FragmentBuilder,
) -> BlockResult {
    let b = tree.get(id);
    debug_assert!(
        b.ifc.is_none()
            || b.children
                .iter()
                .all(|&c| tree.get(c).flags.contains(BoxFlags::OUT_OF_FLOW)),
        "a block holds in-flow block children or an inline formatting context, not both"
    );
    if matches!(b.kind, BoxKind::Flex | BoxKind::Grid) {
        return container::layout_container(cx, tree, id, space, out);
    }
    let m = constraint::resolve_box(cx, tree, id, space);
    let direction = constraint::own_direction(&b.style);
    let establishes_bfc = b.flags.contains(BoxFlags::ESTABLISHES_BFC);
    let replaced = matches!(b.kind, BoxKind::Replaced(_));
    let constrained = !space.content_block_size && constraint::has_block_size_constraints(&b.style);

    let definite_height = match m.content_height {
        Some(h) if constrained => Some(block_sizes::constrain_block_size(cx, tree, id, space, h)),
        other => other,
    };
    let content_origin = Edges {
        left: m.border.left + m.padding.left,
        top: m.border.top + m.padding.top,
        ..Edges::default()
    };
    let bfc_origin = if establishes_bfc {
        (0.0, 0.0)
    } else {
        (
            space.bfc_offset.0 + m.margin.left + content_origin.left,
            space.bfc_offset.1 + m.margin.top + content_origin.top,
        )
    };
    let child_space = constraint::child_space(
        space,
        m.content_width,
        definite_height,
        direction,
        bfc_origin,
    );

    let vertical_clear = m.border.vertical() == 0.0 && m.padding.vertical() == 0.0;
    let mut flow = Flow::new(!establishes_bfc && m.border.top == 0.0 && m.padding.top == 0.0);
    let mut kids: SmallVec<[FragId; 4]> = SmallVec::new();
    let mut overflow = Rect::default();
    let marker = match &b.kind {
        BoxKind::ListItem { marker } => *marker,
        _ => None,
    };
    let has_marker = marker.is_some();

    let mut has_lines = false;
    let mut statics: SmallVec<[positioned::Candidate; 2]> = SmallVec::new();
    if !replaced {
        for &child in &b.children {
            let flags = tree.get(child).flags;
            if flags.contains(BoxFlags::OUT_OF_FLOW) {
                if flags.contains(BoxFlags::ABSPOS) && b.ifc.is_none() {
                    statics.push(positioned::flow_candidate(
                        child,
                        (content_origin.left, content_origin.top),
                        flow.cursor + flow.pending.collapsed(),
                        m.content_width,
                        direction,
                        kids.len(),
                    ));
                }
                continue;
            }
            let mut space_for_child = child_space;
            space_for_child.bfc_offset.1 += flow.cursor + flow.pending.collapsed();
            let result = layout_block(cx, tree, child, &space_for_child, out);
            let (height, first, last) = {
                let f = out.get(result.frag);
                (f.rect.h, result.first_baseline, result.last_baseline)
            };
            let y = if result.self_collapsing {
                flow.pass_through(&result.margin_before, &result.margin_after)
            } else {
                flow.place(
                    &result.margin_before,
                    &result.margin_after,
                    height,
                    first,
                    last,
                )
            };
            let frag = out.get_mut(result.frag);
            frag.rect.x += content_origin.left;
            frag.rect.y += content_origin.top + y;
            kids.push(result.frag);
        }
        if let Some(ifc) = &b.ifc {
            let mut floats = NoFloats;
            let inline = layout_inline(cx, tree, ifc, &child_space, &mut floats, out);
            has_lines = !inline.lines.is_empty();
            for &line in &inline.lines {
                let f = out.get_mut(line);
                f.rect.x += content_origin.left;
                f.rect.y += content_origin.top;
            }
            let first_line = kids.len();
            kids.extend(inline.lines.iter().copied());
            let marks = out.take_inline_statics(id);
            if !marks.is_empty() {
                let containing = positioned::inline_containing_blocks(tree, ifc);
                for mark in marks {
                    statics.push(positioned::inline_candidate(
                        &mark,
                        (content_origin.left, content_origin.top),
                        first_line + mark.line + 1,
                        containing.get(&mark.box_id).copied(),
                    ));
                }
            }
            overflow = overflow.union(
                &inline
                    .overflow
                    .translated(content_origin.left, content_origin.top),
            );
            if has_lines {
                flow.place_lines(&inline);
            }
        }
    }
    flow.settle_top();
    if has_marker && !has_lines {
        // An item with a marker is at least one line tall, even when empty.
        flow.cursor = flow.cursor.max(list::strut_height(cx, &b.style));
    }
    if let Some(marker) = marker {
        // The marker follows the first line, so it waits for the content.
        let item = list::ItemGeometry {
            width: m.border_box_width(),
            content_top: content_origin.top,
            first_baseline: flow.first_baseline.map(|y| y + content_origin.top),
        };
        let frag = list::layout_marker(cx, tree, marker, item, out);
        kids.insert(0, frag);
    }

    // The last child's bottom margin may escape through an auto-height box,
    // but only if min-height/max-height leave the content height alone.
    let may_collapse_bottom = !establishes_bfc
        && m.border.bottom == 0.0
        && m.padding.bottom == 0.0
        && m.content_height.is_none();
    let auto_height = |cx: &mut LayoutCx, flow_height: f32| {
        let h = if cx.quirks {
            quirks::fill_viewport_height(cx, tree, id, flow_height)
        } else {
            flow_height
        };
        if constrained {
            block_sizes::constrain_block_size(cx, tree, id, space, h)
        } else {
            h
        }
    };
    let (collapses_bottom, content_height) = match definite_height {
        Some(h) => (false, h),
        None => {
            if may_collapse_bottom {
                // A clamp that changes the height drops the pending margin.
                let tight = auto_height(cx, flow.cursor);
                (tight == flow.cursor, tight)
            } else {
                (
                    false,
                    auto_height(cx, flow.cursor + flow.pending.collapsed()),
                )
            }
        }
    };

    let mut margin_before = flow.before;
    margin_before.append(m.margin.top);
    let mut margin_after = MarginStrut::default();
    if collapses_bottom {
        margin_after.merge(&flow.pending);
    }
    margin_after.append(m.margin.bottom);
    // A BFC root keeps its top and bottom margins apart even when empty.
    let self_collapsing = vertical_clear
        && !establishes_bfc
        && !replaced
        && !has_marker
        && !flow.placed
        && !has_lines
        && content_height == 0.0
        && constraint::min_height_is_zero(&b.style, || {
            quirks::percentage_limit_base(cx, tree, id, space)
        });

    let border_box = Rect::new(
        0.0,
        0.0,
        m.border_box_width(),
        m.border_box_height(content_height),
    );
    let (dx, dy) = relative_shift(tree, id, space);
    let kind = match b.kind {
        BoxKind::Replaced(kind) => FragmentKind::Replaced(kind),
        _ => FragmentKind::Box,
    };
    let rect = Rect::new(m.margin.left + dx, dy, border_box.w, border_box.h);
    let mut fragment = Fragment::new(b.node, kind, tree.paint_style_for(id, border_box), rect);
    fragment.border = m.border;
    fragment.padding = m.padding;
    fragment.margin = m.margin;
    fragment.flags = fragment_flags(cx, tree, id);
    fragment.overflow = child_overflow(out, &kids, overflow.union(&border_box));
    fragment.children = kids;
    let frag = out.push(fragment);
    for mut candidate in statics {
        candidate.at += usize::from(has_marker);
        out.add_candidate(frag, candidate);
    }

    BlockResult {
        frag,
        margin_before,
        margin_after,
        first_baseline: flow.first_baseline.map(|y| y + content_origin.top),
        last_baseline: flow.last_baseline.map(|y| y + content_origin.top),
        floats_out: Vec::new(),
        exclusions: Vec::new(),
        self_collapsing,
    }
}

/// The `position: relative` offset of box `id` inside the containing block
/// described by `space`.
pub(crate) fn relative_shift(tree: &BoxTree, id: BoxId, space: &ConstraintSpace) -> (f32, f32) {
    if tree.get(id).paint.position != PositionKind::Relative {
        return (0.0, 0.0);
    }
    let containing_width = space.percentage_base.0.unwrap_or(match space.inline_size {
        AvailableSize::Definite(w) => w,
        _ => 0.0,
    });
    relative::relative_offset_in(tree, id, (containing_width, space.percentage_base.1))
}

/// `own` grown by the scrollable overflow of the child fragments `kids`; a child
/// that clips contributes only its border box.
pub(crate) fn child_overflow(out: &FragmentBuilder, kids: &[FragId], own: Rect) -> Rect {
    let mut overflow = own;
    for &kid in kids {
        let k = out.get(kid);
        let contribution = if k.flags.contains(FragFlags::CLIPS_OVERFLOW) {
            k.rect
        } else {
            k.overflow.translated(k.rect.x, k.rect.y)
        };
        overflow = overflow.union(&contribution);
    }
    overflow
}

/// The fragment flags that follow from the box alone.
pub(crate) fn fragment_flags(cx: &LayoutCx, tree: &BoxTree, id: BoxId) -> FragFlags {
    let b = tree.get(id);
    let mut flags = FragFlags::empty();
    if b.flags.contains(BoxFlags::ESTABLISHES_BFC) {
        flags |= FragFlags::ESTABLISHES_BFC;
    }
    let clips = |o: OverflowKind| o != OverflowKind::Visible;
    if (clips(b.paint.overflow_x) || clips(b.paint.overflow_y))
        && !overflow_goes_to_viewport(cx, tree, id)
    {
        flags |= FragFlags::CLIPS_OVERFLOW;
    }
    // Items of a flex or grid container paint atomically only when that
    // container really laid them out as items, so the container's own layout
    // sets the flag. A container still laid out as a block stacks its children
    // in normal flow, and painting them atomically would put them above the
    // backgrounds of later blocks, which no flow layout does.
    if tree.is_paint_atomic(id) && !tree.is_flex_grid_item(id) {
        flags |= FragFlags::PAINT_ATOMIC;
    }
    // `z-index` applies to flex and grid items although they are not
    // positioned, and then makes them stacking contexts.
    if b.paint.z_index.is_some() && tree.is_flex_grid_item(id) {
        flags |= FragFlags::STACKING_CONTEXT;
    }
    flags | crate::positioned::containing_block_flags(&b.style)
}

/// CSS Overflow 3 section 3.3: when the root's overflow is `visible`, the
/// body's overflow applies to the viewport and the body itself does not clip.
fn overflow_goes_to_viewport(cx: &LayoutCx, tree: &BoxTree, id: BoxId) -> bool {
    let b = tree.get(id);
    if b.parent == BoxId::NONE || tree.canvas_source(cx.doc) != Some(id) {
        return false;
    }
    let root = &tree.get(b.parent).paint;
    root.overflow_x == OverflowKind::Visible && root.overflow_y == OverflowKind::Visible
}

/// Vertical stacking state of one block's content box.
struct Flow {
    /// Bottom of the last placed in-flow content, from the content-box top.
    cursor: f32,
    /// Margins adjoining the cursor that no box has been placed after yet.
    pending: MarginStrut,
    /// Margins adjoining the block's own top edge: those that collapsed
    /// through its first children.
    before: MarginStrut,
    /// The block's top margin can still collapse with what comes next.
    top_open: bool,
    /// Nothing has been placed yet.
    placed: bool,
    first_baseline: Option<f32>,
    last_baseline: Option<f32>,
}

impl Flow {
    /// `collapses_top` is true when nothing separates the block's top margin
    /// from its first child's (no border, padding, clearance or BFC).
    fn new(collapses_top: bool) -> Self {
        Self {
            cursor: 0.0,
            pending: MarginStrut::default(),
            before: MarginStrut::default(),
            top_open: collapses_top,
            placed: false,
            first_baseline: None,
            last_baseline: None,
        }
    }

    /// Places a child that has a height of its own and returns its top edge.
    fn place(
        &mut self,
        child_before: &MarginStrut,
        child_after: &MarginStrut,
        height: f32,
        first_baseline: Option<f32>,
        last_baseline: Option<f32>,
    ) -> f32 {
        let mut strut = self.pending;
        strut.merge(child_before);
        let y = if self.top_open {
            self.before.merge(&strut);
            self.cursor
        } else {
            self.cursor + strut.collapsed()
        };
        self.top_open = false;
        self.placed = true;
        self.first_baseline = self.first_baseline.or(first_baseline.map(|b| y + b));
        self.last_baseline = last_baseline.map(|b| y + b).or(self.last_baseline);
        self.cursor = y + height;
        self.pending = *child_after;
        y
    }

    /// Absorbs the margins of an empty child, which collapse with everything
    /// around them, and returns where its (zero-height) box sits.
    fn pass_through(&mut self, child_before: &MarginStrut, child_after: &MarginStrut) -> f32 {
        let y = if self.top_open {
            self.cursor
        } else {
            let mut strut = self.pending;
            strut.merge(child_before);
            self.cursor + strut.collapsed()
        };
        self.pending.merge(child_before);
        self.pending.merge(child_after);
        y
    }

    /// Records the lines of the block's inline formatting context, which start
    /// at the cursor.
    fn place_lines(&mut self, inline: &crate::space::InlineResult) {
        self.top_open = false;
        self.placed = true;
        self.first_baseline = inline.first_baseline;
        self.last_baseline = inline.last_baseline;
        self.cursor += inline.block_size;
    }

    /// Hands margins that collapsed through every child to the block's own top
    /// margin when it is still open.
    fn settle_top(&mut self) {
        if self.top_open {
            let pending = std::mem::take(&mut self.pending);
            self.before.merge(&pending);
        }
    }
}
