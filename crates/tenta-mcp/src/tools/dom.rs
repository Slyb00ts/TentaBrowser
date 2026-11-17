//! What the acting tools share: resolving a ref to a node, naming it in
//! messages, running the page-side script and following the navigation an
//! action starts.

use serde_json::{Value, json};
use tenta_browser::{Completion, EvalOptions, Goto, Navigation, Page, RemoteValue};
use tenta_core::NodeId;
use tenta_dom::Document;

use crate::error::{ErrorKind, ToolError};
use crate::session::{BrowserSession, read_doc};

/// An element an action targets.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub node: NodeId,
    /// Child indexes from the document, the page-side script's way to name it.
    pub path: Vec<u32>,
    pub description: String,
}

/// The step of a path that enters an element's shadow root, where every
/// other step is a child index. The page-side `nodeAt` knows it by the same
/// number.
pub const SHADOW_ROOT_STEP: u32 = u32::MAX;

/// The way from the document to `node` for the page-side script: child
/// indexes, with [`SHADOW_ROOT_STEP`] where the way enters a shadow root.
/// `None` for a node that is not connected, and for one in a closed shadow
/// tree, which script cannot reach from its host.
pub fn path_of(doc: &Document, node: NodeId) -> Option<Vec<u32>> {
    if !doc.is_live(node) {
        return None;
    }
    let root = doc.document_node();
    let mut path = Vec::new();
    let mut cursor = node;
    while cursor != root {
        let parent = doc.parent(cursor);
        if parent == NodeId::NONE {
            let host = doc.shadow_host(cursor)?;
            if doc.shadow_root_mode(cursor) != Some(tenta_dom::ShadowRootMode::Open) {
                return None;
            }
            path.push(SHADOW_ROOT_STEP);
            cursor = host;
            continue;
        }
        let index = doc.children(parent).position(|child| child == cursor)?;
        path.push(u32::try_from(index).ok()?);
        cursor = parent;
    }
    path.reverse();
    Some(path)
}

/// `<button#go.primary>`: enough for an agent to recognise the element.
pub fn describe(doc: &Document, node: NodeId) -> String {
    let tag = doc
        .tag(node)
        .map(|tag| tag.to_string())
        .unwrap_or_else(|| "node".to_owned());
    let mut out = format!("<{tag}");
    if let Some(id) = doc.element_id(node).filter(|id| !id.is_empty()) {
        out.push('#');
        out.push_str(id);
    }
    for class in doc.classes(node).take(2) {
        out.push('.');
        out.push_str(class);
    }
    out.push('>');
    out
}

pub fn resolve_node(doc: &Document, node: NodeId) -> Option<Resolved> {
    Some(Resolved {
        node,
        path: path_of(doc, node)?,
        description: describe(doc, node),
    })
}

/// The element a ref names. A ref from before a navigation, or one whose
/// element was removed, is `element_not_found` with advice to re-snapshot.
pub async fn resolve_ref(
    session: &BrowserSession,
    page: &Page,
    reference: &str,
) -> Result<Resolved, ToolError> {
    let issued = session
        .locked_refs()
        .lookup(reference)
        .map_err(|error| error.into_tool_error(reference))?;
    let outcome = read_doc(page, move |doc| {
        let node = issued.resolve(doc)?;
        resolve_node(doc, node).ok_or(crate::session::RefError::Stale)
    })
    .await?;
    outcome.map_err(|error| error.into_tool_error(reference))
}

/// What the page-side script reports back.
#[derive(Debug, Default)]
pub struct Report {
    pub navigate: Option<String>,
    pub unsupported: Option<String>,
    /// The navigation whose document the script ran in, to tell afterwards
    /// whether the page has navigated since ([`shown_document`]).
    pub document: u64,
}

/// Identifies the document the page shows: the id of the navigation that
/// loaded it, 0 before any.
pub fn shown_document(page: &Page) -> u64 {
    page.navigation()
        .map_or(0, |navigation| navigation.navigation_id)
}

/// The navigation the page started by itself since it showed `document`, once
/// it has finished: a form submitted by POST, a script that assigned
/// `location`, a link the page's own script followed. `None` when the page
/// still shows that document.
pub async fn page_navigation(
    session: &BrowserSession,
    page: &Page,
    document: u64,
) -> Option<Navigation> {
    if page.navigation_in_flight() {
        let _ = tokio::time::timeout(
            session.options().navigation_timeout,
            page.navigation_settled(),
        )
        .await;
    }
    page.navigation()
        .filter(|navigation| navigation.navigation_id != document)
}

/// The URL of the document the page shows, which follows `history.pushState`
/// and fragment navigations as the URL of the navigation that loaded it does
/// not.
pub async fn current_url(page: &Page) -> String {
    let from_document = read_doc(page, |doc| doc.url().map(|url| url.to_string()))
        .await
        .ok()
        .flatten();
    from_document
        .or_else(|| {
            page.navigation()
                .map(|navigation| navigation.final_url.to_string())
        })
        .unwrap_or_else(|| "about:blank".to_owned())
}

fn parse_report(value: RemoteValue) -> Report {
    let mut report = Report::default();
    if let RemoteValue::Object(members) = value {
        for (name, value) in members {
            match (name.as_str(), value) {
                ("navigate", RemoteValue::String(url)) => report.navigate = Some(url),
                ("unsupported", RemoteValue::String(what)) => report.unsupported = Some(what),
                _ => {}
            }
        }
    }
    report
}

/// Runs `expression` (a call of the page-side script) in the main world and
/// reads its report.
pub async fn run_script(
    session: &BrowserSession,
    page: &Page,
    expression: &str,
) -> Result<Report, ToolError> {
    require_javascript(session)?;
    let options = EvalOptions {
        return_by_value: true,
        ..EvalOptions::default()
    };
    let document = shown_document(page);
    let evaluation = page.evaluate_with(expression, options).await?;
    match evaluation.into_completion() {
        Completion::Value(value) => Ok(Report {
            document,
            ..parse_report(value)
        }),
        Completion::Threw(details) => Err(ToolError::new(
            ErrorKind::ScriptError,
            format!(
                "the page could not carry out the action: {}",
                details.message
            ),
        )),
    }
}

pub fn require_javascript(session: &BrowserSession) -> Result<(), ToolError> {
    if session.javascript() {
        Ok(())
    } else {
        Err(ToolError::tier_denied(
            "this action runs in the page's script engine, and this build or engine ceiling has none (build tentabrowser with --features v8)",
        ))
    }
}

/// Wraps `body` in a call of the page-side script. `t` is the element at
/// `path` (the document for an empty path), `A` is `__tentaCdp`, and `m` folds
/// the report of one `A.mouse` or `A.key` call into the script's result.
pub fn script(path: &[u32], body: &str) -> String {
    let path = serde_json::to_string(path).expect("a list of numbers");
    format!(
        "(function(){{var A=globalThis.__tentaCdp;\
         if(!A)throw new Error('the interaction script is not installed in this page');\
         var t=A.nodeAt({path});if(!t)throw new Error('the element left the page');\
         var nav=null,uns=null;\
         function m(r){{if(r){{if(r.navigate)nav=r.navigate;if(r.unsupported)uns=r.unsupported;}}}}\
         {body}\
         return {{navigate:nav,unsupported:uns}};}})()"
    )
}

/// Starts the navigation an action caused and waits for it, as a click on a
/// link is followed by a person. Only http(s) targets are followed.
pub async fn follow(
    session: &BrowserSession,
    page: &Page,
    report: &Report,
) -> Result<Option<Navigation>, ToolError> {
    let Some(url) = &report.navigate else {
        return Ok(None);
    };
    if !url::Url::parse(url).is_ok_and(|url| matches!(url.scheme(), "http" | "https")) {
        return Ok(None);
    }
    let goto = Goto::new().timeout(session.options().navigation_timeout);
    let pending = page.begin_goto(url, goto)?;
    Ok(Some(pending.finish().await?))
}

pub fn navigation_json(navigation: &Navigation) -> Value {
    json!({
        "url": navigation.final_url.as_str(),
        "status": navigation.status,
        "error": navigation.error,
    })
}

/// A report that the page side refused part of the action.
pub fn check_supported(report: &Report) -> Result<(), ToolError> {
    match &report.unsupported {
        Some(what) => Err(ToolError::new(
            ErrorKind::Unsupported,
            format!("editing a {what} element is not supported"),
        )),
        None => Ok(()),
    }
}

/// Whether the element takes typed text in this engine.
pub fn is_text_entry(doc: &Document, node: NodeId) -> bool {
    use tenta_core::atom::atoms;
    let Some(tag) = doc.tag(node) else {
        return false;
    };
    if tag == atoms::TEXTAREA {
        return true;
    }
    if tag != atoms::INPUT {
        return false;
    }
    let kind = doc
        .attribute_str(node, "type")
        .map(|kind| kind.trim().to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        kind.as_str(),
        "" | "text" | "search" | "tel" | "url" | "email" | "password" | "number"
    )
}

/// Where an element is on the page.
#[cfg_attr(not(feature = "render"), allow(dead_code))]
pub enum Placement {
    /// The centre of its first border box, in CSS pixels.
    Box { x: f64, y: f64 },
    /// It has no box, or an empty one.
    Hidden,
    /// This build or engine cannot lay the page out.
    Unavailable,
}

#[cfg(feature = "render")]
pub async fn place(page: &Page, node: NodeId) -> Result<Placement, ToolError> {
    use tenta_core::Error;
    match page.node_boxes(node).await {
        Ok(boxes) => Ok(boxes
            .first()
            .map(|quads| quads.border.bounds())
            .filter(|(_, _, width, height)| *width > 0.0 && *height > 0.0)
            .map_or(Placement::Hidden, |(x, y, width, height)| Placement::Box {
                x: x + width / 2.0,
                y: y + height / 2.0,
            })),
        Err(Error::TierDenied { .. } | Error::Unsupported { .. }) => Ok(Placement::Unavailable),
        Err(error) => Err(error.into()),
    }
}

#[cfg(not(feature = "render"))]
pub async fn place(_page: &Page, _node: NodeId) -> Result<Placement, ToolError> {
    Ok(Placement::Unavailable)
}

/// The element at a point, from the layout.
#[cfg(feature = "render")]
pub async fn hit(page: &Page, x: f64, y: f64) -> Result<Option<NodeId>, ToolError> {
    use tenta_core::Error;
    match page.hit_test(x, y).await {
        Ok(node) => Ok(node),
        Err(Error::TierDenied { .. } | Error::Unsupported { .. }) => Err(no_layout()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(not(feature = "render"))]
pub async fn hit(_page: &Page, _x: f64, _y: f64) -> Result<Option<NodeId>, ToolError> {
    Err(no_layout())
}

fn no_layout() -> ToolError {
    ToolError::tier_denied(
        "clicking at coordinates needs the render tier, which this build or engine ceiling lacks (build tentabrowser with --features render); click by ref instead",
    )
}
