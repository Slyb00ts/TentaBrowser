//! Stacking contexts and CSS 2.1 Appendix E painting order.
//!
//! Painting a context takes two passes. The first walks the fragments below
//! the context root and sorts them into the Appendix E layers without emitting
//! anything; the second emits the layers in order. Descendants that belong to
//! an ancestor's context (positioned boxes with `z-index: auto` hide the real
//! contexts below them) are hoisted into that ancestor's layers during the
//! first pass, which is why the passes cannot be fused.
//!
//! Overflow clips are not part of the layers. Each fragment remembers the
//! chain of clipping ancestors it sits under, and the painter keeps the chain
//! that is currently open, so a positive-`z-index` child is still cut by the
//! `overflow: hidden` box it was hoisted out of and consecutive fragments under
//! one clip share a single push/pop pair.
//!
//! Outlines: the in-flow outlines of an atomic unit (a context or a group) are
//! painted right after that unit's blocks, floats and inline content, so they
//! sit above its in-flow content and below its positioned layers; the unit's own
//! outline comes last. This follows Blink's outline phases rather than the
//! single end-of-context step of CSS 2.1.

use std::collections::HashMap;

use tenta_layout::{
    BlendMode, FragFlags, FragId, Fragment, FragmentKind, FragmentTree, PositionKind, Rect,
};

use super::clip::{self, ClipScope};
use super::effects::{self, EffectScope};
use super::{background, border, marker, replaced, shadow, text};
use crate::PaintContext;
use crate::dl::Item;

/// The calls into the clip, effect, text and marker emitters. The painter is
/// generic over them so tests can record the push/pop sequences the walk asks
/// for without depending on how those emitters draw.
pub(super) trait Seams {
    /// Pushes the overflow clip of `frag`.
    fn open_clip(&self, frag: &Fragment, border_box: Rect, items: &mut Vec<Item>) -> ClipScope;
    /// Pops the clip pushed by [`Seams::open_clip`].
    fn close_clip(&self, scope: ClipScope, items: &mut Vec<Item>);
    /// Pushes the opacity group and transform of `frag`. A transform composes
    /// as `translate(origin) * M * translate(-origin)` in document space, so
    /// the items inside keep absolute coordinates. Fixed and sticky boxes are
    /// not compensated for scrolling.
    fn open_effects(&self, frag: &Fragment, border_box: Rect, items: &mut Vec<Item>)
    -> EffectScope;
    /// Pops what [`Seams::open_effects`] pushed.
    fn close_effects(&self, scope: EffectScope, items: &mut Vec<Item>);
    /// Pushes the items of a text fragment.
    fn text(&self, frag: &Fragment, border_box: Rect, items: &mut Vec<Item>);
    /// Pushes the shape of a list-marker fragment.
    fn marker(&self, frag: &Fragment, border_box: Rect, items: &mut Vec<Item>);
}

/// The real emitters.
struct Emitters;

impl Seams for Emitters {
    fn open_clip(&self, frag: &Fragment, border_box: Rect, items: &mut Vec<Item>) -> ClipScope {
        clip::open(frag, border_box, items)
    }

    fn close_clip(&self, scope: ClipScope, items: &mut Vec<Item>) {
        clip::close(scope, items);
    }

    fn open_effects(
        &self,
        frag: &Fragment,
        border_box: Rect,
        items: &mut Vec<Item>,
    ) -> EffectScope {
        effects::open(frag, border_box, items)
    }

    fn close_effects(&self, scope: EffectScope, items: &mut Vec<Item>) {
        effects::close(scope, items);
    }

    fn text(&self, frag: &Fragment, border_box: Rect, items: &mut Vec<Item>) {
        text::emit(frag, border_box, items);
    }

    fn marker(&self, frag: &Fragment, border_box: Rect, items: &mut Vec<Item>) {
        marker::emit(frag, border_box, items);
    }
}

/// Index into the clip arena of a clipping ancestor.
type ClipRef = Option<u32>;

#[derive(Copy, Clone)]
struct ClipNode {
    frag: FragId,
    border_box: Rect,
    parent: ClipRef,
}

/// A fragment with its document position and the clips above it.
#[derive(Copy, Clone)]
struct Placed {
    id: FragId,
    border_box: Rect,
    clip: ClipRef,
    /// Whether the fragment's children are inline-level content of a line, as
    /// opposed to blocks.
    inline_children: bool,
}

/// A fragment painted atomically, as if it were a stacking context, whose own
/// positioned and `z-index` descendants still belong to the enclosing context.
struct Group {
    placed: Placed,
    flow: Flow,
}

/// Content painted in normal-flow phases, in tree order inside each phase.
#[derive(Default)]
struct Flow {
    blocks: Vec<Placed>,
    floats: Vec<Group>,
    inline: Vec<InlineEntry>,
    outlines: Vec<Placed>,
}

enum InlineEntry {
    Leaf(Placed),
    Group(Group),
}

enum Layer {
    Context(Placed),
    Group(Group),
}

/// The `z-index` layers of one stacking context.
#[derive(Default)]
struct Hoisted {
    negative: Vec<(i32, Placed)>,
    /// `None` is a slot reserved for a group whose content is still being
    /// collected, which keeps the group ahead of the layers found inside it.
    zero: Vec<Option<Layer>>,
    positive: Vec<(i32, Placed)>,
}

struct Painter<'a, S> {
    frags: &'a FragmentTree,
    cx: PaintContext<'a>,
    seams: S,
    items: Vec<Item>,
    clips: Vec<ClipNode>,
    /// The clips currently pushed, outermost first. Always a root path of the
    /// clip tree.
    open: Vec<(u32, ClipScope)>,
    /// Clips below this depth belong to an enclosing effect group and stay open
    /// until it closes.
    barrier: usize,
    /// The fragment whose background went to the canvas instead.
    propagated: Option<FragId>,
    /// The clip a containing block's content sits under, kept for the
    /// absolutely positioned boxes it contains.
    content_clips: HashMap<FragId, ClipRef>,
}

/// Paints the whole tree in Appendix E order.
pub(super) fn paint(frags: &FragmentTree, cx: &PaintContext) -> Vec<Item> {
    paint_with(frags, cx, Emitters)
}

pub(super) fn paint_with<S: Seams>(frags: &FragmentTree, cx: &PaintContext, seams: S) -> Vec<Item> {
    let mut painter = Painter {
        frags,
        cx: *cx,
        seams,
        items: Vec::new(),
        clips: Vec::new(),
        open: Vec::new(),
        barrier: 0,
        propagated: None,
        content_clips: HashMap::new(),
    };
    let root = frags.get(frags.root);
    let root_box = root.rect;
    painter.paint_canvas(root_box);
    if root.paint.opacity > 0.0 {
        let placed = Placed {
            id: frags.root,
            border_box: root_box,
            clip: None,
            inline_children: false,
        };
        painter.paint_context(placed);
    }
    painter.close_clips_to(0);
    painter.items
}

/// Whether a fragment starts a stacking context of its own.
///
/// `filter` and `mix-blend-mode` create contexts here although nothing
/// renders them yet; the contexts keep the paint order right for when they do.
pub(super) fn creates_context(frag: &Fragment) -> bool {
    let style = &frag.paint;
    frag.flags.contains(FragFlags::STACKING_CONTEXT)
        || style.opacity < 1.0
        || style.transform.is_some()
        || !style.filter.is_empty()
        || style.blend != BlendMode::Normal
        || style.isolate
        || matches!(style.position, PositionKind::Fixed | PositionKind::Sticky)
        || (style.position != PositionKind::Static && style.z_index.is_some())
}

/// The clips from the outermost down to `target`.
fn clip_chain(clips: &[ClipNode], target: ClipRef) -> Vec<u32> {
    let mut chain = Vec::new();
    let mut cursor = target;
    while let Some(index) = cursor {
        chain.push(index);
        cursor = clips[index as usize].parent;
    }
    chain.reverse();
    chain
}

impl<'a, S: Seams> Painter<'a, S> {
    /// The root element's background, or the body's when the root has none and
    /// layout marked the body as the canvas source, covers the whole canvas and
    /// is then not painted again by its element.
    fn paint_canvas(&mut self, root_box: Rect) {
        let frags = self.frags;
        // A root element taken out of flow (`position: absolute`) hangs below a
        // synthetic root box; the element is what the canvas takes from.
        let synthetic = frags.get(frags.root);
        let (root_id, root_box) = if !synthetic.node.is_some() {
            synthetic
                .children
                .iter()
                .copied()
                .find(|&child| frags.get(child).node.is_some())
                .map_or((frags.root, root_box), |element| {
                    (element, frags.absolute_border_box(element))
                })
        } else {
            (frags.root, root_box)
        };
        let root = frags.get(root_id);
        let source = if background::paints(&root.paint) {
            Some(root_id)
        } else {
            root.children.iter().copied().find(|&child| {
                let frag = frags.get(child);
                frag.flags.contains(FragFlags::CANVAS_SOURCE) && background::paints(&frag.paint)
            })
        };
        let Some(source) = source else { return };
        let canvas = frags.viewport.union(&frags.scroll_extent).union(&root_box);
        background::emit_canvas(
            frags.get(source),
            (root, root_box),
            canvas,
            &self.cx,
            &mut self.items,
        );
        self.propagated = Some(source);
    }

    fn clip_node(&mut self, id: FragId, border_box: Rect, parent: ClipRef) -> ClipRef {
        if !self.frags.get(id).flags.contains(FragFlags::CLIPS_OVERFLOW) {
            return parent;
        }
        self.clips.push(ClipNode {
            frag: id,
            border_box,
            parent,
        });
        Some(self.clips.len() as u32 - 1)
    }

    fn close_clips_to(&mut self, depth: usize) {
        while self.open.len() > depth {
            let (_, scope) = self.open.pop().expect("loop guard keeps it non-empty");
            self.seams.close_clip(scope, &mut self.items);
        }
    }

    /// Makes the open clips exactly the chain of `target`, closing and opening
    /// only the differing tail.
    fn set_clip(&mut self, target: ClipRef) {
        if self.open.last().map(|&(index, _)| index) == target {
            return;
        }
        let chain = clip_chain(&self.clips, target);
        let common = self
            .open
            .iter()
            .zip(&chain)
            .take_while(|((open, _), want)| open == *want)
            .count();
        if common < self.barrier {
            // The box wants a clip chain that leaves clips open outside the
            // effect group it is painted in; a display list nests them, so the
            // group's outer clips stay (a known gap for an escaping box).
            self.close_clips_to(self.barrier);
            return;
        }
        self.close_clips_to(common);
        for &index in &chain[common..] {
            let node = self.clips[index as usize];
            let scope =
                self.seams
                    .open_clip(self.frags.get(node.frag), node.border_box, &mut self.items);
            self.open.push((index, scope));
        }
    }

    /// First pass: sorts the descendants of `parent` into layers.
    fn collect(&mut self, parent: Placed, clip: ClipRef, hoisted: &mut Hoisted, flow: &mut Flow) {
        let frags = self.frags;
        for &child in &frags.get(parent.id).children {
            let frag = frags.get(child);
            if frag.paint.opacity <= 0.0 {
                continue;
            }
            let border_box = Rect::new(
                parent.border_box.x + frag.rect.x,
                parent.border_box.y + frag.rect.y,
                frag.rect.w,
                frag.rect.h,
            );
            // An absolutely positioned box hangs below the fragment where it sits
            // in the tree, so it joins that fragment's stacking context and
            // effect groups, but only its containing block's overflow clips cut
            // it.
            let clip = frags
                .containing_block_of(child)
                .and_then(|block| self.content_clips.get(&block).copied())
                .unwrap_or(clip);
            let float = frag.flags.contains(FragFlags::FLOAT);
            let atomic = frag.flags.contains(FragFlags::PAINT_ATOMIC);
            let blockified = float
                || atomic
                || matches!(
                    frag.paint.position,
                    PositionKind::Absolute | PositionKind::Fixed
                );
            let placed = Placed {
                id: child,
                border_box,
                clip,
                inline_children: match frag.kind {
                    FragmentKind::Box => parent.inline_children && !blockified,
                    _ => true,
                },
            };
            if creates_context(frag) {
                match frag.paint.z_index.unwrap_or(0) {
                    z if z < 0 => hoisted.negative.push((z, placed)),
                    z if z > 0 => hoisted.positive.push((z, placed)),
                    _ => hoisted.zero.push(Some(Layer::Context(placed))),
                }
                continue;
            }
            let inner = self.clip_node(child, border_box, clip);
            if frag
                .flags
                .intersects(FragFlags::ABSPOS_CB | FragFlags::FIXED_CB)
            {
                self.content_clips.insert(child, inner);
            }
            let positioned = frag.paint.position != PositionKind::Static;
            if atomic || float || positioned {
                let slot = positioned.then(|| {
                    hoisted.zero.push(None);
                    hoisted.zero.len() - 1
                });
                let mut local = Flow::default();
                self.collect(placed, inner, hoisted, &mut local);
                let group = Group {
                    placed,
                    flow: local,
                };
                match slot {
                    Some(slot) => hoisted.zero[slot] = Some(Layer::Group(group)),
                    None if float => flow.floats.push(group),
                    None => flow.inline.push(InlineEntry::Group(group)),
                }
                continue;
            }
            match frag.kind {
                FragmentKind::Line => {}
                FragmentKind::Box | FragmentKind::Replaced(_) if !parent.inline_children => {
                    flow.blocks.push(placed)
                }
                _ => flow.inline.push(InlineEntry::Leaf(placed)),
            }
            if frag.paint.outline.is_some() {
                flow.outlines.push(placed);
            }
            if !frag.children.is_empty() {
                self.collect(placed, inner, hoisted, flow);
            }
        }
    }

    /// Emits what belongs to the fragment itself, not its descendants.
    fn paint_self(&mut self, placed: Placed) {
        let frag = self.frags.get(placed.id);
        if frag.paint.visibility_hidden {
            return;
        }
        self.set_clip(placed.clip);
        let border_box = placed.border_box;
        match frag.kind {
            FragmentKind::Text(_) => self.seams.text(frag, border_box, &mut self.items),
            FragmentKind::Marker(_) => self.seams.marker(frag, border_box, &mut self.items),
            FragmentKind::Line => {}
            FragmentKind::Box | FragmentKind::Replaced(_) => {
                shadow::emit_outer(frag, border_box, &mut self.items);
                if self.propagated != Some(placed.id) {
                    background::emit(frag, border_box, &self.cx, &mut self.items);
                }
                shadow::emit_inset(frag, border_box, &mut self.items);
                border::emit(frag, border_box, &mut self.items);
                if matches!(frag.kind, FragmentKind::Replaced(_))
                    && replaced::images_drawable(&self.cx)
                {
                    replaced::emit(frag, border_box, &self.cx, &mut self.items);
                }
            }
        }
    }

    fn paint_outline(&mut self, placed: Placed) {
        let frag = self.frags.get(placed.id);
        if !frag.paint.visibility_hidden {
            self.set_clip(placed.clip);
            border::emit_outline(frag, placed.border_box, &mut self.items);
        }
    }

    /// Blocks, floats and inline content, then their outlines.
    fn paint_flow(&mut self, flow: &Flow) {
        for &placed in &flow.blocks {
            self.paint_self(placed);
        }
        for group in &flow.floats {
            self.paint_group(group);
        }
        for entry in &flow.inline {
            match entry {
                InlineEntry::Leaf(placed) => self.paint_self(*placed),
                InlineEntry::Group(group) => self.paint_group(group),
            }
        }
        for &placed in &flow.outlines {
            self.paint_outline(placed);
        }
    }

    fn paint_group(&mut self, group: &Group) {
        self.paint_self(group.placed);
        self.paint_flow(&group.flow);
        self.paint_outline(group.placed);
    }

    fn paint_layer(&mut self, layer: &Layer) {
        match layer {
            Layer::Context(placed) => self.paint_context(*placed),
            Layer::Group(group) => self.paint_group(group),
        }
    }

    /// Paints a stacking context rooted at `placed`.
    fn paint_context(&mut self, placed: Placed) {
        let frag = self.frags.get(placed.id);
        if frag.paint.opacity >= 1.0 && frag.paint.transform.is_none() {
            return self.paint_context_content(placed);
        }
        self.set_clip(placed.clip);
        let scope = self
            .seams
            .open_effects(frag, placed.border_box, &mut self.items);
        let outer = std::mem::replace(&mut self.barrier, self.open.len());
        self.paint_context_content(placed);
        self.close_clips_to(self.barrier);
        self.barrier = outer;
        self.seams.close_effects(scope, &mut self.items);
    }

    fn paint_context_content(&mut self, placed: Placed) {
        self.paint_self(placed);
        let own = self.clip_node(placed.id, placed.border_box, placed.clip);
        let block_flags = FragFlags::ABSPOS_CB | FragFlags::FIXED_CB;
        if placed.id == self.frags.root || self.frags.get(placed.id).flags.intersects(block_flags) {
            self.content_clips.insert(placed.id, own);
        }
        let mut hoisted = Hoisted::default();
        let mut flow = Flow::default();
        self.collect(placed, own, &mut hoisted, &mut flow);
        hoisted.negative.sort_by_key(|&(z, _)| z);
        hoisted.positive.sort_by_key(|&(z, _)| z);
        for &(_, below) in &hoisted.negative {
            self.paint_context(below);
        }
        self.paint_flow(&flow);
        for layer in hoisted.zero.iter().flatten() {
            self.paint_layer(layer);
        }
        for &(_, above) in &hoisted.positive {
            self.paint_context(above);
        }
        self.paint_outline(placed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_chains_run_outermost_first() {
        let node = |parent| ClipNode {
            frag: FragId(0),
            border_box: Rect::default(),
            parent,
        };
        let clips = [node(None), node(Some(0)), node(Some(1))];
        assert_eq!(clip_chain(&clips, Some(2)), vec![0, 1, 2]);
        assert_eq!(clip_chain(&clips, Some(0)), vec![0]);
        assert_eq!(clip_chain(&clips, None), Vec::<u32>::new());
    }
}
