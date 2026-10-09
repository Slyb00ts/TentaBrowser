//! The `Security` domain: certificate-error handling.

use std::sync::atomic::Ordering;

use serde::Deserialize;
use serde_json::{Value, json};

use super::target::IgnoreCertificateErrors;
use crate::domain::{CommandContext, CommandFuture, DomainHandler, DomainRegistry, parse_params};
use crate::error::CdpError;

pub(super) fn register(registry: &mut DomainRegistry) {
    registry.register(Security);
}

struct Security;

#[derive(Deserialize)]
struct SetIgnoreCertificateErrors {
    ignore: bool,
}

impl DomainHandler for Security {
    fn domain(&self) -> &'static str {
        "Security"
    }

    fn methods(&self) -> &'static [&'static str] {
        &["enable", "disable", "setIgnoreCertificateErrors"]
    }

    fn handle<'a>(
        &'a self,
        cx: CommandContext,
        method: &'a str,
        params: Value,
    ) -> CommandFuture<'a> {
        Box::pin(async move {
            match method {
                // No security-state events exist yet.
                "enable" | "disable" => Ok(json!({})),
                // Known limitation: a built context cannot change its TLS
                // verification, so the flag reaches only the contexts this
                // connection creates afterwards. The default and existing
                // contexts stay verified, which fails closed. The command
                // still succeeds because both clients await it without a
                // catch, and an error would fail the connect.
                "setIgnoreCertificateErrors" => {
                    let SetIgnoreCertificateErrors { ignore } = parse_params(params)?;
                    cx.connection()
                        .root()
                        .state::<IgnoreCertificateErrors>()
                        .0
                        .store(ignore, Ordering::Release);
                    Ok(json!({}))
                }
                other => Err(CdpError::method_not_found(&format!("Security.{other}"))),
            }
        })
    }
}
