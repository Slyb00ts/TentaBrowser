//! Engine tiers (`docs/spec/02-architecture.md` §1). A page declares what
//! it needs and nothing above that is initialised.

use std::fmt;

#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum Tier {
    /// HTTP, TLS with fingerprint, cookies, cache, proxies, DNS.
    Net = 0,
    /// Streaming HTML parse into the arena DOM, selectors, extraction.
    Dom = 1,
    /// JavaScript engine, bindings, event loop, CSSOM, fetch, workers.
    Js = 2,
    /// Style, layout, geometry, paint, screenshots, PDF, screencast.
    Render = 3,
    /// Web Audio, media elements, MSE, MediaStream, WebRTC.
    Media = 4,
    /// CLI, CDP server, MCP server, FFI.
    Host = 5,
}

impl Tier {
    pub const ALL: [Tier; 6] = [
        Tier::Net,
        Tier::Dom,
        Tier::Js,
        Tier::Render,
        Tier::Media,
        Tier::Host,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Tier::Net => "net",
            Tier::Dom => "dom",
            Tier::Js => "js",
            Tier::Render => "render",
            Tier::Media => "media",
            Tier::Host => "host",
        }
    }

    pub fn from_name(name: &str) -> Option<Tier> {
        Tier::ALL.into_iter().find(|t| t.name() == name)
    }

    /// Checks `self` against a ceiling, producing the public error.
    pub fn check(self, ceiling: Tier) -> crate::Result<()> {
        if self <= ceiling {
            Ok(())
        } else {
            Err(crate::Error::TierDenied {
                needed: self,
                ceiling,
            })
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_are_ordered() {
        assert!(Tier::Net < Tier::Dom && Tier::Dom < Tier::Js && Tier::Js < Tier::Render);
        assert!(Tier::Render < Tier::Media && Tier::Media < Tier::Host);
        assert!(Tier::Js.check(Tier::Media).is_ok());
        assert!(Tier::Media.check(Tier::Dom).is_err());
        assert_eq!(Tier::from_name("render"), Some(Tier::Render));
        assert_eq!(Tier::from_name("gpu"), None);
    }
}
