//! Pseudo-element styles and the on-demand resolve path.
//!
//! Two questions are answered here, and they differ on purpose:
//!
//! - *Does this pseudo-element get a box?* [`StyleEngine::pseudo_style`] reads
//!   what the flush stored and returns a style only when it generates one.
//!   Layout builds `::before`/`::after` boxes from it and reads the parsed
//!   `content` list from `style.get_counters().content`.
//! - *What does `getComputedStyle` report?* The `resolve_*` methods take the
//!   document, work for any element (including the descendants of a
//!   `display: none` element, which the flush leaves unstyled) and always
//!   return a style for a pseudo-element, as browsers do, whether or not a
//!   rule matched.
//!
//! `::marker` is lazily cascaded by Stylo: matching it needs the element, and
//! so the document. It is therefore only reachable through
//! [`StyleEngine::resolve_pseudo_style`].

use std::panic::{AssertUnwindSafe, catch_unwind};

use style::context::{CascadeInputs, StyleContext, ThreadLocalStyleContext};
use style::properties::longhands::display::computed_value::T as Display;
use style::selector_parser::{PseudoElement, SnapshotMap};
use style::stylist::RuleInclusion;
use style::traversal::resolve_style;
use tenta_core::NodeId;
use tenta_dom::Document;

use super::dom::{FlushView, Handle};
use super::engine::StyleEngine;
use super::traverse::{LayoutThreadState, shared_context};
use super::types::{PseudoKind, Style};

/// The Stylo pseudo-element of an eagerly cascaded kind.
fn eager_pseudo(pseudo: PseudoKind) -> Option<PseudoElement> {
    match pseudo {
        PseudoKind::Before => Some(PseudoElement::Before),
        PseudoKind::After => Some(PseudoElement::After),
        PseudoKind::Marker | PseudoKind::FirstLine | PseudoKind::FirstLetter => None,
    }
}

/// Whether a `::before`/`::after` style produces a box: not `display: none`
/// and a `content` that is a non-empty item list. Stylo drops most such styles
/// before storing them, but not when `display` or `content` depend on an
/// inherited value.
fn generates_box(style: &Style) -> bool {
    style.get_box().clone_display() != Display::None && !style.ineffective_content_property()
}

/// Why an on-demand query could not answer, as opposed to "there is no such
/// style" (`Ok(None)`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ResolveError {
    /// The document is not the one the last flush styled: it was mutated since,
    /// or it is not the document the engine was flushed with. Flush first.
    Stale,
    /// Stylo panicked while resolving; the panic was contained and the engine
    /// remains usable.
    Failed,
}

impl StyleEngine {
    /// The style of a `::before` or `::after` of `node` that generates a box,
    /// from the last flush. `None` when there is no such pseudo-element, when
    /// it is `display: none`, or when its `content` is `none`, `normal` or an
    /// empty list. A `content` of one empty string (`''`) does generate a box.
    ///
    /// `::marker` needs the document to be matched and is served by
    /// [`StyleEngine::resolve_pseudo_style`]; `::first-line` and
    /// `::first-letter` are not generated yet. Both return `None` here.
    pub fn pseudo_style(&self, node: NodeId, pseudo: PseudoKind) -> Option<Style> {
        let pseudo_element = eager_pseudo(pseudo)?;
        let style = self.stored_pseudo(node, &pseudo_element)?;
        generates_box(&style).then_some(style)
    }

    /// The computed style of `node`, resolved on demand when the flush left it
    /// unstyled: the descendants of a `display: none` element. Equal to
    /// [`StyleEngine::style`] for every other element.
    ///
    /// Nothing is stored, so the engine's data and `style()` do not change.
    /// `Ok(None)` for non-elements and detached nodes; [`ResolveError::Stale`]
    /// when `doc` is not the document of the last flush (checked first).
    pub fn resolve_style(
        &self,
        doc: &Document,
        node: NodeId,
    ) -> Result<Option<Style>, ResolveError> {
        if !self.resolvable(doc, node)? {
            return Ok(None);
        }
        if let Some(style) = self.style(node) {
            return Ok(Some(style));
        }
        self.with_context(doc, node, |context, element| {
            resolve_style(context, element, RuleInclusion::All, None, None)
                .primary()
                .clone()
        })
        .map(Some)
    }

    /// The style `getComputedStyle(element, pseudo)` reports for a
    /// pseudo-element of `node`, whether or not it generates a box.
    ///
    /// `::before` and `::after` get a style cascaded from the matching rules,
    /// or from none (inheriting only) when no rule matches. `::marker` is
    /// matched lazily and answered for any element, list item or not.
    /// `::first-line` and `::first-letter` give `Ok(None)`. The errors and the
    /// other `None` cases are those of [`StyleEngine::resolve_style`].
    pub fn resolve_pseudo_style(
        &self,
        doc: &Document,
        node: NodeId,
        pseudo: PseudoKind,
    ) -> Result<Option<Style>, ResolveError> {
        if !self.resolvable(doc, node)? {
            return Ok(None);
        }
        match eager_pseudo(pseudo) {
            Some(pseudo_element) => {
                if let Some(style) = self.stored_pseudo(node, &pseudo_element) {
                    return Ok(Some(style));
                }
                self.with_context(doc, node, |context, element| {
                    let styles = resolve_style(
                        context,
                        element,
                        RuleInclusion::All,
                        Some(&pseudo_element),
                        None,
                    );
                    styles
                        .pseudos
                        .get(&pseudo_element)
                        .cloned()
                        .unwrap_or_else(|| {
                            let shared = context.shared;
                            shared.stylist.compute_pseudo_element_style_with_inputs(
                                CascadeInputs::default(),
                                &pseudo_element,
                                &shared.guards,
                                Some(styles.primary()),
                                Some(element),
                            )
                        })
                })
                .map(Some)
            }
            None if pseudo == PseudoKind::Marker => {
                self.with_context(doc, node, |context, element| {
                    let primary = match self.style(node) {
                        Some(style) => style,
                        None => resolve_style(context, element, RuleInclusion::All, None, None)
                            .primary()
                            .clone(),
                    };
                    context.shared.stylist.lazily_compute_pseudo_element_style(
                        &context.shared.guards,
                        element,
                        &PseudoElement::Marker,
                        RuleInclusion::All,
                        &primary,
                        false,
                        None,
                    )
                })
            }
            None => Ok(None),
        }
    }

    fn stored_pseudo(&self, node: NodeId, pseudo: &PseudoElement) -> Option<Style> {
        let entry = self.store.entries().get(node.raw() as usize)?;
        if !entry.has_data.get() {
            return None;
        }
        entry.data.borrow().styles.pseudos.get(pseudo).cloned()
    }

    /// Whether `node` can be resolved in `doc`: a connected element. Errors
    /// when `doc` is not what the last flush styled, before anything else is
    /// looked at.
    ///
    /// The engine keeps no document identity, so "the same document" is the
    /// flushed mutation generation plus, for a node the store knows, its arena
    /// generation and refresh stamp. Another document that matches all of them
    /// for this node would be accepted. A node created after the flush and
    /// never attached has no store entry, is not connected, and is `Ok(false)`:
    /// creating a node does not change the generation, so asking for a flush
    /// would never clear it.
    pub(super) fn resolvable(&self, doc: &Document, node: NodeId) -> Result<bool, ResolveError> {
        let generation = doc.mutation_generation();
        if self.flushed != Some(generation) {
            return Err(ResolveError::Stale);
        }
        if (node.raw() as usize) >= self.store.len() || (node.raw() as usize) >= doc.slots() {
            return Ok(false);
        }
        let known = self
            .store
            .get(doc.node_ref(node))
            .is_some_and(|entry| entry.refreshed_generation.get() == generation);
        if !known {
            return Err(ResolveError::Stale);
        }
        Ok(doc.is_element(node) && doc.is_connected(node))
    }

    /// Runs `run` with a style context for the element `node` of `doc`, on
    /// the calling thread, the way the flush's traversal builds one. The caller
    /// has checked [`StyleEngine::resolvable`]. A panic inside Stylo is
    /// contained and reported as [`ResolveError::Failed`].
    fn with_context<R>(
        &self,
        doc: &Document,
        node: NodeId,
        run: impl for<'d, 'c> FnOnce(&'c mut StyleContext<'_, Handle<'d>>, Handle<'d>) -> R,
    ) -> Result<R, ResolveError> {
        catch_unwind(AssertUnwindSafe(|| {
            let view = FlushView::new(doc, &self.atoms, &self.lock);
            let slots = view.make_slots(&self.store);
            assert!(view.install_slots(&slots), "a fresh view has no slot table");
            let element = view.handle(node).expect("a resolvable node has a slot");

            let guard = self.lock.read();
            let snapshots = SnapshotMap::new();
            let shared = shared_context(&self.stylist, &guard, &snapshots);
            // Declared before the thread-local context: its task list asserts
            // the layout state when dropped, and the sharing cache it owns is
            // released on unwind.
            let _state = LayoutThreadState::enter();
            let mut thread_local = ThreadLocalStyleContext::new();
            let mut context = StyleContext {
                shared: &shared,
                thread_local: &mut thread_local,
            };
            run(&mut context, element)
        }))
        .map_err(|_| ResolveError::Failed)
    }
}
