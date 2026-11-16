//! Tree members that are small over `tenta-dom`: node comparison, element
//! siblings, the `insertAdjacent*` pair, `normalize`, and the document's
//! element collections.

use tenta_core::NodeId;
use tenta_core::atom::atoms;
use tenta_dom::{Mutation, NodeKind, compare};

use super::{
    HostObject, TAG_NODE, TAG_STATIC_HTML_COLLECTION, TAG_STATIC_NODE_LIST, decode, encode_node,
    encode_snapshot, is_attr_handle, mutation_error,
};
use crate::bindings::{Element, HTMLCollection, Handle, JsError, Node, NodeList};

impl HostObject {
    /// The node an argument names when it is a node of this document's tree;
    /// `None` for a node of another document, an `Attr`, or a handle that is
    /// not a node at all.
    fn tree_node_of(&self, handle: u64) -> Option<NodeId> {
        if self.is_attr() || is_attr_handle(self.context(), handle) {
            return None;
        }
        let decoded = decode(handle);
        (decoded.tag == TAG_NODE && decoded.slot == self.slot()).then_some(decoded.node_id)
    }

    fn node_handle<T>(&self, node: NodeId) -> Handle<T> {
        let (doc, _) = self.anchor();
        Handle::from_bits(encode_node(self.slot(), doc.borrow().node_ref(node)))
    }

    pub(super) fn base_uri_value(&self) -> String {
        let (doc, _) = self.anchor();
        let doc = doc.borrow();
        doc.base_url()
            .or_else(|| doc.url())
            .map_or_else(|| "about:blank".to_owned(), |url| url.to_string())
    }

    pub(super) fn equals_node(&self, other: Option<Handle<Node>>) -> bool {
        let Some(other) = other else {
            return false;
        };
        if other.bits() == self.handle_bits() {
            return true;
        }
        // Nodes of two documents are compared only by identity: reading the
        // other document's arena needs the world's document table.
        let Some(other) = self.tree_node_of(other.bits()) else {
            return false;
        };
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        compare::is_equal_node(&doc, id, &doc, other)
    }

    pub(super) fn is_same(&self, other: Option<Handle<Node>>) -> bool {
        other.is_some_and(|other| other.bits() == self.handle_bits())
    }

    pub(super) fn document_position(&self, other: Handle<Node>) -> u16 {
        if other.bits() == self.handle_bits() {
            return 0;
        }
        let Some(other) = self.tree_node_of(other.bits()) else {
            // Another document, or an attribute: disconnected, in an order
            // that is at least the same every time.
            let direction = if other.bits() < self.handle_bits() {
                compare::PRECEDING
            } else {
                compare::FOLLOWING
            };
            return compare::DISCONNECTED | compare::IMPLEMENTATION_SPECIFIC | direction;
        };
        let (doc, id) = self.anchor();
        compare::compare_document_position(&doc.borrow(), id, other)
    }

    pub(super) fn root_node(&self) -> Handle<Node> {
        if self.is_attr() {
            return Handle::from_bits(self.handle_bits());
        }
        let (doc, id) = self.anchor();
        let root = doc.borrow().root_of(id);
        self.node_handle(root)
    }

    /// `normalize()`: empty text nodes are removed and adjacent ones merged
    /// into the first, throughout the subtree.
    pub(super) fn normalize_subtree(&self) -> Result<(), JsError> {
        if self.is_attr() {
            return Ok(());
        }
        let (doc, id) = self.anchor();
        let mut doc = doc.borrow_mut();
        let texts: Vec<NodeId> = doc
            .descendants(id)
            .filter(|node| doc.kind(*node) == NodeKind::Text)
            .collect();
        for text in texts {
            // Merged into an earlier sibling, or removed, by a previous step.
            if !doc.is_live(text) || doc.parent(text) == NodeId::NONE {
                continue;
            }
            let mut merged = doc.text_data(text).unwrap_or("").to_owned();
            let mut absorbed = Vec::new();
            let mut next = doc.next_sibling(text);
            while next != NodeId::NONE && doc.kind(next) == NodeKind::Text {
                merged.push_str(doc.text_data(next).unwrap_or(""));
                absorbed.push(next);
                next = doc.next_sibling(next);
            }
            for node in absorbed {
                doc.mutate(Mutation::Remove { child: node })
                    .map_err(|error| mutation_error(&error))?;
            }
            if merged.is_empty() {
                doc.mutate(Mutation::Remove { child: text })
                    .map_err(|error| mutation_error(&error))?;
            } else if doc.text_data(text) != Some(merged.as_str()) {
                doc.mutate(Mutation::SetText {
                    node: text,
                    text: merged,
                })
                .map_err(|error| mutation_error(&error))?;
            }
        }
        Ok(())
    }

    pub(super) fn element_sibling(&self, forward: bool) -> Option<Handle<Element>> {
        if self.is_attr() {
            return None;
        }
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let step = |node: NodeId| {
            if forward {
                doc.next_sibling(node)
            } else {
                doc.previous_sibling(node)
            }
        };
        let mut cursor = step(id);
        while cursor != NodeId::NONE {
            if doc.kind(cursor) == NodeKind::Element {
                return Some(Handle::from_bits(encode_node(
                    self.slot(),
                    doc.node_ref(cursor),
                )));
            }
            cursor = step(cursor);
        }
        None
    }

    /// Inserts `node` at the position `insertAdjacentElement` and
    /// `insertAdjacentText` name. `Ok(false)` when the position needs a
    /// parent and the element has none.
    fn insert_adjacent(&self, position: &str, node: NodeId) -> Result<bool, JsError> {
        let (doc, id) = self.anchor();
        let mut doc = doc.borrow_mut();
        let parent = doc.parent(id);
        let (target, before) = match position.to_ascii_lowercase().as_str() {
            "beforebegin" if parent == NodeId::NONE => return Ok(false),
            "beforebegin" => (parent, id),
            "afterbegin" => (id, doc.first_child(id)),
            "beforeend" => (id, NodeId::NONE),
            "afterend" if parent == NodeId::NONE => return Ok(false),
            "afterend" => (parent, doc.next_sibling(id)),
            _ => {
                return Err(JsError::dom_exception(
                    "SyntaxError",
                    &format!(
                        "The value provided ('{position}') is not one of 'beforeBegin', 'afterBegin', 'beforeEnd', or 'afterEnd'."
                    ),
                ));
            }
        };
        doc.mutate(Mutation::InsertBefore {
            parent: target,
            child: node,
            before,
        })
        .map_err(|error| mutation_error(&error))?;
        Ok(true)
    }

    pub(super) fn insert_adjacent_node(
        &self,
        position: &str,
        element: Handle<Element>,
    ) -> Result<Option<Handle<Element>>, JsError> {
        let node = self.tree_node_of(element.bits()).ok_or_else(|| {
            JsError::dom_exception(
                "NotSupportedError",
                "the element belongs to another document, and this engine cannot move a node between documents",
            )
        })?;
        Ok(self.insert_adjacent(position, node)?.then_some(element))
    }

    pub(super) fn insert_adjacent_string(&self, position: &str, data: &str) -> Result<(), JsError> {
        let text = {
            let (doc, _) = self.anchor();
            doc.borrow_mut().create_text(data)
        };
        self.insert_adjacent(position, text).map(|_| ())
    }

    /// A snapshot `HTMLCollection` of the document's elements that `keep`
    /// accepts, in tree order.
    pub(super) fn document_elements(
        &self,
        keep: impl Fn(&tenta_dom::Document, NodeId) -> bool,
    ) -> Result<Handle<HTMLCollection>, JsError> {
        let (doc, _) = self.anchor();
        let refs = {
            let doc = doc.borrow();
            doc.descendants(doc.document_node())
                .filter(|node| doc.kind(*node) == NodeKind::Element && keep(&doc, *node))
                .map(|node| doc.node_ref(node))
                .collect()
        };
        let key = self.context().snapshots.borrow_mut().insert(refs)?;
        Ok(Handle::from_bits(encode_snapshot(
            self.slot(),
            TAG_STATIC_HTML_COLLECTION,
            key,
        )))
    }

    pub(super) fn elements_named(&self, name: &str) -> Result<Handle<NodeList>, JsError> {
        let (doc, _) = self.anchor();
        let refs = {
            let doc = doc.borrow();
            doc.descendants(doc.document_node())
                .filter(|node| doc.attribute_str(*node, "name") == Some(name))
                .map(|node| doc.node_ref(node))
                .collect()
        };
        let key = self.context().snapshots.borrow_mut().insert(refs)?;
        Ok(Handle::from_bits(encode_snapshot(
            self.slot(),
            TAG_STATIC_NODE_LIST,
            key,
        )))
    }

    fn foreign_node_error() -> JsError {
        JsError::dom_exception(
            "NotSupportedError",
            "the node belongs to another document, and this engine cannot move a node between documents",
        )
    }

    pub(super) fn import(&self, node: Handle<Node>, deep: bool) -> Result<Handle<Node>, JsError> {
        let source = self
            .tree_node_of(node.bits())
            .ok_or_else(Self::foreign_node_error)?;
        let (doc, _) = self.anchor();
        let copy = {
            let mut doc = doc.borrow_mut();
            if doc.kind(source) == NodeKind::Document {
                return Err(JsError::dom_exception(
                    "NotSupportedError",
                    "A document cannot be imported.",
                ));
            }
            doc.clone_node(source, deep)
                .map_err(|error| mutation_error(&error))?
        };
        self.context().note_cloned_images(copy);
        Ok(self.node_handle(copy))
    }

    pub(super) fn adopt(&self, node: Handle<Node>) -> Result<Handle<Node>, JsError> {
        let source = self
            .tree_node_of(node.bits())
            .ok_or_else(Self::foreign_node_error)?;
        let (doc, _) = self.anchor();
        let mut doc = doc.borrow_mut();
        if doc.kind(source) == NodeKind::Document {
            return Err(JsError::dom_exception(
                "NotSupportedError",
                "A document cannot be adopted.",
            ));
        }
        let parent = doc.parent(source);
        if parent != NodeId::NONE {
            doc.mutate(Mutation::Remove { child: source })
                .map_err(|error| mutation_error(&error))?;
        }
        Ok(node)
    }
}

/// Whether `node` is a link the `links` collection lists: an `a` or `area`
/// element with an `href`.
pub(super) fn is_link(doc: &tenta_dom::Document, node: NodeId) -> bool {
    matches!(doc.tag(node), Some(tag) if tag == atoms::A || tag == atoms::AREA)
        && doc.attribute_str(node, "href").is_some()
}
