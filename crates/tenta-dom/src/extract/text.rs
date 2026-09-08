//! Raw text extraction (`docs/spec/04-dom-and-css.md` §5).
//!
//! [`text`] is the `--dump text` shape: one line per block-level element in
//! document order, inline whitespace collapsed, table cells separated by tabs.
//! It is deliberately not `innerText`: that needs style and therefore Tier 3.
//! Elements with no user-visible content (`script`, `style`, `head` and its
//! children, `template`, `noscript`) contribute nothing.

use tenta_core::NodeId;
use tenta_core::atom::atoms;

use crate::TRUNCATION_MARKER;
use crate::arena::NodeKind;
use crate::document::Document;
use crate::extract::{
    BlockWriter, CollapsingLine, Event, TAG_BLOCK, TAG_CELL, TAG_PRE, TAG_SKIP, Walker, is_skipped,
    tag_class, text_of,
};

/// Block-formatted text of the subtree rooted at `root`.
///
/// Inline whitespace is collapsed, except inside `<pre>`, `<textarea>` and
/// `<listing>`, whose line structure is content and is kept — the markdown
/// extractor already does this, and the two dump modes have to agree
/// (`docs/spec/04-dom-and-css.md` §5).
pub fn text(document: &Document, root: NodeId) -> String {
    let mut writer = BlockWriter::with_capacity(1024);
    let mut skip_depth: Option<usize> = None;
    let mut pre_depth: Option<usize> = None;
    let mut walker = Walker::visible(document, root);
    for event in walker.by_ref() {
        match event {
            Event::Enter { node, depth } => {
                if skip_depth.is_some() {
                    continue;
                }
                match document.kind(node) {
                    NodeKind::Text | NodeKind::CData => {
                        if let Some(chunk) = document.text_data(node) {
                            if pre_depth.is_some() {
                                writer.push_preformatted(chunk);
                            } else {
                                writer.push_text(chunk);
                            }
                        }
                    }
                    NodeKind::Element => {
                        let tag = document.element_tag(node);
                        let class = tag_class(tag);
                        if class & TAG_SKIP != 0 {
                            skip_depth = Some(depth);
                        } else if class & TAG_CELL != 0 {
                            if !writer.line_is_empty() {
                                writer.push_separator('\t');
                            }
                        } else if class & TAG_BLOCK != 0 || tag == atoms::BR {
                            writer.end_line();
                        }
                        if class & TAG_PRE != 0 && pre_depth.is_none() {
                            pre_depth = Some(depth);
                        }
                    }
                    _ => {}
                }
            }
            Event::Exit { node, depth } => {
                if let Some(skipped) = skip_depth {
                    if depth == skipped {
                        skip_depth = None;
                    }
                    continue;
                }
                if pre_depth == Some(depth) {
                    pre_depth = None;
                    writer.end_line();
                }
                if document.kind(node) == NodeKind::Element
                    && tag_class(document.element_tag(node)) & TAG_BLOCK != 0
                {
                    writer.end_line();
                }
            }
        }
    }
    let truncated = walker.truncated();
    let mut out = writer.finish();
    if truncated {
        out.push_str(TRUNCATION_MARKER);
        out.push('\n');
    }
    out
}

/// `textContent`: every character-data descendant concatenated, with no
/// formatting and no whitespace collapsing. Constant stack: it uses the
/// arena's next-node walk rather than an explicit one.
pub fn text_content(document: &Document, root: NodeId) -> String {
    let mut out = String::new();
    for node in document.descendants_inclusive(root) {
        if let Some(chunk) = text_of(document, node) {
            out.push_str(chunk);
        }
    }
    out
}

/// `textContent` with whitespace collapsed and trimmed; the shape link text,
/// labels and accessible names want.
pub fn collapsed_text(document: &Document, root: NodeId) -> String {
    let mut line = CollapsingLine::new();
    for node in document.descendants_inclusive(root) {
        if is_skipped(document, node) {
            continue;
        }
        if let Some(chunk) = text_of(document, node) {
            line.push_text(chunk);
        }
    }
    line.take()
}

impl Document {
    /// Block-formatted text of the whole document.
    pub fn text(&self) -> String {
        text(self, self.document_node())
    }

    /// `textContent` of a node.
    pub fn text_content(&self, node: NodeId) -> String {
        text_content(self, node)
    }
}
