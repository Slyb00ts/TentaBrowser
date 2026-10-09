//! The `Audits` domain: Puppeteer enables it on every page to receive
//! inspector issues. The engine has none to report, so enabling succeeds and
//! no `issueAdded` event is ever sent.

use serde_json::{Value, json};

use crate::domain::{CommandContext, CommandFuture, DomainHandler, DomainRegistry};
use crate::error::CdpError;

pub(super) fn register(registry: &mut DomainRegistry) {
    registry.register(Audits);
}

struct Audits;

impl DomainHandler for Audits {
    fn domain(&self) -> &'static str {
        "Audits"
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
                "enable" | "disable" => {
                    cx.target()?;
                    Ok(json!({}))
                }
                other => Err(CdpError::method_not_found(&format!("Audits.{other}"))),
            }
        })
    }
}
