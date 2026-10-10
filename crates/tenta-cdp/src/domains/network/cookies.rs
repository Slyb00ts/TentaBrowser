//! The cookie commands of `Network`, over the browser context's jar: the same
//! store `Storage` and the page's requests use.

use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;

use crate::domain::{CommandContext, parse_params};
use crate::domains::storage::{CookieParam, INVALID_COOKIE, apply_cookies, cookie_values};
use crate::error::CdpError;

#[derive(Deserialize)]
struct GetCookies {
    urls: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct SetCookies {
    cookies: Vec<CookieParam>,
}

#[derive(Deserialize)]
struct DeleteCookies {
    name: String,
    url: Option<String>,
    domain: Option<String>,
    path: Option<String>,
}

fn parse_url(text: &str) -> Result<Url, CdpError> {
    Url::parse(text).map_err(|_| CdpError::invalid_params(format!("Invalid URL: {text}")))
}

/// RFC 6265 section 5.1.3: a domain cookie (leading dot here) matches its host
/// and subdomains, a host-only cookie only its host.
fn domain_matches(host: &str, cookie_domain: &str) -> bool {
    match cookie_domain.strip_prefix('.') {
        Some(domain) => host == domain || host.ends_with(&format!(".{domain}")),
        None => host == cookie_domain,
    }
}

/// RFC 6265 section 5.1.4.
fn path_matches(request_path: &str, cookie_path: &str) -> bool {
    request_path == cookie_path
        || (request_path.starts_with(cookie_path)
            && (cookie_path.ends_with('/') || request_path[cookie_path.len()..].starts_with('/')))
}

fn applies_to(url: &Url, domain: &str, path: &str, secure: bool) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let request_path = if url.path().is_empty() {
        "/"
    } else {
        url.path()
    };
    domain_matches(&host.to_ascii_lowercase(), domain)
        && path_matches(request_path, path)
        && (!secure || url.scheme() == "https")
}

pub(super) fn handle(cx: &CommandContext, method: &str, params: Value) -> Result<Value, CdpError> {
    let target = cx.target()?;
    let context = cx
        .backend()
        .context(target.context_id())
        .ok_or_else(|| CdpError::server("The target's browser context is gone"))?;
    match method {
        "getAllCookies" => Ok(json!({ "cookies": cookie_values(&context, |_, _, _| true) })),
        "getCookies" => {
            let GetCookies { urls } = parse_params(params)?;
            let urls = match urls {
                Some(urls) => urls,
                None => vec![target.url()],
            };
            let urls = urls
                .iter()
                .map(|url| parse_url(url))
                .collect::<Result<Vec<_>, _>>()?;
            let cookies = cookie_values(&context, |domain, path, secure| {
                urls.iter().any(|url| applies_to(url, domain, path, secure))
            });
            Ok(json!({ "cookies": cookies }))
        }
        "setCookie" => {
            let cookie: CookieParam = parse_params(params)?;
            match apply_cookies(&context, vec![cookie]) {
                Ok(()) => Ok(json!({ "success": true })),
                // A refused cookie is not a protocol error for this command.
                Err(error) if error.message == INVALID_COOKIE => Ok(json!({ "success": false })),
                Err(error) => Err(error),
            }
        }
        "setCookies" => {
            let SetCookies { cookies } = parse_params(params)?;
            apply_cookies(&context, cookies)?;
            Ok(json!({}))
        }
        "deleteCookies" => {
            let DeleteCookies {
                name,
                url,
                domain,
                path,
            } = parse_params(params)?;
            let url = url.as_deref().map(parse_url).transpose()?;
            let doomed = cookie_values(&context, |cookie_domain, cookie_path, secure| {
                // The domain is the cookie's exactly: `example.com` names a
                // host-only cookie and `.example.com` a domain cookie.
                domain
                    .as_deref()
                    .is_none_or(|wanted| cookie_domain == wanted)
                    && path.as_deref().is_none_or(|wanted| wanted == cookie_path)
                    && url
                        .as_ref()
                        .is_none_or(|url| applies_to(url, cookie_domain, cookie_path, secure))
            });
            let jar = context.cookies();
            for cookie in doomed {
                if cookie["name"] == name.as_str()
                    && let (Some(domain), Some(path)) =
                        (cookie["domain"].as_str(), cookie["path"].as_str())
                {
                    jar.remove(&name, domain.trim_start_matches('.'), path);
                }
            }
            Ok(json!({}))
        }
        "clearBrowserCookies" => {
            context.cookies().clear();
            Ok(json!({}))
        }
        other => Err(CdpError::method_not_found(&format!("Network.{other}"))),
    }
}
