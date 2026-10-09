//! Box-tree construction from the document and its computed styles.

mod anonymous;
mod builder;
mod content;
mod counters;
mod paint;
mod replaced;
// `FontSystem::new_embedded`, needed to build a `LayoutCx`, exists only with the bundled fonts.
#[cfg(all(test, feature = "embedded-fonts"))]
mod tests;

use std::sync::Arc;

use tenta_core::atom::atoms;
use tenta_css::cascade::StyleDiff;
use tenta_dom::Document;

use crate::box_tree::{BoxFlags, BoxId, BoxKind, BoxTree, LayoutCx};
use crate::types::{PaintStyle, Rect};
use builder::Builder;

impl BoxTree {
    /// Builds the box tree for the document in `cx`, stamped with the
    /// document's mutation generation.
    ///
    /// Elements whose style is `display: none` and their descendants generate
    /// no box. Flex, grid and table containers are recognised by kind and
    /// their children are built as for a block container; flex and grid lay
    /// out through Taffy, tables as blocks until their own algorithm exists.
    pub fn build(cx: &LayoutCx) -> BoxTree {
        Builder::new(cx).run()
    }

    /// Builds the tree and reports how many `::marker` cascades it ran.
    #[cfg(all(test, feature = "embedded-fonts"))]
    fn build_counting_marker_resolves(cx: &LayoutCx) -> (BoxTree, usize) {
        let mut builder = Builder::new(cx);
        let tree = builder.run();
        (tree, builder.marker_resolves)
    }

    /// Brings the tree up to date after a style flush. A `StyleDiff::None`
    /// keeps the tree; anything else rebuilds it.
    pub fn update(&mut self, cx: &LayoutCx, diff: &StyleDiff) {
        if !matches!(diff, StyleDiff::None) {
            *self = Self::build(cx);
        }
    }

    /// The box whose background the canvas takes over: the `body` box of an
    /// HTML document whose root `html` element has a `body` child. Paint uses
    /// it instead of guessing which element propagates (CSS Backgrounds 3
    /// section 2.11.2); layout copies the answer to the fragment.
    pub fn canvas_source(&self, doc: &Document) -> Option<BoxId> {
        let root = doc.document_element()?;
        if doc.tag(root) != Some(atoms::HTML) || doc.namespace(root) != Some(atoms::NS_HTML) {
            return None;
        }
        let body = doc.element_children(root).find(|&c| {
            doc.tag(c) == Some(atoms::BODY) && doc.namespace(c) == Some(atoms::NS_HTML)
        })?;
        self.boxes_of(body).first().copied()
    }

    /// The paint style of a fragment of box `id` whose border box is
    /// `border_box`. Boxes flagged `PAINT_SIZE_DEPENDENT` get the percentage
    /// translations and `to <corner>` gradient angles resolved for that size;
    /// every other box shares its stored `Arc`.
    pub fn paint_style_for(&self, id: BoxId, border_box: Rect) -> Arc<PaintStyle> {
        let b = self.get(id);
        if !b.flags.contains(BoxFlags::PAINT_SIZE_DEPENDENT) {
            return b.paint.clone();
        }
        Arc::new(paint::resolve_for_size(&b.paint, &b.style, border_box))
    }

    /// True when the box paints as one atomic unit in its parent's stacking
    /// order (CSS 2.1 Appendix E): inline-blocks and the other atomic
    /// inlines that contain content, and items of flex and grid containers.
    pub fn is_paint_atomic(&self, id: BoxId) -> bool {
        let b = self.get(id);
        let atomic_inline = b.flags.contains(BoxFlags::INLINE_LEVEL)
            && matches!(
                b.kind,
                BoxKind::InlineBlock | BoxKind::Flex | BoxKind::Grid | BoxKind::Table(_)
            );
        atomic_inline || self.is_flex_grid_item(id)
    }

    /// True for the in-flow children of a flex or grid container.
    pub fn is_flex_grid_item(&self, id: BoxId) -> bool {
        let b = self.get(id);
        b.parent != BoxId::NONE
            && matches!(self.get(b.parent).kind, BoxKind::Flex | BoxKind::Grid)
            && !b.flags.contains(BoxFlags::OUT_OF_FLOW)
    }
}
