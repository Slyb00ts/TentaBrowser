//! The `Browser` domain.

use serde_json::{Value, json};

use crate::domain::{CommandContext, CommandFuture, DomainHandler, DomainRegistry};
use crate::error::CdpError;

pub(super) fn register(registry: &mut DomainRegistry) {
    registry.register(Browser);
}

struct Browser;

impl DomainHandler for Browser {
    fn domain(&self) -> &'static str {
        "Browser"
    }

    fn methods(&self) -> &'static [&'static str] {
        &["getVersion"]
    }

    fn handle<'a>(
        &'a self,
        cx: CommandContext,
        method: &'a str,
        _params: Value,
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
                other => Err(CdpError::method_not_found(&format!("Browser.{other}"))),
            }
        })
    }
}
