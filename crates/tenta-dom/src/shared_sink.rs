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
//! * **Nothing frees nodes while a parse is in flight.** The tree builder's
//!   stack of open elements holds bare `NodeId`s, and arena slots are
//!   recycled, so a `Document::sweep_detached` through the shared handle
//!   between two `feed` calls could hand a recycled slot back to the parser.
//!   The owning sink made that impossible; sharing makes it merely illegal.

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::NodeId;
use tenta_html::{
    Attribute, ElementFlags, NodeOrText, ParseControl, QualName, QuirksMode, TreeSink,
};

use crate::document::Document;

/// A document held jointly by the parser and the script that runs against it.
pub type SharedDocument = Rc<RefCell<Document>>;

/// Writes the parse into a [`SharedDocument`].
///
/// Every method borrows for its own body and no longer. A sink call never
/// calls into script — the parser reports a closed `<script>` to *its* caller
/// instead (`ParseStatus::Script`), which runs the script with no sink borrow
/// outstanding.
#[derive(Debug)]
pub struct SharedSink(SharedDocument);

impl SharedSink {
    /// Wraps `document`. The caller keeps its own clone: that handle is what
    /// the script engine binds `document` to.
    pub fn new(document: SharedDocument) -> SharedSink {
        SharedSink(document)
    }

    /// A sink over a fresh empty document, and the handle to it.
    pub fn with_new_document() -> (SharedSink, SharedDocument) {
        let document = Rc::new(RefCell::new(Document::new()));
        (SharedSink(document.clone()), document)
    }

    /// The shared document.
    pub fn document(&self) -> SharedDocument {
        self.0.clone()
    }
}

impl TreeSink for SharedSink {
    type Handle = NodeId;
    type Output = SharedDocument;

    fn document(&mut self) -> NodeId {
        TreeSink::document(&mut *self.0.borrow_mut())
    }

    fn create_element(
        &mut self,
        name: QualName,
        attrs: Vec<Attribute>,
        flags: ElementFlags,
    ) -> NodeId {
        TreeSink::create_element(&mut *self.0.borrow_mut(), name, attrs, flags)
    }

    fn create_comment(&mut self, text: &str) -> NodeId {
        TreeSink::create_comment(&mut *self.0.borrow_mut(), text)
    }

    fn create_pi(&mut self, target: &str, data: &str) -> NodeId {
        TreeSink::create_pi(&mut *self.0.borrow_mut(), target, data)
    }

    fn append(&mut self, parent: &NodeId, child: NodeOrText<'_, NodeId>) {
        TreeSink::append(&mut *self.0.borrow_mut(), parent, child);
    }

    fn append_before_sibling(&mut self, sibling: &NodeId, child: NodeOrText<'_, NodeId>) {
        TreeSink::append_before_sibling(&mut *self.0.borrow_mut(), sibling, child);
    }

    fn append_based_on_parent_node(
        &mut self,
        element: &NodeId,
        prev_element: &NodeId,
        child: NodeOrText<'_, NodeId>,
    ) {
        TreeSink::append_based_on_parent_node(
            &mut *self.0.borrow_mut(),
            element,
            prev_element,
            child,
        );
    }

    fn append_doctype_to_document(&mut self, name: &str, public_id: &str, system_id: &str) {
        TreeSink::append_doctype_to_document(&mut *self.0.borrow_mut(), name, public_id, system_id);
    }

    fn add_attrs_if_missing(&mut self, target: &NodeId, attrs: Vec<Attribute>) {
        TreeSink::add_attrs_if_missing(&mut *self.0.borrow_mut(), target, attrs);
    }

    fn remove_from_parent(&mut self, target: &NodeId) {
        TreeSink::remove_from_parent(&mut *self.0.borrow_mut(), target);
    }

    fn reparent_children(&mut self, node: &NodeId, new_parent: &NodeId) {
        TreeSink::reparent_children(&mut *self.0.borrow_mut(), node, new_parent);
    }

    fn get_template_contents(&mut self, target: &NodeId) -> NodeId {
        TreeSink::get_template_contents(&mut *self.0.borrow_mut(), target)
    }

    fn element_name(&self, target: &NodeId) -> QualName {
        TreeSink::element_name(&*self.0.borrow(), target)
    }

    fn is_mathml_annotation_xml_integration_point(&self, target: &NodeId) -> bool {
        TreeSink::is_mathml_annotation_xml_integration_point(&*self.0.borrow(), target)
    }

    fn set_quirks_mode(&mut self, mode: QuirksMode) {
        TreeSink::set_quirks_mode(&mut *self.0.borrow_mut(), mode);
    }

    fn mark_script_already_started(&mut self, node: &NodeId) {
        TreeSink::mark_script_already_started(&mut *self.0.borrow_mut(), node);
    }

    fn pop(&mut self, node: &NodeId) -> ParseControl {
        TreeSink::pop(&mut *self.0.borrow_mut(), node)
    }

    fn maybe_clone_option_into_selectedcontent(&mut self, option: &NodeId) {
        TreeSink::maybe_clone_option_into_selectedcontent(&mut *self.0.borrow_mut(), option);
    }

    fn parse_error(&mut self, message: Cow<'static, str>) {
        TreeSink::parse_error(&mut *self.0.borrow_mut(), message);
    }

    fn set_current_line(&mut self, line: u64) {
        TreeSink::set_current_line(&mut *self.0.borrow_mut(), line);
    }

    /// Hands back the shared handle rather than the document by value: the
    /// script engine holds a clone of it, so the document cannot be moved out.
    fn finish(self) -> SharedDocument {
        self.0
    }
}
