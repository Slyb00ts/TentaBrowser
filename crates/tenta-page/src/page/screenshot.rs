//! `Page::screenshot`: the page-thread half of a render
//! (`docs/spec/11-library-api.md` §1, `docs/spec/06-layout-and-render.md` §4).
//!
//! A screenshot is three steps on the page thread. The document is only
//! borrowed in the first and the last, and both are synchronous, so nothing
//! can run scripts or navigate while the render holds it:
//!
//! 1. list the stylesheets the document needs (nothing is awaited);
//! 2. fetch the ones the page has not fetched yet, without any borrow of the
//!    document, so timers, evaluations and settle calls keep running;
//! 3. hand the session the sheets, if they changed, and render.
//!
//! A page that keeps an engine has its tree in the engine's shared cell, which
//! the render borrows for step 3. A page that does not has the tree in its
//! state; step 3 lends it out of there so the state lock is not held while
//! rendering, and it comes back however the step ends.

use std::cell::RefCell;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tenta_core::{Error, Result, Tier};
use tenta_dom::Document as DomDocument;
use tenta_render::RenderSession;
use tokio::sync::oneshot;
use url::Url;

use super::{Page, PageInner, PageState, Viewport, retained};
use crate::event::PageEvent;
use crate::fetch_policy::FetchPolicy;
use crate::render::{self, ContentSize, Screenshot, ScreenshotOptions, Sheets, WorkCounters};
use crate::runtime;
use crate::styles::{StyleContext, fetch_planned, plan_fetches};
use crate::task::{catch_unwind_future, panic_message};

/// How long a screenshot waits for the stylesheets it has to fetch. The same
/// as the default navigation timeout.
const STYLESHEET_WAIT: Duration = Duration::from_secs(30);

/// How many times a screenshot lists the document and fetches what is new. A
/// page that keeps adding stylesheets past this is rendered with what it has.
const MAX_FETCH_ROUNDS: usize = 3;

impl Page {
    /// Renders the page's current document to an image.
    ///
    /// Raises the page to the render tier. Linked stylesheets and their
    /// `@import`s are fetched first, each bounded by a 30 s wait for the whole
    /// set, and a sheet that cannot be loaded is reported as
    /// [`PageEvent::StylesheetFailed`] and left out. Sheets are fetched once
    /// per navigation, and the session restyles only when the set of sheets
    /// changed. The screenshot does not wait for the page's scripts: settle the
    /// page first to capture what they build.
    ///
    /// Concurrent screenshots of one page run one after the other. Fails with
    /// [`Error::TierDenied`] when the page's ceiling is below the render tier,
    /// [`Error::InvalidArgument`] when there is no document, a navigation is
    /// replacing it, or the selector matches nothing, and [`Error::Closed`] or
    /// [`Error::Crashed`] when the page ended meanwhile. A capture beyond the
    /// size limits is [`Error::BudgetExceeded`] (page memory, component
    /// `screenshot`), an element without a box or a JPEG quality outside 1 to
    /// 100 is [`Error::InvalidArgument`], and WebP is [`Error::Unsupported`].
    /// A panic in the render crashes the page.
    /// Dropping the future abandons the render; the document is back in place.
    pub async fn screenshot(&self, options: ScreenshotOptions) -> Result<Screenshot> {
        self.on_page_thread(move |inner| screenshot(inner, options))
            .await
    }

    /// The size of the document's scrollable extent in CSS pixels, laid out at
    /// the page's viewport with the same stylesheets a screenshot would use
    /// (fetched first, under the same bounds). The extent is at least the
    /// viewport. Fails like [`Page::screenshot`].
    pub async fn content_size(&self) -> Result<ContentSize> {
        self.on_page_thread(content_size).await
    }

    /// The units of work the page's render session has done, `None` before the
    /// first screenshot. For tests that prove a render did no restyle.
    #[doc(hidden)]
    pub async fn render_counters(&self) -> Result<Option<WorkCounters>> {
        let page = self.inner.config.id;
        self.on_page_thread(move |_| async move { Ok(render::counters(page)) })
            .await
    }

    /// Runs `job` on the page thread and returns what it returns. A panic in
    /// it crashes the page; the caller dropping its future abandons the job at
    /// its next await.
    async fn on_page_thread<T, F, Fut>(&self, job: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(Arc<PageInner>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + 'static,
    {
        let inner = Arc::clone(&self.inner);
        let (reply, answer) = oneshot::channel();
        self.inner.config.thread.post_job(Box::new(move || {
            Box::pin(async move {
                let mut reply = reply;
                let outcome = tokio::select! {
                    biased;
                    () = reply.closed() => return,
                    outcome = catch_unwind_future(job(Arc::clone(&inner))) => outcome,
                };
                let outcome = outcome.unwrap_or_else(|payload| {
                    let reason = panic_message(&payload);
                    inner.mark_crashed(&reason);
                    retained::teardown(&inner);
                    Err(Error::Crashed { reason })
                });
                let _ = reply.send(outcome);
            })
        }))?;
        answer.await.map_err(|_| Error::Closed)?
    }
}

/// Drops the render session of a page that was closed, for a close that has
/// nothing to await with.
pub(super) fn release_later(inner: &Arc<PageInner>) {
    let page = inner.config.id;
    // A thread that is gone took the session with it.
    let _ = inner
        .config
        .thread
        .post_task(Box::new(move || render::release(page)));
}

async fn screenshot(inner: Arc<PageInner>, options: ScreenshotOptions) -> Result<Screenshot> {
    rendered(inner, |session, sheets, doc, base_url, viewport| {
        render::render(session, sheets, doc, base_url, viewport, &options)
    })
    .await
}

async fn content_size(inner: Arc<PageInner>) -> Result<ContentSize> {
    rendered(inner, render::content_size).await
}

/// Fetches the stylesheets the document needs, then runs `render` over the
/// document, the session and the sheets.
async fn rendered<R>(
    inner: Arc<PageInner>,
    render: impl FnOnce(&RefCell<RenderSession>, &mut Sheets, &DomDocument, &Url, Viewport) -> Result<R>,
) -> Result<R> {
    let viewport = inner.config.viewport;
    let (navigation, generation) = admit(&inner)?;
    let handles = render::handles(
        inner.config.id,
        navigation,
        viewport,
        inner.config.system_fonts,
    );
    // Held to the end: it orders this screenshot against the others of the
    // page, and the fetches below need the cache it guards.
    let mut sheets = handles.sheets.lock().await;

    // One deadline for all rounds.
    let deadline = inner.config.clock.now() + STYLESHEET_WAIT;
    let mut rounds = 0;
    let (base_url, unwaited) = loop {
        // A script may add a stylesheet while an earlier one is being fetched,
        // so the document is listed again after every round.
        let (document_url, base_url, plan) = with_document(&inner, navigation, |doc| {
            let blank = || Url::parse("about:blank").expect("a constant URL");
            let base = doc.base_url().or(doc.url()).cloned().unwrap_or_else(blank);
            let url = doc.url().cloned().unwrap_or_else(|| base.clone());
            let plan = plan_fetches(doc, &base, &style_context(&inner, &url, deadline));
            (url, base, plan)
        })?;
        let Some(plan) = plan else {
            break (base_url, Vec::new());
        };
        let pending = plan.unfetched(&sheets.state);
        if pending.is_empty() || rounds == MAX_FETCH_ROUNDS {
            break (base_url, pending);
        }
        rounds += 1;
        let ctx = style_context(&inner, &document_url, deadline);
        let fetched = until_replaced(
            &inner,
            generation,
            fetch_planned(plan, &base_url, &ctx, &mut sheets.state),
        )
        .await;
        for failure in sheets.state.take_failures() {
            inner.emit(PageEvent::StylesheetFailed {
                url: failure.url,
                reason: failure.reason,
            });
        }
        if fetched.is_none() {
            return Err(replaced(&inner));
        }
    };
    for url in unwaited {
        inner.emit(PageEvent::StylesheetFailed {
            url,
            reason: format!(
                "not waited for: the document was still adding stylesheets after {MAX_FETCH_ROUNDS} fetch rounds"
            ),
        });
    }

    with_document(&inner, navigation, |doc| {
        render(&handles.session, &mut sheets, doc, &base_url, viewport)
    })?
}

/// What fetching the sheets of the document at `document_url` needs.
fn style_context<'a>(
    inner: &'a PageInner,
    document_url: &'a Url,
    deadline: Instant,
) -> StyleContext<'a> {
    StyleContext {
        client: &inner.config.client,
        fs_policy: &inner.config.fs_policy,
        budgets: &inner.config.budgets,
        clock: &*inner.config.clock,
        extra_headers: &inner.config.extra_headers,
        document_url,
        deadline,
        fetch_policy: FetchPolicy::for_tier(Tier::Render),
    }
}

/// Checks the page can be rendered and raises it to the render tier. Returns
/// the navigation of the document to render and the page's current generation.
fn admit(inner: &Arc<PageInner>) -> Result<(u64, u64)> {
    let mut state = inner.state.lock();
    check_usable(&state)?;
    if inner.config.max_tier < Tier::Render {
        return Err(Error::TierDenied {
            needed: Tier::Render,
            ceiling: inner.config.max_tier,
        });
    }
    check_settled(&state)?;
    let raised = state.tier < Tier::Render;
    if raised {
        state.tier = Tier::Render;
    }
    let ids = (state.document_navigation_id, state.navigation_generation);
    drop(state);
    if raised {
        inner.emit(PageEvent::TierChanged { tier: Tier::Render });
    }
    Ok(ids)
}

fn check_usable(state: &PageState) -> Result<()> {
    if state.closed {
        return Err(Error::Closed);
    }
    match &state.crashed {
        Some(reason) => Err(Error::Crashed {
            reason: reason.clone(),
        }),
        None => Ok(()),
    }
}

/// A document is there to render and no navigation is about to replace it.
fn check_settled(state: &PageState) -> Result<()> {
    if state.navigations_in_flight > 0 {
        return Err(Error::invalid(
            "a navigation is in progress; wait for its goto before taking a screenshot",
        ));
    }
    if state.document.is_none() && !state.engine_held {
        return Err(Error::invalid("the page has no document yet"));
    }
    Ok(())
}

/// Why a wait for the page ended early.
fn replaced(inner: &PageInner) -> Error {
    match check_usable(&inner.state.lock()) {
        Err(error) => error,
        Ok(()) => Error::invalid(
            "the page navigated, or ended its engine, while the screenshot was waiting",
        ),
    }
}

/// Drives `work` unless the page closes, crashes or starts another navigation
/// first, which `None` reports.
async fn until_replaced<T>(
    inner: &PageInner,
    generation: u64,
    work: impl Future<Output = T>,
) -> Option<T> {
    // Registered before the check, so a change between the two is not lost.
    let changed = inner.generation_changed.notified();
    tokio::pin!(changed);
    changed.as_mut().enable();
    if inner.state.lock().navigation_generation != generation {
        return None;
    }
    tokio::select! {
        biased;
        () = &mut changed => None,
        output = work => Some(output),
    }
}

/// Runs `read` on the page's document without awaiting.
///
/// The document of the navigation `navigation` is the engine's shared tree
/// when the page keeps an engine, and otherwise the page's own, lent out of
/// its state for the length of the call.
fn with_document<R>(
    inner: &Arc<PageInner>,
    navigation: u64,
    read: impl FnOnce(&DomDocument) -> R,
) -> Result<R> {
    let mut state = inner.state.lock();
    check_usable(&state)?;
    check_settled(&state)?;
    if state.document_navigation_id != navigation {
        return Err(replaced(inner));
    }
    if state.engine_held {
        drop(state);
        // Gone when the engine was discarded after a crash, or released.
        let shared = runtime::with_runtime(inner.config.id, |runtime| runtime.document().clone())
            .ok_or_else(|| replaced(inner))?;
        let output = read(&shared.borrow());
        return Ok(output);
    }
    let document = state
        .document
        .take()
        .expect("check_settled found a document or an engine");
    state.cache_title(&document);
    state.document_lent = true;
    drop(state);
    let lent = Lent {
        inner,
        document: Some(document),
    };
    Ok(read(lent.document.as_ref().expect("lent above")))
}

/// The page's document while a render has it. Puts it back when dropped, a
/// panic in the render included, unless the page crashed meanwhile.
struct Lent<'a> {
    inner: &'a PageInner,
    document: Option<DomDocument>,
}

impl Drop for Lent<'_> {
    fn drop(&mut self) {
        let document = self.document.take();
        let mut state = self.inner.state.lock();
        state.document_lent = false;
        if state.crashed.is_none() {
            state.document = document;
        }
        drop(state);
    }
}
