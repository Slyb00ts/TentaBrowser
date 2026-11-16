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
use tenta_render::{
    FontPolicy, ImagePlan, ImageWant, LazyRegion, RenderConfig, RenderError, RenderSession,
};
pub use tenta_render::{ImageFormat, RasterStats, Screenshot, StageTimes, WorkCounters};
use url::Url;

use crate::images::{ImageLedger, Loaded};
use crate::page::{ImageLoader, SheetLoader, Viewport};
use crate::styles::StyleState;

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

/// The size of a document's scrollable extent in CSS pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContentSize {
    pub width: f32,
    pub height: f32,
}

/// The stylesheets of the document the session renders, and which of them the
/// session has been given.
#[derive(Default)]
pub(crate) struct Sheets {
    /// The fetched stylesheets of the navigation. Shared with the load-time
    /// [`SheetLoader`] and the geometry host, which never wait on the mutex
    /// around this struct, so it is only ever borrowed between awaits.
    pub state: Rc<RefCell<StyleState>>,
    /// What this navigation's images have cost so far.
    pub images: ImageLedger,
}

struct PageRender {
    /// The navigation whose document this session renders.
    navigation: u64,
    session: Rc<RefCell<RenderSession>>,
    sheets: Rc<tokio::sync::Mutex<Sheets>>,
    /// Loads the document's images as they enter it; it shares the session's
    /// store and the sheets' image ledger.
    loader: Rc<ImageLoader>,
    /// Fetches the document's stylesheets as they enter it.
    sheet_loader: Rc<SheetLoader>,
}

/// What a screenshot works with.
pub(crate) struct Handles {
    pub session: Rc<RefCell<RenderSession>>,
    pub sheets: Rc<tokio::sync::Mutex<Sheets>>,
    pub loader: Rc<ImageLoader>,
    pub sheet_loader: Rc<SheetLoader>,
}

thread_local! {
    /// The render sessions of this page thread, by page.
    static RENDERS: RefCell<HashMap<PageId, PageRender>> = RefCell::new(HashMap::new());
}

/// The session and sheets of `page` for the document of `navigation`, created
/// on first use. A session built for an earlier document is replaced: its
/// styles, boxes and fetched sheets all belong to that document.
pub(crate) fn handles(
    page: PageId,
    navigation: u64,
    viewport: Viewport,
    system_fonts: bool,
) -> Handles {
    RENDERS.with(|cell| {
        let mut renders = cell.borrow_mut();
        let fresh = renders
            .get(&page)
            .is_none_or(|render| render.navigation != navigation);
        if fresh {
            let session = RenderSession::new(RenderConfig {
                viewport: render_viewport(viewport),
                fonts: if system_fonts {
                    FontPolicy::SystemFallback
                } else {
                    FontPolicy::Embedded
                },
                ..RenderConfig::default()
            });
            let session = Rc::new(RefCell::new(session));
            let loader = ImageLoader::new(Rc::downgrade(&session));
            let sheets = Sheets::default();
            let sheet_loader = SheetLoader::new(Rc::clone(&sheets.state));
            renders.insert(
                page,
                PageRender {
                    navigation,
                    session,
                    sheets: Rc::new(tokio::sync::Mutex::new(sheets)),
                    loader,
                    sheet_loader,
                },
            );
        }
        let render = &renders[&page];
        Handles {
            session: Rc::clone(&render.session),
            sheets: Rc::clone(&render.sheets),
            loader: Rc::clone(&render.loader),
            sheet_loader: Rc::clone(&render.sheet_loader),
        }
    })
}

/// The image loader of `page`'s current render session, if it has one.
pub(crate) fn loader_of(page: PageId) -> Option<Rc<ImageLoader>> {
    RENDERS.with(|cell| {
        cell.borrow()
            .get(&page)
            .map(|render| Rc::clone(&render.loader))
    })
}

/// The stylesheet loader of `page`'s render session for the document of
/// `navigation`, if it has one.
pub(crate) fn sheet_loader_of(page: PageId, navigation: u64) -> Option<Rc<SheetLoader>> {
    RENDERS.with(|cell| {
        cell.borrow()
            .get(&page)
            .filter(|render| render.navigation == navigation)
            .map(|render| Rc::clone(&render.sheet_loader))
    })
}

/// Whether `page` has a render session for the document of `navigation`.
pub(crate) fn loader_exists(page: PageId, navigation: u64) -> bool {
    RENDERS.with(|cell| {
        cell.borrow()
            .get(&page)
            .is_some_and(|render| render.navigation == navigation)
    })
}

/// Drops the session of `page` unless it renders the document of
/// `navigation`: a document that installs keeps the session its images began
/// loading into while it was parsed, and ends the one of the document it
/// replaces.
pub(crate) fn release_unless(page: PageId, navigation: u64) {
    let render = RENDERS.with(|cell| {
        let mut renders = cell.borrow_mut();
        if renders
            .get(&page)
            .is_some_and(|render| render.navigation == navigation)
        {
            return None;
        }
        renders.remove(&page)
    });
    drop(render);
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
    prepare(&mut session, sheets, doc, base_url, viewport);
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

/// Lays `doc` out and reports its scrollable extent, with the same sheets and
/// the same viewport a screenshot would use.
pub(crate) fn content_size(
    session: &RefCell<RenderSession>,
    sheets: &mut Sheets,
    doc: &Document,
    base_url: &Url,
    viewport: Viewport,
) -> Result<ContentSize> {
    if doc.document_element().is_none() {
        return Err(Error::invalid("the document has no root element"));
    }
    let mut session = session.borrow_mut();
    prepare(&mut session, sheets, doc, base_url, viewport);
    let extent = session.layout(doc).scroll_extent;
    Ok(ContentSize {
        width: extent.right(),
        height: extent.bottom(),
    })
}

/// Lists the images the document needs and the session lacks, after giving the
/// session the sheets the cache holds.
pub(crate) fn plan_images(
    session: &RefCell<RenderSession>,
    sheets: &mut Sheets,
    doc: &Document,
    base_url: &Url,
    viewport: Viewport,
    region: LazyRegion,
) -> ImagePlan {
    let mut session = session.borrow_mut();
    prepare(&mut session, sheets, doc, base_url, viewport);
    session.plan_images(doc, base_url, region)
}

/// What handing an image load to the session came to.
pub(crate) struct Installed {
    /// Images that failed, as `(want, reason)`, for the page to report.
    pub failures: Vec<(ImageWant, String)>,
    /// Keys whose failure was transient (busy, timed out, cancelled): reported,
    /// but not remembered, so a later render asks for them again.
    pub transient: Vec<tenta_render::ImageKey>,
}

/// Hands the outcomes of an image load to the session.
pub(crate) fn install_images(session: &RefCell<RenderSession>, loaded: Vec<Loaded>) -> Installed {
    let mut session = session.borrow_mut();
    let mut done = Installed {
        failures: Vec::new(),
        transient: Vec::new(),
    };
    for Loaded { want, result } in loaded {
        // A failed decode at a new size leaves the first decode in place.
        let again = want.rerender.is_some();
        match result {
            Ok(decoded) => {
                if let Err(error) = session.install_image(want.key.clone(), decoded)
                    && !again
                {
                    let reason = format!("image memory budget exceeded: {error}");
                    session.fail_image(want.key.clone(), reason.clone());
                    done.failures.push((want, reason));
                }
            }
            Err(_) if again => {}
            Err(reason) if crate::images::is_transient(&reason) => {
                done.transient.push(want.key.clone());
                done.failures.push((want, reason));
            }
            Err(reason) => {
                session.fail_image(want.key.clone(), reason.clone());
                done.failures.push((want, reason));
            }
        }
    }
    done
}

fn prepare(
    session: &mut RenderSession,
    sheets: &mut Sheets,
    doc: &Document,
    base_url: &Url,
    viewport: Viewport,
) {
    prepare_shared(session, &sheets.state, doc, base_url, viewport);
}

/// Gives `session` the viewport and, when their signature moved, the sheets
/// `style` holds for `doc`. The state is only borrowed for the length of the
/// call, so the load-time loader's tasks never wait on it.
pub(crate) fn prepare_shared(
    session: &mut RenderSession,
    style: &RefCell<StyleState>,
    doc: &Document,
    base_url: &Url,
    viewport: Viewport,
) {
    session.set_viewport(render_viewport(viewport));
    let mut state = style.borrow_mut();
    let signature = state.signature(doc);
    if state.applied != Some(signature) {
        session.set_style_sources(state.cached_sources(doc, base_url));
        state.applied = Some(signature);
    }
}

/// What a render session of a page is created with.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RenderSettings {
    pub viewport: Viewport,
    pub system_fonts: bool,
}

/// What script geometry works with.
pub(crate) struct ScriptRender {
    pub session: Rc<RefCell<RenderSession>>,
    pub style: Rc<RefCell<StyleState>>,
}

/// The fetched sheets of `page`'s render session, when it is the session of the
/// navigation `navigation`'s document `document_id`. Creates nothing.
pub(crate) fn style_of(
    page: PageId,
    navigation: u64,
    document_id: u64,
) -> Option<Rc<RefCell<StyleState>>> {
    let loader = sheet_loader_of(page, navigation)?;
    loader.claim(document_id).then(|| loader.style())
}

/// The render session of `page` for script geometry, created on first use.
///
/// `navigation` is the generation of the navigation the asking engine belongs
/// to, which is also what keys the session. A session of an older navigation is
/// replaced (its document is on its way out and the asking one is the newer);
/// a session of a newer navigation is not the asker's to read, and `None` says
/// the asker has no layout. A session that serves another tree of the same
/// navigation (a restarted parse) is refused for the same reason.
pub(crate) fn for_script(
    page: PageId,
    settings: RenderSettings,
    navigation: u64,
    document_id: u64,
) -> Option<ScriptRender> {
    let newer = RENDERS.with(|cell| {
        cell.borrow()
            .get(&page)
            .is_some_and(|render| render.navigation > navigation)
    });
    if newer {
        return None;
    }
    let handles = handles(page, navigation, settings.viewport, settings.system_fonts);
    if !handles.sheet_loader.claim(document_id) {
        return None;
    }
    Some(ScriptRender {
        session: handles.session,
        style: handles.sheet_loader.style(),
    })
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
        let first = handles(page(900), 1, viewport, false);
        let same = handles(page(900), 1, viewport, false);
        assert!(Rc::ptr_eq(&first.session, &same.session));
        let next = handles(page(900), 2, viewport, false);
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
        let one = handles(page(901), 1, viewport, false);
        let other = handles(page(902), 1, viewport, false);
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

    #[test]
    fn busy_timed_out_and_cancelled_images_are_not_remembered_as_failed() {
        use tenta_render::{DecodeError, ImageKey, ImageWant, WantOrigin};
        let want = |name: &str| {
            let url = Url::parse(&format!("http://fixture.test/{name}.png")).unwrap();
            ImageWant {
                key: ImageKey(url.as_str().into()),
                url,
                origin: WantOrigin::Element,
                rerender: None,
            }
        };
        let busy = "unsupported SVG feature: svg busy: too many SVG renders in progress";
        let session = RefCell::new(RenderSession::new(RenderConfig::default()));
        let loaded = vec![
            Loaded {
                want: want("busy"),
                result: Err(busy.to_owned()),
            },
            Loaded {
                want: want("slow"),
                result: Err(DecodeError::TimedOut.to_string()),
            },
            Loaded {
                want: want("cancelled"),
                result: Err(DecodeError::Cancelled.to_string()),
            },
            Loaded {
                want: want("late"),
                result: Err("the image did not arrive before the wait ended".to_owned()),
            },
            Loaded {
                want: want("gone"),
                result: Err("the origin answered 404".to_owned()),
            },
        ];
        let installed = install_images(&session, loaded);
        assert_eq!(installed.failures.len(), 5, "all are reported");
        assert_eq!(installed.transient.len(), 4);
        let session = session.borrow();
        for name in ["busy", "slow", "cancelled", "late"] {
            let key = ImageKey(format!("http://fixture.test/{name}.png").into());
            assert!(
                session.image_failure(&key).is_none(),
                "{name} may be tried again"
            );
        }
        let gone = ImageKey("http://fixture.test/gone.png".into());
        assert!(
            session.image_failure(&gone).is_some(),
            "a 404 is remembered"
        );
    }
}
