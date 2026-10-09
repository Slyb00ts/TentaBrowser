//! The `Storage` domain: cookies of a browser context.
//!
//! The context's jar is the only cookie store, so these commands and the
//! requests of its pages see the same cookies.

use std::net::IpAddr;
use std::time::SystemTime;

use serde::Deserialize;
use serde_json::{Value, json};

use super::target::find_context;
use crate::domain::{CommandContext, CommandFuture, DomainHandler, DomainRegistry, parse_params};
use crate::error::CdpError;

pub(super) fn register(registry: &mut DomainRegistry) {
    registry.register(Storage);
}

struct Storage;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContextParams {
    browser_context_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetCookies {
    cookies: Vec<CookieParam>,
    browser_context_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CookieParam {
    name: String,
    value: String,
    url: Option<String>,
    domain: Option<String>,
    path: Option<String>,
    secure: Option<bool>,
    http_only: Option<bool>,
    same_site: Option<String>,
    /// Seconds since the epoch; absent or not positive is a session cookie.
    expires: Option<f64>,
}

/// The directory part of a request path, per RFC 6265 section 5.1.4.
fn default_path(path: &str) -> String {
    match path.rfind('/') {
        None | Some(0) => "/".to_owned(),
        Some(last) => path[..last].to_owned(),
    }
}

/// A cookie in the shape of the storage-state import, which validates it
/// and applies the whole batch or none of it.
fn storage_cookie(param: CookieParam) -> Result<Value, CdpError> {
    let url = param
        .url
        .as_deref()
        .filter(|url| !url.is_empty())
        .map(tenta_browser::context::parse_url)
        .transpose()
        .map_err(|error| CdpError::invalid_params(error.to_string()))?;
    let domain = match (param.domain.as_deref().filter(|d| !d.is_empty()), &url) {
        // A domain makes a cookie for the host and its subdomains, which the
        // import spells with a leading dot; an IP address has no subdomains.
        (Some(domain), _) if domain.starts_with('.') || domain.parse::<IpAddr>().is_ok() => {
            domain.to_owned()
        }
        (Some(domain), _) => format!(".{domain}"),
        (None, Some(url)) => url
            .host_str()
            .ok_or_else(|| CdpError::invalid_params("The url has no host"))?
            .to_owned(),
        (None, None) => {
            return Err(CdpError::invalid_params(
                "At least one of the url and domain needs to be specified",
            ));
        }
    };
    let path = match (param.path.as_deref().filter(|p| !p.is_empty()), &url) {
        (Some(path), _) => path.to_owned(),
        (None, Some(url)) => default_path(url.path()),
        (None, None) => "/".to_owned(),
    };
    let same_site = match param.same_site.as_deref() {
        None => "Lax",
        Some(value @ ("Strict" | "Lax" | "None")) => value,
        Some(other) => {
            return Err(CdpError::invalid_params(format!(
                "Unknown sameSite value: {other}"
            )));
        }
    };
    Ok(json!({
        "name": param.name,
        "value": param.value,
        "domain": domain,
        "path": path,
        "expires": param.expires.filter(|expires| *expires > 0.0).unwrap_or(-1.0),
        "httpOnly": param.http_only.unwrap_or(false),
        "secure": param.secure.unwrap_or(false),
        "sameSite": same_site,
    }))
}

impl DomainHandler for Storage {
    fn domain(&self) -> &'static str {
        "Storage"
    }

    fn methods(&self) -> &'static [&'static str] {
        &["getCookies", "setCookies", "clearCookies"]
    }

    fn handle<'a>(
        &'a self,
        cx: CommandContext,
        method: &'a str,
        params: Value,
    ) -> CommandFuture<'a> {
        Box::pin(async move {
            match method {
                "getCookies" => {
                    let ContextParams { browser_context_id } = parse_params(params)?;
                    let (_, context) = find_context(cx.backend(), browser_context_id.as_deref())?;
                    let jar = context.cookies();
                    jar.purge_expired(SystemTime::now());
                    let cookies: Vec<Value> = jar
                        .to_storage_state()
                        .cookies
                        .into_iter()
                        .map(|cookie| {
                            let secure = cookie.secure;
                            json!({
                                "size": cookie.name.len() + cookie.value.len(),
                                "name": cookie.name,
                                "value": cookie.value,
                                "domain": cookie.domain,
                                "path": cookie.path,
                                "expires": cookie.expires,
                                "httpOnly": cookie.http_only,
                                "secure": secure,
                                "session": cookie.expires < 0.0,
                                "sameSite": cookie.same_site,
                                "priority": "Medium",
                                "sameParty": false,
                                "sourceScheme": if secure { "Secure" } else { "NonSecure" },
                                "sourcePort": if secure { 443 } else { 80 },
                            })
                        })
                        .collect();
                    Ok(json!({ "cookies": cookies }))
                }
                "setCookies" => {
                    let SetCookies {
                        cookies,
                        browser_context_id,
                    } = parse_params(params)?;
                    let (_, context) = find_context(cx.backend(), browser_context_id.as_deref())?;
                    let cookies = cookies
                        .into_iter()
                        .map(storage_cookie)
                        .collect::<Result<Vec<_>, _>>()?;
                    let state = serde_json::from_value(json!({ "cookies": cookies }))
                        .map_err(|error| CdpError::invalid_params(error.to_string()))?;
                    context.set_storage_state(&state).map_err(|error| {
                        CdpError::server("Invalid cookie fields").with_data(error.to_string())
                    })?;
                    Ok(json!({}))
                }
                "clearCookies" => {
                    let ContextParams { browser_context_id } = parse_params(params)?;
                    let (_, context) = find_context(cx.backend(), browser_context_id.as_deref())?;
                    context.cookies().clear();
                    Ok(json!({}))
                }
                other => Err(CdpError::method_not_found(&format!("Storage.{other}"))),
            }
        })
    }
}
