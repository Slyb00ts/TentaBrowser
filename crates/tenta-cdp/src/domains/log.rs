//! The `Log` domain. Nothing produces log entries yet (console and network
//! messages arrive with the Runtime and Network domains), so enabling and
//! disabling only has to succeed for clients that always send them.

use serde_json::{Value, json};

use crate::domain::{CommandContext, CommandFuture, DomainHandler, DomainRegistry};
use crate::error::CdpError;

pub(super) fn register(registry: &mut DomainRegistry) {
    registry.register(Log);
}

struct Log;

impl DomainHandler for Log {
    fn domain(&self) -> &'static str {
        "Log"
    }

    fn methods(&self) -> &'static [&'static str] {
        &["enable", "disable"]
    }

    fn handle<'a>(
        &'a self,
        _cx: CommandContext,
        method: &'a str,
        _params: Value,
    ) -> CommandFuture<'a> {
        Box::pin(async move {
            match method {
                "enable" | "disable" => Ok(json!({})),
                other => Err(CdpError::method_not_found(&format!("Log.{other}"))),
            }
        })
    }
}
