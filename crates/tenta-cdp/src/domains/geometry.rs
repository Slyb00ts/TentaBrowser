//! Layout answers for the `DOM` and `Input` domains: the boxes of a node and
//! the element under a point. They need the render pipeline; without the
//! `render` feature every call answers an error saying so.

use tenta_core::NodeId;

use crate::backend::Target;
use crate::error::CdpError;

/// The four boxes of a node as `[x1, y1, ..., x4, y4]` quads, clockwise from
/// the top-left corner, and the border box size.
pub(super) struct Boxes {
    pub content: [f64; 8],
    pub padding: [f64; 8],
    pub border: [f64; 8],
    pub margin: [f64; 8],
    pub width: f64,
    pub height: f64,
}

#[cfg(feature = "render")]
pub(super) use built::{element_at, node_boxes};
#[cfg(not(feature = "render"))]
pub(super) use missing::{element_at, node_boxes};

#[cfg(feature = "render")]
mod built {
    use tenta_browser::{BoxQuads, Quad};

    use super::{Boxes, CdpError, NodeId, Target};
    use crate::domains::nodes::page_error;

    fn flat(quad: &Quad) -> [f64; 8] {
        let [a, b, c, d] = quad.0;
        [a.0, a.1, b.0, b.1, c.0, c.1, d.0, d.1]
    }

    fn boxes(quads: &BoxQuads) -> Boxes {
        let (_, _, width, height) = quads.border.bounds();
        Boxes {
            content: flat(&quads.content),
            padding: flat(&quads.padding),
            border: flat(&quads.border),
            margin: flat(&quads.margin),
            width,
            height,
        }
    }

    /// One entry per fragment of the node; empty when it has no box.
    pub(in crate::domains) async fn node_boxes(
        target: &Target,
        node: NodeId,
    ) -> Result<Vec<Boxes>, CdpError> {
        let quads = target.page().node_boxes(node).await.map_err(page_error)?;
        Ok(quads.iter().map(boxes).collect())
    }

    pub(in crate::domains) async fn element_at(
        target: &Target,
        x: f64,
        y: f64,
    ) -> Result<Option<NodeId>, CdpError> {
        target.page().hit_test(x, y).await.map_err(page_error)
    }
}

#[cfg(not(feature = "render"))]
mod missing {
    use super::{Boxes, CdpError, NodeId, Target};

    fn unavailable() -> CdpError {
        CdpError::server("This build has no renderer; enable the render feature")
    }

    pub(in crate::domains) async fn node_boxes(
        _target: &Target,
        _node: NodeId,
    ) -> Result<Vec<Boxes>, CdpError> {
        Err(unavailable())
    }

    pub(in crate::domains) async fn element_at(
        _target: &Target,
        _x: f64,
        _y: f64,
    ) -> Result<Option<NodeId>, CdpError> {
        Err(unavailable())
    }
}
