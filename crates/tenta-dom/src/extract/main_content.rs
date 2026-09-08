//! Readability-style main-content extraction
//! (`docs/spec/04-dom-and-css.md` §5).
//!
//! Paragraph-like blocks are scored by text length and comma count, the score
//! flows up to the parent and half of it to the grandparent, and the winning
//! container is the one with the best score once link density is discounted.
//! Two iterative passes over the arena, no intermediate tree.

use hashbrown::HashMap;
use tenta_core::NodeId;
use tenta_core::atom::atoms;

use crate::document::Document;
use crate::extract::text::text;
use crate::extract::{Event, Walker, is_skipped, text_of};

/// The chosen container and why.
#[derive(Clone, PartialEq, Debug)]
pub struct MainContent {
    /// The element judged to hold the article body.
    pub node: NodeId,
    /// Its score after the link-density discount.
    pub score: f32,
    /// Characters of text below it.
    pub text_length: usize,
    /// Fraction of that text inside links, 0.0 to 1.0.
    pub link_density: f32,
    /// Block-formatted text of the container.
    pub text: String,
}

#[derive(Clone, Copy, Default)]
struct Metrics {
    text_len: usize,
    link_len: usize,
    commas: usize,
}

/// Scores the subtree and returns the best container, or `None` when there is
/// no text at all.
pub fn main_content(document: &Document, root: NodeId) -> Option<MainContent> {
    let mut metrics: HashMap<NodeId, Metrics> = HashMap::new();
    let mut skip_depth: Option<usize> = None;

    // Pass 1: text length, link text length and comma count per element,
    // accumulated on the way out so a child is finished before its parent.
    for event in Walker::new(document, root) {
        match event {
            Event::Enter { node, depth } => {
                if skip_depth.is_some() {
                    continue;
                }
                if is_skipped(document, node) {
                    skip_depth = Some(depth);
                    continue;
                }
                if let Some(chunk) = text_of(document, node) {
                    let parent = document.parent(node);
                    if parent.is_some() {
                        let entry = metrics.entry(parent).or_default();
                        entry.text_len += chunk.trim().len();
                        entry.commas += chunk.matches(',').count();
                    }
                }
            }
            Event::Exit { node, depth } => {
                if let Some(skipped) = skip_depth {
                    if depth == skipped {
                        skip_depth = None;
                    }
                    continue;
                }
                let own = metrics.get(&node).copied().unwrap_or_default();
                let parent = document.parent(node);
                if !parent.is_some() || node == root {
                    continue;
                }
                let is_link = document.tag(node) == Some(atoms::A)
                    && document.has_attribute(node, atoms::HREF);
                let entry = metrics.entry(parent).or_default();
                entry.text_len += own.text_len;
                entry.commas += own.commas;
                entry.link_len += if is_link { own.text_len } else { own.link_len };
            }
        }
    }

    // Pass 2: score paragraph-like blocks and propagate upwards.
    let mut scores: HashMap<NodeId, f32> = HashMap::new();
    for (node, metric) in metrics.iter() {
        let Some(tag) = document.tag(*node) else {
            continue;
        };
        let paragraph_like = tag == atoms::P
            || tag == atoms::TD
            || tag == atoms::PRE
            || tag == atoms::BLOCKQUOTE
            || tag == atoms::LI;
        if !paragraph_like || metric.text_len < 25 {
            continue;
        }
        let base = 1.0 + metric.commas as f32 + (metric.text_len as f32 / 100.0).min(3.0);
        let parent = document.parent(*node);
        if parent.is_some() {
            *scores.entry(parent).or_default() += base;
            let grandparent = document.parent(parent);
            if grandparent.is_some() {
                *scores.entry(grandparent).or_default() += base / 2.0;
            }
        }
    }

    let mut best: Option<MainContent> = None;
    for (node, raw) in scores {
        if !document.is_element(node) {
            continue;
        }
        let metric = metrics.get(&node).copied().unwrap_or_default();
        if metric.text_len == 0 {
            continue;
        }
        let link_density = metric.link_len as f32 / metric.text_len as f32;
        let score = raw * (1.0 - link_density.min(1.0));
        let tag = document.tag(node).unwrap_or(atoms::EMPTY);
        // <body> and <html> collect every paragraph in the page, so they win
        // on raw score without being the article; require a real container.
        let container_bonus = if tag == atoms::ARTICLE || tag == atoms::MAIN {
            1.25
        } else if tag == atoms::BODY || tag == atoms::HTML {
            0.5
        } else {
            1.0
        };
        let score = score * container_bonus;
        if best.as_ref().is_none_or(|current| score > current.score) {
            best = Some(MainContent {
                node,
                score,
                text_length: metric.text_len,
                link_density,
                text: String::new(),
            });
        }
    }
    let mut best = best?;
    best.text = text(document, best.node);
    Some(best)
}

impl Document {
    /// The main content of the document, if it has any.
    pub fn main_content(&self) -> Option<MainContent> {
        main_content(self, self.document_node())
    }
}
