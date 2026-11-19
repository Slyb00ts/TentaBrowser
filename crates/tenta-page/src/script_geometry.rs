//! The page's [`GeometryHost`]: layout-backed answers for script
//! (`getBoundingClientRect`, `offsetWidth`, `getComputedStyle`,
//! `elementFromPoint`, `innerWidth`, ...), served from the page's render
//! session.
//!
//! Every call runs inside an engine callback, so it borrows the document only
//! for its own length, never holds a borrow across script, and answers with the
//! neutral value (no box, no style) when it cannot: the document is mutably
//! borrowed, the render session is busy, or the session belongs to another
//! document. Style and layout are brought up to date lazily, on the call that
//! needs them; the session does no work while the document generation, the
//! sheets and the viewport are unchanged, so a script that reads geometry in a
//! loop pays for one layout.
//!
//! A sheet whose fetch has not completed is absent from the layout, as in a
//! browser mid-load; its arrival changes the sheets' signature and restyles on
//! the next call.

use std::cell::RefCell;
use std::rc::Weak;
use std::sync::{Arc, OnceLock};

use tenta_core::{NodeId, PageId};
use tenta_dom::Document as DomDocument;
use tenta_js::geometry::{
    ClientMetrics, GeometryHost, OffsetMetrics, RectF64, ScrollMetrics, ViewportMetrics,
};
use tenta_render::{ElementMetrics, PseudoArgument, RenderSession, parse_pseudo};
use url::Url;

use crate::render::{self, RenderSettings};
use crate::runtime::ScriptBudget;

/// The geometry host of one navigation's engine.
pub(crate) struct PageGeometry {
    page: PageId,
    /// The generation of the navigation whose engine asks, which keys the
    /// page's render session.
    navigation: u64,
    document: Weak<RefCell<DomDocument>>,
    budget: Arc<dyn ScriptBudget>,
}

impl PageGeometry {
    pub(crate) fn new(
        page: PageId,
        navigation: u64,
        document: Weak<RefCell<DomDocument>>,
        budget: Arc<dyn ScriptBudget>,
    ) -> PageGeometry {
        PageGeometry {
            page,
            navigation,
            document,
            budget,
        }
    }

    /// Runs `read` over the page's up-to-date session and document, or says
    /// there is no layout to ask.
    fn with_layout<R>(
        &self,
        read: impl FnOnce(&mut RenderSession, &DomDocument) -> R,
    ) -> Option<R> {
        let settings = self.budget.render_settings()?;
        let shared = self.document.upgrade()?;
        let doc = shared.try_borrow().ok()?;
        let render = render::for_script(self.page, settings, self.navigation, doc.document_id())?;
        let base = doc
            .base_url()
            .or(doc.url())
            .cloned()
            .unwrap_or_else(|| Url::parse("about:blank").expect("a constant URL"));
        let mut session = render.session.try_borrow_mut().ok()?;
        render::prepare_shared(&mut session, &render.style, &doc, &base, settings.viewport);
        Some(read(&mut session, &doc))
    }

    fn settings(&self) -> Option<RenderSettings> {
        self.budget.render_settings()
    }

    fn metrics(&self, node: NodeId) -> Option<ElementMetrics> {
        self.with_layout(|session, doc| session.element_metrics(doc, node))
            .flatten()
    }
}

impl GeometryHost for PageGeometry {
    fn viewport(&self) -> ViewportMetrics {
        let Some(settings) = self.settings() else {
            return ViewportMetrics::default();
        };
        let width = f64::from(settings.viewport.width);
        let height = f64::from(settings.viewport.height);
        ViewportMetrics {
            inner_width: width,
            inner_height: height,
            outer_width: width,
            outer_height: height,
            device_pixel_ratio: f64::from(settings.viewport.device_scale_factor),
            scroll_x: 0.0,
            scroll_y: 0.0,
        }
    }

    fn bounding_rect(&self, node: NodeId) -> Option<RectF64> {
        let rects = self.client_rects(node);
        let first = rects.first()?;
        let (mut left, mut top) = (first.x, first.y);
        let (mut right, mut bottom) = (first.x + first.width, first.y + first.height);
        for rect in &rects[1..] {
            left = left.min(rect.x);
            top = top.min(rect.y);
            right = right.max(rect.x + rect.width);
            bottom = bottom.max(rect.y + rect.height);
        }
        Some(RectF64 {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
        })
    }

    fn client_rects(&self, node: NodeId) -> Vec<RectF64> {
        self.with_layout(|session, doc| session.element_rects(doc, node))
            .unwrap_or_default()
            .into_iter()
            .map(|[x, y, width, height]| RectF64 {
                x,
                y,
                width,
                height,
            })
            .collect()
    }

    fn offset_metrics(&self, node: NodeId) -> Option<OffsetMetrics> {
        let metrics = self.metrics(node)?;
        Some(OffsetMetrics {
            top: metrics.offset_top,
            left: metrics.offset_left,
            width: metrics.offset_width,
            height: metrics.offset_height,
            parent: metrics.offset_parent,
        })
    }

    fn client_metrics(&self, node: NodeId) -> Option<ClientMetrics> {
        let metrics = self.metrics(node)?;
        Some(ClientMetrics {
            top: metrics.client_top,
            left: metrics.client_left,
            width: metrics.client_width,
            height: metrics.client_height,
        })
    }

    fn scroll_metrics(&self, node: NodeId) -> Option<ScrollMetrics> {
        let metrics = self.metrics(node)?;
        Some(ScrollMetrics {
            top: 0.0,
            left: 0.0,
            width: metrics.scroll_width,
            height: metrics.scroll_height,
        })
    }

    fn computed_value(&self, node: NodeId, pseudo: Option<&str>, property: &str) -> Option<String> {
        let pseudo = match parse_pseudo(pseudo) {
            PseudoArgument::None => None,
            PseudoArgument::Element(pseudo) => Some(pseudo),
            // A pseudo-element this engine does not resolve has an empty
            // declaration, not "no layout".
            PseudoArgument::Unknown => return self.settings().map(|_| String::new()),
        };
        self.with_layout(|session, doc| session.resolved_value(doc, node, pseudo, property))
            .map(Option::unwrap_or_default)
    }

    fn computed_property_names(&self) -> &'static [&'static str] {
        static NAMES: OnceLock<Vec<&'static str>> = OnceLock::new();
        NAMES.get_or_init(tenta_css::cascade::StyleEngine::computed_property_names)
    }

    fn element_from_point(&self, x: f64, y: f64) -> Option<NodeId> {
        self.with_layout(|session, doc| session.element_at(doc, x, y))
            .flatten()
    }
}

/// The page's linked stylesheets as script reads them (`link.sheet`).
pub(crate) struct PageStyleHost {
    page: PageId,
    navigation: u64,
    document: Weak<RefCell<DomDocument>>,
    /// What script did to the document's sheets, made when script first has
    /// a sheet to report.
    script: std::cell::OnceCell<std::rc::Rc<crate::script_sheets::ScriptSheets>>,
}

impl PageStyleHost {
    pub(crate) fn new(
        page: PageId,
        navigation: u64,
        document: Weak<RefCell<DomDocument>>,
    ) -> PageStyleHost {
        PageStyleHost {
            page,
            navigation,
            document,
            script: std::cell::OnceCell::new(),
        }
    }

    /// The document's registry of script sheets. `None` while the document
    /// cannot be read, which a caller treats as "not told".
    fn script_sheets(&self) -> Option<&std::rc::Rc<crate::script_sheets::ScriptSheets>> {
        if let Some(sheets) = self.script.get() {
            return Some(sheets);
        }
        let id = self.document.upgrade()?.try_borrow().ok()?.document_id();
        Some(
            self.script
                .get_or_init(|| crate::script_sheets::ScriptSheets::for_document(id)),
        )
    }
}

impl tenta_js::stylesheets::StyleHost for PageStyleHost {
    fn epoch(&self) -> u64 {
        let Some(shared) = self.document.upgrade() else {
            return 0;
        };
        let Ok(doc) = shared.try_borrow() else {
            return 0;
        };
        render::style_of(self.page, self.navigation, doc.document_id())
            .and_then(|style| style.try_borrow().ok().map(|s| s.completed_fetches() + 1))
            .unwrap_or(0)
    }

    fn element_sheet(
        &self,
        owner: tenta_core::NodeRef,
        source: u64,
        sheet: std::rc::Rc<dyn tenta_js::stylesheets::ScriptSheet>,
    ) {
        if let Some(sheets) = self.script_sheets() {
            sheets.set_element(owner, source, &sheet);
        }
    }

    fn adopted_sheets(
        &self,
        scope: Option<tenta_core::NodeRef>,
        sheets: Vec<std::rc::Rc<dyn tenta_js::stylesheets::ScriptSheet>>,
    ) {
        if let Some(registry) = self.script_sheets() {
            registry.set_adopted(scope, sheets);
        }
    }

    fn linked_sheet(&self, node: NodeId) -> Option<tenta_js::stylesheets::LinkedSheet> {
        let shared = self.document.upgrade()?;
        let doc = shared.try_borrow().ok()?;
        let base = doc.base_url().or(doc.url())?;
        let href = doc.attribute(node, tenta_core::atom::atoms::HREF)?.trim();
        if href.is_empty() {
            return None;
        }
        let mut key = base.join(href).ok()?;
        key.set_fragment(None);
        let style = render::style_of(self.page, self.navigation, doc.document_id())?;
        let (url, text, cors) = style.try_borrow().ok()?.loaded_sheet(&key)?;
        let opaque = !cors && !tenta_net::request::same_origin(&url, doc.url()?);
        Some(tenta_js::stylesheets::LinkedSheet { url, text, opaque })
    }
}
