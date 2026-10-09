//! [`TreeSink`] over a document the parser shares with running script
//! (`docs/spec/05-javascript.md` §6).
//!
//! The Tier 1 sink is `Document` itself: the parser owns it and hands it back
//! from `finish`. That cannot work once a `<script>` runs during the parse,
//! because the script needs the same document at the same time. [`SharedSink`]
//! keeps the document behind an `Rc<RefCell<_>>` and borrows it for the body
//! of each sink call only, so the borrow is always released before control can
//! reach script.
//!
//! Three invariants hold that discipline up, and each one is load-bearing:
//!
//! * **Nothing `Document` calls from inside a sink method may reach back
//!   through the shared handle.** Two hooks run inside a `borrow_mut`: the
//!   mutation-observer sink (`Document::mutate`) and the stop predicate
//!   (`TreeSink::pop`). Both are `Send`, and `Rc<RefCell<Document>>` is not,
//!   so neither can hold this handle today. A real `MutationObserver` that
//!   queues records for script must keep that property — it has to record
//!   into the document it was handed, not borrow the shared one.
//! * **The parser never calls script from inside a sink call.** A closed
//!   `<script>` is reported to the parser's *caller*
//!   (`tenta_html::ParseStatus::Script`), with no borrow outstanding.
//! * **Every handle published to the parser stays pinned until it finishes.**
//!   The tree builder keeps bare `NodeId`s, while the sink owns one native
//!   `NodeHandle` for each published node. A pressure sweep between `feed`
//!   calls can reclaim unrelated script-created subtrees, but cannot recycle
//!   a parser handle or discard observable wrapper state on its subtree.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use tenta_core::{NodeId, NodeRef};
use tenta_html::{
    Attribute, ElementFlags, NodeOrText, ParseControl, QualName, QuirksMode, TreeSink,
};

use crate::document::{Document, NodeHandle};

/// A document held jointly by the parser and the script that runs against it.
pub type SharedDocument = Rc<RefCell<Document>>;

/// Writes the parse into a [`SharedDocument`].
///
/// Every method borrows for its own body and no longer. A sink call never
/// calls into script — the parser reports a closed `<script>` to *its* caller
/// instead (`ParseStatus::Script`), which runs the script with no sink borrow
/// outstanding.
#[derive(Debug)]
pub struct SharedSink {
    document: SharedDocument,
    pins: HashMap<NodeRef, NodeHandle>,
}

impl SharedSink {
    /// Wraps `document`. The caller keeps its own clone: that handle is what
    /// the script engine binds `document` to.
    pub fn new(document: SharedDocument) -> SharedSink {
        SharedSink {
            document,
            pins: HashMap::new(),
        }
    }

    /// A sink over a fresh empty document, and the handle to it.
    pub fn with_new_document() -> (SharedSink, SharedDocument) {
        let document = Rc::new(RefCell::new(Document::new()));
        (SharedSink::new(document.clone()), document)
    }

    /// The shared document.
    pub fn document(&self) -> SharedDocument {
        self.document.clone()
    }

    // NodeId::NONE already denotes a failed native allocation. Every live
    // handle returned to the tree builder is retained exactly once; clones
    // of its cheap NodeId cannot outlive this sink's parsing lifetime.
    fn pin(&mut self, node: NodeId) -> NodeId {
        if node.is_some() {
            let mut document = self.document.borrow_mut();
            let reference = document.node_ref(node);
            self.pins.entry(reference).or_insert_with(|| {
                document
                    .handle(node)
                    .expect("a published parser node is live")
            });
        }
        node
    }
}

impl Drop for SharedSink {
    fn drop(&mut self) {
        if self.pins.is_empty() {
            return;
        }
        match self.document.try_borrow_mut() {
            Ok(mut document) => {
                for (_, pin) in self.pins.drain() {
                    document.release(pin);
                }
            }
            Err(_) if std::thread::panicking() => {
                // The failed document will tear down with its remaining pins.
                // Releasing through an active native borrow is impossible, and
                // a second panic here would abort instead of isolating failure.
            }
            Err(_) => panic!("parser sink dropped while its document is borrowed"),
        }
    }
}

impl TreeSink for SharedSink {
    type Handle = NodeId;
    type Output = SharedDocument;

    fn document(&mut self) -> NodeId {
        let node = TreeSink::document(&mut *self.document.borrow_mut());
        self.pin(node)
    }

    fn create_element(
        &mut self,
        name: QualName,
        attrs: Vec<Attribute>,
        flags: ElementFlags,
    ) -> NodeId {
        let node = TreeSink::create_element(&mut *self.document.borrow_mut(), name, attrs, flags);
        self.pin(node)
    }

    fn create_comment(&mut self, text: &str) -> NodeId {
        let node = TreeSink::create_comment(&mut *self.document.borrow_mut(), text);
        self.pin(node)
    }

    fn create_pi(&mut self, target: &str, data: &str) -> NodeId {
        let node = TreeSink::create_pi(&mut *self.document.borrow_mut(), target, data);
        self.pin(node)
    }

    fn append(&mut self, parent: &NodeId, child: NodeOrText<'_, NodeId>) {
        TreeSink::append(&mut *self.document.borrow_mut(), parent, child);
    }

    fn append_before_sibling(&mut self, sibling: &NodeId, child: NodeOrText<'_, NodeId>) {
        TreeSink::append_before_sibling(&mut *self.document.borrow_mut(), sibling, child);
    }

    fn append_based_on_parent_node(
        &mut self,
        element: &NodeId,
        prev_element: &NodeId,
        child: NodeOrText<'_, NodeId>,
    ) {
        TreeSink::append_based_on_parent_node(
            &mut *self.document.borrow_mut(),
            element,
            prev_element,
            child,
        );
    }

    fn append_doctype_to_document(&mut self, name: &str, public_id: &str, system_id: &str) {
        TreeSink::append_doctype_to_document(
            &mut *self.document.borrow_mut(),
            name,
            public_id,
            system_id,
        );
    }

    fn add_attrs_if_missing(&mut self, target: &NodeId, attrs: Vec<Attribute>) {
        TreeSink::add_attrs_if_missing(&mut *self.document.borrow_mut(), target, attrs);
    }

    fn remove_from_parent(&mut self, target: &NodeId) {
        TreeSink::remove_from_parent(&mut *self.document.borrow_mut(), target);
    }

    fn reparent_children(&mut self, node: &NodeId, new_parent: &NodeId) {
        TreeSink::reparent_children(&mut *self.document.borrow_mut(), node, new_parent);
    }

    fn get_template_contents(&mut self, target: &NodeId) -> NodeId {
        let node = TreeSink::get_template_contents(&mut *self.document.borrow_mut(), target);
        self.pin(node)
    }

    fn element_name(&self, target: &NodeId) -> QualName {
        TreeSink::element_name(&*self.document.borrow(), target)
    }

    fn is_mathml_annotation_xml_integration_point(&self, target: &NodeId) -> bool {
        TreeSink::is_mathml_annotation_xml_integration_point(&*self.document.borrow(), target)
    }

    fn set_quirks_mode(&mut self, mode: QuirksMode) {
        TreeSink::set_quirks_mode(&mut *self.document.borrow_mut(), mode);
    }

    fn mark_script_already_started(&mut self, node: &NodeId) {
        TreeSink::mark_script_already_started(&mut *self.document.borrow_mut(), node);
    }

    fn pop(&mut self, node: &NodeId) -> ParseControl {
        TreeSink::pop(&mut *self.document.borrow_mut(), node)
    }

    fn maybe_clone_option_into_selectedcontent(&mut self, option: &NodeId) {
        TreeSink::maybe_clone_option_into_selectedcontent(&mut *self.document.borrow_mut(), option);
    }

    fn parse_error(&mut self, message: Cow<'static, str>) {
        TreeSink::parse_error(&mut *self.document.borrow_mut(), message);
    }

    fn set_current_line(&mut self, line: u64) {
        TreeSink::set_current_line(&mut *self.document.borrow_mut(), line);
    }

    /// Hands back the shared handle rather than the document by value: the
    /// script engine holds a clone of it, so the document cannot be moved out.
    fn finish(self) -> SharedDocument {
        self.document.clone()
    }
}
