//! The URL side of `history.pushState` and `history.replaceState`
//! (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#shared-history-push/replace-state-steps>).
//!
//! The entries themselves hold script values, so each engine keeps them in
//! its own heap; what is engine-independent is which URL a document may be
//! rewritten to.

use url::Url;

/// Why `pushState` or `replaceState` refused its URL argument. Both are a
/// `SecurityError` to script.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum RewriteError {
    /// The argument does not parse against the document's base URL.
    Unparsable,
    /// The URL differs from the document's in a component a same-document
    /// navigation may not change.
    CrossOrigin,
}

impl RewriteError {
    /// The message of the `SecurityError`.
    pub fn message(self) -> &'static str {
        match self {
            RewriteError::Unparsable => "The URL is not valid.",
            RewriteError::CrossOrigin => {
                "A history state object with that URL cannot be created in a document with this origin."
            }
        }
    }
}

/// Whether a document at `document` can have its URL rewritten to `target`
/// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#can-have-its-url-rewritten>).
pub fn can_rewrite(document: &Url, target: &Url) -> bool {
    if document.scheme() != target.scheme()
        || document.username() != target.username()
        || document.password() != target.password()
        || document.host() != target.host()
        || document.port() != target.port()
    {
        return false;
    }
    match target.scheme() {
        "http" | "https" => true,
        "file" => document.path() == target.path(),
        _ => document.path() == target.path() && document.query() == target.query(),
    }
}

/// The URL a `pushState` or `replaceState` call rewrites the document to:
/// `input` parsed against `base`, checked against the document's URL.
pub fn rewritten_url(document: &Url, base: &Url, input: &str) -> Result<Url, RewriteError> {
    let target = base.join(input).map_err(|_| RewriteError::Unparsable)?;
    if can_rewrite(document, &target) {
        Ok(target)
    } else {
        Err(RewriteError::CrossOrigin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).expect("a URL")
    }

    #[test]
    fn http_documents_may_change_path_query_and_fragment_only() {
        let document = url("https://example.com/app/list?page=1#top");
        for (input, expected) in [
            ("/other", "https://example.com/other"),
            ("item/7", "https://example.com/app/item/7"),
            ("?page=2", "https://example.com/app/list?page=2"),
            ("#bottom", "https://example.com/app/list?page=1#bottom"),
            ("https://example.com/x", "https://example.com/x"),
        ] {
            assert_eq!(
                rewritten_url(&document, &document, input),
                Ok(url(expected)),
                "{input}"
            );
        }
        for input in [
            "https://other.example/app",
            "http://example.com/app",
            "https://example.com:8443/app",
            "https://user@example.com/app",
            "//other.example/app",
        ] {
            assert_eq!(
                rewritten_url(&document, &document, input),
                Err(RewriteError::CrossOrigin),
                "{input}"
            );
        }
        assert_eq!(
            rewritten_url(&document, &document, "https://exa mple.com/"),
            Err(RewriteError::Unparsable)
        );
    }

    #[test]
    fn the_base_url_resolves_the_argument_and_the_document_url_judges_it() {
        let document = url("https://example.com/app/");
        let base = url("https://example.com/static/");
        assert_eq!(
            rewritten_url(&document, &base, "page"),
            Ok(url("https://example.com/static/page"))
        );
        let foreign_base = url("https://cdn.example/");
        assert_eq!(
            rewritten_url(&document, &foreign_base, "page"),
            Err(RewriteError::CrossOrigin)
        );
    }

    #[test]
    fn other_schemes_may_change_less() {
        let file = url("file:///home/user/page.html");
        assert!(can_rewrite(&file, &url("file:///home/user/page.html#part")));
        assert!(can_rewrite(&file, &url("file:///home/user/page.html?q=1")));
        assert!(!can_rewrite(&file, &url("file:///home/user/other.html")));

        let blank = url("about:blank");
        assert!(can_rewrite(&blank, &url("about:blank#part")));
        assert!(!can_rewrite(&blank, &url("about:blank?q=1")));
        assert!(!can_rewrite(&blank, &url("about:srcdoc")));
    }
}
