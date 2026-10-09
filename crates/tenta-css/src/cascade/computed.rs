//! Serialisation of computed values.

use tenta_core::NodeId;

use super::engine::StyleEngine;
use super::types::PseudoKind;

impl StyleEngine {
    /// The computed value of `property` for `node` (or one of its pseudo
    /// elements) as CSS text, the form `getComputedStyle` reports before
    /// layout-dependent fix-ups.
    pub fn computed_value(
        &self,
        node: NodeId,
        pseudo: Option<PseudoKind>,
        property: &str,
    ) -> Option<String> {
        let _ = (node, pseudo, property);
        todo!("C2c")
    }
}
