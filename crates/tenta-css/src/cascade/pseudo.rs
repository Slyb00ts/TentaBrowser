//! Pseudo-element styles.

use tenta_core::NodeId;

use super::engine::StyleEngine;
use super::types::{PseudoKind, Style};

impl StyleEngine {
    /// The style of a pseudo-element of `node`, when the cascade generates
    /// one. `::first-line` and `::first-letter` return `None`.
    pub fn pseudo_style(&self, node: NodeId, pseudo: PseudoKind) -> Option<Style> {
        let _ = (node, pseudo);
        todo!("C2c")
    }
}
