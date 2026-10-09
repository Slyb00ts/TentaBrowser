//! One agent's browser: a `BrowserContext` and the page its tools drive.
//!
//! The session opens its page on the first tool call and again after a
//! `browser_close` or a crash, so a tool never fails only because the
//! previous page is gone. Element refs live here: they name arena slots of
//! the current document and die with it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tenta_browser::{
    BrowserContext, ContextOptions, Engine, Page, PageOptions, Settle, SettleState, Viewport,
};
use tenta_core::{Error, NodeId, NodeRef, Tier};
use tenta_dom::Document;

use crate::error::{ErrorKind, ToolError};

/// What a server is configured with. Every session gets its own copy of the
/// per-page values.
#[derive(Clone, Debug)]
pub struct McpOptions {
    /// The options each session's `BrowserContext` is created with.
    pub context: ContextOptions,
    /// All sessions use one context (shared cookies and storage) instead of
    /// one each.
    pub share_context: bool,
    pub viewport: Viewport,
    /// The longest a navigation, including its settle, may take.
    pub navigation_timeout: Duration,
    /// Sessions that may exist at once (streamable HTTP).
    pub max_sessions: usize,
    /// Fall back to platform fonts for scripts the bundled fonts lack.
    pub system_fonts: bool,
    /// An HTTP session that has made no tool call is dropped after this long.
    pub unused_session_ttl: Duration,
}

impl Default for McpOptions {
    fn default() -> Self {
        Self {
            context: ContextOptions::default(),
            share_context: false,
            viewport: Viewport::default(),
            navigation_timeout: Duration::from_secs(30),
            max_sessions: 16,
            system_fonts: false,
            unused_session_ttl: Duration::from_secs(60),
        }
    }
}

/// Element refs handed out by `browser_snapshot`.
///
/// A ref is `e<n>`. It maps to a [`NodeRef`] (arena slot and generation) of
/// the document that was current when it was issued, so a ref stays the same
/// across snapshots of one document and fails clearly after the node is
/// removed or the page navigates. Numbers are never reused within a session:
/// a navigation retires every number issued so far, so a ref kept from the
/// previous page cannot silently name an element of the next one.
#[derive(Debug, Default)]
pub struct RefTable {
    document_id: u64,
    /// Numbers issued for earlier documents; this document's start after it.
    retired: u32,
    by_node: HashMap<NodeRef, u32>,
    nodes: Vec<NodeRef>,
}

/// Why a ref could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefError {
    Malformed,
    Unknown,
    Stale,
}

impl RefError {
    pub fn into_tool_error(self, reference: &str) -> ToolError {
        match self {
            RefError::Malformed => ToolError::invalid(format!(
                "`{reference}` is not an element ref; refs look like `e12` and come from browser_snapshot"
            )),
            RefError::Unknown => ToolError::element_not_found(format!(
                "no element has ref `{reference}`; take a new browser_snapshot"
            )),
            RefError::Stale => ToolError::element_not_found(format!(
                "ref `{reference}` is stale: its element left the page or the page navigated; take a new browser_snapshot"
            )),
        }
    }
}

/// A ref looked up in the table, to be checked against the document.
#[derive(Clone, Copy, Debug)]
pub struct IssuedRef {
    document_id: u64,
    node: NodeRef,
}

impl IssuedRef {
    /// The live node, or why it is not.
    pub fn resolve(self, doc: &Document) -> Result<NodeId, RefError> {
        if self.document_id != doc.document_id() {
            return Err(RefError::Stale);
        }
        let id = doc.resolve(self.node).map_err(|_| RefError::Stale)?;
        if doc.is_connected(id) {
            Ok(id)
        } else {
            Err(RefError::Stale)
        }
    }
}

impl RefTable {
    /// Starts a snapshot of `doc`: refs of another document are dropped.
    pub fn begin(&mut self, doc: &Document) {
        if self.document_id != doc.document_id() {
            *self = RefTable {
                document_id: doc.document_id(),
                retired: self.retired + u32::try_from(self.nodes.len()).unwrap_or(u32::MAX),
                ..RefTable::default()
            };
        }
    }

    /// Retires every ref issued so far, as a closed page does.
    pub fn retire_all(&mut self) {
        *self = RefTable {
            retired: self.retired + u32::try_from(self.nodes.len()).unwrap_or(u32::MAX),
            ..RefTable::default()
        };
    }

    /// The ref of `node`, issuing one on first sight.
    pub fn issue(&mut self, doc: &Document, node: NodeId) -> String {
        let node_ref = doc.node_ref(node);
        let retired = self.retired;
        let number = *self.by_node.entry(node_ref).or_insert_with(|| {
            self.nodes.push(node_ref);
            retired.saturating_add(u32::try_from(self.nodes.len()).unwrap_or(u32::MAX))
        });
        format!("e{number}")
    }

    pub fn lookup(&self, reference: &str) -> Result<IssuedRef, RefError> {
        let number = reference
            .strip_prefix('e')
            .filter(|digits| !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|digits| digits.parse::<usize>().ok())
            .filter(|number| *number > 0)
            .ok_or(RefError::Malformed)?;
        let retired = self.retired as usize;
        if number <= retired {
            return Err(RefError::Stale);
        }
        let node = *self
            .nodes
            .get(number - retired - 1)
            .ok_or(RefError::Unknown)?;
        Ok(IssuedRef {
            document_id: self.document_id,
            node,
        })
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

/// The text of the last snapshot, kept so a continuation token pages through
/// exactly what the agent already holds refs for.
pub struct StoredSnapshot {
    pub id: u64,
    /// Names this snapshot's content-boundary markers; the page cannot guess it.
    pub nonce: String,
    pub lines: Vec<String>,
}

/// An agent's browser.
pub struct BrowserSession {
    engine: Engine,
    options: Arc<McpOptions>,
    context: BrowserContext,
    owns_context: bool,
    page: Option<Page>,
    /// Whether pages are created with a script engine.
    javascript: bool,
    pub refs: Arc<Mutex<RefTable>>,
    pub snapshot: Option<StoredSnapshot>,
    next_snapshot: u64,
}

impl BrowserSession {
    pub fn new(
        engine: Engine,
        options: Arc<McpOptions>,
        shared: Option<BrowserContext>,
    ) -> Result<Self, ToolError> {
        let owns_context = shared.is_none();
        let context = match shared {
            Some(context) => context,
            None => engine.new_context(options.context.clone())?,
        };
        let javascript = engine.max_tier() >= Tier::Js;
        Ok(Self {
            engine,
            options,
            context,
            owns_context,
            page: None,
            javascript,
            refs: Arc::default(),
            snapshot: None,
            next_snapshot: 1,
        })
    }

    pub fn options(&self) -> &McpOptions {
        &self.options
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Whether the page runs scripts: click, type and evaluate need it.
    pub fn javascript(&self) -> bool {
        self.javascript
    }

    pub fn has_page(&self) -> bool {
        self.page.as_ref().is_some_and(usable)
    }

    pub fn locked_refs(&self) -> MutexGuard<'_, RefTable> {
        self.refs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn next_snapshot_id(&mut self) -> u64 {
        let id = self.next_snapshot;
        self.next_snapshot += 1;
        id
    }

    /// The page, opened if there is none or the last one ended.
    pub async fn page(&mut self) -> Result<Page, ToolError> {
        if let Some(page) = &self.page
            && usable(page)
        {
            return Ok(page.clone());
        }
        self.discard_page().await;
        let options = PageOptions {
            javascript: self.javascript,
            viewport: self.options.viewport,
            system_fonts: self.options.system_fonts,
            ..PageOptions::default()
        };
        let page = self.context.new_page(options).await?;
        if self.javascript {
            page.add_init_script(crate::interaction::SOURCE);
        }
        self.page = Some(page.clone());
        Ok(page)
    }

    /// Closes the page; the context lives on.
    pub async fn discard_page(&mut self) {
        self.snapshot = None;
        self.locked_refs().retire_all();
        if let Some(page) = self.page.take() {
            let _ = page.close().await;
        }
    }

    /// Ends the session: the page, and the context unless it is shared.
    pub async fn close(&mut self) {
        self.discard_page().await;
        if self.owns_context {
            self.context.close();
        }
    }
}

fn usable(page: &Page) -> bool {
    !page.is_closed() && page.crash_reason().is_none()
}

/// Runs `read` over the page's document. A page that keeps no engine holds
/// its tree itself; otherwise the read happens inside the engine's entry.
pub async fn read_doc<R: Send + 'static>(
    page: &Page,
    read: impl FnOnce(&Document) -> R + Send + 'static,
) -> Result<R, ToolError> {
    let mut read = Some(read);
    let resident = page
        .document()
        .and_then(|doc| read.take().map(|read| read(&doc)));
    if let Some(output) = resident {
        return Ok(output);
    }
    let read = read.expect("not consumed without a resident document");
    let output = page
        .with_engine(move |engine| {
            let shared = engine.document();
            let doc = shared.borrow();
            read(&doc)
        })
        .await?;
    Ok(output)
}

/// How a settle ended, for tool results.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settled {
    /// The deadline expired before the page was quiet.
    pub timed_out: bool,
    /// The load chain never ended by itself within `busy_cap`.
    pub busy: bool,
}

impl Settled {
    pub fn from_state(state: SettleState) -> Self {
        match state {
            SettleState::Settled { busy } => Settled {
                timed_out: false,
                busy,
            },
            SettleState::Timeout { .. } => Settled {
                timed_out: true,
                busy: true,
            },
            _ => Settled {
                timed_out: false,
                busy: false,
            },
        }
    }

    pub fn to_json(self) -> serde_json::Value {
        serde_json::json!({ "timed_out": self.timed_out, "busy": self.busy })
    }
}

/// Waits for the page to be quiet after an action: a navigation the action
/// started first, then the settle machine under the engine's MCP tunables
/// (`busy_cap` 500 ms, `foreground_request_max` 2 s).
pub async fn settle_after_action(
    page: &Page,
    session: &BrowserSession,
) -> Result<Settled, ToolError> {
    let navigation_deadline = session.options.navigation_timeout;
    if page.navigation_in_flight()
        && tokio::time::timeout(navigation_deadline, page.navigation_settled())
            .await
            .is_err()
    {
        return Ok(Settled {
            timed_out: true,
            busy: true,
        });
    }
    let config = session.engine.options().settle;
    let patience = config.busy_cap + config.foreground_request_max + Duration::from_secs(1);
    match page.wait_for_settle(Settle::NetworkIdle, patience).await {
        Ok(state) => Ok(Settled::from_state(state)),
        // A navigation the action started replaced the document under the
        // wait; the new document's own load is awaited above on the next call.
        Err(Error::InvalidArgument { .. } | Error::Other { .. }) => Ok(Settled {
            timed_out: false,
            busy: false,
        }),
        Err(error) if matches!(error, Error::Closed | Error::Crashed { .. }) => {
            Err(ToolError::new(ErrorKind::PageClosed, error.to_string()))
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A document whose `<html>` holds `count` divs, returned with them.
    fn document_with(count: usize) -> (Document, Vec<NodeId>) {
        use tenta_core::atom::atoms;
        let mut doc = Document::new();
        let html = doc.create_html_element(atoms::HTML);
        let root = doc.document_node();
        doc.mutate(tenta_dom::Mutation::Append {
            parent: root,
            child: html,
        })
        .expect("a document element");
        let divs = (0..count)
            .map(|_| {
                let div = doc.create_html_element(atoms::DIV);
                doc.mutate(tenta_dom::Mutation::Append {
                    parent: html,
                    child: div,
                })
                .expect("attached");
                div
            })
            .collect();
        (doc, divs)
    }

    #[test]
    fn refs_are_stable_within_a_document_and_never_reused_across_documents() {
        let (first, divs) = document_with(2);
        let (a, b) = (divs[0], divs[1]);
        let mut table = RefTable::default();
        table.begin(&first);
        assert_eq!(table.issue(&first, a), "e1");
        assert_eq!(table.issue(&first, b), "e2");
        assert_eq!(table.issue(&first, a), "e1");
        assert_eq!(table.lookup("e2").unwrap().resolve(&first), Ok(b));

        let (second, divs) = document_with(1);
        let c = divs[0];
        // Before the new document is snapshotted the old ref cannot resolve in it.
        assert_eq!(
            table.lookup("e1").unwrap().resolve(&second),
            Err(RefError::Stale)
        );
        table.begin(&second);
        assert_eq!(table.issue(&second, c), "e3", "numbering continues");
        assert_eq!(table.lookup("e1").unwrap_err(), RefError::Stale);
        assert_eq!(table.lookup("e2").unwrap_err(), RefError::Stale);
        assert_eq!(table.lookup("e3").unwrap().resolve(&second), Ok(c));
        assert_eq!(table.lookup("e4").unwrap_err(), RefError::Unknown);
        assert_eq!(table.lookup("x3").unwrap_err(), RefError::Malformed);
        assert_eq!(table.lookup("e0").unwrap_err(), RefError::Malformed);

        table.retire_all();
        assert_eq!(table.lookup("e3").unwrap_err(), RefError::Stale);
    }
}
