//! How the `DOM` and `Input` domains name nodes, and the calls into the page
//! they share.
//!
//! A protocol node id is the arena slot of the node plus one (0 means "no
//! node" on the wire). Remote objects are bridged by a child-index path from
//! the document, which the page-side script (`interaction.js`) computes and
//! resolves, so neither side needs the other's handles. Both ids stay valid
//! as long as the slot does, which is the lifetime of the document.

use serde_json::{Value, json};
use tenta_browser::{Completion, EvalOptions, Page, RemoteValue};
use tenta_core::{Error, NodeId};
use tenta_dom::{Document, NodeKind};

use super::runtime::remote_object;
use crate::backend::Target;
use crate::domain::CommandContext;
use crate::error::CdpError;

const HTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";

pub(super) fn backend_id(node: NodeId) -> i64 {
    i64::from(node.raw()) + 1
}

/// The node a protocol id names; ids below 1 and above the arena's range are
/// the client's mistake, not a missing node.
pub(super) fn node_of(id: i64) -> Result<NodeId, CdpError> {
    u32::try_from(id - 1)
        .ok()
        .filter(|raw| *raw != u32::MAX)
        .map(NodeId)
        .ok_or_else(|| CdpError::server("Could not find node with given id"))
}

/// A page error in the words Chromium's clients match on.
pub(super) fn page_error(error: Error) -> CdpError {
    match error {
        Error::Closed => CdpError::server("Target closed"),
        Error::InvalidArgument { reason } if reason.contains("could not find an object") => {
            CdpError::server("Could not find object with given id")
        }
        other => CdpError::from(other),
    }
}

/// Waits for a navigation in flight to install its document, releasing the
/// session's order first so later commands are not stuck behind a slow page.
pub(super) async fn after_navigation(cx: &CommandContext, page: &Page) {
    if page.navigation_in_flight() {
        cx.release_order();
        page.navigation_settled().await;
    }
}

/// Runs `read` over the page's document. A page that keeps no engine holds
/// its tree itself; otherwise the read happens inside the engine's entry.
pub(super) async fn read<R: Send + 'static>(
    page: &Page,
    read: impl FnOnce(&Document) -> R + Send + 'static,
) -> Result<R, CdpError> {
    let mut read = Some(read);
    let resident = page
        .document()
        .and_then(|doc| read.take().map(|read| read(&doc)));
    if let Some(output) = resident {
        return Ok(output);
    }
    let read = read.expect("not consumed without a resident document");
    page.with_engine(move |engine| {
        let shared = engine.document();
        let doc = shared.borrow();
        read(&doc)
    })
    .await
    .map_err(page_error)
}

// ---- paths --------------------------------------------------------------

/// Child indexes from the document to `node`; `None` for a node outside the
/// document's tree (detached, or inside a shadow root).
pub(super) fn path_of(doc: &Document, node: NodeId) -> Option<Vec<u32>> {
    if !doc.is_live(node) {
        return None;
    }
    let root = doc.document_node();
    let mut path = Vec::new();
    let mut cursor = node;
    while cursor != root {
        let parent = doc.parent(cursor);
        if parent == NodeId::NONE {
            return None;
        }
        let index = doc.children(parent).position(|child| child == cursor)?;
        path.push(u32::try_from(index).ok()?);
        cursor = parent;
    }
    path.reverse();
    Some(path)
}

pub(super) fn node_at(doc: &Document, path: &[u32]) -> Option<NodeId> {
    path.iter().try_fold(doc.document_node(), |cursor, index| {
        doc.children(cursor).nth(*index as usize)
    })
}

/// A node that is in the document.
pub(super) async fn require_attached(page: &Page, node: NodeId) -> Result<(), CdpError> {
    let attached = read(page, move |doc| path_of(doc, node).is_some()).await?;
    if attached {
        Ok(())
    } else {
        Err(CdpError::server("Could not find node with given id"))
    }
}

// ---- calls into the page ----------------------------------------------

fn by_value() -> EvalOptions {
    EvalOptions {
        return_by_value: true,
        ..EvalOptions::default()
    }
}

/// Evaluates `expression` in the main world and returns its value, or the
/// exception's text as the command's error.
pub(super) async fn run(
    cx: &CommandContext,
    target: &Target,
    expression: &str,
) -> Result<RemoteValue, CdpError> {
    after_navigation(cx, target.page()).await;
    let evaluation = target
        .page()
        .evaluate_with(expression, by_value())
        .await
        .map_err(page_error)?;
    completed(evaluation.into_completion())
}

fn completed(completion: Completion) -> Result<RemoteValue, CdpError> {
    match completion {
        Completion::Value(value) => Ok(value),
        Completion::Threw(details) => Err(CdpError::server(format!(
            "The page could not carry out the command: {}",
            details.text
        ))),
    }
}

/// The node a remote object is, by way of its path in the document.
pub(super) async fn node_of_object(
    cx: &CommandContext,
    target: &Target,
    object_id: &str,
) -> Result<NodeId, CdpError> {
    after_navigation(cx, target.page()).await;
    let evaluation = target
        .page()
        .call_function_on_id(
            Some(object_id),
            "function () { return globalThis.__tentaCdp.pathOf(this); }",
            Vec::new(),
            by_value(),
        )
        .await
        .map_err(page_error)?;
    let path = match completed(evaluation.into_completion())? {
        RemoteValue::Array(items) => items
            .iter()
            .map(|item| match item {
                RemoteValue::Number(n) if *n >= 0.0 => Some(*n as u32),
                _ => None,
            })
            .collect::<Option<Vec<u32>>>(),
        RemoteValue::Bool(false) => {
            return Err(CdpError::server("Object id doesn't reference a Node"));
        }
        _ => None,
    }
    .ok_or_else(|| CdpError::server("Node is detached from document"))?;
    read(target.page(), move |doc| node_at(doc, &path))
        .await?
        .ok_or_else(|| CdpError::server("Node is detached from document"))
}

/// The remote object of a node, in the main world.
pub(super) async fn resolve(
    cx: &CommandContext,
    target: &Target,
    node: NodeId,
    object_group: Option<String>,
) -> Result<Value, CdpError> {
    let path = read(target.page(), move |doc| path_of(doc, node))
        .await?
        .ok_or_else(|| CdpError::server("No node with given id found"))?;
    after_navigation(cx, target.page()).await;
    let expression = format!(
        "globalThis.__tentaCdp.nodeAt({})",
        serde_json::to_string(&path).expect("a list of numbers")
    );
    let options = EvalOptions {
        object_group,
        ..EvalOptions::default()
    };
    let evaluation = target
        .page()
        .evaluate_with(&expression, options)
        .await
        .map_err(page_error)?;
    match completed(evaluation.into_completion())? {
        value @ RemoteValue::Handle(_) => Ok(remote_object(&value)),
        _ => Err(CdpError::server("No node with given id found")),
    }
}

// ---- the protocol's view of a node ------------------------------------

fn node_type(kind: NodeKind) -> u32 {
    match kind {
        NodeKind::Element => 1,
        NodeKind::Text => 3,
        NodeKind::CData => 4,
        NodeKind::ProcessingInstruction => 7,
        NodeKind::Comment => 8,
        NodeKind::Document => 9,
        NodeKind::DocType => 10,
        NodeKind::Fragment | NodeKind::ShadowRoot => 11,
    }
}

fn node_name(doc: &Document, node: NodeId) -> String {
    match doc.kind(node) {
        NodeKind::Element => {
            let local = doc.tag(node).map_or("", |tag| tag.as_str());
            if doc
                .namespace(node)
                .is_some_and(|ns| ns.as_str() == HTML_NAMESPACE)
            {
                local.to_ascii_uppercase()
            } else {
                match doc.element_prefix(node) {
                    Some(prefix) => format!("{}:{local}", prefix.as_str()),
                    None => local.to_owned(),
                }
            }
        }
        NodeKind::Text => "#text".to_owned(),
        NodeKind::CData => "#cdata-section".to_owned(),
        NodeKind::Comment => "#comment".to_owned(),
        NodeKind::Document => "#document".to_owned(),
        NodeKind::Fragment | NodeKind::ShadowRoot => "#document-fragment".to_owned(),
        NodeKind::DocType => doc
            .doctype(node)
            .map(|d| d.name.clone())
            .unwrap_or_default(),
        NodeKind::ProcessingInstruction => doc
            .processing_instruction(node)
            .map(|(target, _)| target.to_owned())
            .unwrap_or_default(),
    }
}

fn node_value(doc: &Document, node: NodeId) -> String {
    match doc.kind(node) {
        NodeKind::Text | NodeKind::CData => doc.text_data(node).unwrap_or_default().to_owned(),
        NodeKind::Comment => doc.comment_data(node).unwrap_or_default().to_owned(),
        NodeKind::ProcessingInstruction => doc
            .processing_instruction(node)
            .map(|(_, data)| data.to_owned())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// The children the protocol shows: Chromium leaves out whitespace-only text.
fn shown_children(doc: &Document, node: NodeId) -> Vec<NodeId> {
    doc.children(node)
        .filter(|child| {
            doc.kind(*child) != NodeKind::Text
                || doc
                    .text_data(*child)
                    .is_some_and(|text| !text.bytes().all(|b| b.is_ascii_whitespace()))
        })
        .collect()
}

fn attributes(doc: &Document, node: NodeId) -> Vec<String> {
    doc.attributes(node)
        .flat_map(|(name, value)| {
            let local = name.local.as_str();
            let name = match name.prefix {
                Some(prefix) => format!("{}:{local}", prefix.as_str()),
                None => local.to_owned(),
            };
            [name, value.to_owned()]
        })
        .collect()
}

/// A `DOM.Node`. `depth` counts the levels of children to include (-1 for
/// all, 0 for none).
pub(super) fn node_json(doc: &Document, node: NodeId, depth: i64, frame_id: &str) -> Value {
    let kind = doc.kind(node);
    let mut out = serde_json::Map::new();
    out.insert("nodeId".into(), backend_id(node).into());
    out.insert("backendNodeId".into(), backend_id(node).into());
    let parent = doc.parent(node);
    if parent != NodeId::NONE {
        out.insert("parentId".into(), backend_id(parent).into());
    }
    out.insert("nodeType".into(), node_type(kind).into());
    out.insert("nodeName".into(), node_name(doc, node).into());
    out.insert(
        "localName".into(),
        doc.tag(node).map_or("", |tag| tag.as_str()).into(),
    );
    out.insert("nodeValue".into(), node_value(doc, node).into());
    let children = shown_children(doc, node);
    if kind.is_container() || !children.is_empty() {
        out.insert("childNodeCount".into(), children.len().into());
        if depth != 0 {
            let next = if depth > 0 { depth - 1 } else { depth };
            let rendered: Vec<Value> = children
                .iter()
                .map(|child| node_json(doc, *child, next, frame_id))
                .collect();
            out.insert("children".into(), rendered.into());
        }
    }
    match kind {
        NodeKind::Element => {
            out.insert("attributes".into(), attributes(doc, node).into());
        }
        NodeKind::Document => {
            let url = doc.url().map_or("about:blank", |url| url.as_str());
            out.insert("documentURL".into(), url.into());
            out.insert(
                "baseURL".into(),
                doc.base_url().map_or(url, |url| url.as_str()).into(),
            );
            out.insert("xmlVersion".into(), "".into());
            out.insert("frameId".into(), frame_id.into());
            out.insert(
                "compatibilityMode".into(),
                format!("{:?}Mode", doc.quirks_mode()).into(),
            );
        }
        NodeKind::DocType => {
            if let Some(doctype) = doc.doctype(node) {
                out.insert("publicId".into(), doctype.public_id.clone().into());
                out.insert("systemId".into(), doctype.system_id.clone().into());
            }
        }
        _ => {}
    }
    Value::Object(out)
}

/// The result of a `DOM.querySelector` that found nothing.
pub(super) fn no_node() -> Value {
    json!({ "nodeId": 0 })
}

/// Focuses the element the node is, with the focus and blur events a user
/// would cause.
pub(super) async fn focus(
    cx: &CommandContext,
    target: &Target,
    node: NodeId,
) -> Result<(), CdpError> {
    let path = read(target.page(), move |doc| path_of(doc, node))
        .await?
        .ok_or_else(|| CdpError::server("Node is detached from document"))?;
    let path = serde_json::to_string(&path).expect("a list of numbers");
    run(
        cx,
        target,
        &format!("globalThis.__tentaCdp.focus(globalThis.__tentaCdp.nodeAt({path}))"),
    )
    .await?;
    Ok(())
}
