//! The error document a failed navigation leaves behind
//! (`docs/spec/20-platform-behaviour.md` §1).
//!
//! Chromium replaces the previous document with an internal error page at
//! `chrome-error://chromewebdata/` carrying the `net::ERR_*` name, and
//! Playwright's clients depend on that shape. Ours lives at
//! [`ERROR_URL`] and carries the same name.

use tenta_dom::Document as DomDocument;
use tenta_html::{Parser, ParserOptions};
use url::Url;

/// Where a failed navigation's document lives.
pub const ERROR_URL: &str = "tenta-error://tentawebdata/";

/// Builds the error document for a failed navigation of `failed_url`.
///
/// `name` is the `net::ERR_*` name; `message` is the transport's detail.
pub fn error_document(failed_url: &Url, name: &str, message: &str) -> DomDocument {
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{name}</title></head>\
         <body><h1>{name}</h1><p>{message}</p><p>{url}</p></body></html>",
        name = escape(name),
        message = escape(message),
        url = escape(failed_url.as_str()),
    );
    let url = Url::parse(ERROR_URL).expect("the error URL is a constant");
    let mut document = DomDocument::with_url(url);
    document.set_content_type("text/html");
    document.set_encoding("UTF-8");
    let mut parser = Parser::new(
        document,
        ParserOptions {
            scripting_enabled: false,
            encoding: Some(tenta_html::Encoding::for_label(b"utf-8").expect("utf-8 is a label")),
            ..ParserOptions::default()
        },
    );
    let _ = parser.feed_str(&html);
    parser.finish()
}

/// Whether `url` is an error document's address.
pub fn is_error_url(url: &Url) -> bool {
    url.scheme() == "tenta-error"
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_error_document_names_the_chromium_error() {
        let failed = Url::parse("http://127.0.0.1:9/gone").expect("url");
        let document = error_document(
            &failed,
            "net::ERR_CONNECTION_REFUSED",
            "connection refused by 127.0.0.1:9",
        );
        assert_eq!(document.url().map(Url::as_str), Some(ERROR_URL));
        assert!(is_error_url(document.url().expect("url")));
        let text = document.text();
        assert!(
            text.contains("net::ERR_CONNECTION_REFUSED"),
            "text was {text:?}"
        );
        assert!(text.contains("http://127.0.0.1:9/gone"));
        assert_eq!(
            document.metadata().title.as_deref(),
            Some("net::ERR_CONNECTION_REFUSED")
        );
    }

    #[test]
    fn markup_in_the_message_cannot_escape_into_the_document() {
        let failed = Url::parse("http://example.test/").expect("url");
        let document = error_document(&failed, "net::ERR_FAILED", "<script>boom()</script>");
        assert!(
            document
                .query_selector(document.document_node(), "script")
                .expect("valid selector")
                .is_none(),
            "the message must not become markup"
        );
        assert!(document.text().contains("<script>boom()</script>"));
    }
}
