//! Tree navigation: the node, document and shadow-root views Stylo walks.

use std::convert::Infallible;
use std::fmt;
use std::marker::PhantomData;

use style::context::QuirksMode;
use style::dom::{NodeInfo, OpaqueNode, TDocument, TNode, TShadowRoot};
use style::shared_lock::SharedRwLock;
use style::stylist::CascadeData;
use tenta_core::NodeId;
use tenta_dom::NodeKind;

use super::{Handle, Slot};

/// Any node of the tree, as Stylo's `TNode`.
#[derive(Copy, Clone)]
pub struct NodeHandle<'a>(pub(super) &'a Slot<'a>);

/// The document node, as Stylo's `TDocument`.
#[derive(Copy, Clone)]
pub struct DocHandle<'a>(pub(super) &'a Slot<'a>);

/// Stylo requires a shadow-root type even when none is ever produced. Shadow
/// trees are not styled, so [`TNode::as_shadow_root`] and
/// [`style::dom::TElement::shadow_root`] always answer `None` and this type has
/// no values.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ShadowRootHandle<'a> {
    /// Uninhabited: the second field has no values.
    Never(PhantomData<&'a ()>, Infallible),
}

impl<'a> Slot<'a> {
    /// The slot of `node`, if the table covers it. `NodeId::NONE` is never
    /// covered.
    pub(super) fn slot_of(&self, node: NodeId) -> Option<&'a Slot<'a>> {
        if !node.is_some() {
            return None;
        }
        self.view.handle(node).map(|handle| handle.0)
    }

    fn kind(&self) -> NodeKind {
        self.view.doc.kind(self.node)
    }
}

impl<'a> NodeHandle<'a> {
    fn wrap(self, node: NodeId) -> Option<Self> {
        self.0.slot_of(node).map(NodeHandle)
    }

    /// The node this handle refers to.
    pub fn node(&self) -> NodeId {
        self.0.node
    }
}

impl<'a> DocHandle<'a> {
    /// The node this handle refers to.
    pub fn node(&self) -> NodeId {
        self.0.node
    }
}

impl fmt::Debug for NodeHandle<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let doc = self.0.view.doc;
        match doc.tag(self.0.node) {
            Some(tag) => write!(f, "<{} #{}>", tag.as_str(), self.0.node.raw()),
            None => write!(f, "[{:?} #{}]", doc.kind(self.0.node), self.0.node.raw()),
        }
    }
}

impl PartialEq for NodeHandle<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}

impl fmt::Debug for DocHandle<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[document #{}]", self.0.node.raw())
    }
}

impl NodeInfo for NodeHandle<'_> {
    fn is_element(&self) -> bool {
        self.0.kind() == NodeKind::Element
    }

    fn is_text_node(&self) -> bool {
        matches!(self.0.kind(), NodeKind::Text | NodeKind::CData)
    }
}

impl<'a> TDocument for DocHandle<'a> {
    type ConcreteNode = NodeHandle<'a>;

    fn as_node(&self) -> NodeHandle<'a> {
        NodeHandle(self.0)
    }

    fn is_html_document(&self) -> bool {
        self.0.view.doc.is_html_document()
    }

    fn quirks_mode(&self) -> QuirksMode {
        self.0.view.quirks
    }

    fn shared_lock(&self) -> &SharedRwLock {
        self.0.view.lock
    }
}

impl<'a> TNode for NodeHandle<'a> {
    type ConcreteElement = Handle<'a>;
    type ConcreteDocument = DocHandle<'a>;
    type ConcreteShadowRoot = ShadowRootHandle<'a>;

    fn parent_node(&self) -> Option<Self> {
        self.wrap(self.0.view.doc.parent(self.0.node))
    }

    fn first_child(&self) -> Option<Self> {
        self.wrap(self.0.view.doc.first_child(self.0.node))
    }

    fn last_child(&self) -> Option<Self> {
        self.wrap(self.0.view.doc.last_child(self.0.node))
    }

    fn prev_sibling(&self) -> Option<Self> {
        self.wrap(self.0.view.doc.previous_sibling(self.0.node))
    }

    fn next_sibling(&self) -> Option<Self> {
        self.wrap(self.0.view.doc.next_sibling(self.0.node))
    }

    fn owner_doc(&self) -> DocHandle<'a> {
        let document = self.0.view.doc.document_node();
        DocHandle(
            self.0
                .slot_of(document)
                .expect("the slot table covers the document node"),
        )
    }

    fn is_in_document(&self) -> bool {
        self.0.view.doc.is_connected(self.0.node)
    }

    fn traversal_parent(&self) -> Option<Handle<'a>> {
        self.parent_node().and_then(|parent| parent.as_element())
    }

    fn opaque(&self) -> OpaqueNode {
        OpaqueNode(self.0.node.raw() as usize)
    }

    fn debug_id(self) -> usize {
        self.0.node.raw() as usize
    }

    fn as_element(&self) -> Option<Handle<'a>> {
        self.is_element().then_some(Handle(self.0))
    }

    fn as_document(&self) -> Option<DocHandle<'a>> {
        (self.0.kind() == NodeKind::Document).then_some(DocHandle(self.0))
    }

    fn as_shadow_root(&self) -> Option<ShadowRootHandle<'a>> {
        None
    }
}

impl<'a> TShadowRoot for ShadowRootHandle<'a> {
    type ConcreteNode = NodeHandle<'a>;

    fn as_node(&self) -> NodeHandle<'a> {
        match *self {
            ShadowRootHandle::Never(_, never) => match never {},
        }
    }

    fn host(&self) -> Handle<'a> {
        match *self {
            ShadowRootHandle::Never(_, never) => match never {},
        }
    }

    fn style_data<'b>(&self) -> Option<&'b CascadeData>
    where
        Self: 'b,
    {
        None
    }
}

/// The children of an element in document order, for `traversal_children`.
pub struct ChildNodes<'a>(pub(super) Option<NodeHandle<'a>>);

impl<'a> Iterator for ChildNodes<'a> {
    type Item = NodeHandle<'a>;

    fn next(&mut self) -> Option<NodeHandle<'a>> {
        let current = self.0.take()?;
        self.0 = current.next_sibling();
        Some(current)
    }
}
