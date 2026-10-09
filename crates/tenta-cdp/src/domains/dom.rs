//! The `DOM` domain, as far as automation clients use it: node lookup and
//! description, selector queries, markup and geometry of a node.
//!
//! A node id is the backend id (the arena slot plus one, see `nodes`), so a
//! client may mix `nodeId` and `backendNodeId` freely. The domain pushes no
//! `setChildNodes` or mutation events: tree views are not an automation
//! need, and every answer is read from the live document when asked.
//!
//! Boxes come from the render session's layout of the document as it is now.
//! There is no scrolling, so `scrollIntoViewIfNeeded` only checks the node
//! has a box. `DOM.setFileInputFiles` is refused by name until uploads go
//! through the file-system policy.

use serde::Deserialize;
use serde_json::{Value, json};
use tenta_core::NodeId;
use tenta_dom::NodeKind;

use super::geometry::{self, Boxes};
use super::nodes::{
    self, backend_id, no_node, node_json, node_of, node_of_object, read, require_attached,
};
use crate::backend::Target;
use crate::domain::{CommandContext, CommandFuture, DomainHandler, DomainRegistry, parse_params};
use crate::error::CdpError;

pub(super) fn register(registry: &mut DomainRegistry) {
    registry.register(Dom);
}

struct Dom;

/// Every way a command names a node.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct NodeSelector {
    node_id: Option<i64>,
    backend_node_id: Option<i64>,
    object_id: Option<String>,
}

#[derive(Deserialize)]
struct GetDocument {
    depth: Option<i64>,
}

#[derive(Deserialize)]
struct Describe {
    #[serde(flatten)]
    node: NodeSelector,
    depth: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Resolve {
    #[serde(flatten)]
    node: NodeSelector,
    object_group: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestNode {
    object_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Query {
    node_id: i64,
    selector: String,
}

#[derive(Deserialize)]
struct Location {
    x: f64,
    y: f64,
}

/// A node, however the command named it, that is in the document.
async fn node_from(
    cx: &CommandContext,
    target: &Target,
    selector: &NodeSelector,
) -> Result<NodeId, CdpError> {
    let node = if let Some(object_id) = &selector.object_id {
        node_of_object(cx, target, object_id).await?
    } else if let Some(id) = selector.backend_node_id.or(selector.node_id) {
        node_of(id)?
    } else {
        return Err(CdpError::invalid_params(
            "Either nodeId, backendNodeId or objectId must be specified",
        ));
    };
    require_attached(target.page(), node).await?;
    Ok(node)
}

fn quad(points: &[f64; 8]) -> Value {
    Value::Array(points.iter().map(|point| json!(point)).collect())
}

async fn first_box(target: &Target, node: NodeId, failure: &str) -> Result<Boxes, CdpError> {
    geometry::node_boxes(target, node)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| CdpError::server(failure))
}

async fn query(target: &Target, params: Query, all: bool) -> Result<Value, CdpError> {
    let Query { node_id, selector } = params;
    let root = node_of(node_id)?;
    let found = read(target.page(), move |doc| {
        if !doc.is_live(root) {
            return Err("Could not find node with given id");
        }
        if !matches!(
            doc.kind(root),
            NodeKind::Element | NodeKind::Document | NodeKind::Fragment | NodeKind::ShadowRoot
        ) {
            return Err("Node is not an Element or Document");
        }
        if all {
            doc.query_selector_all(root, &selector)
        } else {
            doc.query_selector(root, &selector)
                .map(|found| found.into_iter().collect())
        }
        .map_err(|_| "DOM Error while querying")
    })
    .await?
    .map_err(CdpError::server)?;
    Ok(if all {
        json!({ "nodeIds": found.into_iter().map(backend_id).collect::<Vec<_>>() })
    } else {
        found
            .first()
            .map_or_else(no_node, |node| json!({ "nodeId": backend_id(*node) }))
    })
}

impl DomainHandler for Dom {
    fn domain(&self) -> &'static str {
        "DOM"
    }

    fn methods(&self) -> &'static [&'static str] {
        &[
            "enable",
            "disable",
            "getDocument",
            "describeNode",
            "resolveNode",
            "requestNode",
            "querySelector",
            "querySelectorAll",
            "getOuterHTML",
            "getBoxModel",
            "getContentQuads",
            "scrollIntoViewIfNeeded",
            "getNodeForLocation",
            "focus",
            "setFileInputFiles",
        ]
    }

    fn handle<'a>(
        &'a self,
        cx: CommandContext,
        method: &'a str,
        params: Value,
    ) -> CommandFuture<'a> {
        Box::pin(async move {
            match method {
                "enable" | "disable" => Ok(json!({})),
                "getDocument" => {
                    let GetDocument { depth } = parse_params(params)?;
                    let target = cx.target()?;
                    nodes::after_navigation(&cx, target.page()).await;
                    let frame = target.id().to_owned();
                    let root = read(target.page(), move |doc| {
                        node_json(doc, doc.document_node(), depth.unwrap_or(1), &frame)
                    })
                    .await?;
                    Ok(json!({ "root": root }))
                }
                "describeNode" => {
                    let Describe { node, depth } = parse_params(params)?;
                    let target = cx.target()?;
                    let node = node_from(&cx, target, &node).await?;
                    let frame = target.id().to_owned();
                    let described = read(target.page(), move |doc| {
                        node_json(doc, node, depth.unwrap_or(0), &frame)
                    })
                    .await?;
                    Ok(json!({ "node": described }))
                }
                "resolveNode" => {
                    let Resolve { node, object_group } = parse_params(params)?;
                    let target = cx.target()?;
                    let node = node_from(&cx, target, &node).await?;
                    let object = nodes::resolve(&cx, target, node, object_group).await?;
                    Ok(json!({ "object": object }))
                }
                "requestNode" => {
                    let RequestNode { object_id } = parse_params(params)?;
                    let target = cx.target()?;
                    let node = node_of_object(&cx, target, &object_id).await?;
                    Ok(json!({ "nodeId": backend_id(node) }))
                }
                "querySelector" => query(cx.target()?, parse_params(params)?, false).await,
                "querySelectorAll" => query(cx.target()?, parse_params(params)?, true).await,
                "getOuterHTML" => {
                    let selector: NodeSelector = parse_params(params)?;
                    let target = cx.target()?;
                    let node = node_from(&cx, target, &selector).await?;
                    let html = read(target.page(), move |doc| match doc.kind(node) {
                        NodeKind::Document => doc.inner_html(node),
                        _ => doc.outer_html(node),
                    })
                    .await?;
                    Ok(json!({ "outerHTML": html }))
                }
                "getBoxModel" => {
                    let selector: NodeSelector = parse_params(params)?;
                    let target = cx.target()?;
                    let node = node_from(&cx, target, &selector).await?;
                    let boxes = first_box(target, node, "Could not compute box model.").await?;
                    Ok(json!({ "model": {
                        "content": quad(&boxes.content),
                        "padding": quad(&boxes.padding),
                        "border": quad(&boxes.border),
                        "margin": quad(&boxes.margin),
                        "width": boxes.width.round() as i64,
                        "height": boxes.height.round() as i64,
                    }}))
                }
                "getContentQuads" => {
                    let selector: NodeSelector = parse_params(params)?;
                    let target = cx.target()?;
                    let node = node_from(&cx, target, &selector).await?;
                    let boxes = geometry::node_boxes(target, node).await?;
                    if boxes.is_empty() {
                        return Err(CdpError::server("Could not compute content quads."));
                    }
                    let quads: Vec<Value> = boxes.iter().map(|b| quad(&b.content)).collect();
                    Ok(json!({ "quads": quads }))
                }
                "scrollIntoViewIfNeeded" => {
                    let selector: NodeSelector = parse_params(params)?;
                    let target = cx.target()?;
                    let node = node_from(&cx, target, &selector).await?;
                    first_box(target, node, "Node does not have a layout object").await?;
                    Ok(json!({}))
                }
                "getNodeForLocation" => {
                    let Location { x, y } = parse_params(params)?;
                    let target = cx.target()?;
                    let node = geometry::element_at(target, x, y)
                        .await?
                        .ok_or_else(|| CdpError::server("No node found at given location"))?;
                    Ok(json!({
                        "backendNodeId": backend_id(node),
                        "frameId": target.id(),
                        "nodeId": backend_id(node),
                    }))
                }
                "focus" => {
                    let selector: NodeSelector = parse_params(params)?;
                    let target = cx.target()?;
                    let node = node_from(&cx, target, &selector).await?;
                    nodes::focus(&cx, target, node).await?;
                    Ok(json!({}))
                }
                "setFileInputFiles" => Err(CdpError::server(
                    "DOM.setFileInputFiles is not supported: uploads from the host file system are not built",
                )),
                other => Err(CdpError::method_not_found(&format!("DOM.{other}"))),
            }
        })
    }
}
