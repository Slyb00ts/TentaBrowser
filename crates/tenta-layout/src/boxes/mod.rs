//! Box-tree construction from the document and its computed styles.

use tenta_css::cascade::StyleDiff;

use crate::box_tree::{BoxTree, LayoutCx};

impl BoxTree {
    /// Builds the box tree for the document in `cx`, stamped with the
    /// document's mutation generation.
    pub fn build(cx: &LayoutCx) -> BoxTree {
        let _ = cx;
        todo!("L1")
    }

    /// Brings the tree up to date after a style flush. A `StyleDiff::None`
    /// keeps the tree; anything else rebuilds it.
    pub fn update(&mut self, cx: &LayoutCx, diff: &StyleDiff) {
        let _ = (cx, diff);
        todo!("L1")
    }
}
