//! The read side of `window.location`: the URL components the `Location`
//! interface reports
//! (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#the-location-interface>).
//!
//! The getters are the URL Standard's, so the `URL` interface answers through
//! [`LocationPart::read`] as well.

use url::Url;

/// One URL component, named as its attribute.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum LocationPart {
    /// `href`
    Href,
    /// `origin`
    Origin,
    /// `protocol`
    Protocol,
    /// `host`
    Host,
    /// `hostname`
    Hostname,
    /// `port`
    Port,
    /// `pathname`
    Pathname,
    /// `search`
    Search,
    /// `hash`
    Hash,
}

impl LocationPart {
    /// Every attribute, in the order the interface declares them.
    pub const ALL: [LocationPart; 9] = [
        LocationPart::Href,
        LocationPart::Origin,
        LocationPart::Protocol,
        LocationPart::Host,
        LocationPart::Hostname,
        LocationPart::Port,
        LocationPart::Pathname,
        LocationPart::Search,
        LocationPart::Hash,
    ];

    /// The attribute's name.
    pub fn name(self) -> &'static str {
        match self {
            LocationPart::Href => "href",
            LocationPart::Origin => "origin",
            LocationPart::Protocol => "protocol",
            LocationPart::Host => "host",
            LocationPart::Hostname => "hostname",
            LocationPart::Port => "port",
            LocationPart::Pathname => "pathname",
            LocationPart::Search => "search",
            LocationPart::Hash => "hash",
        }
    }

    /// The URL Standard's getter for this component of `url`.
    pub fn read(self, url: &Url) -> String {
        match self {
            LocationPart::Href => url.as_str().to_owned(),
            // An opaque origin serializes as `"null"`.
            LocationPart::Origin => url.origin().ascii_serialization(),
            // The parser keeps the delimiter out of `scheme()`.
            LocationPart::Protocol => format!("{}:", url.scheme()),
            LocationPart::Host => {
                let host = url.host_str().unwrap_or("");
                match url.port() {
                    Some(port) => format!("{host}:{port}"),
                    None => host.to_owned(),
                }
            }
            LocationPart::Hostname => url.host_str().unwrap_or("").to_owned(),
            LocationPart::Port => url.port().map(|port| port.to_string()).unwrap_or_default(),
            LocationPart::Pathname => url.path().to_owned(),
            // A null and an empty query both read "": a bare `?` is not part
            // of `search`, and a bare `#` is not part of `hash`.
            LocationPart::Search => match url.query() {
                Some(query) if !query.is_empty() => format!("?{query}"),
                _ => String::new(),
            },
            LocationPart::Hash => match url.fragment() {
                Some(fragment) if !fragment.is_empty() => format!("#{fragment}"),
                _ => String::new(),
            },
        }
    }
}

impl LocationPart {
    /// The URL Standard's setter for this component, applied to `url`; `href`
    /// and `origin` have none here (`href` is parsed against a base URL by
    /// the caller, `origin` is read-only). A value the setter refuses leaves
    /// `url` as it was, as the standard says.
    pub fn write(self, url: &mut Url, value: &str) {
        use url::quirks;
        match self {
            LocationPart::Href | LocationPart::Origin => {}
            LocationPart::Protocol => {
                let _ = quirks::set_protocol(url, value);
            }
            LocationPart::Host => {
                let _ = quirks::set_host(url, value);
            }
            LocationPart::Hostname => {
                let _ = quirks::set_hostname(url, value);
            }
            LocationPart::Port => {
                let _ = quirks::set_port(url, value);
            }
            LocationPart::Pathname => quirks::set_pathname(url, value),
            LocationPart::Search => quirks::set_search(url, value),
            LocationPart::Hash => quirks::set_hash(url, value),
        }
    }
}

/// What a navigation started through `location` or an activated link did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Navigated {
    /// The page was asked to load another document.
    Requested,
    /// The target differed from the document's URL in its fragment at most:
    /// the document stays and its URL is now `new_url`.
    Fragment { old_url: String, new_url: String },
    /// The target is not one a page may navigate itself to (`javascript:`).
    Ignored,
}

/// Whether navigating a document at `current` to `target` stays in the
/// document: the URLs are equal apart from their fragments and the target
/// has one
/// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#navigate>, the
/// fragment step).
pub fn is_fragment_navigation(current: &Url, target: &Url) -> bool {
    if target.fragment().is_none() {
        return false;
    }
    let (mut from, mut to) = (current.clone(), target.clone());
    from.set_fragment(None);
    to.set_fragment(None);
    from == to
}

/// The URL a document's `Location` reports: the document's own, or
/// `about:blank` for a document that has none.
pub fn document_location(url: Option<&Url>) -> Url {
    url.cloned().unwrap_or_else(|| {
        "about:blank"
            .parse()
            .expect("`about:blank` is a valid URL in every URL Standard")
    })
}

#[cfg(test)]
mod navigation_tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).expect("a URL")
    }

    #[test]
    fn only_a_fragment_change_stays_in_the_document() {
        let page = url("https://example.com/app?x=1#top");
        assert!(is_fragment_navigation(
            &page,
            &url("https://example.com/app?x=1#other")
        ));
        assert!(is_fragment_navigation(
            &page,
            &url("https://example.com/app?x=1#top")
        ));
        assert!(is_fragment_navigation(
            &page,
            &url("https://example.com/app?x=1#")
        ));
        assert!(!is_fragment_navigation(
            &page,
            &url("https://example.com/app?x=1")
        ));
        assert!(!is_fragment_navigation(
            &page,
            &url("https://example.com/app?x=2#top")
        ));
        assert!(!is_fragment_navigation(
            &page,
            &url("https://example.com/other#top")
        ));
    }

    #[test]
    fn component_setters_follow_the_url_standard() {
        let mut target = url("https://example.com/a?x=1#top");
        LocationPart::Hash.write(&mut target, "next");
        LocationPart::Search.write(&mut target, "?y=2");
        LocationPart::Pathname.write(&mut target, "b/c");
        assert_eq!(target.as_str(), "https://example.com/b/c?y=2#next");
        LocationPart::Port.write(&mut target, "8443");
        LocationPart::Hostname.write(&mut target, "other.example");
        assert_eq!(target.as_str(), "https://other.example:8443/b/c?y=2#next");
        // A refused value changes nothing.
        LocationPart::Port.write(&mut target, "not a port");
        LocationPart::Origin.write(&mut target, "https://evil.example");
        assert_eq!(target.as_str(), "https://other.example:8443/b/c?y=2#next");
    }
}

#[cfg(test)]
mod tests {
    use super::{LocationPart, document_location};

    fn parts(url: &str) -> Vec<String> {
        let url = url.parse().expect("url");
        LocationPart::ALL
            .into_iter()
            .map(|part| part.read(&url))
            .collect()
    }

    #[test]
    fn every_component_reads_as_the_url_standard_says() {
        assert_eq!(
            parts("https://user:pw@example.com:8443/a/b.html?x=1&y=2#top"),
            [
                "https://user:pw@example.com:8443/a/b.html?x=1&y=2#top",
                "https://example.com:8443",
                "https:",
                "example.com:8443",
                "example.com",
                "8443",
                "/a/b.html",
                "?x=1&y=2",
                "#top",
            ]
        );
    }

    #[test]
    fn default_ports_and_empty_delimiters_read_as_empty_strings() {
        assert_eq!(
            parts("http://example.com:80/?#"),
            [
                "http://example.com/?#",
                "http://example.com",
                "http:",
                "example.com",
                "example.com",
                "",
                "/",
                "",
                "",
            ]
        );
    }

    #[test]
    fn a_document_without_a_url_is_at_about_blank() {
        let url = document_location(None);
        assert_eq!(LocationPart::Href.read(&url), "about:blank");
        assert_eq!(LocationPart::Origin.read(&url), "null");
        assert_eq!(LocationPart::Protocol.read(&url), "about:");
        assert_eq!(LocationPart::Pathname.read(&url), "blank");
        assert_eq!(LocationPart::Host.read(&url), "");
    }

    #[test]
    fn names_are_the_attribute_names_in_declaration_order() {
        let names: Vec<_> = LocationPart::ALL
            .into_iter()
            .map(LocationPart::name)
            .collect();
        assert_eq!(
            names,
            [
                "href", "origin", "protocol", "host", "hostname", "port", "pathname", "search",
                "hash"
            ]
        );
    }
}
