//! Playwright's `storageState` JSON shape (`docs/spec/03-network.md` §6).
//!
//! The point is interoperability: a jar exported here must load in Playwright
//! and a Playwright export must load here, so the field names, the `-1`
//! sentinel for session cookies and the leading dot on domain cookies all
//! follow Playwright rather than being tidied up.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tenta_core::{Error, Result};

use super::{Cookie, SameSite};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StorageCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    /// Unix seconds, `-1` for a session cookie.
    pub expires: f64,
    #[serde(rename = "httpOnly")]
    pub http_only: bool,
    pub secure: bool,
    #[serde(rename = "sameSite")]
    pub same_site: String,
}

/// One origin's DOM storage. TentaBrowser fills this in from Tier 2 onwards;
/// the field exists now so that exports round-trip through Playwright.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StorageOrigin {
    pub origin: String,
    #[serde(rename = "localStorage", default)]
    pub local_storage: Vec<StorageItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StorageItem {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StorageState {
    #[serde(default)]
    pub cookies: Vec<StorageCookie>,
    #[serde(default)]
    pub origins: Vec<StorageOrigin>,
}

impl StorageState {
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|error| Error::Other {
            code: "storage_state_encode".into(),
            message: error.to_string(),
        })
    }

    pub fn to_json_pretty(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(|error| Error::Other {
            code: "storage_state_encode".into(),
            message: error.to_string(),
        })
    }

    pub fn from_json(text: &str) -> Result<StorageState> {
        serde_json::from_str(text).map_err(|error| Error::InvalidArgument {
            reason: format!("storage state is not valid JSON: {error}"),
        })
    }
}

impl StorageCookie {
    pub fn from_cookie(cookie: &Cookie) -> StorageCookie {
        StorageCookie {
            name: cookie.name.clone(),
            value: cookie.value.clone(),
            // Playwright marks a domain cookie with a leading dot and a
            // host-only cookie without one.
            domain: if cookie.host_only {
                cookie.domain.clone()
            } else {
                format!(".{}", cookie.domain)
            },
            path: cookie.path.clone(),
            expires: match cookie.expires {
                Some(expires) => expires
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs() as f64)
                    .unwrap_or(-1.0),
                None => -1.0,
            },
            http_only: cookie.http_only,
            secure: cookie.secure,
            same_site: cookie.same_site.as_str().to_string(),
        }
    }

    pub fn to_cookie(&self, now: SystemTime) -> Result<Cookie> {
        let host_only = !self.domain.starts_with('.');
        let domain = self
            .domain
            .trim_start_matches('.')
            .trim_end_matches('.')
            .to_ascii_lowercase();
        if domain.is_empty() {
            return Err(Error::InvalidArgument {
                reason: format!("storage state cookie {} has no domain", self.name),
            });
        }
        if self.name.is_empty() && self.value.is_empty() {
            return Err(Error::InvalidArgument {
                reason: "storage state cookie has neither a name nor a value".into(),
            });
        }
        let same_site = match self.same_site.as_str() {
            s if s.eq_ignore_ascii_case("strict") => SameSite::Strict,
            s if s.eq_ignore_ascii_case("none") => SameSite::None,
            _ => SameSite::Lax,
        };
        let expires = if self.expires < 0.0 || !self.expires.is_finite() {
            None
        } else {
            Some(UNIX_EPOCH + Duration::from_secs(self.expires as u64))
        };
        let path = if self.path.starts_with('/') {
            self.path.clone()
        } else {
            "/".to_string()
        };
        Ok(Cookie {
            name: self.name.clone(),
            value: self.value.clone(),
            domain,
            host_only,
            path,
            secure: self.secure,
            http_only: self.http_only,
            same_site,
            partition_key: None,
            expires,
            creation: now,
            last_access: now,
            persistent: expires.is_some(),
        })
    }
}
