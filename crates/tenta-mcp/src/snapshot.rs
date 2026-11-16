//! `browser_snapshot`: the page as a tree an agent can act on.
//!
//! The structure (roles, accessible names, state) comes from
//! `tenta_dom::extract::snapshot`. This module decides what an agent sees of
//! it: layout-less wrappers are flattened, text that only repeats a control's
//! name is dropped, hidden elements are left out, and every remaining element
//! gets a ref that stays the same for one document.
//!
//! Hidden-ness has two sources. With a layout (the render tier), an element
//! without a box is hidden (`display: none` itself or inherited), and the
//! computed `visibility` and `opacity` of the layout apply, whatever sheet or
//! attribute they come from. Without one, the `hidden` and `aria-hidden`
//! attributes (applied by the extractor) and an inline `style` that sets
//! `display: none` or `visibility: hidden` are all there is; stylesheets are
//! not consulted.

use std::collections::HashMap;

use serde_json::Value;
use tenta_core::NodeId;
use tenta_core::atom::atoms;
use tenta_dom::Document;
use tenta_dom::extract::SnapshotNode;

use crate::session::RefTable;

/// What layout says about showing one element that has a box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// Border box, `[x, y, width, height]`.
    pub rect: [f64; 4],
    /// The computed `visibility` is `hidden` or `collapse`. It is inherited, so
    /// it already accounts for the ancestors and for a child turning it back on.
    pub visibility_hidden: bool,
    /// The computed `opacity` is 0.
    pub transparent: bool,
}

/// The look of every element, indexed like `querySelectorAll('*')`; `None` for
/// an element without a box.
pub type Layout = Vec<Option<Look>>;

pub struct Options {
    /// Elements and text lines emitted before the output is cut.
    pub max_nodes: usize,
    pub include_boxes: bool,
    /// Snapshot only this element's subtree (a ref or selector scope).
    pub scope: Option<NodeId>,
}

/// An element the structured result lists.
#[derive(Clone, Debug)]
pub struct Entry {
    pub reference: String,
    pub role: String,
    pub name: String,
}

pub struct Built {
    pub lines: Vec<String>,
    /// Interactive elements, for programmatic use of the result.
    pub interactive: Vec<Entry>,
    pub elements: usize,
    pub truncated: bool,
    pub hidden: usize,
    pub layout_used: bool,
}

/// Roles that carry no meaning of their own: their children are shown in
/// their place.
const TRANSPARENT: &[&str] = &[
    "generic",
    "document",
    "presentation",
    "none",
    "doc:end-notes",
];

/// Roles an agent can act on.
const INTERACTIVE: &[&str] = &[
    "link",
    "button",
    "textbox",
    "searchbox",
    "checkbox",
    "radio",
    "combobox",
    "listbox",
    "option",
    "slider",
    "spinbutton",
    "switch",
    "tab",
    "menuitem",
    "clickable",
];

/// Roles whose name is their content: a text child would only repeat it.
const NAMED_BY_CONTENT: &[&str] = &[
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
    "tab",
    "menuitem",
];

const MAX_NAME_CHARS: usize = 160;
/// Options of a select that a snapshot line names, and how long each may be.
const MAX_LISTED_OPTIONS: usize = 40;
const MAX_OPTION_CHARS: usize = 60;
const MAX_TEXT_CHARS: usize = 2000;
const MAX_INDENT: usize = 24;

fn quote(text: &str, limit: usize) -> String {
    let mut out = String::with_capacity(text.len().min(limit) + 2);
    out.push('"');
    for (count, character) in text.chars().enumerate() {
        if count == limit {
            out.push('…');
            break;
        }
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' | '\r' | '\t' => out.push(' '),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// What an element's `style` attribute says about showing it. It is the whole
/// answer only without a layout; with one, the computed `visibility` and
/// `opacity` of [`Look`] replace it, and `display: none` from any source is seen
/// through the missing layout box.
#[derive(Clone, Copy, Default)]
struct Inline {
    display_none: bool,
    visibility_hidden: bool,
    visibility_visible: bool,
    opacity_zero: bool,
}

fn inline_style(doc: &Document, node: NodeId) -> Inline {
    let Some(style) = doc.attribute_str(node, "style") else {
        return Inline::default();
    };
    let compact: String = style
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    let has = |declaration: &str| {
        compact
            .split(';')
            .any(|part| part == declaration || part.strip_suffix("!important") == Some(declaration))
    };
    Inline {
        display_none: has("display:none"),
        visibility_hidden: has("visibility:hidden") || has("visibility:collapse"),
        visibility_visible: has("visibility:visible"),
        opacity_zero: has("opacity:0") || has("opacity:0.0"),
    }
}

fn element_indexes(doc: &Document) -> HashMap<NodeId, usize> {
    doc.descendants(doc.document_node())
        .filter(|node| doc.is_element(*node))
        .enumerate()
        .map(|(index, node)| (node, index))
        .collect()
}

struct Frame<'a> {
    node: &'a SnapshotNode,
    depth: usize,
    /// The parent is named by its content, so a text child repeats it.
    suppress_text: bool,
    /// The parent element has a box (always true without a layout).
    parent_visible: bool,
    /// The parent's `visibility` is hidden, which a child may override.
    parent_invisible: bool,
}

pub fn build(
    doc: &Document,
    table: &mut RefTable,
    layout: Option<&Layout>,
    options: &Options,
) -> Built {
    table.begin(doc);
    let root = match options.scope {
        Some(scope) => tenta_dom::extract::snapshot(doc, scope),
        None => doc.snapshot(),
    };
    let indexes = layout.map(|_| element_indexes(doc));
    // A layout of another length belongs to another tree.
    let layout = layout
        .zip(indexes.as_ref())
        .filter(|(layout, indexes)| layout.len() == indexes.len());
    let mut built = Built {
        lines: Vec::new(),
        interactive: Vec::new(),
        elements: 0,
        truncated: false,
        hidden: 0,
        layout_used: layout.is_some(),
    };
    let mut stack = vec![Frame {
        node: &root,
        depth: 0,
        suppress_text: false,
        parent_visible: true,
        parent_invisible: false,
    }];
    while let Some(Frame {
        node,
        depth,
        suppress_text,
        parent_visible,
        parent_invisible,
    }) = stack.pop()
    {
        let is_text = node.role == "text";
        let inline = if is_text {
            Inline::default()
        } else {
            inline_style(doc, node.node)
        };
        // `display: none` hides the subtree, from a layout or from the style
        // attribute alike.
        if inline.display_none {
            built.hidden += 1;
            continue;
        }
        let (visible, look) = if is_text {
            (parent_visible, None)
        } else {
            match layout {
                Some((layout, indexes)) => {
                    let look = indexes.get(&node.node).and_then(|index| layout[*index]);
                    (look.is_some(), look)
                }
                None => (true, None),
            }
        };
        let geometry = look.map(|look| look.rect);
        // `visibility: hidden` hides the element's own content, and a child
        // may turn it back on. A layout knows the computed value.
        let invisible = match look {
            Some(look) => look.visibility_hidden,
            None if inline.visibility_visible => false,
            None => inline.visibility_hidden || parent_invisible,
        };
        let mut role = node.role.clone();
        if role == "generic"
            && (doc.attribute_str(node.node, "onclick").is_some()
                || doc.attribute_str(node.node, "tabindex").is_some())
        {
            role = "clickable".to_owned();
        }
        let transparent = TRANSPARENT.contains(&role.as_str());
        let mut child_depth = depth;
        let mut child_visible = visible;
        if is_text {
            if !visible || parent_invisible {
                built.hidden += 1;
            } else if !suppress_text {
                emit(
                    &mut built,
                    options,
                    depth,
                    format!("- text: {}", quote(&node.name, MAX_TEXT_CHARS)),
                );
            }
        } else if !visible {
            built.hidden += 1;
            // A box-less element (`display: none`) hides its text; a
            // box-less `display: contents` wrapper still shows its elements.
            child_visible = false;
        } else if invisible {
            built.hidden += 1;
        } else if !transparent {
            if built.lines.len() >= options.max_nodes {
                built.truncated = true;
                break;
            }
            let line = describe(
                doc,
                table,
                node,
                &role,
                geometry,
                look.map_or(inline.opacity_zero, |look| look.transparent),
                options,
                &mut built,
            );
            emit(&mut built, options, depth, line);
            child_depth = depth + 1;
        }
        if built.truncated {
            break;
        }
        let suppress = NAMED_BY_CONTENT.contains(&role.as_str()) && !node.name.is_empty();
        for child in node.children.iter().rev() {
            stack.push(Frame {
                node: child,
                depth: child_depth,
                suppress_text: suppress,
                parent_visible: child_visible,
                parent_invisible: invisible,
            });
        }
    }
    built
}

fn emit(built: &mut Built, options: &Options, depth: usize, line: String) {
    if built.lines.len() >= options.max_nodes {
        built.truncated = true;
        return;
    }
    let indent = "  ".repeat(depth.min(MAX_INDENT));
    built.lines.push(format!("{indent}{line}"));
}

#[allow(clippy::too_many_arguments)] // one call site; a struct would only rename the arguments
fn describe(
    doc: &Document,
    table: &mut RefTable,
    node: &SnapshotNode,
    role: &str,
    geometry: Option<[f64; 4]>,
    opacity_zero: bool,
    options: &Options,
    built: &mut Built,
) -> String {
    built.elements += 1;
    let reference = table.issue(doc, node.node);
    let mut line = format!("- {role}");
    if !node.name.is_empty() {
        line.push(' ');
        line.push_str(&quote(&node.name, MAX_NAME_CHARS));
    }
    line.push_str(&format!(" [ref={reference}]"));
    let mut state = node.state.clone();
    state.remove("hidden");
    let url = state.remove("url");
    // A typed value is not an attribute; the Tier 1 extractor reads attributes.
    let mut value = node.value.clone();
    let tag = doc.tag(node.node);
    if tag == Some(atoms::INPUT) || tag == Some(atoms::TEXTAREA) {
        if let Some(dirty) = doc.dirty_value(node.node) {
            value = Some(dirty.to_owned());
        }
        if let Some(checked) = doc.dirty_checkedness(node.node) {
            if checked {
                state.insert("checked".to_owned(), Value::Bool(true));
            } else {
                state.remove("checked");
            }
        }
        let is_password = doc
            .attribute_str(node.node, "type")
            .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("password"));
        if is_password && value.as_ref().is_some_and(|value| !value.is_empty()) {
            value = Some("********".to_owned());
        }
    }
    // A select shows what is selected now, by the selectedness rules, and
    // names its options: that is what browser_select_option chooses among.
    let mut choices = None;
    if tag == Some(atoms::SELECT) {
        let selected: Vec<String> = tenta_dom::forms::selected_options(doc, node.node)
            .into_iter()
            .map(|option| tenta_dom::forms::option_text(doc, option))
            .collect();
        value = Some(selected.join(", "));
        let all = tenta_dom::forms::select_options(doc, node.node);
        let mut listed: Vec<String> = all
            .iter()
            .take(MAX_LISTED_OPTIONS)
            .map(|option| {
                quote(
                    &tenta_dom::forms::option_text(doc, *option),
                    MAX_OPTION_CHARS,
                )
            })
            .collect();
        if all.len() > MAX_LISTED_OPTIONS {
            listed.push(format!("... {} more", all.len() - MAX_LISTED_OPTIONS));
        }
        choices = Some(listed.join(", "));
    }
    for (key, state) in &state {
        match state {
            Value::Bool(true) => line.push_str(&format!(" [{key}]")),
            Value::Bool(false) => line.push_str(&format!(" [{key}=false]")),
            Value::String(text) => line.push_str(&format!(" [{key}={text}]")),
            other => line.push_str(&format!(" [{key}={other}]")),
        }
    }
    if opacity_zero {
        // Still in the tree and clickable, but a person would not see it.
        line.push_str(" [opacity-0]");
    }
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        line.push_str(&format!(" value={}", quote(&value, MAX_NAME_CHARS)));
    }
    if let Some(choices) = choices {
        line.push_str(&format!(" options=[{choices}]"));
    }
    if let Some(Value::String(url)) = url {
        line.push_str(&format!(" url={url}"));
    }
    if options.include_boxes
        && let Some([x, y, width, height]) = geometry
    {
        line.push_str(&format!(" [box={x:.0},{y:.0},{width:.0},{height:.0}]"));
    }
    if INTERACTIVE.contains(&role) {
        built.interactive.push(Entry {
            reference,
            role: role.to_owned(),
            name: node.name.clone(),
        });
    }
    line
}
