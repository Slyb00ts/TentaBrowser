//! The geometry seam between script engines and the layout-backed render
//! session.
//!
//! This crate is a foundation crate and cannot depend on the render crates, so
//! script geometry (`getBoundingClientRect`, `offsetWidth`, `getComputedStyle`,
//! `innerWidth`, ...) calls through [`GeometryHost`]. The render crate
//! implements it and the page installs it; the trait speaks plain data only.
//! Implementations run inside an engine callback while the document may be
//! borrowed, so they must not hold a document borrow across a script call and
//! must answer with the neutral value (zero rect, `None`) instead of panicking
//! when the document is unavailable.

use tenta_core::NodeId;

/// A rectangle in CSS pixels as the `DOMRect` API reports it.
#[derive(Copy, Clone, Default, PartialEq, Debug)]
pub struct RectF64 {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

/// Window metrics for `innerWidth`, `devicePixelRatio`, `scrollY` and friends.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct ViewportMetrics {
    /// `innerWidth`.
    pub inner_width: f64,
    /// `innerHeight`.
    pub inner_height: f64,
    /// `outerWidth`.
    pub outer_width: f64,
    /// `outerHeight`.
    pub outer_height: f64,
    /// `devicePixelRatio`.
    pub device_pixel_ratio: f64,
    /// `scrollX` / `pageXOffset`.
    pub scroll_x: f64,
    /// `scrollY` / `pageYOffset`.
    pub scroll_y: f64,
}

/// `offsetTop`, `offsetLeft`, `offsetWidth`, `offsetHeight` and `offsetParent`.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct OffsetMetrics {
    /// `offsetTop`.
    pub top: f64,
    /// `offsetLeft`.
    pub left: f64,
    /// `offsetWidth`.
    pub width: f64,
    /// `offsetHeight`.
    pub height: f64,
    /// The `offsetParent` element, when it has one.
    pub parent: Option<NodeId>,
}

/// `clientTop`, `clientLeft`, `clientWidth` and `clientHeight`.
#[derive(Copy, Clone, Default, PartialEq, Debug)]
pub struct ClientMetrics {
    /// `clientTop`.
    pub top: f64,
    /// `clientLeft`.
    pub left: f64,
    /// `clientWidth`.
    pub width: f64,
    /// `clientHeight`.
    pub height: f64,
}

/// `scrollTop`, `scrollLeft`, `scrollWidth` and `scrollHeight`.
#[derive(Copy, Clone, Default, PartialEq, Debug)]
pub struct ScrollMetrics {
    /// `scrollTop`.
    pub top: f64,
    /// `scrollLeft`.
    pub left: f64,
    /// `scrollWidth`.
    pub width: f64,
    /// `scrollHeight`.
    pub height: f64,
}

/// Layout-backed answers for script geometry APIs.
pub trait GeometryHost {
    /// Window metrics.
    fn viewport(&self) -> ViewportMetrics;

    /// Border box of `node` in viewport coordinates; `None` when it has no
    /// box.
    fn bounding_rect(&self, node: NodeId) -> Option<RectF64>;

    /// One rectangle per fragment of `node`; empty when it has no box.
    fn client_rects(&self, node: NodeId) -> Vec<RectF64>;

    /// `offset*` metrics.
    fn offset_metrics(&self, node: NodeId) -> Option<OffsetMetrics>;

    /// `client*` metrics.
    fn client_metrics(&self, node: NodeId) -> Option<ClientMetrics>;

    /// `scroll*` metrics.
    fn scroll_metrics(&self, node: NodeId) -> Option<ScrollMetrics>;

    /// The resolved value of `property` for `node` or one of its pseudo
    /// elements (`"::before"`), as `getComputedStyle` reports it.
    fn computed_value(&self, node: NodeId, pseudo: Option<&str>, property: &str) -> Option<String>;

    /// Property names `getComputedStyle` enumerates, in order.
    fn computed_property_names(&self) -> &'static [&'static str];

    /// The topmost element at a viewport point.
    fn element_from_point(&self, x: f64, y: f64) -> Option<NodeId>;
}
