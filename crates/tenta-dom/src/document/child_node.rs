//! The `ParentNode` and `ChildNode` convenience mutations
//! (<https://dom.spec.whatwg.org/#interface-parentnode>,
//! <https://dom.spec.whatwg.org/#interface-childnode>).
//!
//! Each one is *convert nodes into a node* followed by a pre-insert, replace
//! or replace-all, and every tree change goes through [`Document::mutate`].
//! Adjacent Text nodes are never merged: the standard has no such step.

use tenta_core::{NodeId, Result};

use super::Document;
use crate::arena::NodeKind;
use crate::mutation::{Mutation, ReplaceAllWith};

/// One argument of a variadic `(Node or DOMString)...` list: a node of this
/// document, or a string that becomes a Text node.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum NodeOrText {
    Node(NodeId),
    Text(String),
}

impl Document {
    /// `ParentNode.append()` on `parent`.
    ///
    /// # Errors
    ///
    /// Returns the pre-insert's `HierarchyRequestError`.
    pub fn append_nodes(&mut self, parent: NodeId, nodes: Vec<NodeOrText>) -> Result<()> {
        let node = self.convert_nodes_into_node(nodes)?;
        self.mutate(Mutation::Append {
            parent,
            child: node,
        })
    }

    /// `ParentNode.prepend()` on `parent`.
    ///
    /// # Errors
    ///
    /// Returns the pre-insert's `HierarchyRequestError`.
    pub fn prepend_nodes(&mut self, parent: NodeId, nodes: Vec<NodeOrText>) -> Result<()> {
        let node = self.convert_nodes_into_node(nodes)?;
        let first = self.first_child(parent);
        self.mutate(Mutation::InsertBefore {
            parent,
            child: node,
            before: first,
        })
    }

    /// `ParentNode.replaceChildren()` on `parent`.
    ///
    /// # Errors
    ///
    /// Returns `HierarchyRequestError` when the nodes cannot be children of
    /// `parent`. Nothing is moved out of its old position in that case, apart
    /// from what converting several nodes into one fragment already did.
    pub fn replace_children(&mut self, parent: NodeId, nodes: Vec<NodeOrText>) -> Result<()> {
        let node = self.convert_nodes_into_node(nodes)?;
        self.validate_replace_children(parent, node)?;
        let replacement = match self.kind(node) {
            NodeKind::Fragment => ReplaceAllWith::ChildrenOf(node),
            NodeKind::DocType => {
                // A fragment cannot hold a doctype, so the node is detached
                // here instead and handed over as it is.
                self.mutate(Mutation::Remove { child: node })?;
                ReplaceAllWith::Detached(node)
            }
            _ => {
                let fragment = self.create_fragment();
                self.mutate(Mutation::Append {
                    parent: fragment,
                    child: node,
                })?;
                ReplaceAllWith::ChildrenOf(fragment)
            }
        };
        self.mutate(Mutation::ReplaceAll {
            parent,
            replacement,
        })
    }

    /// `ChildNode.before()` on `node`; a node without a parent does nothing.
    ///
    /// # Errors
    ///
    /// Returns the pre-insert's `HierarchyRequestError`.
    pub fn before_nodes(&mut self, node: NodeId, nodes: Vec<NodeOrText>) -> Result<()> {
        let parent = self.parent(node);
        if !parent.is_some() {
            return Ok(());
        }
        let viable_previous = self.viable_sibling(node, &nodes, Self::previous_sibling);
        let converted = self.convert_nodes_into_node(nodes)?;
        let before = if viable_previous.is_some() {
            self.next_sibling(viable_previous)
        } else {
            self.first_child(parent)
        };
        self.mutate(Mutation::InsertBefore {
            parent,
            child: converted,
            before,
        })
    }

    /// `ChildNode.after()` on `node`; a node without a parent does nothing.
    ///
    /// # Errors
    ///
    /// Returns the pre-insert's `HierarchyRequestError`.
    pub fn after_nodes(&mut self, node: NodeId, nodes: Vec<NodeOrText>) -> Result<()> {
        let parent = self.parent(node);
        if !parent.is_some() {
            return Ok(());
        }
        let before = self.viable_sibling(node, &nodes, Self::next_sibling);
        let converted = self.convert_nodes_into_node(nodes)?;
        self.mutate(Mutation::InsertBefore {
            parent,
            child: converted,
            before,
        })
    }

    /// `ChildNode.replaceWith()` on `node`; a node without a parent does
    /// nothing.
    ///
    /// # Errors
    ///
    /// Returns the pre-insert's or replacement's `HierarchyRequestError`.
    pub fn replace_with_nodes(&mut self, node: NodeId, nodes: Vec<NodeOrText>) -> Result<()> {
        let parent = self.parent(node);
        if !parent.is_some() {
            return Ok(());
        }
        let viable_next = self.viable_sibling(node, &nodes, Self::next_sibling);
        let converted = self.convert_nodes_into_node(nodes)?;
        // Converting can move `node` itself into the fragment, which is why the
        // parent is asked again.
        if self.parent(node) == parent {
            self.mutate(Mutation::ReplaceChild {
                parent,
                new_child: converted,
                old_child: node,
            })
        } else {
            self.mutate(Mutation::InsertBefore {
                parent,
                child: converted,
                before: viable_next,
            })
        }
    }

    /// The first sibling of `node` in the direction of `step` that is not one
    /// of the argument nodes, or [`NodeId::NONE`].
    fn viable_sibling(
        &self,
        node: NodeId,
        nodes: &[NodeOrText],
        step: fn(&Self, NodeId) -> NodeId,
    ) -> NodeId {
        let argument_nodes = node_arguments(nodes);
        let mut viable = step(self, node);
        while viable.is_some() && argument_nodes.contains(&viable) {
            viable = step(self, viable);
        }
        viable
    }

    /// *Convert nodes into a node*
    /// (<https://dom.spec.whatwg.org/#converting-nodes-into-a-node>): a lone
    /// argument is used as it is and anything else is gathered, in order, into
    /// a new fragment. Each string becomes its own Text node.
    fn convert_nodes_into_node(&mut self, nodes: Vec<NodeOrText>) -> Result<NodeId> {
        let mut nodes = nodes;
        if nodes.len() == 1 {
            return Ok(match nodes.remove(0) {
                NodeOrText::Node(node) => node,
                NodeOrText::Text(text) => self.create_text(&text),
            });
        }
        let fragment = self.create_fragment();
        for item in nodes {
            let child = match item {
                NodeOrText::Node(node) => node,
                NodeOrText::Text(text) => self.create_text(&text),
            };
            self.mutate(Mutation::Append {
                parent: fragment,
                child,
            })?;
        }
        Ok(fragment)
    }
}

fn node_arguments(nodes: &[NodeOrText]) -> Vec<NodeId> {
    nodes
        .iter()
        .filter_map(|item| match item {
            NodeOrText::Node(node) => Some(*node),
            NodeOrText::Text(_) => None,
        })
        .collect()
}
