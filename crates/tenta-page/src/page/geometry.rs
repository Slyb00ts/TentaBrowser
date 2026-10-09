//! Layout queries on the page's current document: where an element's boxes
//! are and which element sits under a point. They bring the render session up
//! to date the way a screenshot does (stylesheets and images are fetched
//! first; images are not, a geometry query lays out with what the session
//! already holds) and answer from its fragment tree; nothing is painted.
//!
//! There is no scrolling yet, so viewport and document coordinates agree.

use tenta_core::{NodeId, Result};

use super::screenshot::rendered_with;
use super::{Page, Viewport};
use crate::render;

pub use tenta_render::{BoxQuads, ElementGeometry, Quad};

impl Page {
    /// Changes the viewport the page lays out and renders at, from the next
    /// layout on. Script-visible window metrics (`innerWidth`) keep the
    /// viewport the document's engine was built with.
    pub fn set_viewport(&self, viewport: Viewport) {
        *self.inner.viewport.lock() = viewport;
    }

    /// The boxes of every fragment `node` produced: empty when the node has
    /// no box (`display: none`, not an element, or not in the document).
    /// Fails like [`Page::screenshot`].
    pub async fn node_boxes(&self, node: NodeId) -> Result<Vec<BoxQuads>> {
        self.on_page_thread(move |inner| {
            rendered_with(
                inner,
                None,
                move |session, sheets, doc, base_url, viewport| {
                    render::content_size(session, sheets, doc, base_url, viewport)?;
                    let mut session = session.borrow_mut();
                    Ok(tenta_render::node_boxes(session.layout(doc), node))
                },
            )
        })
        .await
    }

    /// The element under the point, in CSS pixels from the document's
    /// top-left corner. Fails like [`Page::screenshot`].
    pub async fn hit_test(&self, x: f64, y: f64) -> Result<Option<NodeId>> {
        self.on_page_thread(move |inner| {
            rendered_with(
                inner,
                None,
                move |session, sheets, doc, base_url, viewport| {
                    render::content_size(session, sheets, doc, base_url, viewport)?;
                    let mut session = session.borrow_mut();
                    Ok(tenta_render::hit_test(
                        session.layout(doc),
                        doc,
                        x as f32,
                        y as f32,
                    ))
                },
            )
        })
        .await
    }
}

impl Page {
    /// The geometry of every element, in `querySelectorAll('*')` order, with
    /// the viewport it was laid out at. Fails like [`Page::screenshot`].
    pub async fn element_geometry(&self) -> Result<(Viewport, Vec<Option<ElementGeometry>>)> {
        self.on_page_thread(move |inner| {
            rendered_with(
                inner,
                None,
                move |session, sheets, doc, base_url, viewport| {
                    render::content_size(session, sheets, doc, base_url, viewport)?;
                    let mut session = session.borrow_mut();
                    Ok((
                        viewport,
                        tenta_render::element_geometry(session.layout(doc), doc),
                    ))
                },
            )
        })
        .await
    }
}
