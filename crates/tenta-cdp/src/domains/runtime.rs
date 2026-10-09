//! The part of `Runtime` the target handshake needs. Evaluation, remote
//! objects and execution contexts are added here by the Runtime package.

use serde_json::{Value, json};

use super::target::resume;
use crate::domain::{CommandContext, CommandFuture, DomainHandler, DomainRegistry};
use crate::error::CdpError;

pub(super) fn register(registry: &mut DomainRegistry) {
    registry.register(Runtime);
}

struct Runtime;

impl DomainHandler for Runtime {
    fn domain(&self) -> &'static str {
        "Runtime"
    }

    fn methods(&self) -> &'static [&'static str] {
        &["runIfWaitingForDebugger"]
    }

    fn handle<'a>(
        &'a self,
        cx: CommandContext,
        method: &'a str,
        _params: Value,
    ) -> CommandFuture<'a> {
        Box::pin(async move {
            match method {
                "runIfWaitingForDebugger" => {
                    resume(cx.session());
                    Ok(json!({}))
                }
                other => Err(CdpError::method_not_found(&format!("Runtime.{other}"))),
            }
        })
    }
}
