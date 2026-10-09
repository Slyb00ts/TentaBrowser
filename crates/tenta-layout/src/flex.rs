//! Flex containers: Taffy's flexbox algorithm over the box tree.
//!
//! The container entry and the item measuring are shared with grid; see
//! [`crate::container`] and [`TreeView`].

use style::Atom;
use taffy::{LayoutFlexboxContainer, Style};

use crate::taffy_tree::TreeView;

impl LayoutFlexboxContainer for TreeView<'_, '_> {
    type FlexboxContainerStyle<'s>
        = &'s Style<Atom>
    where
        Self: 's;
    type FlexboxItemStyle<'s>
        = &'s Style<Atom>
    where
        Self: 's;

    fn get_flexbox_container_style(&self, node: taffy::NodeId) -> Self::FlexboxContainerStyle<'_> {
        self.style_of_node(node)
    }

    fn get_flexbox_child_style(&self, node: taffy::NodeId) -> Self::FlexboxItemStyle<'_> {
        self.style_of_node(node)
    }
}
