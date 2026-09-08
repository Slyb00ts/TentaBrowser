//! Which subresource hints a tier issues and which it only records
//! (`docs/spec/02-architecture.md` §1.2).
//!
//! The preload scanner records every hint the markup contains. At Tier 1
//! nothing but the document (and its redirects) is issued: `search` plans and
//! `fetch --dump` never fetch scripts, styles, fonts or images. A recorded
//! hint produces no request and no event; it is replayed in priority order
//! when the page is upgraded to a tier that issues it.

use tenta_core::Tier;
use tenta_net::ResourceType;
use url::Url;

// Re-exported so a host records hints without depending on `tenta-html`.
pub use tenta_html::{PreloadAttrs, PreloadHint, PreloadKind};

/// What the current tier does with a hint.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HintDisposition {
    /// The tier fetches it now.
    Issue,
    /// The tier records it for a later upgrade.
    Record,
    /// Not a fetch at all (`<base href>`, `rel=preconnect`).
    NotAFetch,
}

/// The per-tier fetch policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FetchPolicy {
    tier: Tier,
    /// Images before the first layout, in document order, that Tier 3 issues.
    first_images: u32,
}

impl FetchPolicy {
    pub const DEFAULT_FIRST_IMAGES: u32 = 20;

    pub fn for_tier(tier: Tier) -> Self {
        Self {
            tier,
            first_images: Self::DEFAULT_FIRST_IMAGES,
        }
    }

    pub fn tier(&self) -> Tier {
        self.tier
    }

    /// Images the tier issues before the first layout.
    pub fn first_images(&self) -> u32 {
        self.first_images
    }

    /// What this tier does with `kind`.
    pub fn disposition(&self, kind: PreloadKind) -> HintDisposition {
        match kind {
            PreloadKind::BaseChanged | PreloadKind::Preconnect => HintDisposition::NotAFetch,
            PreloadKind::Script | PreloadKind::Module | PreloadKind::Fetch | PreloadKind::Style => {
                self.issue_at(Tier::Js)
            }
            PreloadKind::Font | PreloadKind::Image => self.issue_at(Tier::Render),
        }
    }

    fn issue_at(&self, needed: Tier) -> HintDisposition {
        if self.tier >= needed {
            HintDisposition::Issue
        } else {
            HintDisposition::Record
        }
    }
}

/// A hint the page saw, with the URL resolved against the document's base and
/// the decision this page's tier made about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedHint {
    pub kind: PreloadKind,
    /// The attribute value as it appeared in the markup.
    pub raw_url: String,
    /// The absolute URL, when it resolved against the document base.
    pub url: Option<Url>,
    pub disposition: HintDisposition,
    /// The `ResourceType` the request would carry once it is issued.
    pub resource_type: ResourceType,
    /// The raw `srcset`, kept so a later tier can re-select with a viewport.
    pub srcset: Option<String>,
    pub r#async: bool,
    pub defer: bool,
}

impl RecordedHint {
    /// Records `hint` under `policy`, resolving its URL against `base`.
    pub fn from_hint(hint: &PreloadHint, base: Option<&Url>, policy: &FetchPolicy) -> Self {
        Self {
            kind: hint.kind,
            raw_url: hint.url.clone(),
            url: base.and_then(|base| base.join(&hint.url).ok()),
            disposition: policy.disposition(hint.kind),
            resource_type: resource_type_for(hint.kind),
            srcset: hint.attrs.srcset.clone(),
            r#async: hint.attrs.r#async,
            defer: hint.attrs.defer,
        }
    }

    /// Whether this hint was recorded rather than fetched.
    pub fn is_recorded_only(&self) -> bool {
        self.disposition == HintDisposition::Record
    }
}

/// The transport's resource type for a hint kind, which decides its priority.
pub fn resource_type_for(kind: PreloadKind) -> ResourceType {
    match kind {
        PreloadKind::Script | PreloadKind::Module => ResourceType::Script,
        PreloadKind::Style => ResourceType::Stylesheet,
        PreloadKind::Image => ResourceType::Image,
        PreloadKind::Font => ResourceType::Font,
        PreloadKind::Fetch => ResourceType::Fetch,
        PreloadKind::Preconnect | PreloadKind::BaseChanged => ResourceType::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint(kind: PreloadKind, url: &str) -> PreloadHint {
        PreloadHint {
            kind,
            url: url.to_owned(),
            attrs: PreloadAttrs::default(),
        }
    }

    #[test]
    fn tier_one_issues_nothing_but_the_document() {
        let policy = FetchPolicy::for_tier(Tier::Dom);
        for kind in [
            PreloadKind::Script,
            PreloadKind::Module,
            PreloadKind::Style,
            PreloadKind::Fetch,
            PreloadKind::Image,
            PreloadKind::Font,
        ] {
            assert_eq!(
                policy.disposition(kind),
                HintDisposition::Record,
                "{kind:?} must only be recorded at Tier 1"
            );
        }
        assert_eq!(
            policy.disposition(PreloadKind::BaseChanged),
            HintDisposition::NotAFetch
        );
        assert_eq!(
            policy.disposition(PreloadKind::Preconnect),
            HintDisposition::NotAFetch
        );
    }

    #[test]
    fn tier_two_issues_scripts_and_styles_but_not_images_or_fonts() {
        let policy = FetchPolicy::for_tier(Tier::Js);
        assert_eq!(
            policy.disposition(PreloadKind::Script),
            HintDisposition::Issue
        );
        assert_eq!(
            policy.disposition(PreloadKind::Style),
            HintDisposition::Issue
        );
        assert_eq!(
            policy.disposition(PreloadKind::Fetch),
            HintDisposition::Issue
        );
        assert_eq!(
            policy.disposition(PreloadKind::Image),
            HintDisposition::Record
        );
        assert_eq!(
            policy.disposition(PreloadKind::Font),
            HintDisposition::Record
        );
    }

    #[test]
    fn tier_three_issues_fonts_and_images() {
        let policy = FetchPolicy::for_tier(Tier::Render);
        assert_eq!(
            policy.disposition(PreloadKind::Font),
            HintDisposition::Issue
        );
        assert_eq!(
            policy.disposition(PreloadKind::Image),
            HintDisposition::Issue
        );
        assert_eq!(policy.first_images(), 20);
        assert_eq!(policy.tier(), Tier::Render);
    }

    #[test]
    fn a_recorded_hint_resolves_its_url_and_keeps_the_markup_value() {
        let base = Url::parse("http://example.test/dir/page.html").expect("base");
        let policy = FetchPolicy::for_tier(Tier::Dom);
        let recorded = RecordedHint::from_hint(
            &hint(PreloadKind::Image, "../img/a.png"),
            Some(&base),
            &policy,
        );
        assert_eq!(recorded.raw_url, "../img/a.png");
        assert_eq!(
            recorded.url.as_ref().map(Url::as_str),
            Some("http://example.test/img/a.png")
        );
        assert!(recorded.is_recorded_only());
        assert_eq!(recorded.resource_type, ResourceType::Image);
    }

    #[test]
    fn a_hint_without_a_base_keeps_its_raw_url() {
        let policy = FetchPolicy::for_tier(Tier::Dom);
        let recorded = RecordedHint::from_hint(&hint(PreloadKind::Script, "/a.js"), None, &policy);
        assert!(recorded.url.is_none());
        assert_eq!(recorded.raw_url, "/a.js");
        assert_eq!(recorded.resource_type, ResourceType::Script);
    }
}
