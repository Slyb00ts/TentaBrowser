// Derived from html5ever's `src/tree_builder/mod.rs`.
// Copyright 2014-2017 The html5ever Project Developers, MIT/Apache-2.0.
// See THIRD_PARTY_NOTICES.md ("Derived code").

//! The HTML tree builder.
//!
//! Differences from html5ever (`docs/spec/04-dom-and-css.md` §3.3):
//!
//! * The tree is built directly through [`TreeSink`]; text is appended as
//!   `&str` slices of the input, never as owned buffers.
//! * The stack of open elements caches each element's name, so the hot loops
//!   never call back into the sink.
//! * The stack of open elements is unbounded, but the tree depth is capped
//!   at [`MAX_DOM_DEPTH`]: an element whose parent would exceed the cap is
//!   inserted as a child of the deepest allowed open ancestor instead
//!   (Blink's model). Every stack entry carries its depth and an upper bound
//!   on the depth of its subtree, so the cap holds through the adoption
//!   agency too: a run that would move a subtree past the cap is treated as
//!   having no furthest block, which leaves the tree as it is.
//!   Quadratic operations on the stack (scope checks, adoption agency) are
//!   bounded: scope checks skip deeper than 4096 entries, adoption-agency
//!   searches stop 512 entries below the formatting element.
//! * [`TreeSink::pop`] can end the parse early.

use std::borrow::Cow::{self, Borrowed};
use std::collections::VecDeque;

use tenta_core::Atom;
use tenta_core::atom::atoms as core_atoms;

use crate::names::{ElemName, Local, NameCache, Ns, elem_name_of};
use crate::sink::{
    Attribute, ElementFlags, NodeOrText, ParseControl, QualName, QuirksMode, TreeSink,
};
use crate::tokenizer::interface::Token as TokToken;
use crate::tokenizer::states::RawKind;
use crate::tokenizer::{Doctype, Tag, TagKind, TokenSink, TokenSinkResult};

mod adjust;
mod data;
mod rules;
pub(crate) mod tag_sets;
pub(crate) mod types;

use tag_sets::*;
use types::*;

/// Blink's `kMaximumHTMLParserDOMTreeDepth`.
pub const MAX_DOM_DEPTH: usize = 512;

/// Deepest element that may still receive children without exceeding
/// [`MAX_DOM_DEPTH`]. `<html>` is at depth 1.
const MAX_PARENT_DEPTH: u32 = MAX_DOM_DEPTH as u32 - 1;

/// Highest index in the stack of open elements that can be at
/// [`MAX_PARENT_DEPTH`]: `open_elems[i]` is at depth `i + 1` until the
/// adoption agency or the depth cap itself breaks that correspondence, so
/// the retargeting walk starts here and only climbs further on trees that
/// are already at the cap.
const MAX_PARENT_INDEX: usize = MAX_DOM_DEPTH - 2;

/// Maximum depth to walk when checking if an element is in scope. Prevents
/// quadratic behavior on deeply nested inputs.
pub(crate) const MAX_SCOPE_WALK: usize = 4096;

/// Maximum distance to search from a formatting element when looking for
/// the furthest block in the adoption agency. Prevents quadratic behavior.
const MAX_AA_SEARCH: usize = 512;

/// Longest list of active formatting elements the tree builder keeps.
///
/// The Noah's Ark clause bounds identical entries at three, but distinct
/// attributes defeat it: `<b id=x0><b id=x1>…` grows the list without bound
/// and every reconstruction and every Noah's Ark scan walks all of it. Blink
/// bounds the list the same way; past the cap the oldest entry is dropped, so
/// the scans stay bounded and nesting stays linear. No html5lib or WPT case
/// comes near the cap.
const MAX_ACTIVE_FORMATTING: usize = MAX_DOM_DEPTH;

/// Tree builder configuration.
#[derive(Copy, Clone, Debug)]
pub(crate) struct TreeBuilderOpts {
    /// Whether `<noscript>` contents are parsed as text.
    pub scripting_enabled: bool,
    /// Whether the document comes from an `<iframe srcdoc>`.
    pub iframe_srcdoc: bool,
    /// Whether to drop the DOCTYPE instead of appending it.
    pub drop_doctype: bool,
    /// Initial quirks mode.
    pub quirks_mode: QuirksMode,
    /// Maximum number of nodes (elements, text, comments) the tree builder
    /// creates. Past this limit, formatting reconstruction becomes a no-op and
    /// new element start tags are treated as parse errors (no node created, no
    /// stack push). Text and comment insertion continues unbounded.
    pub max_dom_nodes: usize,
}

impl Default for TreeBuilderOpts {
    fn default() -> Self {
        TreeBuilderOpts {
            scripting_enabled: true,
            iframe_srcdoc: false,
            drop_doctype: false,
            quirks_mode: QuirksMode::NoQuirks,
            max_dom_nodes: 4_194_304, // 2^22
        }
    }
}

/// Whether an element is pushed on the stack of open elements.
enum PushFlag {
    Push,
    NoPush,
}

enum Bookmark<Handle> {
    Replace(Handle),
    InsertAfter(Handle),
}

/// The HTML tree builder.
pub(crate) struct TreeBuilder<Handle, Sink> {
    opts: TreeBuilderOpts,
    /// The tree this builder writes into.
    pub sink: Sink,
    mode: InsertionMode,
    orig_mode: Option<InsertionMode>,
    template_modes: Vec<InsertionMode>,
    pending_table_text: Vec<(SplitStatus, String)>,
    quirks_mode: QuirksMode,
    doc_handle: Handle,
    open_elems: Vec<OpenElem<Handle>>,
    active_formatting: Vec<FormatEntry<Handle>>,
    head_elem: Option<Handle>,
    form_elem: Option<Handle>,
    frameset_ok: bool,
    ignore_lf: bool,
    foster_parenting: bool,
    context_elem: Option<OpenElem<Handle>>,
    current_line: u64,
    /// Set when the sink returned [`ParseControl::Stop`] from `pop`.
    stopped: bool,
    names: NameCache,
    /// How many HTML elements with each name are on the stack of open
    /// elements. A scope search for a name that is not there at all is the
    /// common case and must not walk a deep stack
    /// (`docs/spec/04-dom-and-css.md` §3.3: deep nesting must stay linear).
    html_name_counts: Box<[u32; Local::COUNT]>,
    /// Number of nodes (elements, text, comments) created so far.
    nodes_created: usize,
}

impl<Handle, Sink> TreeBuilder<Handle, Sink>
where
    Handle: Clone + Eq,
    Sink: TreeSink<Handle = Handle>,
{
    /// A tree builder for a whole document.
    pub(crate) fn new(mut sink: Sink, opts: TreeBuilderOpts) -> Self {
        let doc_handle = sink.document();
        TreeBuilder {
            opts,
            sink,
            mode: InsertionMode::Initial,
            orig_mode: None,
            template_modes: Vec::new(),
            pending_table_text: Vec::new(),
            quirks_mode: opts.quirks_mode,
            doc_handle,
            open_elems: Vec::new(),
            active_formatting: Vec::new(),
            head_elem: None,
            form_elem: None,
            frameset_ok: true,
            ignore_lf: false,
            foster_parenting: false,
            context_elem: None,
            current_line: 1,
            stopped: false,
            names: NameCache::default(),
            html_name_counts: Box::new([0; Local::COUNT]),
            nodes_created: 0,
        }
    }

    /// A tree builder for the fragment parsing algorithm.
    pub(crate) fn new_for_fragment(
        mut sink: Sink,
        context: Handle,
        form_elem: Option<Handle>,
        opts: TreeBuilderOpts,
    ) -> Self {
        let doc_handle = sink.document();
        let context_name = elem_name_of(&sink.element_name(&context));
        let annotation_xml_integration_point =
            sink.is_mathml_annotation_xml_integration_point(&context);
        let template_modes = if context_name.is_html(Local::Template) {
            vec![InsertionMode::InTemplate]
        } else {
            Vec::new()
        };

        let mut builder = TreeBuilder {
            opts,
            sink,
            mode: InsertionMode::Initial,
            orig_mode: None,
            template_modes,
            pending_table_text: Vec::new(),
            quirks_mode: opts.quirks_mode,
            doc_handle,
            open_elems: Vec::new(),
            active_formatting: Vec::new(),
            head_elem: None,
            form_elem,
            frameset_ok: true,
            ignore_lf: false,
            foster_parenting: false,
            context_elem: Some(OpenElem {
                handle: context,
                name: context_name,
                annotation_xml_integration_point,
                depth: 1,   // Context element is at depth 1 (like html).
                deepest: 1, // Will be updated during parsing.
            }),
            current_line: 1,
            stopped: false,
            names: NameCache::default(),
            html_name_counts: Box::new([0; Local::COUNT]),
            nodes_created: 0,
        };

        // <https://html.spec.whatwg.org/multipage/#parsing-html-fragments>
        // steps 5-7: a bare <html> root holds the fragment's children.
        builder.create_root(Vec::new());
        builder.mode = builder.reset_insertion_mode();
        builder
    }

    /// The tokenizer state the fragment's context element implies.
    pub(crate) fn tokenizer_state_for_context_elem(
        &self,
        context_allows_scripting: bool,
    ) -> crate::tokenizer::states::State {
        use crate::tokenizer::states::{RawKind, State};
        let Some(context) = &self.context_elem else {
            return State::Data;
        };
        if context.name.ns != Ns::Html {
            return State::Data;
        }
        match context.name.local {
            Local::Title | Local::Textarea => State::RawData(RawKind::Rcdata),
            Local::Style | Local::Xmp | Local::Iframe | Local::Noembed | Local::Noframes => {
                State::RawData(RawKind::Rawtext)
            }
            Local::Script => State::RawData(RawKind::ScriptData),
            Local::Noscript => {
                if context_allows_scripting {
                    State::RawData(RawKind::Rawtext)
                } else {
                    State::Data
                }
            }
            Local::Plaintext => State::Plaintext,
            _ => State::Data,
        }
    }

    /// Whether this is a fragment parse.
    pub(crate) fn is_fragment(&self) -> bool {
        self.context_elem.is_some()
    }

    /// Hands the tree back.
    pub(crate) fn finish(self) -> Sink::Output {
        self.sink.finish()
    }

    //§ tree-construction driver

    fn process_to_completion<'a>(&mut self, mut token: Token<'a>) -> TokenSinkResult<Handle> {
        // Stays empty except when a character token is split.
        let mut more_tokens: VecDeque<Token<'a>> = VecDeque::new();

        loop {
            if self.stopped {
                return TokenSinkResult::Stop;
            }
            let should_ack_self_closing = matches!(
                token,
                Token::Tag(Tag {
                    self_closing: true,
                    kind: TagKind::StartTag,
                    ..
                })
            );
            let result = if self.is_foreign(&token) {
                self.step_foreign(token)
            } else {
                let mode = self.mode;
                self.step(mode, token)
            };
            match result {
                ProcessResult::Done => {
                    if should_ack_self_closing {
                        self.sink
                            .parse_error(Borrowed("Unacknowledged self-closing tag"));
                    }
                    let Some(next) = more_tokens.pop_front() else {
                        return TokenSinkResult::Continue;
                    };
                    token = next;
                }
                ProcessResult::DoneAckSelfClosing => {
                    let Some(next) = more_tokens.pop_front() else {
                        return TokenSinkResult::Continue;
                    };
                    token = next;
                }
                ProcessResult::Reprocess(mode, next) => {
                    self.mode = mode;
                    token = next;
                }
                ProcessResult::SplitWhitespace(text) => {
                    let Some((first, is_whitespace, rest)) = split_whitespace_run(text) else {
                        return TokenSinkResult::Continue;
                    };
                    let status = if is_whitespace {
                        SplitStatus::Whitespace
                    } else {
                        SplitStatus::NotWhitespace
                    };
                    token = Token::Characters(status, first);
                    if !rest.is_empty() {
                        more_tokens.push_back(Token::Characters(SplitStatus::NotSplit, rest));
                    }
                }
                ProcessResult::Script(node) => return TokenSinkResult::Script(node),
                ProcessResult::ToPlaintext => return TokenSinkResult::Plaintext,
                ProcessResult::ToRawData(kind) => return TokenSinkResult::RawData(kind),
                ProcessResult::EncodingIndicator(label) => {
                    return TokenSinkResult::EncodingIndicator(label);
                }
            }
        }
    }

    //§ appropriate place for inserting a node

    /// The appropriate place for inserting a node, and the depth an element
    /// inserted there gets.
    ///
    /// `override_target` is an index into the stack of open elements (the
    /// adoption agency's common ancestor). Without it the target is the
    /// current node, retargeted to the deepest open ancestor that is still
    /// allowed to have children once the tree is at [`MAX_DOM_DEPTH`].
    fn appropriate_place_for_insertion(
        &mut self,
        override_target: Option<usize>,
    ) -> (InsertionPoint<Handle>, u32) {
        let index = match override_target {
            Some(index) => index,
            None => {
                // Blink's depth cap: keep inserting into the deepest allowed
                // ancestor instead of growing the tree without bound. Entries
                // past MAX_PARENT_INDEX are at the cap by construction; the
                // walk only climbs when the adoption agency pushed shallower
                // entries down to it.
                let mut index = (self.open_elems.len() - 1).min(MAX_PARENT_INDEX);
                while index > 0 && self.open_elems[index].depth > MAX_PARENT_DEPTH {
                    index -= 1;
                }
                // A retargeted insertion (by index or by depth) never lands
                // on a table-section element: the current node would have
                // been foster-parented there, and a stray child of <table>
                // or <tr> breaks the table model. Template contents are
                // fine.
                if index + 1 < self.open_elems.len() {
                    while index > 0 {
                        let name = self.open_elems[index].name;
                        if name.is_html(Local::Template) {
                            // A template's contents act as if at a shallower level.
                            break;
                        }
                        if name.ns == Ns::Html
                            && matches!(
                                name.local,
                                Local::Table
                                    | Local::Tbody
                                    | Local::Thead
                                    | Local::Tfoot
                                    | Local::Tr
                            )
                        {
                            index -= 1;
                        } else {
                            break;
                        }
                    }
                }
                index
            }
        };
        let target = &self.open_elems[index];
        let target_name = target.name;
        let child_depth = target.depth + 1;
        let target = target.handle.clone();

        // The foster parenting test applies to the override target too: the
        // adoption agency inserts into a common ancestor that can be a table.
        if !(self.foster_parenting && foster_target(target_name)) {
            return if target_name.is_html(Local::Template) {
                (
                    InsertionPoint::LastChild(self.sink.get_template_contents(&target)),
                    child_depth,
                )
            } else {
                (InsertionPoint::LastChild(target), child_depth)
            };
        }

        // Foster parenting.
        for index in (0..self.open_elems.len()).rev() {
            let elem = &self.open_elems[index];
            if elem.name.is_html(Local::Template) {
                let handle = elem.handle.clone();
                let depth = elem.depth + 1;
                return (
                    InsertionPoint::LastChild(self.sink.get_template_contents(&handle)),
                    depth,
                );
            }
            if elem.name.is_html(Local::Table) {
                // A sibling of the table, or the last child of the table's
                // predecessor: the table's depth bounds both.
                return (
                    InsertionPoint::TableFosterParenting {
                        element: elem.handle.clone(),
                        prev_element: self.open_elems[index - 1].handle.clone(),
                    },
                    elem.depth,
                );
            }
        }
        (
            InsertionPoint::LastChild(self.open_elems[0].handle.clone()),
            2,
        )
    }

    fn insert_at(&mut self, point: InsertionPoint<Handle>, child: NodeOrText<'_, Handle>) {
        match point {
            InsertionPoint::LastChild(parent) => self.sink.append(&parent, child),
            InsertionPoint::TableFosterParenting {
                element,
                prev_element,
            } => self
                .sink
                .append_based_on_parent_node(&element, &prev_element, child),
        }
    }

    fn insert_appropriately(
        &mut self,
        child: NodeOrText<'_, Handle>,
        override_target: Option<usize>,
    ) {
        let (point, _) = self.appropriate_place_for_insertion(override_target);
        self.insert_at(point, child);
    }

    //§ stack of open elements

    fn current_name(&self) -> ElemName {
        self.open_elems.last().expect("no current element").name
    }

    fn current_handle(&self) -> Handle {
        self.open_elems
            .last()
            .expect("no current element")
            .handle
            .clone()
    }

    fn adjusted_current(&self) -> &OpenElem<Handle> {
        if self.open_elems.len() == 1
            && let Some(context) = &self.context_elem
        {
            return context;
        }
        self.open_elems.last().expect("no current element")
    }

    fn current_node_in(&self, set: impl Fn(ElemName) -> bool) -> bool {
        set(self.current_name())
    }

    fn current_node_named(&self, local: Local) -> bool {
        self.current_name().is_html(local)
    }

    fn in_html_elem_named(&self, local: Local) -> bool {
        self.html_name_counts[local as usize] > 0
    }

    fn elem_in(&self, index: usize, set: impl Fn(ElemName) -> bool) -> bool {
        set(self.open_elems[index].name)
    }

    fn count_name(&mut self, elem: &OpenElem<Handle>, delta: i32) {
        if elem.name.ns == Ns::Html {
            let slot = &mut self.html_name_counts[elem.name.local as usize];
            debug_assert!(delta > 0 || *slot > 0, "stack name count underflow");
            *slot = slot.wrapping_add_signed(delta);
        }
    }

    /// Increments the node counter and returns true if the limit has been reached.
    fn increment_node_count(&mut self) -> bool {
        self.nodes_created += 1;
        self.nodes_created >= self.opts.max_dom_nodes
    }

    /// Folds a subtree depth bound into the entry above `index`.
    ///
    /// The entry above is the parent in the common case and a sibling once
    /// the depth cap has retargeted insertions; the bound is an upper bound
    /// either way, and it reaches the real parent as the siblings close.
    fn raise_deepest_above(&mut self, index: usize, deepest: u32) {
        if index > 0 {
            let above = &mut self.open_elems[index - 1];
            above.deepest = above.deepest.max(deepest);
        }
    }

    fn push_elem(&mut self, elem: OpenElem<Handle>) {
        self.count_name(&elem, 1);
        self.raise_deepest_above(self.open_elems.len(), elem.deepest);
        self.open_elems.push(elem);
    }

    fn pop_elem(&mut self) -> Option<OpenElem<Handle>> {
        let elem = self.open_elems.pop()?;
        self.count_name(&elem, -1);
        self.raise_deepest_above(self.open_elems.len(), elem.deepest);
        self.clear_formatting_flag(&elem);
        Some(elem)
    }

    fn remove_elem(&mut self, index: usize) -> OpenElem<Handle> {
        let elem = self.open_elems.remove(index);
        self.count_name(&elem, -1);
        self.raise_deepest_above(index, elem.deepest);
        self.clear_formatting_flag(&elem);
        elem
    }

    fn insert_elem(&mut self, index: usize, elem: OpenElem<Handle>) {
        self.count_name(&elem, 1);
        self.raise_deepest_above(index, elem.deepest);
        self.open_elems.insert(index, elem);
    }

    fn replace_elem(&mut self, index: usize, elem: OpenElem<Handle>) {
        self.count_name(&elem, 1);
        let old = std::mem::replace(&mut self.open_elems[index], elem);
        self.count_name(&old, -1);
        // The replaced element stays in the tree; its subtree bound must not
        // be lost.
        let deepest = old.deepest.max(self.open_elems[index].deepest);
        self.open_elems[index].deepest = deepest;
    }

    /// Pops the current node, notifying the sink.
    fn pop(&mut self) -> Handle {
        let elem = self.pop_elem().expect("no current element");
        self.notify_pop(&elem);
        elem.handle
    }

    fn notify_pop(&mut self, elem: &OpenElem<Handle>) {
        // A closed <option> may have to be mirrored into the
        // <selectedcontent> of its select; only the sink can clone nodes.
        if elem.name.is_html(Local::Option) {
            self.sink
                .maybe_clone_option_into_selectedcontent(&elem.handle);
        }
        if self.sink.pop(&elem.handle) == ParseControl::Stop {
            self.stopped = true;
        }
    }

    /// Truncates the stack to `len`, notifying the sink for each element.
    fn truncate_stack(&mut self, len: usize) {
        while self.open_elems.len() > len {
            let elem = self.pop_elem().expect("stack underflow");
            self.notify_pop(&elem);
        }
    }

    fn remove_from_stack(&mut self, handle: &Handle) {
        if let Some(index) = self
            .open_elems
            .iter()
            .rposition(|e| self.sink.same_node(&e.handle, handle))
        {
            let elem = self.remove_elem(index);
            self.notify_pop(&elem);
        }
    }

    /// Removes an element from the stack without telling the sink; used by the
    /// adoption agency, which relocates rather than closes elements.
    fn detach_from_stack(&mut self, index: usize) -> OpenElem<Handle> {
        self.remove_elem(index)
    }

    fn in_scope(
        &self,
        scope: impl Fn(ElemName) -> bool,
        pred: impl Fn(&OpenElem<Handle>) -> bool,
    ) -> bool {
        // Cap the walk to prevent quadratic behavior on deeply nested inputs.
        // If we don't find the element within MAX_SCOPE_WALK entries, treat
        // it as not in scope.
        let limit = self.open_elems.len().saturating_sub(MAX_SCOPE_WALK);
        for elem in self.open_elems[limit..].iter().rev() {
            if pred(elem) {
                return true;
            }
            if scope(elem.name) {
                return false;
            }
        }
        false
    }

    fn in_scope_named(&self, scope: impl Fn(ElemName) -> bool, local: Local) -> bool {
        if self.html_name_counts[local as usize] == 0 {
            return false;
        }
        self.in_scope(scope, |elem| elem.name.is_html(local))
    }

    fn in_scope_handle(&self, scope: impl Fn(ElemName) -> bool, handle: &Handle) -> bool {
        self.in_scope(scope, |elem| self.sink.same_node(&elem.handle, handle))
    }

    fn generate_implied_end_tags(&mut self, set: impl Fn(ElemName) -> bool) {
        while !self.stopped {
            match self.open_elems.last() {
                Some(elem) if set(elem.name) => {}
                _ => return,
            }
            self.pop();
        }
    }

    fn generate_implied_end_except(&mut self, except: Local) {
        self.generate_implied_end_tags(|name| {
            if name.ns == Ns::Html && name.local == except {
                false
            } else {
                cursory_implied_end(name)
            }
        });
    }

    fn pop_until_current(&mut self, set: impl Fn(ElemName) -> bool) {
        while !self.current_node_in(&set) {
            self.pop();
            if self.open_elems.is_empty() {
                return;
            }
        }
    }

    /// Pops until an element from the set has been popped; returns how many.
    fn pop_until(&mut self, pred: impl Fn(ElemName) -> bool) -> usize {
        let mut count = 0;
        loop {
            count += 1;
            match self.pop_elem() {
                None => break,
                Some(elem) => {
                    let matched = pred(elem.name);
                    self.notify_pop(&elem);
                    if matched {
                        break;
                    }
                }
            }
        }
        count
    }

    fn pop_until_named(&mut self, local: Local) -> usize {
        self.pop_until(|name| name.is_html(local))
    }

    fn expect_to_close(&mut self, local: Local) {
        if self.pop_until_named(local) != 1 {
            self.sink.parse_error(Borrowed("Unexpected open element"));
        }
    }

    fn close_p_element(&mut self) {
        self.generate_implied_end_tags(|name| {
            if name.is_html(Local::P) {
                false
            } else {
                cursory_implied_end(name)
            }
        });
        self.expect_to_close(Local::P);
    }

    fn close_p_element_in_button_scope(&mut self) {
        if self.in_scope_named(button_scope, Local::P) {
            self.close_p_element();
        }
    }

    fn close_the_cell(&mut self) {
        self.generate_implied_end_tags(cursory_implied_end);
        if self.pop_until(td_th) != 1 {
            self.sink
                .parse_error(Borrowed("expected to close <td> or <th> with cell"));
        }
        self.clear_active_formatting_to_marker();
    }

    fn check_body_end(&mut self) {
        for elem in &self.open_elems {
            if !body_end_ok(elem.name) {
                self.sink
                    .parse_error(Borrowed("Unexpected open tag at end of body"));
                return;
            }
        }
    }

    //§ list of active formatting elements

    fn position_in_active_formatting(&self, handle: &Handle) -> Option<usize> {
        self.active_formatting.iter().position(|entry| match entry {
            FormatEntry::Marker => false,
            FormatEntry::Element(candidate, _, _) => self.sink.same_node(candidate, handle),
        })
    }

    /// Index of the most recent formatting entry with this tag name, stopping
    /// at the last marker.
    fn active_formatting_index_named(&self, name: Atom) -> Option<usize> {
        for (index, entry) in self.active_formatting.iter().enumerate().rev() {
            match entry {
                FormatEntry::Marker => return None,
                FormatEntry::Element(_, tag, _) if tag.name == name => return Some(index),
                FormatEntry::Element(..) => {}
            }
        }
        None
    }

    fn is_marker_or_open(&self, entry: &FormatEntry<Handle>) -> bool {
        match entry {
            FormatEntry::Marker => true,
            // The open flag is set when the entry is pushed onto the formatting
            // list and cleared when the element is popped or removed from the
            // open elements stack, avoiding a full-stack scan per reconstruction.
            FormatEntry::Element(_, _, is_open) => *is_open,
        }
    }

    /// Clears the open flag of the active-formatting entry for an element
    /// that left the stack of open elements (pop, truncate, remove).
    ///
    /// Only formatting elements can have an entry, so every other pop skips
    /// the list; the list itself is bounded by `MAX_ACTIVE_FORMATTING`.
    fn clear_formatting_flag(&mut self, elem: &OpenElem<Handle>) {
        if self.active_formatting.is_empty() || !formatting_tag(elem.name) {
            return;
        }
        for entry in self.active_formatting.iter_mut().rev() {
            if let FormatEntry::Element(candidate, _, is_open) = entry
                && *is_open
                && self.sink.same_node(candidate, &elem.handle)
            {
                *is_open = false;
                return;
            }
        }
    }

    fn clear_active_formatting_to_marker(&mut self) {
        while let Some(entry) = self.active_formatting.pop() {
            if matches!(entry, FormatEntry::Marker) {
                break;
            }
        }
    }

    /// <https://html.spec.whatwg.org/#reconstruct-the-active-formatting-elements>
    fn reconstruct_active_formatting_elements(&mut self) {
        // Past the node budget, skip formatting reconstruction entirely.
        if self.nodes_created >= self.opts.max_dom_nodes {
            return;
        }

        let Some(last) = self.active_formatting.last() else {
            return;
        };
        if self.is_marker_or_open(last) {
            return;
        }

        let mut index = self.active_formatting.len() - 1;
        loop {
            if index == 0 {
                break;
            }
            index -= 1;
            if self.is_marker_or_open(&self.active_formatting[index]) {
                index += 1;
                break;
            }
        }

        loop {
            let tag = match &self.active_formatting[index] {
                FormatEntry::Element(_, tag, _) => tag.clone(),
                FormatEntry::Marker => {
                    unreachable!("marker during formatting element reconstruction")
                }
            };
            let elem = self.insert_element(PushFlag::Push, Ns::Html, &tag, tag.attrs.clone());
            self.active_formatting[index] = FormatEntry::Element(elem, tag, true);
            if index == self.active_formatting.len() - 1 {
                break;
            }
            index += 1;
        }
    }

    fn create_formatting_element_for(&mut self, tag: Tag) -> Handle {
        // "Noah's ark": at most three identical entries. The list holds at
        // most three matches already, so the third one found walking back is
        // the earliest and the scan can stop there.
        let mut first_match = None;
        let mut matches = 0usize;
        for (index, entry) in self.active_formatting.iter().enumerate().rev() {
            match entry {
                FormatEntry::Marker => break,
                FormatEntry::Element(_, old, _) => {
                    if tag.equiv_modulo_attr_order(old) {
                        first_match = Some(index);
                        matches += 1;
                        if matches == 3 {
                            break;
                        }
                    }
                }
            }
        }
        if matches >= 3 {
            self.active_formatting
                .remove(first_match.expect("matches with no index"));
        }

        let elem = self.insert_element(PushFlag::Push, Ns::Html, &tag, tag.attrs.clone());
        self.push_active_formatting(FormatEntry::Element(elem.clone(), tag, true));
        elem
    }

    /// Appends to the list of active formatting elements, dropping the oldest
    /// entry once [`MAX_ACTIVE_FORMATTING`] is reached.
    fn push_active_formatting(&mut self, entry: FormatEntry<Handle>) {
        if self.active_formatting.len() >= MAX_ACTIVE_FORMATTING {
            // Drop the oldest entry that is not a marker: markers delimit the
            // scopes the clearing rules depend on.
            let oldest = self
                .active_formatting
                .iter()
                .position(|e| !matches!(e, FormatEntry::Marker));
            match oldest {
                Some(index) => {
                    self.active_formatting.remove(index);
                }
                None => {
                    self.active_formatting.remove(0);
                }
            }
        }
        self.active_formatting.push(entry);
    }

    //§ creating and inserting nodes

    fn create_element(&mut self, name: ElemName, attrs: Vec<Attribute>) -> OpenElem<Handle> {
        let (flags, integration_point) = element_flags(name.ns, name.local, &attrs);
        let handle = self.sink.create_element(name.qual(), attrs, flags);
        // The depth is known once the insertion point is; `<html>` and the
        // fragment context element are at depth 1.
        OpenElem {
            handle,
            name,
            annotation_xml_integration_point: integration_point,
            depth: 1,
            deepest: 1,
        }
    }

    fn create_root(&mut self, attrs: Vec<Attribute>) {
        let name = ElemName::html(Local::Html, core_atoms::HTML);
        let elem = self.create_element(name, attrs);
        self.sink
            .append(&self.doc_handle, NodeOrText::Node(elem.handle.clone()));
        self.push_elem(elem);
    }

    /// <https://html.spec.whatwg.org/multipage/#create-an-element-for-the-token>
    fn insert_element(
        &mut self,
        push: PushFlag,
        ns: Ns,
        tag: &Tag,
        attrs: Vec<Attribute>,
    ) -> Handle {
        let name = ElemName {
            ns,
            local: tag.local,
            atom: tag.name,
        };
        let mut elem = self.create_element(name, attrs);
        let (point, depth) = self.appropriate_place_for_insertion(None);
        // Past the node budget the element is still created and pushed, so
        // the algorithm's stack invariants hold (`</p>` with no open <p>
        // inserts one and closes it, which would drain the stack otherwise),
        // but it is never inserted: the tree stops growing while the parse
        // continues. Detached nodes are bounded by the input; the tree by the
        // budget.
        if self.increment_node_count() {
            self.parse_error(Borrowed("DOM node budget exceeded"));
        } else {
            self.insert_at(point, NodeOrText::Node(elem.handle.clone()));
        }
        let handle = elem.handle.clone();
        match push {
            // Always push the element onto the stack. The tree depth is bounded
            // by retargeting the insertion parent, not by the stack size.
            PushFlag::Push => {
                elem.depth = depth;
                elem.deepest = depth;
                self.push_elem(elem);
            }
            // A void element never reaches the stack, but it still deepens
            // the subtree of whatever it was inserted into.
            PushFlag::NoPush => self.raise_deepest_above(self.open_elems.len(), depth),
        }
        handle
    }

    fn insert_element_for(&mut self, tag: Tag) -> Handle {
        let attrs = tag.attrs.clone();
        self.insert_element(PushFlag::Push, Ns::Html, &tag, attrs)
    }

    fn insert_and_pop_element_for(&mut self, tag: Tag) -> Handle {
        let attrs = tag.attrs.clone();
        self.insert_element(PushFlag::NoPush, Ns::Html, &tag, attrs)
    }

    fn insert_phantom(&mut self, local: Local, atom: Atom) -> Handle {
        let tag = Tag {
            kind: TagKind::StartTag,
            name: atom,
            local,
            self_closing: false,
            attrs: Vec::new(),
        };
        self.insert_element(PushFlag::Push, Ns::Html, &tag, Vec::new())
    }

    fn append_text(&mut self, text: &str) {
        self.insert_appropriately(NodeOrText::Text(text), None);
    }

    fn create_comment_node(&mut self, data: CommentData<'_>) -> Handle {
        match data {
            CommentData::Comment(text) => self.sink.create_comment(text),
            CommentData::Pi { target, data } => self.sink.create_pi(target, data),
        }
    }

    fn append_comment(&mut self, data: CommentData<'_>) {
        let comment = self.create_comment_node(data);
        self.insert_appropriately(NodeOrText::Node(comment), None);
    }

    fn append_comment_to_doc(&mut self, data: CommentData<'_>) {
        let comment = self.create_comment_node(data);
        let doc = self.doc_handle.clone();
        self.sink.append(&doc, NodeOrText::Node(comment));
    }

    fn append_comment_to_html(&mut self, data: CommentData<'_>) {
        let comment = self.create_comment_node(data);
        let target = self.open_elems[0].handle.clone();
        self.sink.append(&target, NodeOrText::Node(comment));
    }

    fn set_quirks_mode(&mut self, mode: QuirksMode) {
        self.quirks_mode = mode;
        self.sink.set_quirks_mode(mode);
    }

    fn unexpected<T: std::fmt::Debug>(&mut self, _thing: &T) {
        self.sink.parse_error(Borrowed("Unexpected token"));
    }

    fn parse_error(&mut self, msg: Cow<'static, str>) {
        self.sink.parse_error(msg);
    }

    //§ reset the insertion mode appropriately

    fn reset_insertion_mode(&mut self) -> InsertionMode {
        // Bound the scan to the top `MAX_SCOPE_WALK` entries (owner Q5). The
        // structural ancestor this looks for (a table section, template, body,
        // …) sits near the top of the stack in every non-adversarial document,
        // so capping the walk is identical for any stack under the cap and only
        // bounds the quadratic worst case on a pathologically deep stack, where
        // it falls through to the `InBody` default (N5-1).
        let floor = self.open_elems.len().saturating_sub(MAX_SCOPE_WALK);
        for index in (floor..self.open_elems.len()).rev() {
            let last = index == 0;
            let name = if last && let Some(context) = &self.context_elem {
                context.name
            } else {
                self.open_elems[index].name
            };
            if name.ns != Ns::Html {
                continue;
            }
            match name.local {
                Local::Td | Local::Th if !last => return InsertionMode::InCell,
                Local::Tr => return InsertionMode::InRow,
                Local::Tbody | Local::Thead | Local::Tfoot => return InsertionMode::InTableBody,
                Local::Caption => return InsertionMode::InCaption,
                Local::Colgroup => return InsertionMode::InColumnGroup,
                Local::Table => return InsertionMode::InTable,
                Local::Template => {
                    return *self
                        .template_modes
                        .last()
                        .expect("template mode stack is empty");
                }
                Local::Head if !last => return InsertionMode::InHead,
                Local::Body => return InsertionMode::InBody,
                Local::Frameset => return InsertionMode::InFrameset,
                Local::Html => {
                    return match self.head_elem {
                        None => InsertionMode::BeforeHead,
                        Some(_) => InsertionMode::AfterHead,
                    };
                }
                _ => {}
            }
        }
        InsertionMode::InBody
    }

    //§ raw text

    fn switch_to_raw_text_mode(&mut self, kind: RawKind) -> ProcessResult<'static, Handle> {
        self.orig_mode = Some(self.mode);
        self.mode = InsertionMode::Text;
        ProcessResult::ToRawData(kind)
    }

    fn parse_raw_data(&mut self, tag: Tag, kind: RawKind) -> ProcessResult<'static, Handle> {
        self.insert_element_for(tag);
        self.switch_to_raw_text_mode(kind)
    }

    //§ misc helpers

    fn is_type_hidden(&self, tag: &Tag) -> bool {
        tag.attrs
            .iter()
            .find(|a| a.name.prefix.is_none() && a.name.local == core_atoms::TYPE)
            .is_some_and(|a| a.value.eq_ignore_ascii_case("hidden"))
    }

    fn foster_parent_in_body<'a>(&mut self, token: Token<'a>) -> ProcessResult<'a, Handle> {
        self.foster_parenting = true;
        let result = self.step(InsertionMode::InBody, token);
        self.foster_parenting = false;
        result
    }

    fn process_chars_in_table<'a>(&mut self, token: Token<'a>) -> ProcessResult<'a, Handle> {
        if self.current_node_in(foster_target) {
            debug_assert!(self.pending_table_text.is_empty());
            self.orig_mode = Some(self.mode);
            ProcessResult::Reprocess(InsertionMode::InTableText, token)
        } else {
            self.parse_error(Borrowed("Unexpected characters in table"));
            self.foster_parent_in_body(token)
        }
    }

    //§ the adoption agency algorithm

    fn adoption_agency(&mut self, subject: Atom, subject_local: Local) {
        // Step 1.
        if self.current_name().ns == Ns::Html
            && self.current_name().atom == subject
            && self
                .position_in_active_formatting(&self.current_handle())
                .is_none()
        {
            self.pop();
            return;
        }

        for _ in 0..8 {
            let Some(fmt_index) = self.active_formatting_index_named(subject) else {
                return self.process_end_tag_in_body(Tag {
                    kind: TagKind::EndTag,
                    name: subject,
                    local: subject_local,
                    self_closing: false,
                    attrs: Vec::new(),
                });
            };
            let (fmt_elem, fmt_tag) = match &self.active_formatting[fmt_index] {
                FormatEntry::Element(handle, tag, _) => (handle.clone(), tag.clone()),
                FormatEntry::Marker => unreachable!("marker in adoption agency"),
            };

            // Bounded like every other stack walk: a formatting element more
            // than MAX_SCOPE_WALK entries below the top counts as not open.
            let walk_start = self.open_elems.len().saturating_sub(MAX_SCOPE_WALK);
            let Some(fmt_stack_index) = self.open_elems[walk_start..]
                .iter()
                .rposition(|e| self.sink.same_node(&e.handle, &fmt_elem))
                .map(|offset| walk_start + offset)
            else {
                self.parse_error(Borrowed("Formatting element not open"));
                self.active_formatting.remove(fmt_index);
                return;
            };

            if !self.in_scope_handle(default_scope, &fmt_elem) {
                self.parse_error(Borrowed("Formatting element not in scope"));
                return;
            }

            if !self.sink.same_node(&self.current_handle(), &fmt_elem) {
                self.parse_error(Borrowed("Formatting element not current node"));
            }

            // Step 9: the furthest block.
            // Cap the search to prevent quadratic behavior on deeply nested inputs.
            let search_limit = (fmt_stack_index + MAX_AA_SEARCH).min(self.open_elems.len());
            let furthest = (fmt_stack_index..search_limit)
                .find(|&index| self.elem_in(index, special_tag))
                .map(|index| (index, self.open_elems[index].clone()));

            let Some((furthest_index, furthest_block)) = furthest else {
                // Step 10.
                self.truncate_stack(fmt_stack_index);
                self.active_formatting.remove(fmt_index);
                return;
            };

            let common_ancestor = fmt_stack_index - 1;
            let mut bookmark = Bookmark::Replace(fmt_elem.clone());

            // Depth cap. The moves below put the furthest block under the
            // common ancestor beneath a chain of clones and wrap its children
            // in one more element; the entries at and below it bound how deep
            // its subtree already reaches. A run that would cross the cap is
            // treated as having no furthest block (step 10), which leaves the
            // tree untouched; only trees already at the cap can get here.
            let fb_old_depth = furthest_block.depth;
            let fb_old_deepest = self.open_elems[furthest_index..]
                .iter()
                .map(|elem| elem.deepest)
                .max()
                .unwrap_or(fb_old_depth);
            let clone_count = (fmt_stack_index + 1..furthest_index)
                .rev()
                .take(3)
                .filter(|&index| {
                    self.position_in_active_formatting(&self.open_elems[index].handle)
                        .is_some()
                })
                .count() as u32;
            let (_, chain_depth) = self.appropriate_place_for_insertion(Some(common_ancestor));
            let fb_new_depth = chain_depth + clone_count;
            let subtree_height = fb_old_deepest - fb_old_depth;
            if fb_new_depth + 1 + subtree_height > MAX_DOM_DEPTH as u32 {
                self.truncate_stack(fmt_stack_index);
                self.active_formatting.remove(fmt_index);
                return;
            }
            let growth = (fb_new_depth + 1).saturating_sub(fb_old_depth);

            let mut node_index = furthest_index;
            let mut last_node = furthest_block.handle.clone();
            let mut inner_counter = 0;
            let mut clones_made = 0;

            loop {
                inner_counter += 1;
                node_index -= 1;
                let node = self.open_elems[node_index].clone();

                if self.sink.same_node(&node.handle, &fmt_elem) {
                    break;
                }

                if inner_counter > 3 {
                    if let Some(position) = self.position_in_active_formatting(&node.handle) {
                        self.active_formatting.remove(position);
                    }
                    self.detach_from_stack(node_index);
                    continue;
                }

                let Some(node_fmt_index) = self.position_in_active_formatting(&node.handle) else {
                    self.detach_from_stack(node_index);
                    continue;
                };

                let tag = match &self.active_formatting[node_fmt_index] {
                    FormatEntry::Element(_, tag, _) => tag.clone(),
                    FormatEntry::Marker => unreachable!("marker in adoption agency"),
                };
                let name = ElemName::html(tag.local, tag.name);
                let mut new_elem = self.create_element(name, tag.attrs.clone());
                clones_made += 1;
                // Clones sit between the common ancestor and the furthest
                // block, the first one made directly above the block.
                new_elem.depth = fb_new_depth - clones_made;
                new_elem.deepest = fb_old_deepest + growth;
                self.replace_elem(node_index, new_elem.clone());
                self.active_formatting[node_fmt_index] =
                    FormatEntry::Element(new_elem.handle.clone(), tag, true);
                let node_handle = new_elem.handle;

                if self.sink.same_node(&last_node, &furthest_block.handle) {
                    bookmark = Bookmark::InsertAfter(node_handle.clone());
                }

                self.sink.remove_from_parent(&last_node);
                self.sink
                    .append(&node_handle, NodeOrText::Node(last_node.clone()));
                last_node = node_handle;
            }
            debug_assert_eq!(clones_made, clone_count, "adoption agency clone count");

            // Step 14.
            self.sink.remove_from_parent(&last_node);
            self.insert_appropriately(NodeOrText::Node(last_node), Some(common_ancestor));

            // Steps 15-17.
            let name = ElemName::html(fmt_tag.local, fmt_tag.name);
            let mut new_elem = self.create_element(name, fmt_tag.attrs.clone());
            new_elem.depth = fb_new_depth + 1;
            new_elem.deepest = fb_old_deepest + growth;
            self.sink
                .reparent_children(&furthest_block.handle, &new_elem.handle);
            self.sink.append(
                &furthest_block.handle,
                NodeOrText::Node(new_elem.handle.clone()),
            );

            // The furthest block and everything below it on the stack moved
            // by `growth`; entries that are retargeted siblings rather than
            // descendants only get over-estimated, which is safe.
            let fb_index = fmt_stack_index + clone_count as usize + 1;
            debug_assert!(
                self.sink
                    .same_node(&self.open_elems[fb_index].handle, &furthest_block.handle),
                "furthest block moved on the stack"
            );
            self.open_elems[fb_index].depth = fb_new_depth;
            for elem in &mut self.open_elems[fb_index..] {
                elem.deepest += growth;
            }
            for elem in &mut self.open_elems[fb_index + 1..] {
                elem.depth += growth;
            }
            let fb_deepest = self.open_elems[fb_index].deepest.max(new_elem.deepest);
            self.open_elems[fb_index].deepest = fb_deepest;
            self.raise_deepest_above(fmt_stack_index, fb_deepest);

            // Step 18.
            let new_entry = FormatEntry::Element(new_elem.handle.clone(), fmt_tag, true);
            match bookmark {
                Bookmark::Replace(target) => {
                    let index = self
                        .position_in_active_formatting(&target)
                        .expect("bookmark not in the active formatting list");
                    self.active_formatting[index] = new_entry;
                }
                Bookmark::InsertAfter(previous) => {
                    let index = self
                        .position_in_active_formatting(&previous)
                        .expect("bookmark not in the active formatting list")
                        + 1;
                    self.active_formatting.insert(index, new_entry);
                    let old = self
                        .position_in_active_formatting(&fmt_elem)
                        .expect("formatting element not in the active formatting list");
                    self.active_formatting.remove(old);
                }
            }

            // Step 19. Nothing below the formatting element moved during the
            // inner loop, so both searches start there instead of at the
            // bottom of the (unbounded) stack.
            if let Some(index) = self.open_elems[fmt_stack_index..]
                .iter()
                .rposition(|e| self.sink.same_node(&e.handle, &fmt_elem))
            {
                self.detach_from_stack(fmt_stack_index + index);
            }
            let insert_at = self.open_elems[fmt_stack_index..]
                .iter()
                .position(|e| self.sink.same_node(&e.handle, &furthest_block.handle))
                .expect("furthest block missing from the stack of open elements")
                + fmt_stack_index
                + 1;
            self.insert_elem(insert_at, new_elem);
        }
    }

    fn process_end_tag_in_body(&mut self, tag: Tag) {
        let mut match_index = None;
        let start_index = self.open_elems.len();
        let search_start = start_index.saturating_sub(MAX_SCOPE_WALK);
        for index in (search_start..start_index).rev() {
            if self.open_elems[index].name.ns == Ns::Html
                && self.open_elems[index].name.atom == tag.name
            {
                match_index = Some(index);
                break;
            }
            if self.elem_in(index, special_tag) {
                self.parse_error(Borrowed("Found special tag while closing generic tag"));
                return;
            }
        }

        let Some(match_index) = match_index else {
            // Cap reached or not found: treat as unmatched.
            self.unexpected(&tag);
            return;
        };

        self.generate_implied_end_except(tag.local);

        if match_index != self.open_elems.len() - 1 {
            self.unexpected(&tag);
        }
        self.truncate_stack(match_index);
    }

    fn handle_misnested_a_tags(&mut self, tag: &Tag) {
        let node = self
            .active_formatting
            .iter()
            .rev()
            .take_while(|entry| !matches!(entry, FormatEntry::Marker))
            .find_map(|entry| match entry {
                FormatEntry::Element(handle, t, _) if t.local == Local::A => Some(handle.clone()),
                _ => None,
            });
        let Some(node) = node else { return };

        self.unexpected(tag);
        self.adoption_agency(tag.name, Local::A);
        // The entry's open flag says whether the stack still holds the
        // element: after the agency popped it, a scan for it would walk the
        // whole stack for nothing, quadratic on `<span>`×n `<a>`×n.
        if let Some(index) = self.position_in_active_formatting(&node) {
            let still_open = matches!(
                self.active_formatting[index],
                FormatEntry::Element(_, _, true)
            );
            self.active_formatting.remove(index);
            if still_open {
                self.remove_from_stack(&node);
            }
        }
    }

    //§ foreign content

    fn is_foreign(&self, token: &Token<'_>) -> bool {
        if matches!(token, Token::Eof) || self.open_elems.is_empty() {
            return false;
        }

        let current = self.adjusted_current();
        let name = current.name;
        if name.ns == Ns::Html {
            return false;
        }

        if mathml_text_integration_point(name) {
            match token {
                Token::Characters(..) | Token::NullCharacter => return false,
                Token::Tag(Tag {
                    kind: TagKind::StartTag,
                    local,
                    ..
                }) if !matches!(local, Local::Mglyph | Local::Malignmark) => return false,
                _ => {}
            }
        }

        if svg_html_integration_point(name) {
            match token {
                Token::Characters(..) | Token::NullCharacter => return false,
                Token::Tag(Tag {
                    kind: TagKind::StartTag,
                    ..
                }) => return false,
                _ => {}
            }
        }

        if name.ns == Ns::MathMl && name.local == Local::AnnotationXml {
            match token {
                Token::Tag(Tag {
                    kind: TagKind::StartTag,
                    local: Local::Svg,
                    ..
                }) => return false,
                Token::Characters(..)
                | Token::NullCharacter
                | Token::Tag(Tag {
                    kind: TagKind::StartTag,
                    ..
                }) => return !current.annotation_xml_integration_point,
                _ => {}
            }
        }

        true
    }

    fn enter_foreign(&mut self, mut tag: Tag, ns: Ns) -> ProcessResult<'static, Handle> {
        match ns {
            Ns::MathMl => self.adjust_mathml_attributes(&mut tag),
            Ns::Svg => self.adjust_svg_attributes(&mut tag),
            _ => {}
        }
        self.adjust_foreign_attributes(&mut tag);

        let attrs = tag.attrs.clone();
        if tag.self_closing {
            self.insert_element(PushFlag::NoPush, ns, &tag, attrs);
            ProcessResult::DoneAckSelfClosing
        } else {
            self.insert_element(PushFlag::Push, ns, &tag, attrs);
            ProcessResult::Done
        }
    }

    fn foreign_start_tag(&mut self, mut tag: Tag) -> ProcessResult<'static, Handle> {
        let ns = self.adjusted_current().name.ns;
        match ns {
            Ns::MathMl => self.adjust_mathml_attributes(&mut tag),
            Ns::Svg => {
                self.adjust_svg_tag_name(&mut tag);
                self.adjust_svg_attributes(&mut tag);
            }
            _ => {}
        }
        self.adjust_foreign_attributes(&mut tag);

        let attrs = tag.attrs.clone();
        if tag.self_closing {
            self.insert_element(PushFlag::NoPush, ns, &tag, attrs);
            ProcessResult::DoneAckSelfClosing
        } else {
            self.insert_element(PushFlag::Push, ns, &tag, attrs);
            ProcessResult::Done
        }
    }

    fn adjust_svg_tag_name(&mut self, tag: &mut Tag) {
        if let Some(adjusted) = adjust::adjusted_svg_tag_name(tag.name.as_str()) {
            let (atom, local) = self.names.lookup(adjusted);
            tag.name = atom;
            tag.local = local;
        }
    }

    fn adjust_svg_attributes(&mut self, tag: &mut Tag) {
        for attr in &mut tag.attrs {
            if let Some(adjusted) = adjust::adjusted_svg_attribute_name(attr.name.local.as_str()) {
                attr.name.local = self.names.lookup(adjusted).0;
            }
        }
    }

    fn adjust_mathml_attributes(&mut self, tag: &mut Tag) {
        for attr in &mut tag.attrs {
            if attr.name.local.as_str() == "definitionurl" {
                attr.name.local = core_atoms::DEFINITION_URL;
            }
        }
    }

    /// The `xlink:`, `xml:` and `xmlns:` attributes of foreign content get a
    /// real namespace and prefix.
    fn adjust_foreign_attributes(&mut self, tag: &mut Tag) {
        for attr in &mut tag.attrs {
            let name = attr.name.local.as_str();
            let (ns, prefix, local) = match name {
                "xlink:actuate" | "xlink:arcrole" | "xlink:href" | "xlink:role" | "xlink:show"
                | "xlink:title" | "xlink:type" => (
                    core_atoms::NS_XLINK,
                    Some(core_atoms::XLINK),
                    &name["xlink:".len()..],
                ),
                "xml:lang" | "xml:space" => (
                    core_atoms::NS_XML,
                    Some(core_atoms::XML),
                    &name["xml:".len()..],
                ),
                "xmlns" => (core_atoms::NS_XMLNS, None, "xmlns"),
                "xmlns:xlink" => (
                    core_atoms::NS_XMLNS,
                    Some(core_atoms::XMLNS),
                    &name["xmlns:".len()..],
                ),
                _ => continue,
            };
            let local = self.names.lookup(local).0;
            attr.name = QualName { ns, prefix, local };
        }
    }

    fn unexpected_start_tag_in_foreign_content<'a>(
        &mut self,
        tag: Tag,
    ) -> ProcessResult<'a, Handle> {
        self.unexpected(&tag);
        while !self.current_node_in(|name| {
            name.ns == Ns::Html
                || mathml_text_integration_point(name)
                || svg_html_integration_point(name)
        }) {
            self.pop();
            if self.open_elems.is_empty() {
                break;
            }
        }
        let mode = self.mode;
        self.step(mode, Token::Tag(tag))
    }
}

/// Splits off the leading run of ASCII whitespace (or of non-whitespace).
/// Returns the run, whether it is whitespace, and the remainder.
fn split_whitespace_run(text: &str) -> Option<(&str, bool, &str)> {
    let first = text.chars().next()?;
    let is_whitespace = first.is_ascii_whitespace();
    let end = text
        .find(|c: char| c.is_ascii_whitespace() != is_whitespace)
        .unwrap_or(text.len());
    Some((&text[..end], is_whitespace, &text[end..]))
}

/// The element flags the sink needs at creation time, plus whether a MathML
/// `annotation-xml` is an HTML integration point.
fn element_flags(ns: Ns, local: Local, attrs: &[Attribute]) -> (ElementFlags, bool) {
    let mut flags = ElementFlags::default();
    let mut integration_point = false;
    match (ns, local) {
        (Ns::Html, Local::Template) => flags.template = true,
        (Ns::MathMl, Local::AnnotationXml) => {
            let encoding = Atom::get("encoding");
            integration_point = encoding.is_some_and(|encoding| {
                attrs.iter().any(|attr| {
                    attr.name.prefix.is_none()
                        && attr.name.local == encoding
                        && (attr.value.eq_ignore_ascii_case("text/html")
                            || attr.value.eq_ignore_ascii_case("application/xhtml+xml"))
                })
            });
            flags.mathml_annotation_xml_integration_point = integration_point;
        }
        _ => {}
    }
    (flags, integration_point)
}

impl<Handle, Sink> TokenSink for TreeBuilder<Handle, Sink>
where
    Handle: Clone + Eq,
    Sink: TreeSink<Handle = Handle>,
{
    type Handle = Handle;

    fn process_token(&mut self, token: TokToken<'_>, line_number: u64) -> TokenSinkResult<Handle> {
        if self.stopped {
            return TokenSinkResult::Stop;
        }
        if line_number != self.current_line {
            self.current_line = line_number;
            self.sink.set_current_line(line_number);
        }
        let ignore_lf = std::mem::take(&mut self.ignore_lf);

        let token = match token {
            TokToken::ParseError(error) => {
                self.sink.parse_error(error);
                return TokenSinkResult::Continue;
            }

            TokToken::Doctype(doctype) => {
                if self.mode == InsertionMode::Initial {
                    let (err, quirk) =
                        data::doctype_error_and_quirks(&doctype, self.opts.iframe_srcdoc);
                    if err {
                        self.sink.parse_error(Borrowed("Bad DOCTYPE"));
                    }
                    let Doctype {
                        name,
                        public_id,
                        system_id,
                        force_quirks: _,
                    } = doctype;
                    if !self.opts.drop_doctype {
                        self.sink.append_doctype_to_document(
                            name.as_deref().unwrap_or(""),
                            public_id.as_deref().unwrap_or(""),
                            system_id.as_deref().unwrap_or(""),
                        );
                    }
                    self.set_quirks_mode(quirk);
                    self.mode = InsertionMode::BeforeHtml;
                } else {
                    self.sink.parse_error(Borrowed("DOCTYPE in body"));
                }
                return TokenSinkResult::Continue;
            }

            TokToken::Tag(tag) => Token::Tag(tag),
            TokToken::Comment(text) => Token::Comment(CommentData::Comment(text)),
            TokToken::ProcessingInstruction { target, data } => {
                Token::Comment(CommentData::Pi { target, data })
            }
            TokToken::NullCharacter => Token::NullCharacter,
            TokToken::Eof => Token::Eof,

            TokToken::Characters(mut text) => {
                if ignore_lf && let Some(rest) = text.strip_prefix('\n') {
                    text = rest;
                }
                if text.is_empty() {
                    return TokenSinkResult::Continue;
                }
                Token::Characters(SplitStatus::NotSplit, text)
            }
        };

        self.process_to_completion(token)
    }

    fn end(&mut self) {
        while let Some(elem) = self.pop_elem() {
            if self.stopped {
                return;
            }
            self.notify_pop(&elem);
        }
    }

    fn adjusted_current_node_present_but_not_in_html_namespace(&self) -> bool {
        !self.open_elems.is_empty() && self.adjusted_current().name.ns != Ns::Html
    }
}
