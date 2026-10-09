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
//!
//! # Without a host
//!
//! A page whose engine was never given a host has no layout, as under the
//! fetch-only tiers. Script geometry then answers what a box-less element
//! answers: `getBoundingClientRect()` is an all-zero rect, `getClientRects()`
//! is empty, every `offset*`, `client*` and `scroll*` number is `0`,
//! `offsetParent` is `null`, and `elementFromPoint` is `null`. Window metrics
//! (`innerWidth`, `devicePixelRatio`, `scrollY`, ...) answer the page's
//! configured viewport (`PageEngineConfig::viewport`), not zero, because
//! scripts divide by them and branch on them. A host that is installed but
//! cannot answer returns the same neutral values through its `None` and empty
//! results.

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

impl Default for ViewportMetrics {
    /// The headless desktop window a page without a configured viewport has:
    /// 1280x800 at scale 1 with the outer window equal to the inner one and no
    /// scroll offset. It matches `tenta_page::Viewport::default()`, which is
    /// where a real page's value comes from.
    fn default() -> Self {
        Self {
            inner_width: 1280.0,
            inner_height: 800.0,
            outer_width: 1280.0,
            outer_height: 800.0,
            device_pixel_ratio: 1.0,
            scroll_x: 0.0,
            scroll_y: 0.0,
        }
    }
}

/// The numeric `Window` members geometry answers, in the order the engines
/// install them.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum WindowMetric {
    /// `innerWidth`.
    InnerWidth,
    /// `innerHeight`.
    InnerHeight,
    /// `outerWidth`.
    OuterWidth,
    /// `outerHeight`.
    OuterHeight,
    /// `devicePixelRatio`.
    DevicePixelRatio,
    /// `screenX`.
    ScreenX,
    /// `screenY`.
    ScreenY,
    /// `scrollX`.
    ScrollX,
    /// `scrollY`.
    ScrollY,
    /// `pageXOffset`, an alias of `scrollX`.
    PageXOffset,
    /// `pageYOffset`, an alias of `scrollY`.
    PageYOffset,
}

impl WindowMetric {
    /// Every member, so a backend installs them from one table.
    pub const ALL: [WindowMetric; 11] = [
        WindowMetric::InnerWidth,
        WindowMetric::InnerHeight,
        WindowMetric::OuterWidth,
        WindowMetric::OuterHeight,
        WindowMetric::DevicePixelRatio,
        WindowMetric::ScreenX,
        WindowMetric::ScreenY,
        WindowMetric::ScrollX,
        WindowMetric::ScrollY,
        WindowMetric::PageXOffset,
        WindowMetric::PageYOffset,
    ];

    /// The property name on `window`.
    pub fn name(self) -> &'static str {
        match self {
            WindowMetric::InnerWidth => "innerWidth",
            WindowMetric::InnerHeight => "innerHeight",
            WindowMetric::OuterWidth => "outerWidth",
            WindowMetric::OuterHeight => "outerHeight",
            WindowMetric::DevicePixelRatio => "devicePixelRatio",
            WindowMetric::ScreenX => "screenX",
            WindowMetric::ScreenY => "screenY",
            WindowMetric::ScrollX => "scrollX",
            WindowMetric::ScrollY => "scrollY",
            WindowMetric::PageXOffset => "pageXOffset",
            WindowMetric::PageYOffset => "pageYOffset",
        }
    }

    /// The member's value: from `host` when a layout is installed, otherwise
    /// from `fallback`, the page's configured viewport.
    ///
    /// The window is always at the screen origin, so `screenX` and `screenY`
    /// are 0 either way.
    pub fn read(self, host: Option<&dyn GeometryHost>, fallback: &ViewportMetrics) -> f64 {
        let viewport = host.map_or(*fallback, |host| host.viewport());
        match self {
            WindowMetric::InnerWidth => viewport.inner_width,
            WindowMetric::InnerHeight => viewport.inner_height,
            WindowMetric::OuterWidth => viewport.outer_width,
            WindowMetric::OuterHeight => viewport.outer_height,
            WindowMetric::DevicePixelRatio => viewport.device_pixel_ratio,
            WindowMetric::ScreenX | WindowMetric::ScreenY => 0.0,
            WindowMetric::ScrollX | WindowMetric::PageXOffset => viewport.scroll_x,
            WindowMetric::ScrollY | WindowMetric::PageYOffset => viewport.scroll_y,
        }
    }
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
