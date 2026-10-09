//! The page's render session and screenshot entry points.
//!
//! One [`RenderSession`] exists per page, in a page-thread table keyed by page
//! (`docs/dev/p3-plan.md` §4.4). The session is handed out as an
//! `Rc<RefCell<_>>` so the geometry host can share it later; a borrow of it is
//! never held across a script call or an `.await`. It is dropped with the page
//! and with the document it was built for.
//!
//! The sheets handed to the session live next to it in [`Sheets`], behind an
//! async mutex: that mutex is what serialises two screenshots of one page,
//! because the first holds it across its stylesheet fetches.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use tenta_core::{BudgetKind, Error, PageId, Result};
use tenta_dom::Document;
use tenta_layout::Rect;
pub use tenta_render::{ImageFormat, RasterStats, Screenshot, StageTimes, WorkCounters};
use tenta_render::{RenderConfig, RenderError, RenderSession};
use url::Url;

use crate::page::Viewport;
use crate::styles::{StyleSignature, StyleState};

/// A rectangle of the document in CSS pixels, from its top-left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clip {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// What part of the page a screenshot shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Capture {
    /// The viewport the page was created with.
    Viewport,
    /// The whole scrollable extent, laid out once at the page's viewport. The
    /// result is cut off at the render size limits; [`Screenshot::clamped`]
    /// says so.
    FullPage,
    /// A rectangle in document coordinates, widened outwards to whole device
    /// pixels.
    Clip(Clip),
    /// The border boxes of the first element matching a CSS selector.
    Element(String),
}

/// Options of [`crate::Page::screenshot`].
#[derive(Clone, Debug, PartialEq)]
pub struct ScreenshotOptions {
    /// Encoding: PNG, or JPEG at a quality of 1 to 100 (anything else is
    /// [`Error::InvalidArgument`]). WebP fails with [`Error::Unsupported`].
    pub format: ImageFormat,
    pub capture: Capture,
    /// A multiplier on the raster only, as the DevTools clip scale: the output
    /// has `CSS pixels x the viewport's device scale factor x scale` pixels per
    /// side, and layout and media queries do not see it. `None` is 1.
    pub scale: Option<f32>,
    /// Leave the page background transparent.
    pub omit_background: bool,
}

impl ScreenshotOptions {
    /// A PNG of the page's viewport.
    pub fn viewport() -> Self {
        Self::default()
    }

    /// A PNG of the whole scrollable extent.
    pub fn full_page() -> Self {
        Self {
            capture: Capture::FullPage,
            ..Self::default()
        }
    }

    /// A PNG of the first element matching `selector`.
    pub fn element(selector: impl Into<String>) -> Self {
        Self {
            capture: Capture::Element(selector.into()),
            ..Self::default()
        }
    }

    /// A PNG of a rectangle of the document, in CSS pixels.
    pub fn clip(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            capture: Capture::Clip(Clip {
                x,
                y,
                width,
                height,
            }),
            ..Self::default()
        }
    }
}

impl Default for ScreenshotOptions {
    fn default() -> Self {
        Self {
            format: ImageFormat::Png,
            capture: Capture::Viewport,
            scale: None,
            omit_background: false,
        }
    }
}

/// The stylesheets of the document the session renders, and which of them the
/// session has been given.
#[derive(Default)]
pub(crate) struct Sheets {
    pub state: StyleState,
    /// The signature the session's sources were collected for.
    applied: Option<StyleSignature>,
}

struct PageRender {
    /// The navigation whose document this session renders.
    navigation: u64,
    session: Rc<RefCell<RenderSession>>,
    sheets: Rc<tokio::sync::Mutex<Sheets>>,
}

/// What a screenshot works with.
pub(crate) struct Handles {
    pub session: Rc<RefCell<RenderSession>>,
    pub sheets: Rc<tokio::sync::Mutex<Sheets>>,
}

thread_local! {
    /// The render sessions of this page thread, by page.
    static RENDERS: RefCell<HashMap<PageId, PageRender>> = RefCell::new(HashMap::new());
}

/// The session and sheets of `page` for the document of `navigation`, created
/// on first use. A session built for an earlier document is replaced: its
/// styles, boxes and fetched sheets all belong to that document.
pub(crate) fn handles(page: PageId, navigation: u64, viewport: Viewport) -> Handles {
    RENDERS.with(|cell| {
        let mut renders = cell.borrow_mut();
        let fresh = renders
            .get(&page)
            .is_none_or(|render| render.navigation != navigation);
        if fresh {
            let session = RenderSession::new(RenderConfig {
                viewport: render_viewport(viewport),
                ..RenderConfig::default()
            });
            renders.insert(
                page,
                PageRender {
                    navigation,
                    session: Rc::new(RefCell::new(session)),
                    sheets: Rc::new(tokio::sync::Mutex::new(Sheets::default())),
                },
            );
        }
        let render = &renders[&page];
        Handles {
            session: Rc::clone(&render.session),
            sheets: Rc::clone(&render.sheets),
        }
    })
}

/// Drops the session of `page`, if it has one.
pub(crate) fn release(page: PageId) {
    // Taken out first so the session drops outside the table's borrow.
    let render = RENDERS.with(|cell| cell.borrow_mut().remove(&page));
    drop(render);
}

/// Drops every session of this page thread, while the thread can still tear
/// down what a session holds.
pub(crate) fn release_all() {
    let renders: Vec<PageRender> = RENDERS.with(|cell| {
        cell.borrow_mut()
            .drain()
            .map(|(_, render)| render)
            .collect()
    });
    drop(renders);
}

/// The work counters of the session of `page`.
pub(crate) fn counters(page: PageId) -> Option<WorkCounters> {
    RENDERS.with(|cell| {
        let renders = cell.borrow();
        let session = renders.get(&page)?.session.try_borrow().ok()?;
        Some(session.counters())
    })
}

fn render_viewport(viewport: Viewport) -> tenta_render::Viewport {
    tenta_render::Viewport {
        width: viewport.width,
        height: viewport.height,
        device_pixel_ratio: viewport.device_scale_factor,
    }
}

/// Renders `doc` with the sheets the cache holds for it.
///
/// Synchronous on purpose: nothing else touches the document or the session
/// while it runs, and the sheets are handed to the session only when their
/// signature moved, because installing them restyles the whole document.
pub(crate) fn render(
    session: &RefCell<RenderSession>,
    sheets: &mut Sheets,
    doc: &Document,
    base_url: &Url,
    viewport: Viewport,
    options: &ScreenshotOptions,
) -> Result<Screenshot> {
    let capture = match &options.capture {
        Capture::Viewport => tenta_render::Capture::Viewport,
        Capture::FullPage => tenta_render::Capture::FullPage,
        Capture::Clip(clip) => tenta_render::Capture::Clip(Rect {
            x: clip.x,
            y: clip.y,
            w: clip.width,
            h: clip.height,
        }),
        Capture::Element(selector) => {
            let element = doc
                .query_selector(doc.document_node(), selector)?
                .ok_or_else(|| Error::invalid(format!("no element matches {selector:?}")))?;
            tenta_render::Capture::Element(element)
        }
    };
    let mut session = session.borrow_mut();
    session.set_viewport(render_viewport(viewport));
    let signature = sheets.state.signature(doc);
    if sheets.applied != Some(signature) {
        session.set_style_sources(sheets.state.cached_sources(doc, base_url));
        sheets.applied = Some(signature);
    }
    session
        .screenshot(
            doc,
            &tenta_render::ScreenshotOptions {
                format: options.format,
                capture,
                scale: options.scale,
                omit_background: options.omit_background,
            },
        )
        .map_err(error_of)
}

fn error_of(error: RenderError) -> Error {
    match error {
        RenderError::UnsupportedFormat(format) => {
            Error::unsupported(format!("cannot encode {format:?} screenshots yet"))
        }
        RenderError::InvalidOption(_) | RenderError::NoRoot => Error::invalid(error.to_string()),
        RenderError::TooLarge { .. } => Error::BudgetExceeded {
            kind: BudgetKind::PageMemory,
            component: Some(format!("screenshot: {error}")),
        },
        RenderError::NoBox(_) => Error::invalid(
            "the element generates no box to capture (display: none, or not rendered)",
        ),
        RenderError::Paint(_) => Error::Other {
            code: "screenshot_paint_failed".to_owned(),
            message: error.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(raw: u64) -> PageId {
        PageId(raw)
    }

    #[test]
    fn a_new_navigation_replaces_the_session_and_release_drops_it() {
        let viewport = Viewport::default();
        let first = handles(page(900), 1, viewport);
        let same = handles(page(900), 1, viewport);
        assert!(Rc::ptr_eq(&first.session, &same.session));
        let next = handles(page(900), 2, viewport);
        assert!(!Rc::ptr_eq(&first.session, &next.session));
        assert_eq!(
            Rc::strong_count(&first.session),
            2,
            "the old one is ours alone"
        );

        assert!(counters(page(900)).is_some());
        release(page(900));
        assert!(counters(page(900)).is_none());
    }

    #[test]
    fn two_pages_do_not_share_a_session() {
        let viewport = Viewport::default();
        let one = handles(page(901), 1, viewport);
        let other = handles(page(902), 1, viewport);
        assert!(!Rc::ptr_eq(&one.session, &other.session));
        release_all();
        assert!(counters(page(901)).is_none() && counters(page(902)).is_none());
    }

    #[test]
    fn render_errors_map_to_typed_page_errors() {
        assert!(matches!(
            error_of(RenderError::UnsupportedFormat(ImageFormat::Webp)),
            Error::Unsupported { .. }
        ));
        assert!(matches!(
            error_of(RenderError::TooLarge {
                width: 1,
                height: 1
            }),
            Error::BudgetExceeded { kind: BudgetKind::PageMemory, component: Some(component) }
                if component.contains("1x1")
        ));
        assert!(matches!(
            error_of(RenderError::NoBox(tenta_core::NodeId(3))),
            Error::InvalidArgument { .. }
        ));
        assert!(matches!(
            error_of(RenderError::NoRoot),
            Error::InvalidArgument { .. }
        ));
    }
}
