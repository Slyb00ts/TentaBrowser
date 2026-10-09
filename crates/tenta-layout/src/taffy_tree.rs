//! The Taffy view of the box tree, shared by flex and grid layout.
//!
//! Taffy runs one container algorithm at a time and sees its tree only through
//! [`LayoutPartialTree`]. A [`TreeView`] presents one container and its in-flow
//! children as such a tree: node ids are box ids, children are ordered by the
//! `order` property, and every child is measured by calling back into this
//! crate's own block, inline and replaced layout.
//!
//! Measuring never produces fragments that outlive the question: Taffy asks
//! for sizes and baselines many times per item with different constraints, so
//! each answer is laid out into a scratch builder. Fragments are produced once
//! afterwards, from the final item layouts, by [`TreeView::emit_items`].

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::iter::Copied;
use std::slice::Iter;

use smallvec::SmallVec;
use style::Atom;
use taffy::{
    AvailableSpace, Cache, CacheTree, Layout, LayoutInput, LayoutOutput, LayoutPartialTree, Line,
    NodeId, RequestedAxis, ResolveOrZero, RunMode, Size, SizingMode, Style, TraversePartialTree,
    compute_cached_layout, compute_flexbox_layout, compute_grid_layout,
};

use crate::block::layout_block;
use crate::block_sizes::{
    self, ReplacedHints, SizeLimits, intrinsic_inline_sizes_of_block, natural_ratio,
    replaced_content_size,
};
use crate::box_tree::{BoxFlags, BoxId, BoxKind, BoxTree, LayoutBox, LayoutCx};
use crate::constraint::own_direction;
use crate::fragment::{FragFlags, FragId, FragmentBuilder};
use crate::space::{AvailableSize, ConstraintSpace};
use crate::style_taffy::{
    ConvertedStyle, block_alignment_is_normal, inline_alignment_is_normal, preferred_aspect_ratio,
    resolve_calc, to_taffy_style,
};
use crate::types::{Direction, Edges, OverflowKind, PositionKind};

/// Whether the box is a scroll container, whose automatic minimum size is zero.
fn scrolls(b: &LayoutBox) -> bool {
    let scrolling = |o: OverflowKind| !matches!(o, OverflowKind::Visible | OverflowKind::Clip);
    scrolling(b.paint.overflow_x) || scrolling(b.paint.overflow_y)
}

/// What a scratch layout of an item answered: border-box height and first
/// baseline.
type Measured = (f32, Option<f32>);

/// Every input that `item_space` varies must be part of this key, or a memo hit
/// would return the answer for a different question.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct MeasureKey {
    id: BoxId,
    width: u32,
    percent_base: (Option<u32>, Option<u32>),
    rtl: bool,
}

thread_local! {
    /// Scratch-layout answers of the outermost flex layout in progress.
    ///
    /// A block item that contains flex containers is measured by laying it out,
    /// and its fragments are made by laying it out again, each time descending
    /// through the nested containers. Without a memo that doubles the work per
    /// level of nesting; with one, a repeated question costs a lookup. The memo
    /// lives only while a layout call is on the stack, so it can never outlive
    /// the tree, the fonts or the viewport its answers came from.
    static MEASURED: RefCell<HashMap<MeasureKey, Measured>> = RefCell::new(HashMap::new());
    static ACTIVE_VIEWS: Cell<u32> = const { Cell::new(0) };
}

/// Keeps the thread's [`MEASURED`] memo alive while a view exists; the last
/// view to go clears it.
struct MemoScope;

impl MemoScope {
    fn enter() -> Self {
        ACTIVE_VIEWS.with(|n| n.set(n.get() + 1));
        MemoScope
    }
}

impl Drop for MemoScope {
    fn drop(&mut self) {
        let remaining = ACTIVE_VIEWS.with(|n| {
            n.set(n.get() - 1);
            n.get()
        });
        if remaining == 0 {
            MEASURED.with(|memo| memo.borrow_mut().clear());
        }
    }
}

pub(crate) fn node_id(id: BoxId) -> NodeId {
    NodeId::from(u64::from(id.0))
}

fn box_id(node: NodeId) -> BoxId {
    BoxId(u64::from(node) as u32)
}

/// One container and its in-flow children, as Taffy sees them.
pub(crate) struct TreeView<'a, 'c> {
    pub(crate) cx: &'a mut LayoutCx<'c>,
    tree: &'a BoxTree,
    /// The container Taffy was asked to lay out; only its direct children's
    /// layouts are kept for fragment emission.
    root: BoxId,
    direction: Direction,
    /// Converted styles. They borrow the computed values stored in `tree`, which
    /// outlives the view, so every `calc()` handle Taffy holds stays valid for
    /// as long as it can be resolved.
    styles: HashMap<BoxId, ConvertedStyle<'a>>,
    /// In-flow children of each container that has been prepared, in `order`.
    children: HashMap<BoxId, Vec<NodeId>>,
    caches: HashMap<BoxId, Cache>,
    layouts: HashMap<BoxId, Layout>,
    /// First baselines of the root's children, taken from their final layouts.
    baselines: HashMap<BoxId, Option<f32>>,
    /// The grid area each of a grid root's children was last laid out in, the
    /// containing block its percentages resolve against.
    areas: HashMap<BoxId, Size<Option<f32>>>,
    /// The content width of the root container, which a ratio-only image fills
    /// when Taffy measures it before any width is known (a flex base size).
    inner_width: Option<f32>,
    _memo: MemoScope,
}

impl<'a, 'c> TreeView<'a, 'c> {
    pub(crate) fn new(cx: &'a mut LayoutCx<'c>, tree: &'a BoxTree, root: BoxId) -> Self {
        let mut view = Self {
            cx,
            tree,
            root,
            direction: own_direction(&tree.get(root).style),
            styles: HashMap::new(),
            children: HashMap::new(),
            caches: HashMap::new(),
            layouts: HashMap::new(),
            baselines: HashMap::new(),
            areas: HashMap::new(),
            inner_width: None,
            _memo: MemoScope::enter(),
        };
        view.convert(root);
        view.prepare(root);
        view
    }

    /// Records the content width of the root container.
    pub(crate) fn set_inner_width(&mut self, width: f32) {
        self.inner_width = Some(width);
    }

    fn convert(&mut self, id: BoxId) {
        if self.styles.contains_key(&id) {
            return;
        }
        let tree: &'a BoxTree = self.tree;
        let b = tree.get(id);
        let mut converted = to_taffy_style(&b.style)
            .replaced(matches!(b.kind, BoxKind::Replaced(_)))
            .without_aspect_ratio();
        if b.paint.position != PositionKind::Relative {
            converted = converted.without_insets();
        }
        self.styles.insert(id, converted);
    }

    /// Converts the styles of `container`'s in-flow children and orders them.
    /// `float` does not apply to flex and grid items, so floats take part like
    /// any other child; positioned children wait for positioned layout.
    pub(crate) fn prepare(&mut self, container: BoxId) {
        if self.children.contains_key(&container) {
            return;
        }
        let tree: &'a BoxTree = self.tree;
        let mut items: SmallVec<[BoxId; 16]> = tree
            .get(container)
            .children
            .iter()
            .copied()
            .filter(|&c| !tree.get(c).flags.contains(BoxFlags::ABSPOS))
            .collect();
        // Stable, so equal `order` values keep document order.
        items.sort_by_key(|&c| tree.get(c).style.get_position().order);
        for &item in &items {
            self.convert(item);
        }
        if matches!(tree.get(container).kind, BoxKind::Grid) {
            let container_style = &tree.get(container).style;
            for &item in &items {
                let b = tree.get(item);
                let style = &b.style;
                // `normal` behaves as `start` for items with a preferred aspect
                // ratio, which Taffy no longer sees on the item style. A
                // replaced item with a natural ratio (an image) has one on both
                // axes.
                let natural = matches!(b.kind, BoxKind::Replaced(kind)
                    if natural_ratio(self.cx, b.node, kind).is_some());
                let ratio = natural || preferred_aspect_ratio(style).is_some_and(|r| r > 0.0);
                if ratio && block_alignment_is_normal(style, container_style) {
                    let converted = self.styles.remove(&item).expect("converted above");
                    self.styles.insert(item, converted.start_aligned());
                }
                if natural && inline_alignment_is_normal(style, container_style) {
                    let converted = self.styles.remove(&item).expect("converted above");
                    self.styles.insert(item, converted.start_aligned_inline());
                }
            }
        }
        self.children
            .insert(container, items.into_iter().map(node_id).collect());
    }

    pub(crate) fn style(&self, id: BoxId) -> &Style<Atom> {
        self.styles[&id].style()
    }

    /// The style of a node Taffy asked about.
    pub(crate) fn style_of_node(&self, node: NodeId) -> &Style<Atom> {
        self.style(box_id(node))
    }

    /// The constraint space a box laid out as an item gets: its border-box size
    /// is decided, and percentages resolve against `percent_base`.
    fn item_space(
        &self,
        width: f32,
        height: Option<f32>,
        percent_base: Size<Option<f32>>,
    ) -> ConstraintSpace {
        let mut space = ConstraintSpace::new(
            AvailableSize::Definite(width),
            height.map_or(AvailableSize::MaxContent, AvailableSize::Definite),
        );
        space.percentage_base = (percent_base.width, percent_base.height);
        space.fixed_inline = true;
        space.fixed_block = height.is_some();
        space.content_block_size = height.is_none();
        space.direction = self.direction;
        space
    }

    /// Lays `id` out at a decided width and natural height into a scratch
    /// builder and returns its border-box height and first baseline.
    ///
    /// Sizes the container algorithm decided are applied to the result by the
    /// caller; a baseline does not depend on the height an item is stretched
    /// to, so one measurement serves every question about this width.
    fn measure_by_layout(
        &mut self,
        id: BoxId,
        width: f32,
        percent_base: Size<Option<f32>>,
    ) -> Measured {
        let bits = f32::to_bits;
        let key = MeasureKey {
            id,
            width: bits(width),
            percent_base: (percent_base.width.map(bits), percent_base.height.map(bits)),
            rtl: self.direction == Direction::Rtl,
        };
        if let Some(measured) = MEASURED.with(|memo| memo.borrow().get(&key).copied()) {
            return measured;
        }
        let space = self.item_space(width, None, percent_base);
        let mut scratch = FragmentBuilder::new();
        let result = layout_block(self.cx, self.tree, id, &space, &mut scratch);
        let measured = (scratch.get(result.frag).rect.h, result.first_baseline);
        MEASURED.with(|memo| memo.borrow_mut().insert(key, measured));
        measured
    }

    /// Margins, and padding plus border per axis, resolved against the width of
    /// the containing block.
    fn frame(&self, id: BoxId, basis: Option<f32>) -> (taffy::Rect<f32>, Size<f32>) {
        let style = self.style(id);
        let calc = |handle, basis| self.resolve_calc_value(handle, basis);
        let margin = style.margin.resolve_or_zero(basis, calc);
        let padding = style.padding.resolve_or_zero(basis, calc);
        let border = style.border.resolve_or_zero(basis, calc);
        (margin, (padding + border).sum_axes())
    }

    /// Completes a one-sided known size of a container through its preferred
    /// aspect ratio, which relates the boxes `box-sizing` selects. Other items
    /// apply their ratio while being measured, where content can still enlarge
    /// the ratio-derived size.
    fn apply_aspect_ratio(&self, id: BoxId, inputs: &mut LayoutInput) {
        let b = self.tree.get(id);
        let Some(ratio) = preferred_aspect_ratio(&b.style).filter(|r| *r > 0.0) else {
            return;
        };
        let (_, edges) = self.frame(id, inputs.parent_size.width);
        let content_box = self.style(id).box_sizing == taffy::BoxSizing::ContentBox;
        let known = &mut inputs.known_dimensions;
        match (known.width, known.height) {
            (Some(w), None) if content_box => {
                known.height = Some((w - edges.width).max(0.0) / ratio + edges.height);
            }
            (Some(w), None) => known.height = Some(w / ratio),
            (None, Some(h)) if content_box => {
                known.width = Some((h - edges.height).max(0.0) * ratio + edges.width);
            }
            (None, Some(h)) => known.width = Some(h * ratio),
            _ => {}
        }
    }

    /// Size and first baseline of a box that is not itself a Taffy container.
    ///
    /// Taffy passes border-box `known_dimensions` and the space available to the
    /// margin box. Item size styles are not applied here: the container
    /// algorithm resolves them and hands the result in as known dimensions.
    fn measure_item(&mut self, id: BoxId, inputs: LayoutInput) -> LayoutOutput {
        let tree: &'a BoxTree = self.tree;
        let b = tree.get(id);
        let LayoutInput {
            known_dimensions: known,
            parent_size,
            available_space,
            run_mode,
            axis,
            ..
        } = inputs;
        let (margin, edges) = self.frame(id, parent_size.width);
        let margin_width = margin.horizontal_axis_sum();
        let known_content = Size {
            width: known.width.map(|w| (w - edges.width).max(0.0)),
            height: known.height.map(|h| (h - edges.height).max(0.0)),
        };

        if let BoxKind::Replaced(kind) = b.kind {
            // A ratio-only image fills its container's width.
            let stretch = parent_size
                .width
                .or(self.inner_width)
                .map(|w| w - margin_width - edges.width);
            let (w, h) = replaced_content_size(
                self.cx,
                b.node,
                kind,
                (known_content.width, known_content.height),
                &SizeLimits::NONE,
                ReplacedHints::of(&b.style, stretch),
            );
            return LayoutOutput::from_outer_size(Size {
                width: known.width.unwrap_or(w + edges.width),
                height: known.height.unwrap_or(h + edges.height),
            });
        }

        let ratio = preferred_aspect_ratio(&b.style).filter(|r| *r > 0.0);
        let content_box = self.style(id).box_sizing == taffy::BoxSizing::ContentBox;
        // The ratio relates the boxes `box-sizing` selects.
        let height_for = |width: f32, ratio: f32| {
            if content_box {
                (width - edges.width).max(0.0) / ratio + edges.height
            } else {
                width / ratio
            }
        };
        let width_for = |height: f32, ratio: f32| {
            if content_box {
                (height - edges.height).max(0.0) * ratio + edges.width
            } else {
                height * ratio
            }
        };
        let width = match known.width {
            Some(width) => width,
            None => {
                let (min, max) = intrinsic_inline_sizes_of_block(self.cx, tree, id);
                match (ratio, known.height) {
                    // A definite height gives the width through the ratio, which
                    // content can only widen unless the box scrolls.
                    (Some(ratio), Some(height)) => {
                        let from_ratio = width_for(height, ratio);
                        if scrolls(b) {
                            from_ratio
                        } else {
                            from_ratio.max(min + edges.width)
                        }
                    }
                    _ => {
                        edges.width
                            + match available_space.width {
                                AvailableSpace::MinContent => min,
                                AvailableSpace::MaxContent => max,
                                // The fit-content width of an item that was not stretched.
                                AvailableSpace::Definite(space) => {
                                    max.min(min.max(space - margin_width - edges.width))
                                }
                            }
                    }
                }
            }
        };

        // Height and baseline need a real layout. A width-only measurement does
        // not, and neither does a size query that already knows both.
        let wants_layout = run_mode == RunMode::PerformLayout
            || (known.height.is_none() && axis != RequestedAxis::Horizontal);
        if !wants_layout {
            return LayoutOutput::from_outer_size(Size {
                width,
                height: known.height.unwrap_or(0.0),
            });
        }
        let (mut height, baseline) = self.measure_by_layout(id, width, parent_size);
        if known.height.is_none()
            && let Some(ratio) = ratio
        {
            // The ratio gives an automatic height; content may only enlarge it
            // unless the box scrolls, where the automatic minimum size is zero.
            let from_ratio = height_for(width, ratio);
            height = if scrolls(b) {
                from_ratio
            } else {
                height.max(from_ratio)
            };
        }
        let size = Size {
            width,
            height: known.height.unwrap_or(height),
        };
        let mut output = LayoutOutput::from_outer_size(size);
        output.baselines.first = baseline;
        output
    }

    fn compute_uncached(&mut self, id: BoxId, mut inputs: LayoutInput) -> LayoutOutput {
        if matches!(self.tree.get(id).kind, BoxKind::Flex | BoxKind::Grid) {
            self.apply_aspect_ratio(id, &mut inputs);
            self.prepare(id);
            self.solve(id, inputs)
        } else {
            self.measure_item(id, inputs)
        }
    }

    /// Runs the container algorithm `id`'s `display` selects.
    pub(crate) fn solve(&mut self, id: BoxId, inputs: LayoutInput) -> LayoutOutput {
        match self.tree.get(id).kind {
            BoxKind::Grid => compute_grid_layout(self, node_id(id), inputs),
            _ => compute_flexbox_layout(self, node_id(id), inputs),
        }
    }

    /// The root container's content-box inline size under a min-content or
    /// max-content constraint.
    fn content_width_under(&mut self, space: AvailableSpace) -> f32 {
        let id = self.root;
        let frame = block_sizes::frame_of(self.tree, id, 0.0);
        let edges = frame.padding.horizontal() + frame.border.horizontal();
        let output = self.solve(
            id,
            LayoutInput {
                run_mode: RunMode::ComputeSize,
                sizing_mode: SizingMode::ContentSize,
                axis: RequestedAxis::Horizontal,
                known_dimensions: Size::NONE,
                known_dimensions_are_definite: Size {
                    width: true,
                    height: true,
                },
                parent_size: Size::NONE,
                available_space: Size {
                    width: space,
                    height: AvailableSpace::MaxContent,
                },
                vertical_margins_are_collapsible: Line::FALSE,
            },
        );
        (output.size.width - edges).max(0.0)
    }

    /// Produces the fragments of the root container's children from the layouts
    /// Taffy settled on, in paint order.
    ///
    /// Each item is laid out once more at its final border-box size through the
    /// ordinary block entry point, which routes nested flex containers back here,
    /// and then placed where Taffy put it. `percent_base` is the container's
    /// content box, the base item percentages resolve against.
    pub(crate) fn emit_items(
        &mut self,
        percent_base: Size<Option<f32>>,
        out: &mut FragmentBuilder,
    ) -> SmallVec<[FragId; 4]> {
        let tree: &'a BoxTree = self.tree;
        let items: Vec<NodeId> = self.children[&self.root].clone();
        let mut frags = SmallVec::new();
        for node in items {
            let id = box_id(node);
            let Some(layout) = self.layouts.get(&id).copied() else {
                continue;
            };
            let base = self.areas.get(&id).copied().unwrap_or(percent_base);
            let space = self.item_space(layout.size.width, Some(layout.size.height), base);
            let result = layout_block(self.cx, tree, id, &space, out);
            self.baselines.insert(id, result.first_baseline);
            // Taffy applies `position: relative` offsets and resolves auto
            // margins itself, so its location and margins replace the block
            // entry point's.
            let fragment = out.get_mut(result.frag);
            fragment.rect.x = layout.location.x;
            fragment.rect.y = layout.location.y;
            fragment.margin = Edges {
                top: layout.margin.top,
                right: layout.margin.right,
                bottom: layout.margin.bottom,
                left: layout.margin.left,
            };
            fragment.flags |= FragFlags::PAINT_ATOMIC;
            frags.push(result.frag);
        }
        frags
    }
}

impl TreeView<'_, '_> {
    /// The grid container's first baseline, from the final layouts of its items
    /// (CSS Grid 1, 6.2): the first item of the first row, preferring one that
    /// is baseline-aligned, at its own first baseline or else its bottom edge.
    ///
    /// Taffy reports a baseline only when a row holds two baseline-aligned
    /// items and otherwise answers with the item's bottom edge. The first row
    /// is taken as the items whose margin box starts topmost, which can miss an
    /// item that is aligned away from the top of the same row.
    pub(crate) fn grid_first_baseline(&self) -> Option<f32> {
        let container = self.style(self.root);
        let top = |layout: &Layout| layout.location.y - layout.margin.top;
        let items: Vec<(BoxId, &Layout)> = self.children[&self.root]
            .iter()
            .filter_map(|&node| {
                let id = box_id(node);
                self.layouts.get(&id).map(|layout| (id, layout))
            })
            .collect();
        let first_top = items.iter().map(|(_, l)| top(l)).min_by(f32::total_cmp)?;
        let in_first_row = || {
            items
                .iter()
                .filter(move |(_, l)| top(l) <= first_top + 0.01)
        };
        let baseline_aligned = |id: &BoxId| {
            self.style(*id)
                .align_self
                .or(container.align_items)
                .map(|a| a.keyword)
                == Some(taffy::AlignItemsKeyword::Baseline)
        };
        let (id, layout) = in_first_row()
            .find(|(id, _)| baseline_aligned(id))
            .or_else(|| in_first_row().next())?;
        let own = self.baselines.get(id).copied().flatten();
        Some(layout.location.y + own.unwrap_or(layout.size.height))
    }
}

impl TraversePartialTree for TreeView<'_, '_> {
    type ChildIter<'i>
        = Copied<Iter<'i, NodeId>>
    where
        Self: 'i;

    fn child_ids(&self, parent: NodeId) -> Self::ChildIter<'_> {
        self.children[&box_id(parent)].iter().copied()
    }

    fn child_count(&self, parent: NodeId) -> usize {
        self.children[&box_id(parent)].len()
    }

    fn get_child_id(&self, parent: NodeId, index: usize) -> NodeId {
        self.children[&box_id(parent)][index]
    }
}

impl LayoutPartialTree for TreeView<'_, '_> {
    type CoreContainerStyle<'s>
        = &'s Style<Atom>
    where
        Self: 's;
    type CustomIdent = Atom;

    fn get_core_container_style(&self, node: NodeId) -> Self::CoreContainerStyle<'_> {
        self.style_of_node(node)
    }

    fn resolve_calc_value(&self, handle: *const (), basis: f32) -> f32 {
        // SAFETY: every handle Taffy holds comes from a style converted by this
        // view, and those borrow computed values owned by `self.tree`, which is
        // borrowed for as long as the view lives.
        unsafe { resolve_calc(handle, basis) }
    }

    fn set_unrounded_layout(&mut self, node: NodeId, layout: &Layout) {
        let id = box_id(node);
        // Nested containers are laid out again from scratch when their fragments
        // are made, so only the root's children matter.
        if self.tree.get(id).parent == self.root {
            self.layouts.insert(id, *layout);
        }
    }

    fn compute_child_layout(&mut self, node: NodeId, inputs: LayoutInput) -> LayoutOutput {
        if inputs.run_mode == RunMode::PerformHiddenLayout {
            return LayoutOutput::HIDDEN;
        }
        let id = box_id(node);
        if inputs.run_mode == RunMode::PerformLayout
            && self.tree.get(id).parent == self.root
            && matches!(self.tree.get(self.root).kind, BoxKind::Grid)
        {
            self.areas.insert(id, inputs.parent_size);
        }
        compute_cached_layout(self, node, inputs, |view, node, inputs| {
            view.compute_uncached(box_id(node), inputs)
        })
    }
}

impl CacheTree for TreeView<'_, '_> {
    fn cache_get(&mut self, node: NodeId, input: &LayoutInput) -> Option<LayoutOutput> {
        self.caches.get_mut(&box_id(node))?.get(input)
    }

    fn cache_store(&mut self, node: NodeId, input: &LayoutInput, output: LayoutOutput) {
        self.caches
            .entry(box_id(node))
            .or_default()
            .store(input, output);
    }

    fn cache_clear(&mut self, node: NodeId) {
        self.caches.remove(&box_id(node));
    }
}

/// The `(min-content, max-content)` inline sizes of a flex or grid container's
/// content box, as the container would size itself under each constraint.
pub(crate) fn intrinsic_inline_sizes(cx: &mut LayoutCx, tree: &BoxTree, id: BoxId) -> (f32, f32) {
    let mut view = TreeView::new(cx, tree, id);
    (
        view.content_width_under(AvailableSpace::MinContent),
        view.content_width_under(AvailableSpace::MaxContent),
    )
}
