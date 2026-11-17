//! [`TreeSink`] over the arena: the parser writes elements and text straight
//! into the document, with no intermediate tree
//! (`docs/spec/04-dom-and-css.md` §3.3).
//!
//! Handles are [`NodeId`]s, so every call is an array index. Every structural
//! step goes through [`Document::mutate`], so the parser cannot bypass the
//! index and observer bookkeeping.

use std::borrow::Cow;

use tenta_core::NodeId;
use tenta_core::atom::atoms;
use tenta_html::{
    Attribute, ElementFlags, NodeOrText, ParseControl, QualName, QuirksMode, TreeSink,
};

use crate::arena::{NodeFlags, NodeKind};
use crate::document::Document;
use crate::mutation::{Mutation, ShadowRootMode};

impl Document {
    /// Inserts `child` into `parent`, reporting a parse error rather than
    /// panicking: the tree builder must never produce an invalid insertion,
    /// but a corrupt one must not take the process down.
    fn sink_insert(&mut self, parent: NodeId, child: NodeId, before: NodeId) {
        let mutation = if before.is_some() {
            Mutation::InsertBefore {
                parent,
                child,
                before,
            }
        } else {
            Mutation::Append { parent, child }
        };
        if let Err(error) = self.mutate(mutation) {
            self.parse_error(Cow::Owned(format!("sink insertion refused: {error}")));
        }
    }

    fn sink_append(&mut self, parent: NodeId, child: NodeOrText<'_, NodeId>) {
        match child {
            NodeOrText::Node(node) => self.sink_insert(parent, node, NodeId::NONE),
            NodeOrText::Text(text) => {
                if let Err(error) = self.append_text(parent, text) {
                    self.parse_error(Cow::Owned(format!("sink text append refused: {error}")));
                }
            }
        }
    }
}

impl TreeSink for Document {
    type Handle = NodeId;
    type Output = Document;

    fn document(&mut self) -> NodeId {
        self.document_node()
    }

    fn create_element(
        &mut self,
        name: QualName,
        attrs: Vec<Attribute>,
        flags: ElementFlags,
    ) -> NodeId {
        match Document::create_element(self, name, attrs, flags) {
            Ok(element) => element,
            Err(error) => {
                self.parse_error(Cow::Owned(format!("element creation refused: {error}")));
                // Return a placeholder; the parser will continue with undefined behavior
                // but won't panic. This shouldn't happen in practice since the parser
                // validates names according to the spec.
                NodeId::NONE
            }
        }
    }

    fn create_comment(&mut self, text: &str) -> NodeId {
        Document::create_comment(self, text)
    }

    fn create_pi(&mut self, target: &str, data: &str) -> NodeId {
        Document::create_processing_instruction(self, target, data)
    }

    fn append(&mut self, parent: &NodeId, child: NodeOrText<'_, NodeId>) {
        self.sink_append(*parent, child);
    }

    fn append_before_sibling(&mut self, sibling: &NodeId, child: NodeOrText<'_, NodeId>) {
        let parent = self.parent(*sibling);
        if !parent.is_some() {
            self.parse_error(Cow::Borrowed(
                "append_before_sibling on a node without a parent",
            ));
            return;
        }
        match child {
            NodeOrText::Node(node) => self.sink_insert(parent, node, *sibling),
            NodeOrText::Text(text) => {
                if let Err(error) = self.insert_text_before(*sibling, text) {
                    self.parse_error(Cow::Owned(format!("sink text insert refused: {error}")));
                }
            }
        }
    }

    fn append_based_on_parent_node(
        &mut self,
        element: &NodeId,
        prev_element: &NodeId,
        child: NodeOrText<'_, NodeId>,
    ) {
        if self.parent(*element).is_some() {
            self.append_before_sibling(element, child);
        } else {
            self.sink_append(*prev_element, child);
        }
    }

    fn append_doctype_to_document(&mut self, name: &str, public_id: &str, system_id: &str) {
        let doctype = self.create_doctype(name, public_id, system_id);
        let document = self.document_node();
        self.sink_insert(document, doctype, NodeId::NONE);
    }

    fn add_attrs_if_missing(&mut self, target: &NodeId, attrs: Vec<Attribute>) {
        if let Err(error) = self.add_attributes_if_missing(*target, attrs) {
            self.parse_error(Cow::Owned(format!(
                "sink attribute insertion refused: {error}"
            )));
        }
    }

    fn remove_from_parent(&mut self, target: &NodeId) {
        if let Err(error) = self.mutate(Mutation::Remove { child: *target }) {
            self.parse_error(Cow::Owned(format!("sink removal refused: {error}")));
        }
    }

    fn reparent_children(&mut self, node: &NodeId, new_parent: &NodeId) {
        if let Err(error) = self.mutate(Mutation::ReparentChildren {
            from: *node,
            to: *new_parent,
        }) {
            self.parse_error(Cow::Owned(format!("sink reparent refused: {error}")));
        }
    }

    fn get_template_contents(&mut self, target: &NodeId) -> NodeId {
        match self.template_contents(*target) {
            Some(contents) => contents,
            None => {
                // The tree builder only asks for a <template>'s contents, so
                // reaching here means the element was created without the
                // template flag; give it one rather than fail the parse.
                self.parse_error(Cow::Borrowed(
                    "template contents requested for a non-template element",
                ));
                *target
            }
        }
    }

    fn attach_declarative_shadow_root(&mut self, host: &NodeId, closed: bool) -> Option<NodeId> {
        let mode = if closed {
            ShadowRootMode::Closed
        } else {
            ShadowRootMode::Open
        };
        Document::attach_declarative_shadow_root(self, *host, mode)
    }

    fn element_name(&self, target: &NodeId) -> QualName {
        Document::element_name(self, *target)
            .unwrap_or_else(|| QualName::new(atoms::EMPTY, atoms::EMPTY))
    }

    fn is_mathml_annotation_xml_integration_point(&self, target: &NodeId) -> bool {
        self.flags(*target)
            .contains(NodeFlags::MATHML_INTEGRATION_POINT)
    }

    fn set_quirks_mode(&mut self, mode: QuirksMode) {
        Document::set_quirks_mode(self, mode);
    }

    fn mark_script_already_started(&mut self, node: &NodeId) {
        self.set_script_already_started(*node);
    }

    fn pop(&mut self, node: &NodeId) -> ParseControl {
        // A void TreeSink operation may have reached a sticky observer budget
        // or native failure. Stop before another parser-blocking script; the
        // page reads the original typed failure from the Document.
        if self.check_observer_health().is_err() {
            self.stopped = true;
            return ParseControl::Stop;
        }
        // The predicate is moved out for the call: it takes `&Document`, and
        // it lives in the document it inspects.
        let Some(mut predicate) = self.stop_after.take() else {
            return ParseControl::Continue;
        };
        let stop = predicate(self, *node);
        self.stop_after = Some(predicate);
        if stop {
            self.stopped = true;
            ParseControl::Stop
        } else {
            ParseControl::Continue
        }
    }

    fn maybe_clone_option_into_selectedcontent(&mut self, option: &NodeId) {
        if let Err(error) = self.clone_option_into_selectedcontent(*option) {
            self.parse_error(Cow::Owned(format!(
                "selected-content cloning refused: {error}"
            )));
        }
    }

    fn parse_error(&mut self, message: Cow<'static, str>) {
        self.record_parse_error(message);
    }

    fn set_current_line(&mut self, line: u64) {
        self.set_parse_line(line);
    }

    fn finish(self) -> Document {
        self
    }
}

/// Parses a fragment into an existing arena without attaching the parser's
/// synthetic `<html>` root to the live document. All writes still use the
/// normal document sink and its mutation validation and bookkeeping.
pub(crate) struct FragmentSink<'a> {
    document: &'a mut Document,
    container: NodeId,
}

impl<'a> FragmentSink<'a> {
    pub(crate) fn new(document: &'a mut Document) -> Self {
        let container = document.create_fragment();
        Self {
            document,
            container,
        }
    }
}

impl TreeSink for FragmentSink<'_> {
    type Handle = NodeId;
    type Output = tenta_core::Result<NodeId>;

    fn document(&mut self) -> NodeId {
        self.container
    }

    fn create_element(
        &mut self,
        name: QualName,
        attrs: Vec<Attribute>,
        flags: ElementFlags,
    ) -> NodeId {
        TreeSink::create_element(self.document, name, attrs, flags)
    }

    fn create_comment(&mut self, text: &str) -> NodeId {
        TreeSink::create_comment(self.document, text)
    }

    fn create_pi(&mut self, target: &str, data: &str) -> NodeId {
        TreeSink::create_pi(self.document, target, data)
    }

    fn append(&mut self, parent: &NodeId, child: NodeOrText<'_, NodeId>) {
        TreeSink::append(self.document, parent, child);
    }

    fn append_before_sibling(&mut self, sibling: &NodeId, child: NodeOrText<'_, NodeId>) {
        TreeSink::append_before_sibling(self.document, sibling, child);
    }

    fn append_based_on_parent_node(
        &mut self,
        element: &NodeId,
        prev_element: &NodeId,
        child: NodeOrText<'_, NodeId>,
    ) {
        TreeSink::append_based_on_parent_node(self.document, element, prev_element, child);
    }

    fn append_doctype_to_document(&mut self, name: &str, public_id: &str, system_id: &str) {
        let doctype = self.document.create_doctype(name, public_id, system_id);
        self.document
            .sink_insert(self.container, doctype, NodeId::NONE);
    }

    fn add_attrs_if_missing(&mut self, target: &NodeId, attrs: Vec<Attribute>) {
        TreeSink::add_attrs_if_missing(self.document, target, attrs);
    }

    fn remove_from_parent(&mut self, target: &NodeId) {
        TreeSink::remove_from_parent(self.document, target);
    }

    fn reparent_children(&mut self, node: &NodeId, new_parent: &NodeId) {
        TreeSink::reparent_children(self.document, node, new_parent);
    }

    fn get_template_contents(&mut self, target: &NodeId) -> NodeId {
        TreeSink::get_template_contents(self.document, target)
    }

    fn attach_declarative_shadow_root(&mut self, host: &NodeId, closed: bool) -> Option<NodeId> {
        TreeSink::attach_declarative_shadow_root(self.document, host, closed)
    }

    fn element_name(&self, target: &NodeId) -> QualName {
        TreeSink::element_name(self.document, target)
    }

    fn is_mathml_annotation_xml_integration_point(&self, target: &NodeId) -> bool {
        TreeSink::is_mathml_annotation_xml_integration_point(self.document, target)
    }

    fn set_quirks_mode(&mut self, mode: QuirksMode) {
        TreeSink::set_quirks_mode(self.document, mode);
    }

    fn mark_script_already_started(&mut self, node: &NodeId) {
        TreeSink::mark_script_already_started(self.document, node);
    }

    fn pop(&mut self, node: &NodeId) -> ParseControl {
        TreeSink::pop(self.document, node)
    }

    fn maybe_clone_option_into_selectedcontent(&mut self, option: &NodeId) {
        TreeSink::maybe_clone_option_into_selectedcontent(self.document, option);
    }

    fn parse_error(&mut self, message: Cow<'static, str>) {
        TreeSink::parse_error(self.document, message);
    }

    fn set_current_line(&mut self, line: u64) {
        TreeSink::set_current_line(self.document, line);
    }

    fn finish(self) -> Self::Output {
        let root = self.document.first_child(self.container);
        if root.is_some() {
            Ok(root)
        } else {
            Err(tenta_core::Error::invalid(
                "fragment parsing produced no root",
            ))
        }
    }
}

impl Document {
    /// Marks a `<script>` as already started; the tree builder calls it when
    /// the element is popped.
    pub fn set_script_already_started(&mut self, node: NodeId) {
        if self.kind(node) == NodeKind::Element {
            self.set_flag(node, NodeFlags::SCRIPT_STARTED, true);
        }
    }

    /// Whether a `<script>` element was already started.
    pub fn script_already_started(&self, node: NodeId) -> bool {
        self.flags(node).contains(NodeFlags::SCRIPT_STARTED)
    }
}
