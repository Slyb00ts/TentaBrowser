//! Helpers for tests in other crates, behind the `test-util` feature.

use tenta_dom::Document;

use super::engine::StyleEngine;
use super::types::StyleEnv;

/// Parses `html` into a document and returns it with a style engine flushed
/// for `env`, using fixed-ratio font metrics.
pub fn style_html(html: &str, env: StyleEnv) -> (Document, StyleEngine) {
    let _ = (html, env);
    todo!("C2a")
}
