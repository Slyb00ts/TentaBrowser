//! The `Browser` domain.

use std::path::Path;

use serde::Deserialize;
use serde_json::{Value, json};
use tenta_core::FsPolicy;
use tokio_tungstenite::tungstenite::Message;

use super::target::{find_context, shared};
use crate::backend::lock;
use crate::domain::{CommandContext, CommandFuture, DomainHandler, DomainRegistry, parse_params};
use crate::error::CdpError;

pub(super) fn register(registry: &mut DomainRegistry) {
    registry.register(Browser);
}

struct Browser;

/// The behavior that takes effect: a download directory the engine's
/// `FsPolicy` would not let it write to turns the request into a denial,
/// rather than a promise the policy would break later.
fn effective_download<'a>(
    policy: &FsPolicy,
    behavior: &'a str,
    download_path: Option<&str>,
) -> &'a str {
    if !matches!(behavior, "allow" | "allowAndName") {
        return behavior;
    }
    let permitted = download_path.map(Path::new).is_some_and(|path| {
        policy.check_write(path).is_ok()
            || policy
                .download_dir()
                .is_some_and(|dir| path.starts_with(dir))
    });
    if permitted { behavior } else { "deny" }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetDownloadBehavior {
    behavior: String,
    browser_context_id: Option<String>,
    download_path: Option<String>,
    events_enabled: Option<bool>,
}

impl DomainHandler for Browser {
    fn domain(&self) -> &'static str {
        "Browser"
    }

    fn methods(&self) -> &'static [&'static str] {
        &["getVersion", "close", "setDownloadBehavior"]
    }

    fn handle<'a>(
        &'a self,
        cx: CommandContext,
        method: &'a str,
        params: Value,
    ) -> CommandFuture<'a> {
        Box::pin(async move {
            match method {
                "getVersion" => {
                    let options = cx.backend().options();
                    Ok(json!({
                        "protocolVersion": "1.3",
                        "product": options.product,
                        "revision": "",
                        "userAgent": options.user_agent,
                        "jsVersion": "",
                    }))
                }
                "close" => {
                    let session = cx.session().clone();
                    let engine = cx.backend().engine().clone();
                    // Take the next turn on the session: it resolves once this
                    // command's response has been queued, so neither the
                    // shutdown nor the close frame can overtake the answer,
                    // and no later command starts on a connection that is
                    // going away. The engine is closed before the frame goes
                    // out, so a client that sees the socket end knows the
                    // browser is gone. Only this connection is closed; the
                    // others learn of it through their targets being
                    // destroyed.
                    let (previous, turn) = session.next_turn();
                    tokio::spawn(async move {
                        if let Some(previous) = previous {
                            let _ = previous.await;
                        }
                        // Joining the page threads blocks.
                        let _ = tokio::task::spawn_blocking(move || engine.close()).await;
                        session.send(Message::Close(None)).await;
                        drop(turn);
                    });
                    Ok(json!({}))
                }
                // Downloads are a later phase's policy under `FsPolicy`; for
                // now the choice is validated and kept per context, and
                // nothing downloads.
                "setDownloadBehavior" => {
                    let SetDownloadBehavior {
                        behavior,
                        browser_context_id,
                        download_path,
                        events_enabled,
                    } = parse_params(params)?;
                    if !matches!(
                        behavior.as_str(),
                        "deny" | "allow" | "allowAndName" | "default"
                    ) {
                        return Err(CdpError::invalid_params(format!(
                            "Unknown download behavior: {behavior}"
                        )));
                    }
                    if matches!(behavior.as_str(), "allow" | "allowAndName")
                        && download_path.as_deref().is_none_or(str::is_empty)
                    {
                        return Err(CdpError::invalid_params("downloadPath not provided"));
                    }
                    let (context_id, _) =
                        find_context(cx.backend(), browser_context_id.as_deref())?;
                    let effective = effective_download(
                        cx.backend().engine().fs_policy(),
                        &behavior,
                        download_path.as_deref(),
                    );
                    lock(&shared(&cx)?.downloads).insert(
                        context_id,
                        json!({
                            "behavior": effective,
                            "requested": behavior,
                            "downloadPath": download_path,
                            "eventsEnabled": events_enabled.unwrap_or(false),
                        }),
                    );
                    Ok(json!({}))
                }
                other => Err(CdpError::method_not_found(&format!("Browser.{other}"))),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_download_path_outside_the_policy_becomes_a_denial() {
        let root = std::env::temp_dir();
        let nowhere = "/nonexistent-root/downloads";
        let inside = root.join("tenta-cdp-downloads");
        let inside = inside.to_str().unwrap();

        let closed = FsPolicy::none();
        assert_eq!(effective_download(&closed, "allow", Some(inside)), "deny");

        let open = FsPolicy::none().allow_write(&root);
        assert_eq!(effective_download(&open, "allow", Some(inside)), "allow");
        assert_eq!(
            effective_download(&open, "allowAndName", Some(inside)),
            "allowAndName"
        );
        assert_eq!(effective_download(&open, "allow", Some(nowhere)), "deny");

        let downloads = FsPolicy::none().downloads_to(&root);
        assert_eq!(
            effective_download(&downloads, "allow", Some(inside)),
            "allow"
        );

        // Only the two behaviors that write need a path.
        assert_eq!(effective_download(&closed, "deny", None), "deny");
        assert_eq!(effective_download(&closed, "default", None), "default");
    }
}
