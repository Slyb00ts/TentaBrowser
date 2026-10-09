//! The `Inspector` domain: the crash signal both clients listen to.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};
use tenta_browser::PageEvent;

use crate::backend::Target;
use crate::domain::{CommandContext, CommandFuture, DomainHandler, DomainRegistry};
use crate::error::CdpError;
use crate::session::Session;

pub(super) fn register(registry: &mut DomainRegistry) {
    registry.register(Inspector);
}

struct Inspector;

#[derive(Default)]
struct CrashWatch(AtomicBool);

/// Reports the page's crash on `session` as `Inspector.targetCrashed`.
///
/// Chromium sends the event to every attached session whether or not
/// `Inspector.enable` was called, and Puppeteer never calls it, so attaching
/// starts the watch. Idempotent per session.
pub(super) fn watch_crashes(session: &Arc<Session>, target: &Arc<Target>) {
    if session.state::<CrashWatch>().0.swap(true, Ordering::AcqRel) {
        return;
    }
    let (owner, target) = (Arc::clone(session), Arc::clone(target));
    session.spawn(async move {
        let mut events = target.page().events();
        while let Some(event) = events.next().await {
            if matches!(event, PageEvent::Crashed { .. }) {
                owner.emit("Inspector.targetCrashed", json!({})).await;
            }
        }
    });
}

impl DomainHandler for Inspector {
    fn domain(&self) -> &'static str {
        "Inspector"
    }

    fn methods(&self) -> &'static [&'static str] {
        &["enable", "disable"]
    }

    fn handle<'a>(
        &'a self,
        cx: CommandContext,
        method: &'a str,
        _params: Value,
    ) -> CommandFuture<'a> {
        Box::pin(async move {
            match method {
                // A connection bound to one page (`/devtools/page/{id}`)
                // never passed through an attach, so enabling starts its
                // watch.
                "enable" => {
                    if let Some(target) = cx.session().target() {
                        watch_crashes(cx.session(), target);
                    }
                    Ok(json!({}))
                }
                "disable" => Ok(json!({})),
                other => Err(CdpError::method_not_found(&format!("Inspector.{other}"))),
            }
        })
    }
}
