//! The Tier 1 structural snapshot (`docs/spec/04-dom-and-css.md` §5,
//! `docs/spec/20-platform-behaviour.md` §8): role, accessible name, value and
//! state, as a tree agents can act on instead of a screenshot.
//!
//! Tier 1 has no style, so CSS hidden-ness (`display: none`, `visibility`) is
//! ignored by design; only the `hidden` and `aria-hidden` attributes are
//! honoured. The Tier 3 accessibility tree applies the full rules.

use std::collections::BTreeMap;

use serde_json::{Map, Value};
use tenta_core::NodeId;
use tenta_core::atom::atoms;

use crate::document::Document;
use crate::extract::text::collapsed_text;
use crate::extract::{Event, Walker, heading_level, is_skipped, text_of};

/// One node of the snapshot tree.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize)]
pub struct SnapshotNode {
    /// The element (or text node) this entry describes.
    #[serde(skip)]
    pub node: NodeId,
    /// ARIA role: the `role` attribute if present, otherwise the implicit
    /// role of the element. Text nodes have the role `text`.
    pub role: String,
    /// Accessible name (accname-lite: `aria-label`, `aria-labelledby`, `alt`,
    /// the associated label, `title`, then text content).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Current value of a control.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// State from attributes: `checked`, `disabled`, `required`, `expanded`,
    /// `selected`, `hidden`, `level`, `url`.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub state: BTreeMap<String, Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<SnapshotNode>,
}

/// Dropping the tree is iterative: a snapshot can be
/// [`crate::MAX_TRAVERSAL_DEPTH`] deep, and the derived recursive drop glue
/// would run one stack frame per level.
impl Drop for SnapshotNode {
    fn drop(&mut self) {
        let mut pending = std::mem::take(&mut self.children);
        while let Some(mut node) = pending.pop() {
            pending.append(&mut node.children);
        }
    }
}

enum JsonStep<'a> {
    Visit(&'a SnapshotNode),
    Finish(&'a SnapshotNode, usize),
}

impl SnapshotNode {
    fn own_object(&self) -> Map<String, Value> {
        let mut object = Map::new();
        object.insert("role".to_owned(), Value::String(self.role.clone()));
        if !self.name.is_empty() {
            object.insert("name".to_owned(), Value::String(self.name.clone()));
        }
        if let Some(value) = &self.value {
            object.insert("value".to_owned(), Value::String(value.clone()));
        }
        for (key, value) in &self.state {
            object.insert(key.clone(), value.clone());
        }
        object
    }

    /// The snapshot as JSON. Built with an explicit post-order stack; note
    /// that the resulting `serde_json::Value` is as deep as the snapshot, so
    /// serialising it uses serde_json's own recursion.
    pub fn to_json(&self) -> Value {
        let mut work = vec![JsonStep::Visit(self)];
        let mut done: Vec<Value> = Vec::new();
        while let Some(step) = work.pop() {
            match step {
                JsonStep::Visit(node) => {
                    work.push(JsonStep::Finish(node, node.children.len()));
                    for child in node.children.iter().rev() {
                        work.push(JsonStep::Visit(child));
                    }
                }
                JsonStep::Finish(node, count) => {
                    let children: Vec<Value> = done.split_off(done.len() - count);
                    let mut object = node.own_object();
                    if !children.is_empty() {
                        object.insert("children".to_owned(), Value::Array(children));
                    }
                    done.push(Value::Object(object));
                }
            }
        }
        done.pop().unwrap_or(Value::Null)
    }

    /// A compact indented text form, two spaces per level, capped at 16 levels
    /// to prevent quadratic output on deeply nested structures.
    pub fn to_text(&self) -> String {
        const MAX_INDENT: usize = 16;
        let mut out = String::new();
        // Iterative: the snapshot of a deep document is deep too.
        let mut stack: Vec<(&SnapshotNode, usize)> = vec![(self, 0)];
        while let Some((node, depth)) = stack.pop() {
            // Cap indentation but continue traversing to show all nodes
            let indent_depth = depth.min(MAX_INDENT);
            out.push_str(&"  ".repeat(indent_depth));
            out.push_str(&node.role);
            if !node.name.is_empty() {
                out.push_str(&format!(" \"{}\"", node.name));
            }
            if let Some(value) = &node.value {
                out.push_str(&format!(" value=\"{value}\""));
            }
            for (key, state) in &node.state {
                out.push_str(&format!(" {key}={state}"));
            }
            out.push('\n');
            for child in node.children.iter().rev() {
                stack.push((child, depth + 1));
            }
        }
        out
    }
}

/// The implicit ARIA role of an element, or `None` when it maps to nothing
/// interesting.
fn implicit_role(document: &Document, node: NodeId) -> Option<&'static str> {
    let tag = document.tag(node)?;
    let role = match tag {
        t if t == atoms::A || t == atoms::AREA => {
            if document.has_attribute(node, atoms::HREF) {
                "link"
            } else {
                "generic"
            }
        }
        t if t == atoms::BUTTON => "button",
        t if t == atoms::INPUT => match document
            .attribute(node, atoms::TYPE)
            .map(|t| t.trim().to_ascii_lowercase())
            .unwrap_or_else(|| "text".to_owned())
            .as_str()
        {
            "button" | "submit" | "reset" | "image" => "button",
            "checkbox" => "checkbox",
            "radio" => "radio",
            "range" => "slider",
            "search" => "searchbox",
            "number" => "spinbutton",
            "hidden" => return None,
            _ => "textbox",
        },
        t if t == atoms::SELECT => {
            if document.has_attribute(node, atoms::MULTIPLE) {
                "listbox"
            } else {
                "combobox"
            }
        }
        t if t == atoms::TEXTAREA => "textbox",
        t if t == atoms::OPTION => "option",
        t if t == atoms::OPTGROUP => "group",
        t if t == atoms::IMG => {
            if document.attribute(node, atoms::ALT) == Some("") {
                "presentation"
            } else {
                "image"
            }
        }
        t if heading_level(t).is_some() => "heading",
        t if t == atoms::NAV => "navigation",
        t if t == atoms::MAIN => "main",
        t if t == atoms::HEADER => "banner",
        t if t == atoms::FOOTER => "contentinfo",
        t if t == atoms::ASIDE => "complementary",
        t if t == atoms::SECTION => "region",
        t if t == atoms::ARTICLE => "article",
        t if t == atoms::FORM => "form",
        t if t == atoms::SEARCH => "search",
        t if t == atoms::TABLE => "table",
        t if t == atoms::TR => "row",
        t if t == atoms::TD => "cell",
        t if t == atoms::TH => "columnheader",
        t if t == atoms::THEAD || t == atoms::TBODY || t == atoms::TFOOT => "rowgroup",
        t if t == atoms::CAPTION => "caption",
        t if t == atoms::UL || t == atoms::OL => "list",
        t if t == atoms::LI => "listitem",
        t if t == atoms::DL => "list",
        t if t == atoms::P => "paragraph",
        t if t == atoms::BLOCKQUOTE => "blockquote",
        t if t == atoms::FIGURE => "figure",
        t if t == atoms::FIGCAPTION => "caption",
        t if t == atoms::DETAILS => "group",
        t if t == atoms::SUMMARY => "button",
        t if t == atoms::DIALOG => "dialog",
        t if t == atoms::FIELDSET => "group",
        t if t == atoms::LEGEND => "legend",
        t if t == atoms::LABEL => "label",
        t if t == atoms::PROGRESS => "progressbar",
        t if t == atoms::METER => "meter",
        t if t == atoms::IFRAME => "iframe",
        t if t == atoms::VIDEO => "video",
        t if t == atoms::AUDIO => "audio",
        t if t == atoms::CANVAS => "canvas",
        t if t == atoms::HTML || t == atoms::BODY => "document",
        _ => "generic",
    };
    Some(role)
}

/// Accessible name for a listitem, joining nested content with spaces.
fn accessible_name_for_listitem(document: &Document, node: NodeId) -> String {
    let mut parts = Vec::new();
    for child in document.element_children(node) {
        match document.tag(child) {
            Some(t) if t == atoms::OL || t == atoms::UL => {
                // Skip nested lists, their items will be handled separately
            }
            _ => {
                // Collect text from non-list elements
                let text = collapsed_text(document, child);
                if !text.is_empty() {
                    parts.push(text);
                }
            }
        }
    }
    parts.join(" ")
}

/// Accessible name, accname-lite (`docs/spec/20-platform-behaviour.md` §8:
/// the full algorithm belongs to the Tier 3 tree).
fn accessible_name(document: &Document, node: NodeId, role: &str) -> String {
    if let Some(label) = document.attribute(node, atoms::ARIA_LABEL) {
        let label = label.trim();
        if !label.is_empty() {
            return label.to_owned();
        }
    }
    if let Some(ids) = document.attribute(node, atoms::ARIA_LABELLEDBY) {
        let mut parts = Vec::new();
        for id in ids.split_ascii_whitespace() {
            if let Some(target) = document.get_element_by_id(id) {
                let text = collapsed_text(document, target);
                if !text.is_empty() {
                    parts.push(text);
                }
            }
        }
        if !parts.is_empty() {
            return parts.join(" ");
        }
    }
    let tag = document.tag(node);
    if let Some(tag) = tag {
        if tag == atoms::IMG
            && let Some(alt) = document.attribute(node, atoms::ALT)
        {
            return alt.trim().to_owned();
        }
        if tag == atoms::INPUT || tag == atoms::SELECT || tag == atoms::TEXTAREA {
            if let Some(id) = document.attribute(node, atoms::ID)
                && let Some(label) = document
                    .get_elements_by_tag_name("label")
                    .into_iter()
                    .find(|label| document.attribute(*label, atoms::FOR) == Some(id))
            {
                let text = collapsed_text(document, label);
                if !text.is_empty() {
                    return text;
                }
            }
            if let Some(wrapping) = document
                .ancestors(node)
                .find(|a| document.tag(*a) == Some(atoms::LABEL))
            {
                let text = collapsed_text(document, wrapping);
                if !text.is_empty() {
                    return text;
                }
            }
            if let Some(placeholder) = document.attribute(node, atoms::PLACEHOLDER) {
                return placeholder.trim().to_owned();
            }
            if tag == atoms::INPUT
                && let Some(value) = document.attribute(node, atoms::VALUE)
            {
                return value.trim().to_owned();
            }
        }
    }
    // Roles whose name comes from their contents.
    const FROM_CONTENT: &[&str] = &[
        "link",
        "button",
        "heading",
        "cell",
        "columnheader",
        "rowheader",
        "option",
        "listitem",
        "caption",
        "label",
        "legend",
    ];
    if FROM_CONTENT.contains(&role) {
        let content = if role == "listitem" {
            // For list items, join nested content with spaces
            accessible_name_for_listitem(document, node)
        } else {
            collapsed_text(document, node)
        };
        if !content.is_empty() {
            return content;
        }
        // Fall back to title if content is empty
        if let Some(title) = document.attribute(node, atoms::TITLE) {
            let title = title.trim();
            if !title.is_empty() {
                return title.to_owned();
            }
        }
        return String::new();
    }
    // For other roles, title is the name
    if let Some(title) = document.attribute(node, atoms::TITLE) {
        let title = title.trim();
        if !title.is_empty() {
            return title.to_owned();
        }
    }
    String::new()
}

fn control_value(document: &Document, node: NodeId) -> Option<String> {
    let tag = document.tag(node)?;
    if tag == atoms::INPUT {
        return document.attribute(node, atoms::VALUE).map(str::to_owned);
    }
    if tag == atoms::TEXTAREA {
        return Some(document.text_content(node));
    }
    if tag == atoms::SELECT {
        let selected = document
            .descendants(node)
            .filter(|child| document.tag(*child) == Some(atoms::OPTION))
            .find(|option| document.has_attribute(*option, atoms::SELECTED));
        return selected.map(|option| collapsed_text(document, option));
    }
    if tag == atoms::PROGRESS || tag == atoms::METER {
        return document.attribute(node, atoms::VALUE).map(str::to_owned);
    }
    None
}

fn state_of(document: &Document, node: NodeId, role: &str) -> BTreeMap<String, Value> {
    let mut state = BTreeMap::new();
    if document.has_attribute(node, atoms::DISABLED)
        || document.attribute(node, atoms::ARIA_DISABLED) == Some("true")
    {
        state.insert("disabled".to_owned(), Value::Bool(true));
    }
    if document.has_attribute(node, atoms::CHECKED) {
        state.insert("checked".to_owned(), Value::Bool(true));
    } else if let Some(checked) = document.attribute(node, atoms::ARIA_CHECKED) {
        state.insert("checked".to_owned(), Value::String(checked.to_owned()));
    }
    if document.has_attribute(node, atoms::REQUIRED)
        || document.attribute(node, atoms::ARIA_REQUIRED) == Some("true")
    {
        state.insert("required".to_owned(), Value::Bool(true));
    }
    if document.has_attribute(node, atoms::SELECTED)
        || document.attribute(node, atoms::ARIA_SELECTED) == Some("true")
    {
        state.insert("selected".to_owned(), Value::Bool(true));
    }
    if let Some(expanded) = document.attribute(node, atoms::ARIA_EXPANDED) {
        state.insert("expanded".to_owned(), Value::String(expanded.to_owned()));
    } else if document.tag(node) == Some(atoms::DETAILS) {
        state.insert(
            "expanded".to_owned(),
            Value::Bool(document.has_attribute(node, atoms::OPEN)),
        );
    }
    if document.has_attribute(node, atoms::HIDDEN)
        || document.attribute(node, atoms::ARIA_HIDDEN) == Some("true")
    {
        state.insert("hidden".to_owned(), Value::Bool(true));
    }
    // Level: prefer aria-level if present and valid, otherwise use heading level
    let mut has_level = false;
    if let Some(level_str) = document.attribute(node, atoms::ARIA_LEVEL)
        && let Ok(level_num) = level_str.trim().parse::<u8>()
    {
        state.insert("level".to_owned(), Value::from(level_num));
        has_level = true;
    }
    if !has_level
        && role == "heading"
        && let Some(tag) = document.tag(node)
        && let Some(level) = heading_level(tag)
    {
        state.insert("level".to_owned(), Value::from(level));
    }
    if role == "link"
        && let Some(href) = document.attribute(node, atoms::HREF)
    {
        let url = document
            .resolve_url(href.trim())
            .map(|url| url.to_string())
            .unwrap_or_else(|| href.trim().to_owned());
        state.insert("url".to_owned(), Value::String(url));
    }
    state
}

/// Builds the structural snapshot of the subtree rooted at `root`.
///
/// The tree is assembled from a flat pre-order list, so nothing recurses on
/// the way in; `SnapshotNode::to_text` and `to_json` are iterative too.
/// The snapshot is capped at 100,000 nodes to keep output bounded.
pub fn snapshot(document: &Document, root: NodeId) -> SnapshotNode {
    const MAX_SNAPSHOT_NODES: usize = 100_000;

    let mut flat: Vec<Option<SnapshotNode>> = Vec::new();
    let mut parents: Vec<usize> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    let mut skip_depth: Option<usize> = None;
    let mut omitted_nodes: usize = 0;
    let mut walker = Walker::visible(document, root);

    for event in walker.by_ref() {
        match event {
            Event::Enter { node, depth } => {
                if skip_depth.is_some() {
                    continue;
                }
                if is_skipped(document, node) {
                    skip_depth = Some(depth);
                    continue;
                }

                // Check if we've hit the node cap
                if flat.len() >= MAX_SNAPSHOT_NODES {
                    omitted_nodes += 1;
                    skip_depth = Some(depth);
                    continue;
                }
                if let Some(chunk) = text_of(document, node) {
                    let text = chunk.split_whitespace().collect::<Vec<_>>().join(" ");
                    if text.is_empty() {
                        continue;
                    }
                    let index = flat.len();
                    flat.push(Some(SnapshotNode {
                        node,
                        role: "text".to_owned(),
                        name: text,
                        value: None,
                        state: BTreeMap::new(),
                        children: Vec::new(),
                    }));
                    parents.push(open.last().copied().unwrap_or(usize::MAX));
                    let _ = index;
                    continue;
                }
                if !document.is_element(node) && node != root {
                    continue;
                }

                // Skip non-element roots (like document_node) but still process their children
                if !document.is_element(node) && node == root {
                    // This is a non-element root (e.g., document_node), skip it but process children
                    continue;
                }

                let role = document
                    .attribute(node, atoms::ROLE)
                    .map(|role| role.trim().to_ascii_lowercase())
                    .filter(|role| !role.is_empty())
                    .or_else(|| implicit_role(document, node).map(str::to_owned))
                    .unwrap_or_else(|| "document".to_owned());
                let name = accessible_name(document, node, &role);
                let index = flat.len();
                flat.push(Some(SnapshotNode {
                    node,
                    value: control_value(document, node),
                    state: state_of(document, node, &role),
                    name,
                    role,
                    children: Vec::new(),
                }));
                parents.push(open.last().copied().unwrap_or(usize::MAX));
                open.push(index);
            }
            Event::Exit { node, depth } => {
                if let Some(skipped) = skip_depth {
                    if depth == skipped {
                        skip_depth = None;
                    }
                    continue;
                }
                // Only pop if we actually emitted a node (not for non-element roots)
                if document.is_element(node) {
                    open.pop();
                }
            }
        }
    }

    if flat.is_empty() {
        return SnapshotNode {
            node: root,
            role: "document".to_owned(),
            name: String::new(),
            value: None,
            state: BTreeMap::new(),
            children: Vec::new(),
        };
    }

    // Attach children to parents from the back, so a node is complete before
    // it is moved into its parent.
    for index in (1..flat.len()).rev() {
        let parent = parents[index];
        if parent == usize::MAX || parent >= flat.len() {
            continue;
        }
        let Some(child) = flat[index].take() else {
            continue;
        };
        if let Some(slot) = flat[parent].as_mut() {
            slot.children.push(child);
        }
    }
    let mut root_node = flat[0].take().expect("the root entry is never moved");
    // Children were pushed back-to-front.
    let mut stack = vec![&mut root_node];
    while let Some(node) = stack.pop() {
        node.children.reverse();
        for child in node.children.iter_mut() {
            stack.push(child);
        }
    }

    // Add a marker node if we hit the cap
    if omitted_nodes > 0 {
        let mut marker_state = BTreeMap::new();
        marker_state.insert("omitted".to_owned(), Value::Number(omitted_nodes.into()));
        root_node.children.push(SnapshotNode {
            node: root,
            role: "doc:end-notes".to_owned(),
            name: format!("{} more nodes", omitted_nodes),
            value: None,
            state: marker_state,
            children: Vec::new(),
        });
    }

    root_node
}

impl Document {
    /// Structural snapshot of the document.
    pub fn snapshot(&self) -> SnapshotNode {
        snapshot(self, self.document_node())
    }
}
