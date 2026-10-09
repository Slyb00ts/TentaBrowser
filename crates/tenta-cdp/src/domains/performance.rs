//! The `Performance` domain, reduced to what `page.metrics()` and the
//! clients' start-up sequences need. Only counters the engine can state
//! truthfully are reported.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::domain::{CommandContext, CommandFuture, DomainHandler, DomainRegistry};
use crate::error::CdpError;

pub(super) fn register(registry: &mut DomainRegistry) {
    registry.register(Performance);
}

struct Performance;

impl DomainHandler for Performance {
    fn domain(&self) -> &'static str {
        "Performance"
    }

    fn methods(&self) -> &'static [&'static str] {
        &["enable", "disable", "getMetrics"]
    }

    fn handle<'a>(
        &'a self,
        cx: CommandContext,
        method: &'a str,
        _params: Value,
    ) -> CommandFuture<'a> {
        Box::pin(async move {
            match method {
                "enable" | "disable" => Ok(json!({})),
                "getMetrics" => {
                    cx.target()?;
                    let seconds = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_or(0.0, |elapsed| elapsed.as_secs_f64());
                    Ok(json!({ "metrics": [
                        { "name": "Timestamp", "value": seconds },
                        { "name": "Documents", "value": 1 },
                        { "name": "Frames", "value": 1 },
                    ]}))
                }
                other => Err(CdpError::method_not_found(&format!("Performance.{other}"))),
            }
        })
    }
}
