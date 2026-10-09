//! The style engine: owns sheets, the lock, the style store and the flush.

use std::sync::Arc;

use style::shared_lock::SharedRwLock;
use tenta_core::NodeId;
use tenta_dom::Document;

use super::atoms::AtomMaps;
use super::dom::StyleStore;
use super::types::{FontMetrics, Style, StyleDiff, StyleEnv, StyleSource};
use super::ua;

/// Computes and holds one [`Style`] per element of a document.
///
/// The engine is single-threaded and lives on the page thread. It borrows the
/// document only for the duration of [`StyleEngine::flush`]. Its state is
/// visible to the sibling modules of the cascade, so pseudo-element and
/// computed-value queries read it without editing this file.
pub struct StyleEngine {
    pub(super) env: StyleEnv,
    pub(super) metrics: Arc<dyn FontMetrics>,
    pub(super) sources: Vec<StyleSource>,
    pub(super) lock: SharedRwLock,
    pub(super) store: StyleStore,
    pub(super) atoms: AtomMaps,
    /// Document generation of the last completed flush; `None` forces the
    /// next flush to restyle.
    pub(super) flushed: Option<u64>,
}

impl StyleEngine {
    /// An engine for `env` that answers `ex`/`ch` queries through `metrics`.
    pub fn new(env: StyleEnv, metrics: Arc<dyn FontMetrics>) -> Self {
        Self {
            env,
            metrics,
            sources: Vec::new(),
            lock: SharedRwLock::new(),
            store: StyleStore::new(),
            atoms: AtomMaps::new(),
            flushed: None,
        }
    }

    /// The environment styles are computed for.
    pub fn env(&self) -> &StyleEnv {
        &self.env
    }

    /// Changes the environment and invalidates every style.
    pub fn set_env(&mut self, env: StyleEnv) {
        self.env = env;
        self.flushed = None;
    }

    /// The font metrics source.
    pub fn font_metrics(&self) -> &Arc<dyn FontMetrics> {
        &self.metrics
    }

    /// The only lock in the style system. Sheets, `style=""` blocks and the
    /// document handed to Stylo must all be created against it.
    pub fn lock(&self) -> &SharedRwLock {
        &self.lock
    }

    /// The author sheets in document order.
    pub fn sources(&self) -> &[StyleSource] {
        &self.sources
    }

    /// Replaces the author sheets and invalidates every style. The engine
    /// parses them on the next flush.
    pub fn set_sources(&mut self, sources: Vec<StyleSource>) {
        self.sources = sources;
        self.flushed = None;
    }

    /// Brings styles up to date with `doc`.
    ///
    /// Idempotent while `doc.mutation_generation()`, the environment and the
    /// sources are unchanged: the call then returns [`StyleDiff::None`] without
    /// traversing. Otherwise it restyles the whole tree and returns
    /// [`StyleDiff::Full`].
    ///
    /// The traversal enters `ThreadState::LAYOUT` through an RAII guard, runs
    /// under `catch_unwind`, and after a caught panic clears the thread-local
    /// style-sharing cache before returning the failure to the caller.
    pub fn flush(&mut self, doc: &Document) -> StyleDiff {
        if self.flushed == Some(doc.mutation_generation()) {
            return StyleDiff::None;
        }
        let _ = ua::sheet_texts(self.env.quirks);
        for slot in 0..doc.slots() {
            self.store.claim(doc.node_ref(NodeId(slot as u32)));
        }
        self.atoms.begin_flush(doc);
        todo!("C2a")
    }

    /// The computed style of an element, including `display: none` ones;
    /// `None` for non-elements and nodes not yet styled.
    pub fn style(&self, node: NodeId) -> Option<Style> {
        let _ = node;
        todo!("C2a")
    }
}
