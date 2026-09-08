//! The contract between the parsers in this crate and the tree they build
//! (`docs/spec/04-dom-and-css.md` §3.3: "direct arena construction").
//!
//! `tenta-html` sits above `tenta-dom` in the dependency order, so it never
//! sees the arena. The tree builder drives a [`TreeSink`]; `tenta-dom`
//! implements it over the arena and tests in this crate implement it over a
//! small owned tree. The shape follows html5ever's `TreeSink` (the state
//! machines are derived from html5ever) with two changes: text arrives as
//! `&str` slices of the decoded input, never as owned buffers, and
//! [`TreeSink::pop`] can stop the parse early (the `search` fast path stops
//! once the results container closes, `docs/spec/08-search.md` §3).

use std::borrow::Cow;

use tenta_core::Atom;

/// A namespaced name. `ns` and `local` are always interned; `prefix` is set
/// only for XML input and foreign-content attributes such as `xlink:href`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct QualName {
    pub ns: Atom,
    pub prefix: Option<Atom>,
    pub local: Atom,
}

impl QualName {
    pub const fn new(ns: Atom, local: Atom) -> Self {
        Self {
            ns,
            prefix: None,
            local,
        }
    }
}

/// One attribute as the tokenizer produced it, after duplicate removal.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Attribute {
    pub name: QualName,
    pub value: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum QuirksMode {
    #[default]
    NoQuirks,
    LimitedQuirks,
    Quirks,
}

/// Flags the tree builder knows at element creation time.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ElementFlags {
    /// A `<template>` element; the sink must own a separate contents fragment
    /// that [`TreeSink::get_template_contents`] returns.
    pub template: bool,
    /// A MathML `<annotation-xml>` whose `encoding` makes it an HTML
    /// integration point; the tree builder asks back through
    /// [`TreeSink::is_mathml_annotation_xml_integration_point`].
    pub mathml_annotation_xml_integration_point: bool,
}

/// What gets appended: an existing node or a run of text.
#[derive(Debug)]
pub enum NodeOrText<'a, H> {
    Node(H),
    Text(&'a str),
}

/// Returned by [`TreeSink::pop`] after an element is closed.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ParseControl {
    #[default]
    Continue,
    /// Stop feeding the tree builder; the parser reports `Stopped` and the
    /// tree stays as built so far.
    Stop,
}

/// The tree the parser writes into.
///
/// Handles are cheap to clone (an arena index in `tenta-dom`). Every method
/// is called on the parsing thread only. Methods with a default body are
/// optional notifications.
pub trait TreeSink {
    /// Node handle type.
    type Handle: Clone + Eq;
    /// What `finish` yields (the document, or the sink itself).
    type Output;

    /// The document node every parse starts from.
    fn document(&mut self) -> Self::Handle;

    fn create_element(
        &mut self,
        name: QualName,
        attrs: Vec<Attribute>,
        flags: ElementFlags,
    ) -> Self::Handle;
    fn create_comment(&mut self, text: &str) -> Self::Handle;
    fn create_pi(&mut self, target: &str, data: &str) -> Self::Handle;

    /// Appends `child` as the last child of `parent`. Adjacent text is
    /// merged into one text node by the sink.
    fn append(&mut self, parent: &Self::Handle, child: NodeOrText<'_, Self::Handle>);
    /// Appends `child` before `sibling` (foster parenting and the
    /// "insert before" cases of the tree construction algorithm).
    fn append_before_sibling(
        &mut self,
        sibling: &Self::Handle,
        child: NodeOrText<'_, Self::Handle>,
    );
    /// If `element` has a parent, inserts `child` before `element`;
    /// otherwise appends to `prev_element` (the template case of the
    /// "appropriate place for inserting a node" algorithm).
    fn append_based_on_parent_node(
        &mut self,
        element: &Self::Handle,
        prev_element: &Self::Handle,
        child: NodeOrText<'_, Self::Handle>,
    );
    fn append_doctype_to_document(&mut self, name: &str, public_id: &str, system_id: &str);

    /// Adds attributes that `target` does not already have (`<html>` and
    /// `<body>` start tags seen twice).
    fn add_attrs_if_missing(&mut self, target: &Self::Handle, attrs: Vec<Attribute>);
    fn remove_from_parent(&mut self, target: &Self::Handle);
    fn reparent_children(&mut self, node: &Self::Handle, new_parent: &Self::Handle);
    fn get_template_contents(&mut self, target: &Self::Handle) -> Self::Handle;

    fn same_node(&self, a: &Self::Handle, b: &Self::Handle) -> bool {
        a == b
    }
    fn element_name(&self, target: &Self::Handle) -> QualName;
    fn is_mathml_annotation_xml_integration_point(&self, _target: &Self::Handle) -> bool {
        false
    }

    fn set_quirks_mode(&mut self, mode: QuirksMode);
    /// Called when a `<script>` element is popped off the stack of open
    /// elements. Tier 1 sinks ignore it; the Tier 2 page uses it to run the
    /// script (`docs/spec/05-javascript.md`).
    fn mark_script_already_started(&mut self, _node: &Self::Handle) {}
    /// Called after every end tag with the element that was closed. Return
    /// [`ParseControl::Stop`] to end the parse early.
    fn pop(&mut self, _node: &Self::Handle) -> ParseControl {
        ParseControl::Continue
    }
    /// Called when an `<option>` element is closed, explicitly or
    /// implicitly. A customizable `<select>` mirrors the selected option's
    /// children into the `<selectedcontent>` inside its button; cloning nodes
    /// needs tree access, so the parser only reports the close.
    fn maybe_clone_option_into_selectedcontent(&mut self, _option: &Self::Handle) {}
    fn parse_error(&mut self, _msg: Cow<'static, str>) {}
    fn set_current_line(&mut self, _line: u64) {}

    /// Ends the parse and hands back the built tree.
    fn finish(self) -> Self::Output;
}
