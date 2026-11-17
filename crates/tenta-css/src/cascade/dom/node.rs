//! Tree navigation: the node, document and shadow-root views Stylo walks.

use std::fmt;

use style::context::QuirksMode;
use style::dom::{NodeInfo, OpaqueNode, TDocument, TNode, TShadowRoot};
use style::shared_lock::SharedRwLock;
use style::stylist::CascadeData;
use tenta_core::NodeId;
use tenta_core::atom::atoms;
use tenta_dom::NodeKind;

use super::{Handle, Slot};

/// Any node of the tree, as Stylo's `TNode`.
#[derive(Copy, Clone)]
pub struct NodeHandle<'a>(pub(super) &'a Slot<'a>);

/// The document node, as Stylo's `TDocument`.
#[derive(Copy, Clone)]
pub struct DocHandle<'a>(pub(super) &'a Slot<'a>);

/// A shadow root, as Stylo's `TShadowRoot`.
#[derive(Copy, Clone)]
pub struct ShadowRootHandle<'a>(pub(super) &'a Slot<'a>);

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

impl PartialEq for ShadowRootHandle<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}

impl fmt::Debug for ShadowRootHandle<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[shadow root #{}]", self.0.node.raw())
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
        self.0.view.doc.is_composed_connected(self.0.node)
    }

    // The flat tree: a node slotted into a shadow tree is under its slot, and
    // the children of a shadow root are under the host.
    //
    // The style-sharing cache asks this for every candidate it tests, so a
    // document without shadow trees takes the parent straight from the arena.
    #[inline]
    fn traversal_parent(&self) -> Option<Handle<'a>> {
        if !self.0.view.shadow_trees {
            return self.parent_node().and_then(|parent| parent.as_element());
        }
        self.flat_parent()
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
        (self.0.kind() == NodeKind::ShadowRoot).then_some(ShadowRootHandle(self.0))
    }
}

impl<'a> TShadowRoot for ShadowRootHandle<'a> {
    type ConcreteNode = NodeHandle<'a>;

    fn as_node(&self) -> NodeHandle<'a> {
        NodeHandle(self.0)
    }

    fn host(&self) -> Handle<'a> {
        let host = self
            .0
            .view
            .doc
            .shadow_host(self.0.node)
            .expect("a shadow root has a host");
        Handle(
            self.0
                .slot_of(host)
                .expect("the slot table covers the host"),
        )
    }

    fn style_data<'b>(&self) -> Option<&'b CascadeData>
    where
        Self: 'b,
    {
        let sheets: &'a super::ShadowSheets = self.0.view.shadow_sheets?;
        sheets.get(&self.0.node).map(|styles| &*styles.data)
    }
}

/// The children of an element in the flat tree, for `traversal_children`:
/// its own in document order, or the nodes a slot was assigned.
///
/// It owns nothing: Stylo keeps one of these alive across its hottest
/// functions, and a value with a destructor there costs every document the
/// cleanup paths, shadow trees or not.
#[derive(Copy, Clone)]
pub enum ChildNodes<'a> {
    /// The remaining siblings, starting at this one.
    Siblings(Option<NodeHandle<'a>>),
    /// The remaining children of a shadow host that ask for the slot named
    /// `name`, starting the search at `next`.
    Assigned {
        next: Option<NodeHandle<'a>>,
        name: &'a str,
    },
}

impl<'a> Iterator for ChildNodes<'a> {
    type Item = NodeHandle<'a>;

    #[inline]
    fn next(&mut self) -> Option<NodeHandle<'a>> {
        match self {
            ChildNodes::Siblings(next) => {
                let current = next.take()?;
                *next = current.next_sibling();
                Some(current)
            }
            ChildNodes::Assigned { .. } => self.next_assigned(),
        }
    }
}

impl<'a> ChildNodes<'a> {
    /// The next node assigned to the slot. Out of line, so that the sibling
    /// walk, which is all a document without shadow trees does, stays small
    /// where Stylo inlines it.
    #[cold]
    #[inline(never)]
    fn next_assigned(&mut self) -> Option<NodeHandle<'a>> {
        let ChildNodes::Assigned { next, name } = self else {
            return None;
        };
        while let Some(current) = next.take() {
            *next = current.next_sibling();
            if current.0.view.doc.slot_name_of(current.0.node) == Some(*name) {
                return Some(current);
            }
        }
        None
    }
}

impl<'a> NodeHandle<'a> {
    /// The parent element in the flat tree of a document that has shadow
    /// trees.
    #[inline(never)]
    fn flat_parent(self) -> Option<Handle<'a>> {
        let parent = self.0.view.doc.flat_parent(self.0.node)?;
        self.wrap(parent).and_then(|parent| parent.as_element())
    }

    /// The children of this element in the flat tree.
    #[inline]
    pub(super) fn flat_children(self) -> ChildNodes<'a> {
        if !self.0.view.shadow_trees {
            return ChildNodes::Siblings(self.first_child());
        }
        self.flat_children_with_shadow_trees()
    }

    #[inline(never)]
    fn flat_children_with_shadow_trees(self) -> ChildNodes<'a> {
        let doc = self.0.view.doc;
        if let Some(root) = doc.shadow_root(self.0.node) {
            return ChildNodes::Siblings(self.wrap(doc.first_child(root)));
        }
        if doc.is_slot(self.0.node)
            && let Some(&first) = doc.assigned_nodes(self.0.node).first()
        {
            // The assigned nodes are the host's children that ask for this
            // slot's name, in order, from the first of them on.
            return ChildNodes::Assigned {
                next: self.wrap(first),
                name: doc.attribute(self.0.node, atoms::NAME).unwrap_or(""),
            };
        }
        ChildNodes::Siblings(self.first_child())
    }
}
