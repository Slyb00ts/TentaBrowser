//! Grid containers: Taffy's grid algorithm over the box tree.
//!
//! Track sizing, item placement and alignment are Taffy's; the container's own
//! box model, the item measurements and the final item layouts come from the
//! shared [`TreeView`], exactly as for flex containers. Subgrid and masonry
//! are not supported by Taffy and lay out as grids without explicit tracks.

use style::Atom;
use taffy::{LayoutGridContainer, Style};

use crate::taffy_tree::TreeView;

impl LayoutGridContainer for TreeView<'_, '_> {
    type GridContainerStyle<'s>
        = &'s Style<Atom>
    where
        Self: 's;
    type GridItemStyle<'s>
        = &'s Style<Atom>
    where
        Self: 's;

    fn get_grid_container_style(&self, node: taffy::NodeId) -> Self::GridContainerStyle<'_> {
        self.style_of_node(node)
    }

    fn get_grid_child_style(&self, node: taffy::NodeId) -> Self::GridItemStyle<'_> {
        self.style_of_node(node)
    }
}
