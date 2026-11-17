//! Helpers for tests in other crates, behind the `test-util` feature.

use std::sync::Arc;

use tenta_core::atom::atoms;
use tenta_dom::Document;
use tenta_html::{Parser, ParserOptions};
use url::Url;

use super::engine::StyleEngine;
use super::types::{FontMetrics, FontMetricsData, StyleEnv, StyleSource};

/// Proportions of a typical text face, the same at every size and for every
/// family: what tests get when no font system is involved.
#[derive(Debug, Default)]
pub struct FixedRatioMetrics;

impl FontMetrics for FixedRatioMetrics {
    fn metrics(&self, _: &str, _: u16, _: bool, size_px: f32) -> FontMetricsData {
        FontMetricsData {
            x_height: Some(size_px * 0.5),
            zero_advance: Some(size_px * 0.5),
            cap_height: Some(size_px * 0.7),
            ic_width: Some(size_px),
            ascent: size_px * 0.9,
            script_percent_scale_down: None,
            script_script_percent_scale_down: None,
        }
    }
}

/// Parses `html` into a document and returns it with a style engine flushed
/// for `env`, using fixed-ratio font metrics.
///
/// The document's `<style>` elements become the engine's author sheets, in
/// tree order, with their `media` attribute, and one inside a shadow tree
/// becomes a sheet of that shadow root. `<template shadowrootmode>` declares
/// a shadow root, as in a navigated document. There is no network, so a test
/// that needs `@import` bodies installs its own sources with
/// [`StyleEngine::set_sources`]. The quirks mode is the document's, as in the
/// engine: the doctype overrides `env.quirks`.
pub fn style_html(html: &str, env: StyleEnv) -> (Document, StyleEngine) {
    let mut parser = Parser::new(
        Document::new(),
        ParserOptions {
            declarative_shadow_roots: true,
            ..ParserOptions::default()
        },
    );
    parser.feed_last(html.as_bytes());
    let doc = parser.finish();
    let mut engine = StyleEngine::new(env, Arc::new(FixedRatioMetrics));
    engine.set_sources(style_sources(&doc));
    engine.flush(&doc);
    (doc, engine)
}

fn style_sources(doc: &Document) -> Vec<StyleSource> {
    let base = doc
        .base_url()
        .or_else(|| doc.url())
        .cloned()
        .unwrap_or_else(|| Url::parse("about:blank").expect("about:blank is a valid URL"));
    doc.shadow_including_subtree(doc.document_node())
        .into_iter()
        .filter(|&style| {
            doc.tag(style) == Some(atoms::STYLE) && doc.namespace(style) == Some(atoms::NS_HTML)
        })
        .map(|style| StyleSource {
            owner: Some(style),
            scope: doc.containing_shadow_root(style),
            url: base.clone(),
            text: doc
                .children(style)
                .filter_map(|child| doc.text_data(child))
                .collect(),
            media: doc.attribute(style, atoms::MEDIA).map(str::to_owned),
            disabled: false,
            imports: Vec::new(),
        })
        .collect()
}
