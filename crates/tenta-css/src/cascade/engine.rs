//! The style engine: owns sheets, the lock, the style store and the flush.

mod sheets;
#[cfg(test)]
mod timing;

use std::any::Any;
use std::panic::resume_unwind;
use std::sync::Arc;

use style::context::QuirksMode;
use style::data::ElementData;
use style::shared_lock::{SharedRwLock, StylesheetGuards};
use style::stylist::Stylist;
use tenta_core::NodeId;
use tenta_dom::Document;

use super::atoms::AtomMaps;
use super::device::new_device;
use super::diff::classify;
use super::dom::{FlushView, RestyleBits, StyleStore};
use super::traverse::restyle;
use super::types::{FontMetrics, Style, StyleDiff, StyleEnv, StyleSource};
use super::ua;
use crate::cssom::configure_stylo;

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
    /// The cascade data built from the user-agent sheets and `sources` for
    /// `env`; rebuilt before the next flush when `stylist_stale` is set.
    pub(super) stylist: Stylist,
    stylist_stale: bool,
    pub(super) store: StyleStore,
    pub(super) atoms: AtomMaps,
    /// Document generation of the last completed flush; `None` forces the
    /// next flush to restyle.
    pub(super) flushed: Option<u64>,
    traversals: u64,
}

impl StyleEngine {
    /// An engine for `env` that answers `ex`/`ch` queries through `metrics`.
    pub fn new(env: StyleEnv, metrics: Arc<dyn FontMetrics>) -> Self {
        let lock = SharedRwLock::new();
        let stylist = build_stylist(&env, &metrics, &[], &lock);
        Self {
            env,
            metrics,
            sources: Vec::new(),
            lock,
            stylist,
            stylist_stale: false,
            store: StyleStore::new(),
            atoms: AtomMaps::new(),
            flushed: None,
            traversals: 0,
        }
    }

    /// The environment styles are computed for.
    pub fn env(&self) -> &StyleEnv {
        &self.env
    }

    /// Changes the environment and invalidates every style.
    ///
    /// The next flush rebuilds the whole stylist (sheets are parsed again) and
    /// restyles every element. Nothing is compared with the previous value, so
    /// call it only for a real change.
    pub fn set_env(&mut self, env: StyleEnv) {
        self.env = env;
        self.invalidate_sheets();
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
    ///
    /// This forces a full stylist rebuild and restyle even when `sources` equals
    /// the current ones: no content comparison is made. A caller that can tell
    /// the sheets are unchanged must not call it.
    pub fn set_sources(&mut self, sources: Vec<StyleSource>) {
        self.sources = sources;
        self.invalidate_sheets();
    }

    /// How many times a flush has run the style traversal. An unchanged
    /// flush does not, which is how a host or a test tells it did no work.
    pub fn traversal_count(&self) -> u64 {
        self.traversals
    }

    /// Brings styles up to date with `doc`.
    ///
    /// Idempotent while `doc.mutation_generation()`, the environment and the
    /// sources are unchanged: the call then returns [`StyleDiff::None`] without
    /// traversing. Otherwise it restyles the whole tree and returns
    /// [`StyleDiff::Full`].
    ///
    /// The document, not the caller, decides the quirks mode: a mismatch with
    /// [`StyleEnv::quirks`] replaces the environment's value and restyles.
    ///
    /// The traversal enters `ThreadState::LAYOUT` through an RAII guard, runs
    /// under `catch_unwind`, and after a caught panic clears the thread-local
    /// style-sharing cache. The panic is then resumed: `flush` has no error
    /// value, and half-computed styles must not be served. Every style is
    /// discarded first, and the next flush starts from scratch.
    pub fn flush(&mut self, doc: &Document) -> StyleDiff {
        let quirks = StyleEnv::quirks_of(doc);
        if self.env.quirks != quirks {
            self.env.quirks = quirks;
            self.invalidate_sheets();
        }
        let generation = doc.mutation_generation();
        let diff = classify(self.flushed, generation);
        if diff == StyleDiff::None {
            return diff;
        }

        // Element data holds rule-tree nodes of the stylist about to be
        // replaced, and Stylo skips elements whose data is current, so every
        // element is cleared whether or not the sheets changed.
        self.discard_styles();
        if self.stylist_stale {
            self.stylist = build_stylist(&self.env, &self.metrics, &self.sources, &self.lock);
            self.stylist_stale = false;
        }

        // The order matters: the readers behind the traversal panic on an
        // entry that was not refreshed for this generation, so nothing may
        // mutate the document between these steps and the traversal.
        self.claim_slots(doc);
        self.store.refresh(doc);
        self.atoms.begin_flush(doc);

        let traversed = self.traverse(doc);
        if let Some(outcome) = traversed {
            self.traversals += 1;
            if let Err(panic) = outcome {
                self.discard_styles();
                resume_unwind(panic);
            }
        }
        self.flushed = Some(generation);
        diff
    }

    /// The computed style of an element, including `display: none` ones;
    /// `None` for non-elements and nodes not yet styled. The descendants of a
    /// `display: none` element are never styled either: Stylo drops their data.
    pub fn style(&self, node: NodeId) -> Option<Style> {
        let entry = self.store.entries().get(node.raw() as usize)?;
        if !entry.has_data.get() {
            return None;
        }
        entry.data.borrow().styles.get_primary().cloned()
    }

    fn claim_slots(&mut self, doc: &Document) {
        for slot in 0..doc.slots() {
            self.store.claim(doc.node_ref(NodeId(slot as u32)));
        }
    }

    /// Styles the tree under the document element; `None` for a document
    /// without one. The store must have been claimed and refreshed for `doc`.
    fn traverse(&self, doc: &Document) -> Option<Result<(), Box<dyn Any + Send>>> {
        let view = FlushView::new(doc, &self.atoms, &self.lock);
        let slots = view.make_slots(&self.store);
        assert!(view.install_slots(&slots), "a fresh view has no slot table");
        doc.document_element()
            .and_then(|element| view.handle(element))
            .map(|root| restyle(&self.stylist, &self.lock.read(), root))
    }

    fn invalidate_sheets(&mut self) {
        self.stylist_stale = true;
        self.flushed = None;
    }

    /// Clears every element's Stylo data and traversal bits, and frees the
    /// rule-tree nodes they held.
    fn discard_styles(&mut self) {
        for entry in self.store.entries() {
            if entry.has_data.replace(false) {
                *entry.data.borrow_mut() = ElementData::default();
            }
            entry.dirty_descendants.set(false);
            entry.restyle.set(RestyleBits::default());
        }
        self.flushed = None;
        self.stylist.rule_tree().maybe_gc();
    }
}

/// The cascade data for `env`: the user-agent sheets, then each enabled author
/// source in order, all parsed against `lock`.
fn build_stylist(
    env: &StyleEnv,
    metrics: &Arc<dyn FontMetrics>,
    sources: &[StyleSource],
    lock: &SharedRwLock,
) -> Stylist {
    configure_stylo();
    let quirks = if env.quirks {
        QuirksMode::Quirks
    } else {
        QuirksMode::NoQuirks
    };
    let user_agent = ua::sheet_texts(env.quirks)
        .into_iter()
        .map(|text| sheets::user_agent_sheet(text, lock));
    let author = sources
        .iter()
        .filter_map(|source| sheets::author_sheet(source, lock, quirks));
    let parsed: Vec<_> = user_agent.chain(author).collect();

    let mut stylist = Stylist::new(new_device(env, metrics), quirks);
    let guard = lock.read();
    for sheet in parsed {
        stylist.append_stylesheet(sheet, &guard);
    }
    stylist.flush(&StylesheetGuards::same(&guard));
    stylist
}
