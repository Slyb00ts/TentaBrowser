//! The `Target` domain: the part the transport needs to be usable (listing,
//! creating and closing pages, flattened attachment).

use serde::Deserialize;
use serde_json::{Value, json};

use crate::connection::SessionIdKind;
use crate::domain::{CommandContext, CommandFuture, DomainHandler, DomainRegistry, parse_params};
use crate::error::CdpError;

pub(super) fn register(registry: &mut DomainRegistry) {
    registry.register(Target);
}

struct Target;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateTarget {
    url: Option<String>,
    browser_context_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TargetId {
    target_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachToTarget {
    target_id: String,
    flatten: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetachFromTarget {
    session_id: String,
}

fn no_such_target() -> CdpError {
    CdpError::invalid_params("No target with given id found")
}

impl DomainHandler for Target {
    fn domain(&self) -> &'static str {
        "Target"
    }

    fn methods(&self) -> &'static [&'static str] {
        &[
            "getTargets",
            "createTarget",
            "closeTarget",
            "attachToTarget",
            "detachFromTarget",
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
                "getTargets" => {
                    let infos: Vec<Value> = cx
                        .backend()
                        .targets()
                        .iter()
                        .map(|target| target.info())
                        .collect();
                    Ok(json!({ "targetInfos": infos }))
                }
                "createTarget" => {
                    let CreateTarget {
                        url,
                        browser_context_id,
                    } = parse_params(params)?;
                    let target = cx
                        .backend()
                        .create_target(browser_context_id.as_deref(), url.as_deref())
                        .await?;
                    Ok(json!({ "targetId": target.id() }))
                }
                "closeTarget" => {
                    let TargetId { target_id } = parse_params(params)?;
                    let closed = cx.backend().close_target(&target_id).await?;
                    if closed {
                        Ok(json!({ "success": true }))
                    } else {
                        Err(no_such_target())
                    }
                }
                "attachToTarget" => {
                    let AttachToTarget { target_id, flatten } = parse_params(params)?;
                    if flatten != Some(true) {
                        return Err(CdpError::server(
                            "Only flattened sessions are supported; pass flatten: true",
                        ));
                    }
                    let target = cx.backend().target(&target_id).ok_or_else(no_such_target)?;
                    let session = cx.connection().attach(
                        &target,
                        cx.session().id(),
                        SessionIdKind::Explicit,
                    )?;
                    let session_id = session.id().unwrap_or_default().to_owned();
                    cx.emit(
                        "Target.attachedToTarget",
                        json!({
                            "sessionId": session_id,
                            "targetInfo": target.info(),
                            "waitingForDebugger": false,
                        }),
                    )
                    .await;
                    Ok(json!({ "sessionId": session_id }))
                }
                "detachFromTarget" => {
                    let DetachFromTarget { session_id } = parse_params(params)?;
                    cx.connection()
                        .detach(&session_id)
                        .await
                        .ok_or_else(|| CdpError::invalid_params("Session not found"))?;
                    Ok(json!({}))
                }
                other => Err(CdpError::method_not_found(&format!("Target.{other}"))),
            }
        })
    }
}
