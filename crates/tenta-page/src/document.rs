//! The value a Tier 1 fetch returns: the parsed DOM plus everything the
//! navigation learned about it (`docs/spec/11-library-api.md` §1).
//!
//! It is an owned value, not a handle: the page that produced it is closed,
//! so extraction runs on the caller's thread with no round trip.

use serde_json::{Value, json};
use tenta_core::NodeId;
use tenta_dom::extract::forms::Form;
use tenta_dom::extract::links::Link;
use tenta_dom::extract::main_content::MainContent;
use tenta_dom::extract::metadata::Metadata;
use tenta_dom::extract::snapshot::SnapshotNode;
use tenta_dom::{Document as DomDocument, MemoryReport};
use tenta_net::{RedirectHop, Timing};
use url::Url;

use crate::fetch_policy::RecordedHint;
use crate::page::Navigation;

/// A fetched, parsed document.
#[derive(Debug)]
pub struct Document {
    dom: DomDocument,
    navigation: Navigation,
    hints: Vec<RecordedHint>,
    /// Generation counter of the navigation that produced this document.
    /// Incremented on each new goto or close to cancel in-flight navigations.
    navigation_id: u64,
}

impl Document {
    pub(crate) fn new(
        dom: DomDocument,
        navigation: Navigation,
        hints: Vec<RecordedHint>,
        navigation_id: u64,
    ) -> Self {
        Self {
            dom,
            navigation,
            hints,
            navigation_id,
        }
    }

    /// The URL the navigation was asked for.
    pub fn url(&self) -> &Url {
        &self.navigation.url
    }

    /// The URL the document actually came from, after redirects.
    pub fn final_url(&self) -> &Url {
        &self.navigation.final_url
    }

    pub fn status(&self) -> u16 {
        self.navigation.status
    }

    /// Whether the status is in the 2xx range.
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.navigation.status)
    }

    /// Response headers in the order the server sent them.
    pub fn headers(&self) -> &[(String, String)] {
        &self.navigation.headers
    }

    /// The first value of `name`, case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.navigation.header(name)
    }

    pub fn content_type(&self) -> Option<&str> {
        self.navigation.content_type.as_deref()
    }

    /// The encoding the document was decoded with.
    pub fn encoding(&self) -> &str {
        &self.navigation.encoding
    }

    pub fn timing(&self) -> Timing {
        self.navigation.timing
    }

    /// The hops that led here, oldest first.
    pub fn redirects(&self) -> &[RedirectHop] {
        &self.navigation.redirects
    }

    /// Everything the navigation recorded.
    pub fn navigation(&self) -> &Navigation {
        &self.navigation
    }

    /// The navigation generation that produced this document.
    pub fn navigation_id(&self) -> u64 {
        self.navigation_id
    }

    /// Subresource hints the preload scanner saw. At Tier 1 every one of them
    /// is recorded and none is fetched (`docs/spec/02-architecture.md` §1.2).
    pub fn recorded_hints(&self) -> &[RecordedHint] {
        &self.hints
    }

    /// The underlying arena DOM.
    pub fn dom(&self) -> &DomDocument {
        &self.dom
    }

    /// Takes the arena DOM out of the value.
    pub fn into_dom(self) -> DomDocument {
        self.dom
    }

    // ----------------------------------------------------------- extraction

    /// Block-formatted text of the whole document.
    pub fn text(&self) -> String {
        self.dom.text()
    }

    /// The document as Markdown.
    pub fn markdown(&self) -> String {
        self.dom.markdown()
    }

    /// Every link, with absolute URLs.
    pub fn links(&self) -> Vec<Link> {
        self.dom.links()
    }

    /// Title, language, `<meta>`, Open Graph, JSON-LD and microdata.
    pub fn metadata(&self) -> Metadata {
        self.dom.metadata()
    }

    /// Every form and its fields.
    pub fn forms(&self) -> Vec<Form> {
        self.dom.forms()
    }

    /// The structural snapshot used by MCP and `--dump snapshot`.
    pub fn snapshot(&self) -> SnapshotNode {
        self.dom.snapshot()
    }

    /// The main article content, if the page has one.
    pub fn main_content(&self) -> Option<MainContent> {
        self.dom.main_content()
    }

    /// The document element serialised back to HTML.
    pub fn html(&self) -> String {
        match self.dom.document_element() {
            Some(root) => self.dom.outer_html(root),
            None => String::new(),
        }
    }

    /// The document's title, if it has one.
    pub fn title(&self) -> Option<String> {
        self.dom.metadata().title
    }

    /// The first element matching `selector`, in tree order.
    pub fn query_selector(&self, selector: &str) -> tenta_core::Result<Option<NodeId>> {
        self.dom.query_selector(self.dom.document_node(), selector)
    }

    /// Every element matching `selector`, in tree order.
    pub fn query_selector_all(&self, selector: &str) -> tenta_core::Result<Vec<NodeId>> {
        self.dom
            .query_selector_all(self.dom.document_node(), selector)
    }

    /// Block-formatted text of one node.
    pub fn text_of(&self, node: NodeId) -> String {
        tenta_dom::extract::text::text(&self.dom, node)
    }

    /// What the DOM costs (`docs/spec/14-memory.md` §3).
    pub fn memory_report(&self) -> MemoryReport {
        self.dom.memory_report()
    }

    /// Everything a host usually wants at once: the response facts and every
    /// extraction, as one JSON value.
    pub fn to_json(&self) -> Value {
        let timing = self.navigation.timing;
        json!({
            "url": self.url().as_str(),
            "final_url": self.final_url().as_str(),
            "status": self.status(),
            "ok": self.ok(),
            "content_type": self.content_type(),
            "encoding": self.encoding(),
            "redirects": self.redirects().iter().map(|hop| hop.url.as_str()).collect::<Vec<_>>(),
            "headers": self.headers().iter().map(|(name, value)| json!([name, value])).collect::<Vec<_>>(),
            "timing_ms": {
                "dns": timing.dns.as_secs_f64() * 1000.0,
                "connect": timing.connect.as_secs_f64() * 1000.0,
                "tls": timing.tls.as_secs_f64() * 1000.0,
                "ttfb": timing.ttfb.as_secs_f64() * 1000.0,
                "total": timing.total.as_secs_f64() * 1000.0,
            },
            "title": self.title(),
            "text": self.text(),
            "markdown": self.markdown(),
            "links": self.links(),
            "metadata": self.metadata(),
            "forms": self.forms(),
        })
    }
}
