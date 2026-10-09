//! The fragment tree: positioned output of layout, input of paint.
//!
//! Fragments are stored in an arena and addressed by [`FragId`]. Rectangles
//! are relative to the parent fragment's border-box origin, so a subtree can
//! be laid out again without touching its siblings. The tree is built through
//! [`FragmentBuilder`] and is deterministic: the same pushes give the same
//! ids and the same per-node lists.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use smallvec::SmallVec;
use tenta_core::NodeId;

use crate::box_tree::BoxId;
use crate::positioned::Candidate;
use crate::types::{Edges, GlyphRun, MarkerKind, PaintStyle, Rect, ReplacedKind};

/// Index of a [`Fragment`] in its [`FragmentTree`].
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct FragId(pub u32);

impl FragId {
    /// The arena index.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

bitflags::bitflags! {
    /// Facts about a fragment that paint and geometry branch on.
    #[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
    pub struct FragFlags: u16 {
        /// Starts a stacking context.
        const STACKING_CONTEXT = 1 << 0;
        /// Clips its content to the padding box.
        const CLIPS_OVERFLOW = 1 << 1;
        /// Establishes a block formatting context.
        const ESTABLISHES_BFC = 1 << 2;
        /// A float.
        const FLOAT = 1 << 3;
        /// Out of normal flow.
        const OUT_OF_FLOW = 1 << 4;
        /// The body fragment whose background the canvas takes when the root
        /// `<html>` element paints none (CSS Backgrounds, propagation from
        /// the root's first `<body>` child). Set by layout on at most one
        /// direct child of the root.
        const CANVAS_SOURCE = 1 << 5;
        /// Painted atomically as if it were a stacking context, in the
        /// inline-level phase of its parent (CSS 2.1 Appendix E, and
        /// Flexbox/Grid for their items). Set by layout on flex items, grid
        /// items and inline-blocks; its positioned and z-indexed descendants
        /// still belong to the enclosing stacking context.
        const PAINT_ATOMIC = 1 << 6;
        /// The containing block of its absolutely positioned descendants:
        /// the box is positioned or has a transform, filter, containment or
        /// similar (CSS Positioned Layout 3, 2.1). Set by layout.
        const ABSPOS_CB = 1 << 7;
        /// The containing block of its `position: fixed` descendants: a
        /// transform, filter, containment or similar, but not `position`.
        const FIXED_CB = 1 << 8;
    }
}

/// Shaped text of one fragment.
#[derive(Clone, Debug, PartialEq)]
pub struct TextFragment {
    /// Byte range into the DOM text node's data.
    pub text_range: Range<u32>,
    /// Glyph runs, in visual order. Shared with the display list, so building
    /// it copies no glyphs.
    pub runs: SmallVec<[Arc<GlyphRun>; 1]>,
    /// Baseline offset from the fragment's top.
    pub baseline: f32,
}

/// What a fragment is.
#[derive(Clone, Debug, PartialEq)]
pub enum FragmentKind {
    /// A block, inline or atomic box.
    Box,
    /// A line box.
    Line,
    /// A run of text.
    Text(TextFragment),
    /// Replaced content.
    Replaced(ReplacedKind),
    /// A list marker.
    Marker(MarkerKind),
}

/// One positioned piece of a box.
#[derive(Clone, Debug)]
pub struct Fragment {
    /// Originating node; [`NodeId::NONE`] for anonymous fragments.
    pub node: NodeId,
    /// What the fragment is.
    pub kind: FragmentKind,
    /// Style for paint, shared with the box.
    pub paint: Arc<PaintStyle>,
    /// Border box; the origin is relative to the parent fragment's border box.
    pub rect: Rect,
    /// Used border widths.
    pub border: Edges,
    /// Used padding.
    pub padding: Edges,
    /// Used margins.
    pub margin: Edges,
    /// Scrollable overflow in this fragment's coordinates.
    pub overflow: Rect,
    /// Flags.
    pub flags: FragFlags,
    /// Child fragments in paint-tree order.
    pub children: SmallVec<[FragId; 4]>,
}

impl Fragment {
    /// A childless fragment with zero margins and overflow equal to its
    /// border box.
    pub fn new(node: NodeId, kind: FragmentKind, paint: Arc<PaintStyle>, rect: Rect) -> Self {
        Self {
            node,
            kind,
            paint,
            rect,
            border: Edges::default(),
            padding: Edges::default(),
            margin: Edges::default(),
            overflow: Rect::new(0.0, 0.0, rect.w, rect.h),
            flags: FragFlags::empty(),
            children: SmallVec::new(),
        }
    }
}

/// The place of an absolutely positioned box in a line, in the coordinates of
/// the inline formatting context's content box.
#[derive(Copy, Clone, Debug)]
pub(crate) struct InlineStatic {
    /// The block container whose context holds the placeholder.
    pub(crate) root: BoxId,
    /// The positioned box.
    pub(crate) box_id: BoxId,
    /// Inline offset of the placeholder from the content box's left edge.
    pub(crate) x: f32,
    /// Top of the line holding it.
    pub(crate) y: f32,
    /// Index of that line among the context's lines; a placeholder on a line
    /// that was dropped as empty belongs to the next line.
    pub(crate) line: usize,
    /// The placeholder reads right to left, so the box is aligned by its right
    /// edge.
    pub(crate) rtl: bool,
}

/// Accumulates fragments while layout runs.
#[derive(Debug, Default)]
pub struct FragmentBuilder {
    frags: Vec<Fragment>,
    /// Absolutely positioned boxes whose static parent is the fragment, laid
    /// out once the whole tree is known (see `positioned`).
    static_candidates: Vec<(FragId, Candidate)>,
    /// Where inline layout met the placeholders of absolutely positioned boxes,
    /// until block layout collects them.
    inline_statics: Vec<InlineStatic>,
    /// Floats inline layout placed, until block layout collects them.
    inline_floats: Vec<(BoxId, FragId)>,
    containing_blocks: HashMap<FragId, FragId>,
}

/// How much a [`FragmentBuilder`] held at some moment, so a speculative layout
/// can be discarded.
#[derive(Copy, Clone, Debug)]
pub(crate) struct Checkpoint {
    frags: usize,
    candidates: usize,
    statics: usize,
    floats: usize,
}

impl FragmentBuilder {
    /// Remembers the current contents.
    pub(crate) fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            frags: self.frags.len(),
            candidates: self.static_candidates.len(),
            statics: self.inline_statics.len(),
            floats: self.inline_floats.len(),
        }
    }

    /// Drops everything pushed since `mark`.
    pub(crate) fn truncate(&mut self, mark: Checkpoint) {
        self.frags.truncate(mark.frags);
        self.static_candidates.truncate(mark.candidates);
        self.inline_statics.truncate(mark.statics);
        self.inline_floats.truncate(mark.floats);
    }

    /// Records that inline layout of the context rooted at `root` placed the
    /// float fragment `frag`, in the coordinates of the context's content box.
    #[cfg(feature = "embedded-fonts")]
    pub(crate) fn add_inline_float(&mut self, root: BoxId, frag: FragId) {
        self.inline_floats.push((root, frag));
    }

    /// Removes and returns the floats recorded for the context rooted at `root`.
    pub(crate) fn take_inline_floats(&mut self, root: BoxId) -> Vec<FragId> {
        let mut mine = Vec::new();
        self.inline_floats.retain(|&(owner, frag)| {
            if owner == root {
                mine.push(frag);
                false
            } else {
                true
            }
        });
        mine
    }
}

impl FragmentBuilder {
    /// An empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a fragment and returns its id.
    pub fn push(&mut self, f: Fragment) -> FragId {
        let id = FragId(self.frags.len() as u32);
        self.frags.push(f);
        id
    }

    /// Records that `candidate`, an absolutely positioned box, has `parent` as
    /// its static parent.
    pub(crate) fn add_candidate(&mut self, parent: FragId, candidate: Candidate) {
        self.static_candidates.push((parent, candidate));
    }

    /// Hands the recorded candidates to the positioned-layout pass.
    pub(crate) fn take_candidates(&mut self) -> Vec<(FragId, Candidate)> {
        std::mem::take(&mut self.static_candidates)
    }

    /// Records that the out-of-flow fragment `frag` has `block` as its
    /// containing block although it hangs below another fragment.
    pub(crate) fn set_containing_block(&mut self, frag: FragId, block: FragId) {
        self.containing_blocks.insert(frag, block);
    }

    /// Records where inline layout of the context rooted at `root` met the
    /// placeholder of an absolutely positioned box.
    #[cfg(feature = "embedded-fonts")]
    pub(crate) fn add_inline_static(&mut self, mark: InlineStatic) {
        self.inline_statics.push(mark);
    }

    /// Removes and returns the placeholders recorded for the context rooted at
    /// `root`, in the order the lines met them.
    pub(crate) fn take_inline_statics(&mut self, root: BoxId) -> Vec<InlineStatic> {
        let (mine, rest) = std::mem::take(&mut self.inline_statics)
            .into_iter()
            .partition(|m| m.root == root);
        self.inline_statics = rest;
        mine
    }

    /// Replaces the children of `id`.
    pub fn set_children(&mut self, id: FragId, kids: SmallVec<[FragId; 4]>) {
        self.frags[id.index()].children = kids;
    }

    /// Appends `child` to the children of `parent`.
    pub fn add_child(&mut self, parent: FragId, child: FragId) {
        self.frags[parent.index()].children.push(child);
    }

    /// The fragment with this id.
    pub fn get(&self, id: FragId) -> &Fragment {
        &self.frags[id.index()]
    }

    /// Mutable access to a pushed fragment.
    pub fn get_mut(&mut self, id: FragId) -> &mut Fragment {
        &mut self.frags[id.index()]
    }

    /// Number of fragments pushed so far.
    pub fn len(&self) -> usize {
        self.frags.len()
    }

    /// True when nothing was pushed.
    pub fn is_empty(&self) -> bool {
        self.frags.is_empty()
    }

    /// Seals the builder into a tree rooted at `root`.
    ///
    /// `viewport` is the layout viewport and `scroll_extent` the root's
    /// scrollable overflow. Unreachable fragments keep their ids but have no
    /// parent.
    pub fn finish(self, root: FragId, viewport: Rect, scroll_extent: Rect) -> FragmentTree {
        let mut parents = vec![FragId(u32::MAX); self.frags.len()];
        let mut node_frags: Vec<SmallVec<[FragId; 1]>> = Vec::new();
        for (index, frag) in self.frags.iter().enumerate() {
            for child in &frag.children {
                parents[child.index()] = FragId(index as u32);
            }
            if frag.node.is_some() {
                let slot = frag.node.raw() as usize;
                if node_frags.len() <= slot {
                    node_frags.resize_with(slot + 1, SmallVec::new);
                }
                node_frags[slot].push(FragId(index as u32));
            }
        }
        FragmentTree {
            root,
            viewport,
            scroll_extent,
            frags: self.frags,
            containing_blocks: self.containing_blocks,
            parents,
            node_frags,
        }
    }
}

/// The laid-out document.
#[derive(Debug)]
pub struct FragmentTree {
    /// The root fragment.
    pub root: FragId,
    /// Layout viewport.
    pub viewport: Rect,
    /// Scrollable overflow of the root.
    pub scroll_extent: Rect,
    frags: Vec<Fragment>,
    parents: Vec<FragId>,
    containing_blocks: HashMap<FragId, FragId>,
    /// Indexed by DOM arena slot; inline boxes split across lines have several.
    node_frags: Vec<SmallVec<[FragId; 1]>>,
}

impl FragmentTree {
    /// The fragment with this id. Panics when the id is not from this tree.
    pub fn get(&self, id: FragId) -> &Fragment {
        &self.frags[id.index()]
    }

    /// Number of fragments.
    pub fn len(&self) -> usize {
        self.frags.len()
    }

    /// True when the tree holds no fragment.
    pub fn is_empty(&self) -> bool {
        self.frags.is_empty()
    }

    /// The fragments a node produced, in push order.
    pub fn fragments_of(&self, node: NodeId) -> &[FragId] {
        self.node_frags
            .get(node.raw() as usize)
            .map_or(&[], |frags| frags.as_slice())
    }

    /// The parent of a fragment; `None` for the root and unreachable fragments.
    pub fn parent(&self, id: FragId) -> Option<FragId> {
        let parent = self.parents[id.index()];
        (parent.0 != u32::MAX).then_some(parent)
    }

    /// The containing block of an absolutely positioned fragment, when it is
    /// not the fragment's parent. Such a fragment hangs below the fragment
    /// where its box sits in the box tree (so paint finds the stacking contexts
    /// and effect groups around it), but its overflow clip is its containing
    /// block's, not that of the ancestors in between.
    pub fn containing_block_of(&self, id: FragId) -> Option<FragId> {
        self.containing_blocks.get(&id).copied()
    }

    /// Border box in document coordinates, without scroll offsets.
    pub fn absolute_border_box(&self, id: FragId) -> Rect {
        let rect = self.get(id).rect;
        let (mut x, mut y) = (rect.x, rect.y);
        let mut cursor = self.parent(id);
        while let Some(parent) = cursor {
            let origin = self.get(parent).rect;
            x += origin.x;
            y += origin.y;
            cursor = self.parent(parent);
        }
        Rect::new(x, y, rect.w, rect.h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frag(node: u32, rect: Rect) -> Fragment {
        Fragment::new(
            NodeId(node),
            FragmentKind::Box,
            Arc::new(PaintStyle::default()),
            rect,
        )
    }

    fn sample() -> (FragmentTree, [FragId; 3]) {
        let mut b = FragmentBuilder::new();
        let root = b.push(frag(1, Rect::new(0.0, 0.0, 800.0, 600.0)));
        let mid = b.push(frag(2, Rect::new(10.0, 20.0, 400.0, 100.0)));
        let leaf = b.push(frag(2, Rect::new(5.0, 6.0, 50.0, 10.0)));
        b.set_children(root, SmallVec::from_slice(&[mid]));
        b.set_children(mid, SmallVec::from_slice(&[leaf]));
        let vp = Rect::new(0.0, 0.0, 800.0, 600.0);
        (b.finish(root, vp, vp), [root, mid, leaf])
    }

    #[test]
    fn absolute_border_box_sums_ancestor_origins() {
        let (tree, [root, mid, leaf]) = sample();
        assert_eq!(
            tree.absolute_border_box(root),
            Rect::new(0.0, 0.0, 800.0, 600.0)
        );
        assert_eq!(
            tree.absolute_border_box(mid),
            Rect::new(10.0, 20.0, 400.0, 100.0)
        );
        assert_eq!(
            tree.absolute_border_box(leaf),
            Rect::new(15.0, 26.0, 50.0, 10.0)
        );
        assert_eq!(tree.parent(leaf), Some(mid));
        assert_eq!(tree.parent(root), None);
    }

    #[test]
    fn fragments_of_lists_split_boxes_in_push_order() {
        let (tree, [_, mid, leaf]) = sample();
        assert_eq!(tree.fragments_of(NodeId(2)), &[mid, leaf]);
        assert!(tree.fragments_of(NodeId(99)).is_empty());
        assert_eq!(tree.len(), 3);
    }

    #[test]
    fn identical_builds_are_identical() {
        let (a, _) = sample();
        let (b, _) = sample();
        assert_eq!(a.len(), b.len());
        for i in 0..a.len() as u32 {
            assert_eq!(a.get(FragId(i)).rect, b.get(FragId(i)).rect);
            assert_eq!(a.get(FragId(i)).children, b.get(FragId(i)).children);
        }
    }

    #[test]
    fn anonymous_fragments_are_not_indexed_by_node() {
        let mut b = FragmentBuilder::new();
        let root = b.push(Fragment::new(
            NodeId::NONE,
            FragmentKind::Line,
            Arc::new(PaintStyle::default()),
            Rect::default(),
        ));
        let tree = b.finish(root, Rect::default(), Rect::default());
        assert!(tree.fragments_of(NodeId::NONE).is_empty());
    }
}
