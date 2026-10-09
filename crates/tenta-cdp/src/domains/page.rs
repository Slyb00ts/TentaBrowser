//! The `Page` domain: navigation, the frame's lifecycle, screenshots and the
//! scripts that run in every new document.
//!
//! A page has one frame, whose id is the target id. `Page.navigate` answers
//! when the navigation commits, like Chromium, and leaves the load to the
//! events (`frames`). A navigation the client starts keeps running when the
//! session goes away, as in Chromium; only `Page.stopLoading`, another
//! navigation or closing the page ends it.
//!
//! Isolated worlds are an alias: `Page.createIsolatedWorld` and named init
//! scripts give a utility context of their own whose code runs in the main
//! realm (`World::Utility`).
//!
//! Not built, and answered `-32601`: dialogs (the engine has no `alert`,
//! `confirm` or `prompt`), same-document navigation events (the engine has no
//! `pushState`) and session history.

mod frames;
mod screenshot;
mod scripts;

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};
use tenta_browser::{Goto, PageEvent};
use tenta_core::Error;

use self::frames::{loader_id, set_lifecycle};
use self::scripts::InitScripts;
use crate::backend::Target;
use crate::domain::{CommandContext, CommandFuture, DomainHandler, DomainRegistry, parse_params};
use crate::error::CdpError;
use crate::session::Session;

pub(super) fn register(registry: &mut DomainRegistry) {
    registry.register(Page);
}

struct Page;

impl DomainHandler for Page {
    fn domain(&self) -> &'static str {
        "Page"
    }

    fn methods(&self) -> &'static [&'static str] {
        &[
            "enable",
            "disable",
            "getFrameTree",
            "navigate",
            "reload",
            "stopLoading",
            "setLifecycleEventsEnabled",
            "captureScreenshot",
            "getLayoutMetrics",
            "addScriptToEvaluateOnNewDocument",
            "removeScriptToEvaluateOnNewDocument",
            "setDocumentContent",
            "createIsolatedWorld",
            "setBypassCSP",
            "bringToFront",
            "close",
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
                "enable" => {
                    frames::enable(cx.session(), cx.target()?);
                    Ok(json!({}))
                }
                "disable" => {
                    frames::disable(cx.session());
                    Ok(json!({}))
                }
                "getFrameTree" => {
                    Ok(json!({ "frameTree": { "frame": frames::frame_json(cx.target()?) } }))
                }
                "navigate" => navigate(&cx, parse_params(params)?).await,
                "reload" => reload(&cx).await,
                "stopLoading" => {
                    cx.target()?.page().stop_loading();
                    Ok(json!({}))
                }
                "setLifecycleEventsEnabled" => {
                    let params: LifecycleParams = parse_params(params)?;
                    set_lifecycle(cx.session(), cx.target()?, params.enabled).await;
                    Ok(json!({}))
                }
                "captureScreenshot" => screenshot::capture(cx.target()?, params).await,
                "getLayoutMetrics" => layout_metrics(cx.target()?).await,
                "addScriptToEvaluateOnNewDocument" => {
                    add_script(cx.session(), cx.target()?, parse_params(params)?).await
                }
                "removeScriptToEvaluateOnNewDocument" => {
                    let params: RemoveScriptParams = parse_params(params)?;
                    // Chromium answers an unknown id with an error.
                    if cx
                        .session()
                        .state::<InitScripts>()
                        .remove(&params.identifier)
                    {
                        Ok(json!({}))
                    } else {
                        Err(CdpError::server("No script with given id"))
                    }
                }
                // Puppeteer and Playwright set content with document.open and
                // document.write through Runtime, and the engine has neither
                // yet; replacing the tree from here would skip the scripts and
                // events a real write produces.
                "setDocumentContent" => {
                    Err(CdpError::server("Page.setDocumentContent is not supported"))
                }
                "createIsolatedWorld" => create_isolated_world(&cx, parse_params(params)?).await,
                // No Content-Security-Policy is enforced, so there is nothing
                // to bypass.
                "setBypassCSP" | "bringToFront" => Ok(json!({})),
                "close" => {
                    let target = cx.target()?;
                    cx.backend().close_target(target.id()).await?;
                    Ok(json!({}))
                }
                other => Err(CdpError::method_not_found(&format!("Page.{other}"))),
            }
        })
    }
}

#[derive(Deserialize)]
struct LifecycleParams {
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NavigateParams {
    url: String,
    referrer: Option<String>,
    frame_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IsolatedWorldParams {
    frame_id: String,
    world_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddScriptParams {
    source: String,
    world_name: Option<String>,
    #[serde(default)]
    run_immediately: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoveScriptParams {
    identifier: String,
}

/// The `errorText` a failed navigation reports: its `net::ERR_*` name.
fn error_text(error: &Error) -> String {
    match error {
        Error::Net { code, .. } if code.starts_with("net::") => code.clone(),
        Error::NetDenied { .. } => "net::ERR_BLOCKED_BY_CLIENT".to_owned(),
        Error::Timeout { .. } => "net::ERR_TIMED_OUT".to_owned(),
        _ => "net::ERR_FAILED".to_owned(),
    }
}

/// How a navigation the command started came out at its commit.
enum Started {
    Committed(tenta_browser::Commit),
    /// It ended without committing: the error is why.
    Failed(Error),
}

/// Starts navigating to `url` and waits for that navigation to commit.
///
/// The session's next command may start once the navigation is under way: a
/// client sends its `Runtime.evaluate` while this one still waits for the
/// response head. A newer navigation that replaces this one before it commits
/// ends it as aborted.
async fn start_navigation(
    cx: &CommandContext,
    target: &Target,
    url: String,
    referrer: Option<String>,
) -> Result<Started, CdpError> {
    let page = target.page().clone();
    // Before the navigation starts, so none of its events is missed.
    let mut events = page.events();
    let mut options = Goto::new().timeout(cx.backend().options().navigation_timeout);
    if let Some(referrer) = referrer.filter(|referrer| !referrer.is_empty()) {
        options = options.referer(referrer);
    }
    let pending = page
        .begin_goto(&url, options)
        .map_err(|_| CdpError::server("Cannot navigate to invalid URL"))?;
    let mine = pending.navigation_id();
    // The navigation outlives this command, as in Chromium.
    let mut navigation = tokio::spawn(pending.finish());
    cx.release_order();

    loop {
        tokio::select! {
            event = events.next() => match event {
                Some(PageEvent::NavigationCommitted { commit, .. })
                    if commit.navigation_id == mine =>
                {
                    return Ok(Started::Committed(commit));
                }
                Some(_) => {}
                None => return Err(CdpError::server("Target closed")),
            },
            ended = &mut navigation => {
                // The commit event precedes the end of its navigation; read
                // what is left of the stream before concluding there was none.
                while let Some(event) = events.try_next() {
                    if let PageEvent::NavigationCommitted { commit, .. } = event
                        && commit.navigation_id == mine
                    {
                        return Ok(Started::Committed(commit));
                    }
                }
                return match ended {
                    Ok(Err(error)) => Ok(Started::Failed(error)),
                    Ok(Ok(_)) => Err(CdpError::server("The navigation finished without committing")),
                    Err(error) => Err(CdpError::server(format!("The navigation was lost: {error}"))),
                };
            }
        }
    }
}

async fn navigate(cx: &CommandContext, params: NavigateParams) -> Result<Value, CdpError> {
    let target = cx.target()?;
    if params
        .frame_id
        .as_deref()
        .is_some_and(|id| id != target.id())
    {
        return Err(CdpError::server("Frame with the given id was not found."));
    }
    let mut reply = json!({ "frameId": target.id() });
    match start_navigation(cx, target, params.url, params.referrer).await? {
        Started::Committed(commit) => {
            reply["loaderId"] = json!(loader_id(target, commit.navigation_id));
            if let Some(error) = commit.error {
                reply["errorText"] = json!(error);
            }
        }
        Started::Failed(error) => match error {
            Error::Closed | Error::Crashed { .. } => return Err(error.into()),
            other => reply["errorText"] = json!(error_text(&other)),
        },
    }
    Ok(reply)
}

/// Reloads the document the frame shows; a document that failed to load is
/// reloaded from the URL that failed.
async fn reload(cx: &CommandContext) -> Result<Value, CdpError> {
    let target = cx.target()?;
    let Some(commit) = target.page().frame_state().commit else {
        // The initial blank document has nothing to fetch.
        return Ok(json!({}));
    };
    let url = commit.unreachable_url.unwrap_or(commit.url);
    let reloading = start_navigation(cx, target, url.to_string(), None);
    match reloading.await? {
        Started::Failed(error @ (Error::Closed | Error::Crashed { .. })) => Err(error.into()),
        _ => Ok(json!({})),
    }
}

async fn layout_metrics(target: &Target) -> Result<Value, CdpError> {
    let viewport = target.page().viewport();
    let (width, height) = screenshot::content_size(target).await?;
    let (client_width, client_height) = (viewport.width, viewport.height);
    let layout = json!({
        "pageX": 0, "pageY": 0,
        "clientWidth": client_width, "clientHeight": client_height,
    });
    let visual = json!({
        "offsetX": 0, "offsetY": 0, "pageX": 0, "pageY": 0,
        "clientWidth": client_width, "clientHeight": client_height,
        "scale": 1, "zoom": 1,
    });
    let content = json!({ "x": 0, "y": 0, "width": width, "height": height });
    Ok(json!({
        "layoutViewport": layout,
        "visualViewport": visual,
        "contentSize": content,
        "cssLayoutViewport": layout,
        "cssVisualViewport": visual,
        "cssContentSize": content,
    }))
}

/// `Page.addScriptToEvaluateOnNewDocument`. The script belongs to the session
/// that added it and is removed when the session goes away. A named world gets
/// its context in every document, but the script runs in the main realm.
async fn add_script(
    session: &Session,
    target: &Arc<Target>,
    params: AddScriptParams,
) -> Result<Value, CdpError> {
    if let Some(name) = params.world_name.as_deref().filter(|name| !name.is_empty()) {
        target.contexts().utility_world(name);
    }
    let id = target.page().add_init_script(&params.source);
    session
        .state::<InitScripts>()
        .add(target.page().clone(), id);
    if params.run_immediately {
        // A script that throws is the page's business, as in Chromium.
        let _ = target.page().evaluate(&params.source).await;
    }
    Ok(json!({ "identifier": id.to_string() }))
}

/// `Page.createIsolatedWorld`: the context of the named utility world in the
/// current document. See `World::Utility` for what the world is.
async fn create_isolated_world(
    cx: &CommandContext,
    params: IsolatedWorldParams,
) -> Result<Value, CdpError> {
    let target = cx.target()?;
    if params.frame_id != target.id() {
        return Err(CdpError::server("Frame with the given id was not found."));
    }
    let name = params.world_name.unwrap_or_default();
    let id = super::runtime::utility_context(cx.session(), target, &name).await?;
    Ok(json!({ "executionContextId": id }))
}
