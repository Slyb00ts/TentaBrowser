//! Extraction from the arena without JavaScript
//! (`docs/spec/04-dom-and-css.md` §5).
//!
//! Every extractor streams the arena with the iterative [`Walker`] below; none
//! recurses and none builds an intermediate tree. Past
//! [`crate::MAX_TRAVERSAL_DEPTH`] a walk stops descending and reports
//! [`Walker::truncated`], and the extractors append
//! [`crate::TRUNCATION_MARKER`] so truncated output is visible.

pub mod forms;
pub mod links;
pub mod main_content;
pub mod markdown;
pub mod metadata;
pub mod snapshot;
pub mod text;

use std::sync::OnceLock;

use tenta_core::atom::atoms;
use tenta_core::{Atom, NodeId};

use crate::MAX_TRAVERSAL_DEPTH;
use crate::arena::NodeKind;
use crate::document::Document;

pub use forms::{Field, Form, SelectOption, forms};
pub use links::{Link, links};
pub use main_content::{MainContent, main_content};
pub use markdown::markdown;
pub use metadata::{Metadata, MicrodataItem, metadata};
pub use snapshot::{SnapshotNode, snapshot};
pub use text::{text, text_content};

/// One step of a pre-order walk.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    /// The node was reached; `depth` is 0 for the walk's root.
    Enter { node: NodeId, depth: usize },
    /// Every descendant of the node has been visited.
    Exit { node: NodeId, depth: usize },
}

impl Event {
    #[inline]
    pub fn node(self) -> NodeId {
        match self {
            Event::Enter { node, .. } | Event::Exit { node, .. } => node,
        }
    }

    #[inline]
    pub fn depth(self) -> usize {
        match self {
            Event::Enter { depth, .. } | Event::Exit { depth, .. } => depth,
        }
    }
}

/// An iterative pre-order walk with explicit enter and exit events.
///
/// The stack lives on the heap, so the walk survives trees far deeper than
/// any native stack (`docs/spec/02-architecture.md` §3); depth is still capped
/// at [`crate::MAX_TRAVERSAL_DEPTH`] so output stays bounded, and the visited
/// count is capped at the arena length so a corrupt tree ends the walk.
pub struct Walker<'a> {
    document: &'a Document,
    stack: Vec<Event>,
    visited: usize,
    cap: usize,
    truncated: bool,
    visible_only: bool,
}

impl<'a> Walker<'a> {
    /// Walks the subtree rooted at `root`, `root` included.
    pub fn new(document: &'a Document, root: NodeId) -> Self {
        let cap = document.slots().saturating_mul(2) + 8;
        Self {
            document,
            stack: vec![Event::Enter {
                node: root,
                depth: 0,
            }],
            visited: 0,
            cap,
            truncated: false,
            visible_only: false,
        }
    }

    /// Like [`Walker::new`], but content hidden from text never produces an
    /// event: the [`hides`] verdict is taken once when an element is
    /// entered, so the walk costs O(1) per node instead of an ancestor walk
    /// per node (`docs/spec/04-dom-and-css.md` §5). `root` itself is always
    /// entered; hidden ancestors of `root` are not consulted.
    pub fn visible(document: &'a Document, root: NodeId) -> Self {
        let mut walker = Self::new(document, root);
        walker.visible_only = true;
        walker
    }

    /// Whether the walk stopped descending somewhere.
    #[inline]
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// Skips the children of the node just entered. The matching
    /// [`Event::Exit`] is still delivered.
    pub fn skip_children(&mut self) {
        if let Some(Event::Exit { .. }) = self.stack.last() {
            return;
        }
        // Children were pushed above the exit event; drop them.
        while let Some(top) = self.stack.last() {
            if matches!(top, Event::Exit { .. }) {
                break;
            }
            self.stack.pop();
        }
    }

    fn push_children(&mut self, node: NodeId, depth: usize, only: Option<NodeId>) {
        if depth + 1 > MAX_TRAVERSAL_DEPTH {
            if self.document.first_child(node).is_some() {
                self.truncated = true;
            }
            return;
        }
        if let Some(only) = only {
            self.stack.push(Event::Enter {
                node: only,
                depth: depth + 1,
            });
            return;
        }
        // Pushed forwards and reversed in place: collecting into a temporary
        // vector would allocate once per element on a hot extraction path.
        let start = self.stack.len();
        let mut child = self.document.first_child(node);
        let cap = self.cap;
        while child.is_some() && self.stack.len() - start <= cap {
            self.stack.push(Event::Enter {
                node: child,
                depth: depth + 1,
            });
            child = self.document.next_sibling(child);
        }
        self.stack[start..].reverse();
    }
}

impl Iterator for Walker<'_> {
    type Item = Event;

    fn next(&mut self) -> Option<Event> {
        loop {
            let event = self.stack.pop()?;
            self.visited += 1;
            if self.visited > self.cap {
                self.truncated = true;
                self.stack.clear();
                return None;
            }
            let Event::Enter { node, depth } = event else {
                return Some(event);
            };
            let verdict = if self.visible_only && depth > 0 {
                hides(self.document, node)
            } else {
                Hides::Nothing
            };
            match verdict {
                Hides::Subtree => continue,
                Hides::Nothing => {
                    self.stack.push(Event::Exit { node, depth });
                    self.push_children(node, depth, None);
                }
                Hides::Children => {
                    self.stack.push(Event::Exit { node, depth });
                }
                Hides::AllButFirstSummary => {
                    self.stack.push(Event::Exit { node, depth });
                    if let Some(summary) = first_summary_child(self.document, node) {
                        self.push_children(node, depth, Some(summary));
                    }
                }
            }
            return Some(event);
        }
    }
}

/// Bits of [`tag_class`].
/// The element starts a new line in extracted text.
pub const TAG_BLOCK: u8 = 1 << 0;
/// The element's subtree carries no user-visible content.
pub const TAG_SKIP: u8 = 1 << 1;
/// The element is a table cell: a tab separates it from the previous one.
pub const TAG_CELL: u8 = 1 << 2;
/// The element's text is preformatted: line structure is kept and whitespace
/// is not collapsed (`docs/spec/04-dom-and-css.md` §5).
pub const TAG_PRE: u8 = 1 << 3;

/// Classification of a tag, as a byte lookup indexed by the atom's handle.
///
/// The extractors ask this for every element they see twice (enter and exit),
/// so it is a bounds-checked array read rather than a hash lookup. Tags
/// interned after the table is built fall outside it and classify as 0, which
/// is the right answer for any name that is not in the lists below.
pub fn tag_class(tag: Atom) -> u8 {
    static TABLE: OnceLock<Vec<u8>> = OnceLock::new();
    let table = TABLE.get_or_init(build_tag_table);
    table.get(tag.raw() as usize).copied().unwrap_or(0)
}

fn build_tag_table() -> Vec<u8> {
    let blocks: &[Atom] = &[
        atoms::HTML,
        atoms::BODY,
        atoms::DIV,
        atoms::P,
        atoms::H1,
        atoms::H2,
        atoms::H3,
        atoms::H4,
        atoms::H5,
        atoms::H6,
        atoms::UL,
        atoms::OL,
        atoms::LI,
        atoms::TABLE,
        atoms::TR,
        atoms::FORM,
        atoms::PRE,
        atoms::HR,
        atoms::NAV,
        atoms::MAIN,
        atoms::HEADER,
        atoms::FOOTER,
        atoms::SECTION,
        atoms::ARTICLE,
        atoms::ASIDE,
        atoms::OPTION,
        atoms::ADDRESS,
        atoms::BLOCKQUOTE,
        atoms::CAPTION,
        atoms::CENTER,
        atoms::COLGROUP,
        atoms::DD,
        atoms::DETAILS,
        atoms::DIALOG,
        atoms::DL,
        atoms::DT,
        atoms::FIELDSET,
        atoms::FIGCAPTION,
        atoms::FIGURE,
        atoms::HGROUP,
        atoms::LEGEND,
        atoms::LISTING,
        atoms::MENU,
        atoms::OPTGROUP,
        atoms::SEARCH,
        atoms::SUMMARY,
        atoms::TEXTAREA,
        atoms::TBODY,
        atoms::TFOOT,
        atoms::THEAD,
        atoms::XMP,
    ];
    let skipped: &[Atom] = &[
        atoms::SCRIPT,
        atoms::STYLE,
        atoms::HEAD,
        atoms::TITLE,
        atoms::META,
        atoms::LINK,
        atoms::NOSCRIPT,
        atoms::TEMPLATE,
        atoms::BASE,
    ];
    let cells: &[Atom] = &[atoms::TD, atoms::TH];
    let preformatted: &[Atom] = &[atoms::PRE, atoms::TEXTAREA, atoms::LISTING];
    let size = blocks
        .iter()
        .chain(skipped)
        .chain(cells)
        .chain(preformatted)
        .map(|atom| atom.raw() as usize)
        .max()
        .unwrap_or(0)
        + 1;
    let mut table = vec![0u8; size];
    for (tags, bit) in [
        (blocks, TAG_BLOCK),
        (skipped, TAG_SKIP),
        (cells, TAG_CELL),
        (preformatted, TAG_PRE),
    ] {
        for tag in tags {
            table[tag.raw() as usize] |= bit;
        }
    }
    table
}

/// Skip state shared by the extractors: elements whose subtree carries no
/// user-visible content.
pub fn is_skipped(document: &Document, node: NodeId) -> bool {
    document
        .tag(node)
        .is_some_and(|tag| tag_class(tag) & TAG_SKIP != 0)
}

/// What entering an element hides from text extraction
/// (`docs/spec/04-dom-and-css.md` §5). Decided from the element alone, so
/// a walker can apply it once per node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hides {
    /// The element and its subtree are visible.
    Nothing,
    /// The element and everything below it are hidden: the `hidden`
    /// attribute (including `until-found`), `aria-hidden` ASCII-case-
    /// insensitively `true`, an inline `display: none` as the last `display`
    /// declaration (`!important` included), `<template>` and a `<dialog>`
    /// without `open`.
    Subtree,
    /// The element itself is visible but its children are fallback content:
    /// `<iframe>`, `<object>`, `<canvas>`, `<video>`, `<audio>`.
    Children,
    /// A closed `<details>`: only its first `<summary>` child is visible.
    AllButFirstSummary,
}

/// The [`Hides`] verdict for `node`.
pub fn hides(document: &Document, node: NodeId) -> Hides {
    if document.kind(node) != NodeKind::Element {
        return Hides::Nothing;
    }
    if document.has_attribute(node, atoms::HIDDEN) {
        return Hides::Subtree;
    }
    if let Some(aria_hidden) = document.attribute(node, atoms::ARIA_HIDDEN)
        && aria_hidden.trim().eq_ignore_ascii_case("true")
    {
        return Hides::Subtree;
    }
    if let Some(style) = document.attribute(node, atoms::STYLE)
        && style_has_display_none(style)
    {
        return Hides::Subtree;
    }
    let tag = document.element_tag(node);
    if tag == atoms::TEMPLATE {
        Hides::Subtree
    } else if tag == atoms::DIALOG {
        if document.has_attribute(node, atoms::OPEN) {
            Hides::Nothing
        } else {
            Hides::Subtree
        }
    } else if tag == atoms::DETAILS {
        if document.has_attribute(node, atoms::OPEN) {
            Hides::Nothing
        } else {
            Hides::AllButFirstSummary
        }
    } else if tag == atoms::IFRAME
        || tag == atoms::OBJECT
        || tag == atoms::CANVAS
        || tag == atoms::VIDEO
        || tag == atoms::AUDIO
    {
        Hides::Children
    } else {
        Hides::Nothing
    }
}

/// The `<summary>` a closed `<details>` keeps visible: its first `<summary>`
/// element child.
fn first_summary_child(document: &Document, details: NodeId) -> Option<NodeId> {
    document
        .children(details)
        .find(|child| document.tag(*child) == Some(atoms::SUMMARY))
}

/// Whether `node` is hidden from text extraction, looking at the node and
/// every ancestor. This is the single-node form of [`hides`]; it walks up
/// the tree, so extractors use [`Walker::visible`] instead of calling it per
/// node.
pub fn is_hidden_from_text(document: &Document, node: NodeId) -> bool {
    if hides(document, node) == Hides::Subtree {
        return true;
    }
    let mut child = node;
    for ancestor in document.ancestors(node) {
        match hides(document, ancestor) {
            Hides::Nothing => {}
            Hides::Subtree | Hides::Children => return true,
            Hides::AllButFirstSummary => {
                if first_summary_child(document, ancestor) != Some(child) {
                    return true;
                }
            }
        }
        child = ancestor;
    }
    false
}

/// Whether the inline style's last `display` declaration is `none`
/// (`!important` and any letter case included).
fn style_has_display_none(style: &str) -> bool {
    let mut last_display = None;
    for declaration in style.split(';') {
        if let Some((property, value)) = declaration.split_once(':')
            && property.trim().eq_ignore_ascii_case("display")
        {
            last_display = Some(value);
        }
    }
    let Some(value) = last_display else {
        return false;
    };
    let mut words = value.split_whitespace();
    words
        .next()
        .is_some_and(|word| word.eq_ignore_ascii_case("none"))
        && words
            .next()
            .is_none_or(|word| word.eq_ignore_ascii_case("!important"))
}

/// Elements that start a new line in extracted text.
pub fn is_block(tag: Atom) -> bool {
    tag_class(tag) & TAG_BLOCK != 0
}

/// Table cells: a tab separates them inside a row.
pub fn is_table_cell(tag: Atom) -> bool {
    tag_class(tag) & TAG_CELL != 0
}

/// Elements whose text is preformatted.
pub fn is_preformatted(tag: Atom) -> bool {
    tag_class(tag) & TAG_PRE != 0
}

/// The heading level of `h1`..`h6`, if the tag is one.
pub fn heading_level(tag: Atom) -> Option<u8> {
    let level = match tag {
        t if t == atoms::H1 => 1,
        t if t == atoms::H2 => 2,
        t if t == atoms::H3 => 3,
        t if t == atoms::H4 => 4,
        t if t == atoms::H5 => 5,
        t if t == atoms::H6 => 6,
        _ => return None,
    };
    Some(level)
}

/// ASCII whitespace, the set the HTML parser collapses. U+00A0 and friends
/// are not whitespace for this purpose and are kept.
///
/// One shift and one mask rather than a chain of five comparisons: this runs
/// once per byte of every extraction (`docs/spec/04-dom-and-css.md` §8).
#[inline]
fn is_ascii_space(byte: u8) -> bool {
    const SPACES: u64 = (1 << b' ') | (1 << b'\t') | (1 << b'\n') | (1 << b'\r') | (1 << 0x0C);
    byte < 64 && (SPACES >> byte) & 1 == 1
}

/// Collapses runs of ASCII whitespace into single spaces while appending.
#[derive(Default)]
pub struct CollapsingLine {
    line: String,
    pending_space: bool,
}

impl CollapsingLine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends text, collapsing whitespace runs. Words are copied whole
    /// rather than character by character.
    pub fn push_text(&mut self, text: &str) {
        let bytes = text.as_bytes();
        let mut index = 0usize;
        while index < bytes.len() {
            let space_start = index;
            while index < bytes.len() && is_ascii_space(bytes[index]) {
                index += 1;
            }
            if index > space_start && !self.line.is_empty() {
                self.pending_space = true;
            }
            let word_start = index;
            while index < bytes.len() && !is_ascii_space(bytes[index]) {
                index += 1;
            }
            if index > word_start {
                if self.pending_space {
                    self.line.push(' ');
                    self.pending_space = false;
                }
                self.line.push_str(&text[word_start..index]);
            }
        }
    }

    /// Appends a literal string with no collapsing (markdown markers).
    pub fn push_raw(&mut self, text: &str) {
        if self.pending_space && !self.line.is_empty() {
            self.line.push(' ');
        }
        self.pending_space = false;
        self.line.push_str(text);
    }

    /// Appends a separator, suppressing a pending space around it.
    pub fn push_separator(&mut self, ch: char) {
        self.line.push(ch);
        self.pending_space = false;
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.line.is_empty()
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.line
    }

    /// Takes the line, resetting the buffer.
    pub fn take(&mut self) -> String {
        self.pending_space = false;
        std::mem::take(&mut self.line)
    }
}

/// Block-formatted output built in place: lines are written straight into the
/// result buffer, so extracting the text of a page allocates once rather than
/// once per block.
pub struct BlockWriter {
    out: String,
    line_start: usize,
    pending_space: bool,
}

impl Default for BlockWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockWriter {
    pub fn new() -> Self {
        Self {
            out: String::new(),
            line_start: 0,
            pending_space: false,
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            out: String::with_capacity(capacity),
            line_start: 0,
            pending_space: false,
        }
    }

    /// Whether nothing has been written to the current line yet.
    #[inline]
    pub fn line_is_empty(&self) -> bool {
        self.out.len() == self.line_start
    }

    /// Appends text, collapsing ASCII whitespace runs into single spaces.
    ///
    /// Prose is already collapsed: words separated by exactly one space need
    /// no rewriting, so the longest stretch that is already in the output form
    /// is copied in one `push_str` instead of one per word. Extracting a
    /// megabyte of article text is otherwise thousands of short copies, which
    /// is most of the cost of `--dump text`
    /// (`docs/spec/04-dom-and-css.md` §8).
    pub fn push_text(&mut self, text: &str) {
        let bytes = text.as_bytes();
        let mut index = 0usize;
        while index < bytes.len() {
            let space_start = index;
            while index < bytes.len() && is_ascii_space(bytes[index]) {
                index += 1;
            }
            if index > space_start && !self.line_is_empty() {
                self.pending_space = true;
            }
            if index == bytes.len() {
                break;
            }

            let run_start = index;
            let run_end = loop {
                while index < bytes.len() && !is_ascii_space(bytes[index]) {
                    index += 1;
                }
                let word_end = index;
                // A single plain space followed by more text is already what
                // the output would contain, so the run continues through it.
                if bytes.get(index) == Some(&b' ')
                    && bytes.get(index + 1).is_some_and(|b| !is_ascii_space(*b))
                {
                    index += 1;
                    continue;
                }
                break word_end;
            };
            if self.pending_space {
                self.out.push(' ');
                self.pending_space = false;
            }
            self.out.push_str(&text[run_start..run_end]);
        }
    }

    /// Appends a separator inside the current line, dropping any pending
    /// space around it.
    pub fn push_separator(&mut self, ch: char) {
        self.out.push(ch);
        self.pending_space = false;
    }

    /// Appends a literal string.
    pub fn push_raw(&mut self, text: &str) {
        self.out.push_str(text);
        self.pending_space = false;
    }

    /// Appends preformatted text: whitespace is kept and every newline ends
    /// the current line, so `<pre>` and `<textarea>` keep their shape
    /// (`docs/spec/04-dom-and-css.md` §5).
    pub fn push_preformatted(&mut self, text: &str) {
        self.pending_space = false;
        for (index, line) in text.split('\n').enumerate() {
            if index > 0 {
                // An empty line inside a preformatted block is content, so it
                // is written rather than dropped by `end_line`.
                self.out.push('\n');
                self.line_start = self.out.len();
            }
            self.out.push_str(line.trim_end_matches('\r'));
        }
    }

    /// Ends the current line, dropping it if it is empty.
    pub fn end_line(&mut self) {
        self.pending_space = false;
        if self.line_is_empty() {
            return;
        }
        self.out.push('\n');
        self.line_start = self.out.len();
    }

    /// The finished text, with the last line ended.
    pub fn finish(mut self) -> String {
        self.end_line();
        self.out
    }
}

/// Whether the node is a text or CDATA node with content.
pub(crate) fn text_of(document: &Document, node: NodeId) -> Option<&str> {
    matches!(document.kind(node), NodeKind::Text | NodeKind::CData)
        .then(|| document.text_data(node))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tenta_core::atom::atoms;

    #[test]
    fn collapsing_line_squeezes_whitespace_runs() {
        let mut line = CollapsingLine::new();
        line.push_text("  leading and   inner\n\tspace ");
        line.push_text("joined");
        assert_eq!(line.as_str(), "leading and inner space joined");
        assert_eq!(line.take(), "leading and inner space joined");
        assert!(line.is_empty());
    }

    #[test]
    fn collapsing_line_separators_swallow_the_pending_space() {
        let mut line = CollapsingLine::new();
        line.push_text("cell ");
        line.push_separator('\t');
        line.push_text("next");
        assert_eq!(line.as_str(), "cell\tnext");
    }

    #[test]
    fn block_classification_matches_the_text_shape() {
        assert!(is_block(atoms::P));
        assert!(is_block(atoms::LI));
        assert!(is_block(atoms::TR));
        assert!(is_block(atoms::BLOCKQUOTE));
        assert!(!is_block(atoms::SPAN));
        assert!(!is_block(atoms::A));
        assert!(
            !is_block(atoms::TD),
            "cells are separated by tabs, not lines"
        );
        assert!(is_table_cell(atoms::TD));
        assert!(is_table_cell(atoms::TH));
        assert_eq!(heading_level(atoms::H3), Some(3));
        assert_eq!(heading_level(atoms::P), None);
    }

    #[test]
    fn the_walker_emits_matched_enter_and_exit_events() {
        let mut document = crate::Document::new();
        let root = document.create_html_element(atoms::DIV);
        let a = document.create_html_element(atoms::SPAN);
        let b = document.create_html_element(atoms::SPAN);
        for child in [a, b] {
            document
                .mutate(crate::Mutation::Append {
                    parent: root,
                    child,
                })
                .expect("append");
        }
        let events: Vec<Event> = Walker::new(&document, root).collect();
        assert_eq!(
            events,
            vec![
                Event::Enter {
                    node: root,
                    depth: 0
                },
                Event::Enter { node: a, depth: 1 },
                Event::Exit { node: a, depth: 1 },
                Event::Enter { node: b, depth: 1 },
                Event::Exit { node: b, depth: 1 },
                Event::Exit {
                    node: root,
                    depth: 0
                },
            ]
        );
    }

    #[test]
    fn skip_children_drops_a_subtree_but_keeps_the_exit() {
        let mut document = crate::Document::new();
        let root = document.create_html_element(atoms::DIV);
        let skipped = document.create_html_element(atoms::SPAN);
        let inner = document.create_html_element(atoms::EM);
        document
            .mutate(crate::Mutation::Append {
                parent: root,
                child: skipped,
            })
            .expect("append");
        document
            .mutate(crate::Mutation::Append {
                parent: skipped,
                child: inner,
            })
            .expect("append");

        let mut walker = Walker::new(&document, root);
        let mut seen = Vec::new();
        while let Some(event) = walker.next() {
            seen.push(event);
            if event
                == (Event::Enter {
                    node: skipped,
                    depth: 1,
                })
            {
                walker.skip_children();
            }
        }
        assert!(!seen.iter().any(|event| event.node() == inner));
        assert!(seen.contains(&Event::Exit {
            node: skipped,
            depth: 1
        }));
        assert!(seen.contains(&Event::Exit {
            node: root,
            depth: 0
        }));
    }
}
