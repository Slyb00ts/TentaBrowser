//! Markdown rendering of a subtree (`docs/spec/04-dom-and-css.md` §5):
//! headings, paragraphs, nested lists, tables, links, images, code with a
//! language hint, block quotes and emphasis.
//!
//! One iterative pass with an explicit block buffer. Tables and preformatted
//! blocks are rendered from their own subtree in one step and then skipped,
//! which keeps the streaming state small; a table nested inside a table cell
//! therefore appears as cell text.

use tenta_core::NodeId;
use tenta_core::atom::atoms;

use crate::TRUNCATION_MARKER;
use crate::document::Document;
use crate::extract::text::text_content;
use crate::extract::{
    CollapsingLine, Event, Hides, Walker, heading_level, hides, is_skipped, text_of,
};

/// Backslash-escapes the characters that would otherwise be read as markup.
///
/// Page text is content, not markup: `*not bold*` must render as literal
/// asterisks and `[x](y)` must not become a link
/// (`docs/spec/04-dom-and-css.md` §5). Only the inline constructs are escaped
/// here; the block-level ones (`#`, `>`, `-`, `1.`) only matter at the start
/// of a line and are handled by [`escape_line_start`].
fn escape_inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(
            ch,
            '\\' | '`'
                | '*'
                | '_'
                | '{'
                | '}'
                | '['
                | ']'
                | '('
                | ')'
                | '#'
                | '+'
                | '|'
                | '<'
                | '~'
        ) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Escapes a leading `#`, `>`, `-`, `+`, `=` or `1.` so a run of text cannot
/// open a block the document did not contain.
fn escape_line_start(line: &str) -> String {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return line.to_owned();
    }
    let indent = &line[..line.len() - trimmed.len()];
    let first = trimmed.as_bytes()[0];
    let needs_escape = matches!(first, b'>' | b'-' | b'=')
        || (first.is_ascii_digit()
            && trimmed
                .find(['.', ')'])
                .is_some_and(|position| trimmed[..position].bytes().all(|b| b.is_ascii_digit())));
    if !needs_escape {
        return line.to_owned();
    }
    if first.is_ascii_digit() {
        let position = trimmed.find(['.', ')']).expect("checked above");
        return format!("{indent}{}\\{}", &trimmed[..position], &trimmed[position..]);
    }
    format!("{indent}\\{trimmed}")
}

#[derive(Clone, Copy)]
struct ListLevel {
    ordered: bool,
    index: usize,
}

struct Renderer {
    out: String,
    line: CollapsingLine,
    prefix: String,
    lists: Vec<ListLevel>,
    quote_depth: usize,
    /// Nesting of `<code>`: text inside a code span is already literal.
    code_depth: usize,
    /// Stack tracking open links: true if the link has an href, false if not.
    open_links: Vec<bool>,
}

impl Renderer {
    fn flush(&mut self) {
        if self.line.is_empty() {
            let _ = self.line.take();
            self.prefix.clear();
            return;
        }
        let body = self.line.take();
        // Skip whitespace-only paragraphs (e.g., from <br><br>)
        if body.trim().is_empty() {
            self.prefix.clear();
            return;
        }
        let quote = "> ".repeat(self.quote_depth);
        // A block that already carries a marker cannot be reinterpreted, so
        // only an unprefixed run needs its line start escaped.
        let body = if self.prefix.is_empty() && self.quote_depth == 0 {
            escape_line_start(&body)
        } else {
            body
        };
        self.out.push_str(&quote);
        self.out.push_str(&self.prefix);
        self.out.push_str(&body);
        self.out.push_str("\n\n");
        self.prefix.clear();
    }

    fn start_block(&mut self, prefix: String) {
        self.flush();
        self.prefix = prefix;
    }

    fn push_block(&mut self, block: &str) {
        self.flush();
        let quote = "> ".repeat(self.quote_depth);
        for line in block.lines() {
            self.out.push_str(&quote);
            self.out.push_str(line);
            self.out.push('\n');
        }
        self.out.push('\n');
    }
}

/// Language hint from a `class="language-x"` or `class="lang-x"` attribute on
/// the `<pre>` or its `<code>` child.
fn code_language(document: &Document, node: NodeId) -> Option<String> {
    let mut candidates = vec![node];
    candidates.extend(document.element_children(node));
    for candidate in candidates {
        for class in document.classes(candidate) {
            if let Some(rest) = class
                .strip_prefix("language-")
                .or_else(|| class.strip_prefix("lang-"))
                && !rest.is_empty()
            {
                return Some(rest.to_owned());
            }
        }
    }
    None
}

/// Walks descendants but stops at nested tables, yielding only rows that
/// belong directly to the current table (not to descendant tables).
fn rows_of_table(document: &Document, table: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    document.descendants(table).filter(move |node| {
        if document.tag(*node) != Some(atoms::TR) {
            return false;
        }
        // Check if this row is inside a nested table. If any ancestor
        // between the row and the table is a table, this row belongs to that table.
        for ancestor in document.ancestors(*node) {
            if ancestor == table {
                return true;
            }
            if document.tag(ancestor) == Some(atoms::TABLE) {
                return false;
            }
        }
        true
    })
}

/// Gets immediate cell children of a row (cells at the top level of the row,
/// not inside nested tables or other containers).
fn cells_of_row(document: &Document, row: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    document
        .element_children(row)
        .filter(|cell| matches!(document.tag(*cell), Some(t) if t == atoms::TD || t == atoms::TH))
}

/// Checks if a table is a layout table (contains block-level content or nested tables).
/// Layout tables should be rendered as blocks instead of using pipe syntax.
fn is_layout_table(document: &Document, table: NodeId) -> bool {
    // Block-level elements that indicate a layout table
    let block_tags = [
        atoms::H1,
        atoms::H2,
        atoms::H3,
        atoms::H4,
        atoms::H5,
        atoms::H6,
        atoms::P,
        atoms::DIV,
        atoms::UL,
        atoms::OL,
        atoms::DL,
        atoms::BLOCKQUOTE,
        atoms::PRE,
        atoms::TABLE,
    ];

    for cell in document
        .descendants(table)
        .filter(|n| matches!(document.tag(*n), Some(t) if t == atoms::TD || t == atoms::TH))
    {
        // Check immediate children of the cell
        for child in document.element_children(cell) {
            if let Some(tag) = document.tag(child) {
                // Skip table-related and structural elements
                if matches!(tag, t if t == atoms::TBODY || t == atoms::THEAD ||
                                     t == atoms::TFOOT || t == atoms::TR ||
                                     t == atoms::CAPTION)
                {
                    continue;
                }
                // If we find a block element, it's a layout table
                if block_tags.contains(&tag) {
                    return true;
                }
            }
        }
    }
    false
}

/// Get text content of a table cell, treating <br> as a space.
fn table_cell_text(document: &Document, cell: NodeId) -> String {
    let mut parts = Vec::new();
    let mut walker = Walker::visible(document, cell);
    while let Some(event) = walker.next() {
        let Event::Enter { node, .. } = event else {
            continue;
        };
        // Nested tables never reach a pipe table; a cell that holds one is
        // rendered as blocks instead.
        if document.tag(node) == Some(atoms::TABLE) {
            walker.skip_children();
            continue;
        }
        // <br> becomes a space separator
        if document.tag(node) == Some(atoms::BR) {
            if !parts.is_empty()
                && !parts.last().is_some_and(|s: &String| s.ends_with(' '))
                && let Some(last) = parts.last_mut()
                && !last.is_empty()
            {
                last.push(' ');
            }
            continue;
        }
        if let Some(text) = text_of(document, node) {
            parts.push(text.to_owned());
        }
    }
    let combined = parts.join("");
    let mut line = CollapsingLine::new();
    line.push_text(&combined);
    line.take()
}

fn render_table(document: &Document, table: NodeId) -> String {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut header: Option<Vec<String>> = None;
    for row in rows_of_table(document, table) {
        // A hidden cell is not a column, the same as a hidden row is not a row.
        let visible: Vec<NodeId> = cells_of_row(document, row)
            .filter(|cell| hides(document, *cell) != Hides::Subtree)
            .collect();
        let cells: Vec<String> = visible
            .iter()
            .map(|cell| escape_inline(&table_cell_text(document, *cell)))
            .collect();
        if cells.is_empty() {
            continue;
        }
        let all_headers = visible
            .iter()
            .all(|cell| document.tag(*cell) == Some(atoms::TH));
        if header.is_none() && all_headers {
            header = Some(cells);
        } else {
            rows.push(cells);
        }
    }
    let width = header
        .as_ref()
        .map(Vec::len)
        .into_iter()
        .chain(rows.iter().map(Vec::len))
        .max()
        .unwrap_or(0);
    if width == 0 {
        return String::new();
    }
    let header = header.unwrap_or_else(|| vec![String::new(); width]);
    let mut out = String::new();
    let pad = |cells: &[String]| {
        let mut row: Vec<String> = cells.to_vec();
        row.resize(width, String::new());
        format!("| {} |", row.join(" | "))
    };
    out.push_str(&pad(&header));
    out.push('\n');
    out.push_str("| ");
    out.push_str(&vec!["---"; width].join(" | "));
    out.push_str(" |\n");
    for row in &rows {
        out.push_str(&pad(row));
        out.push('\n');
    }
    out
}

/// Renders the subtree rooted at `root` as Markdown.
pub fn markdown(document: &Document, root: NodeId) -> String {
    let mut renderer = Renderer {
        out: String::new(),
        line: CollapsingLine::new(),
        prefix: String::new(),
        lists: Vec::new(),
        quote_depth: 0,
        code_depth: 0,
        open_links: Vec::new(),
    };
    let mut skip_depth: Option<usize> = None;
    let mut walker = Walker::visible(document, root);
    while let Some(event) = walker.next() {
        match event {
            Event::Enter { node, depth } => {
                if skip_depth.is_some() {
                    continue;
                }
                if is_skipped(document, node) {
                    skip_depth = Some(depth);
                    continue;
                }
                if let Some(chunk) = text_of(document, node) {
                    // Inside a code span the delimiters are literal already.
                    if renderer.code_depth > 0 {
                        renderer.line.push_text(chunk);
                    } else {
                        renderer.line.push_text(&escape_inline(chunk));
                    }
                    continue;
                }
                let Some(tag) = document.tag(node) else {
                    continue;
                };
                if let Some(level) = heading_level(tag) {
                    renderer.start_block(format!("{} ", "#".repeat(level as usize)));
                } else if tag == atoms::P
                    || tag == atoms::DIV
                    || tag == atoms::FIGCAPTION
                    || tag == atoms::NAV
                    || tag == atoms::FOOTER
                    || tag == atoms::HEADER
                    || tag == atoms::SECTION
                    || tag == atoms::ARTICLE
                    || tag == atoms::ASIDE
                    || tag == atoms::MAIN
                    || tag == atoms::TEXTAREA
                    || tag == atoms::DETAILS
                    || tag == atoms::FIGURE
                {
                    renderer.start_block(String::new());
                } else if tag == atoms::UL || tag == atoms::OL {
                    renderer.flush();
                    renderer.lists.push(ListLevel {
                        ordered: tag == atoms::OL,
                        index: 0,
                    });
                } else if tag == atoms::DL {
                    renderer.flush();
                    renderer.lists.push(ListLevel {
                        ordered: false,
                        index: 0,
                    });
                } else if tag == atoms::LI || tag == atoms::DD {
                    let depth = renderer.lists.len().saturating_sub(1);
                    let marker = match renderer.lists.last_mut() {
                        Some(level) => {
                            level.index += 1;
                            if level.ordered {
                                format!("{}. ", level.index)
                            } else {
                                "- ".to_owned()
                            }
                        }
                        None => "- ".to_owned(),
                    };
                    renderer.start_block(format!("{}{marker}", "  ".repeat(depth)));
                } else if tag == atoms::DT {
                    renderer.flush();
                    renderer.line.push_raw("**");
                } else if tag == atoms::SUMMARY {
                    renderer.start_block(String::new());
                } else if tag == atoms::BLOCKQUOTE {
                    renderer.flush();
                    renderer.quote_depth += 1;
                } else if tag == atoms::PRE || tag == atoms::LISTING {
                    let language = code_language(document, node).unwrap_or_default();
                    let body = text_content(document, node);
                    let block = format!("```{language}\n{}\n```", body.trim_end_matches('\n'));
                    renderer.push_block(&block);
                    walker.skip_children();
                } else if tag == atoms::TABLE {
                    // Check if this is a layout table with block-level content
                    if is_layout_table(document, node) {
                        // For layout tables, render content as blocks, not pipe syntax
                        // Don't skip children - let them be processed normally
                    } else {
                        // For data tables, render with pipe syntax
                        let table = render_table(document, node);
                        if !table.is_empty() {
                            renderer.push_block(table.trim_end());
                        }
                        walker.skip_children();
                    }
                } else if tag == atoms::HR {
                    renderer.push_block("---");
                } else if tag == atoms::BR {
                    renderer.line.push_raw("  ");
                    renderer.flush();
                } else if tag == atoms::A {
                    // Check if this link has an href
                    let has_href = document.attribute(node, atoms::HREF).is_some();
                    renderer.open_links.push(has_href);
                    if has_href {
                        renderer.line.push_raw("[");
                    }
                } else if tag == atoms::IMG {
                    // Only render images that have a src attribute
                    if let Some(src_attr) = document.attribute(node, atoms::SRC) {
                        let alt = escape_inline(document.attribute(node, atoms::ALT).unwrap_or(""));
                        let src = document
                            .resolve_url(src_attr.trim())
                            .map(|url| url.to_string())
                            .or_else(|| Some(src_attr.trim().to_owned()))
                            .unwrap_or_default();
                        renderer.line.push_raw(&format!("![{alt}]({src})"));
                    }
                } else if tag == atoms::CODE {
                    renderer.code_depth += 1;
                    renderer.line.push_raw("`");
                } else if tag == atoms::STRONG || tag == atoms::B {
                    renderer.line.push_raw("**");
                } else if tag == atoms::EM || tag == atoms::I {
                    renderer.line.push_raw("*");
                }
            }
            Event::Exit { node, depth } => {
                if let Some(skipped) = skip_depth {
                    if depth == skipped {
                        skip_depth = None;
                    }
                    continue;
                }
                let Some(tag) = document.tag(node) else {
                    continue;
                };
                if heading_level(tag).is_some()
                    || tag == atoms::P
                    || tag == atoms::DIV
                    || tag == atoms::LI
                    || tag == atoms::FIGCAPTION
                    || tag == atoms::NAV
                    || tag == atoms::FOOTER
                    || tag == atoms::HEADER
                    || tag == atoms::SECTION
                    || tag == atoms::ARTICLE
                    || tag == atoms::ASIDE
                    || tag == atoms::MAIN
                    || tag == atoms::TEXTAREA
                    || tag == atoms::DETAILS
                    || tag == atoms::FIGURE
                    || tag == atoms::DD
                {
                    renderer.flush();
                } else if tag == atoms::DT {
                    renderer.line.push_raw("**");
                    renderer.flush();
                } else if tag == atoms::SUMMARY {
                    renderer.flush();
                } else if tag == atoms::UL || tag == atoms::OL || tag == atoms::DL {
                    renderer.flush();
                    renderer.lists.pop();
                } else if tag == atoms::BLOCKQUOTE {
                    renderer.flush();
                    renderer.quote_depth = renderer.quote_depth.saturating_sub(1);
                } else if tag == atoms::A {
                    if let Some(has_href) = renderer.open_links.pop()
                        && has_href
                    {
                        let href = document
                            .attribute(node, atoms::HREF)
                            .and_then(|href| document.resolve_url(href.trim()))
                            .map(|url| url.to_string())
                            .or_else(|| document.attribute(node, atoms::HREF).map(str::to_owned))
                            .unwrap_or_default();
                        renderer.line.push_raw(&format!("]({href})"));
                    }
                    // If no href, we already didn't add the opening bracket, so nothing to do
                } else if tag == atoms::CODE {
                    renderer.code_depth = renderer.code_depth.saturating_sub(1);
                    renderer.line.push_raw("`");
                } else if tag == atoms::STRONG || tag == atoms::B {
                    renderer.line.push_raw("**");
                } else if tag == atoms::EM || tag == atoms::I {
                    renderer.line.push_raw("*");
                }
            }
        }
    }
    renderer.flush();
    if walker.truncated() {
        renderer.out.push_str(TRUNCATION_MARKER);
        renderer.out.push('\n');
    }
    let trimmed = renderer.out.trim_end().to_owned();
    if trimmed.is_empty() {
        trimmed
    } else {
        trimmed + "\n"
    }
}

impl Document {
    /// The whole document as Markdown.
    pub fn markdown(&self) -> String {
        markdown(self, self.document_node())
    }
}
