//! The `Network` domain, reduced to the enable handshake every client opens a
//! page with. No request events are produced yet: `Network.enable` succeeds
//! and the page stays silent, so a client that waits for `requestWillBeSent`
//! waits in vain until the Network package (D4b) lands. Every other command
//! is answered `-32601`.

use serde_json::{Value, json};

use crate::domain::{CommandContext, CommandFuture, DomainHandler, DomainRegistry};
use crate::error::CdpError;

pub(super) fn register(registry: &mut DomainRegistry) {
    registry.register(Network);
}

struct Network;

impl DomainHandler for Network {
    fn domain(&self) -> &'static str {
        "Network"
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
                other => Err(CdpError::method_not_found(&format!("Network.{other}"))),
            }
        })
    }
}
