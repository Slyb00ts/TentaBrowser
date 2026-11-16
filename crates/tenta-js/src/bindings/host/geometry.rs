//! CSSOM View geometry: `DOMRect`, `DOMRectList` and the element, document and
//! window members that answer from the page's layout (CSS1).
//!
//! Layout belongs to the page and the render crates, which this foundation
//! crate cannot depend on, so every member asks the world's [`GeometryHost`]
//! and answers as an element with no box when there is none: a zero rectangle,
//! an empty list, `0` for every offset, client and scroll size, and `null` for
//! `offsetParent` and `elementFromPoint`.

use std::rc::Rc;

use super::*;
use crate::bindings::{
    DOMRect, DOMRectConstructor, DOMRectList, DOMRectListOps, DOMRectOps,
    DOMRectReadOnlyConstructor, DOMRectReadOnlyOps,
};
use crate::geometry::{GeometryHost, RectF64};

/// One rectangle: `x`, `y`, `width`, `height` as the constructor took them.
/// `top`, `right`, `bottom` and `left` are derived on every read, so a
/// negative width puts `left` right of `right` as the standard says.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DomRectRecord {
    /// `x`, `y`, `width`, `height`.
    pub rect: [f64; 4],
    /// A `DOMRectReadOnly`: its setters do not exist, and its prototype is the
    /// read-only one.
    pub read_only: bool,
}

/// The rectangles behind one `DOMRectList`.
#[derive(Clone, Debug, PartialEq)]
pub struct DomRectListRecord {
    /// One `[x, y, width, height]` per rectangle.
    pub rects: Vec<[f64; 4]>,
}

impl DomRectRecord {
    fn top(&self) -> f64 {
        self.rect[1].min(self.rect[1] + self.rect[3])
    }
    fn bottom(&self) -> f64 {
        self.rect[1].max(self.rect[1] + self.rect[3])
    }
    fn left(&self) -> f64 {
        self.rect[0].min(self.rect[0] + self.rect[2])
    }
    fn right(&self) -> f64 {
        self.rect[0].max(self.rect[0] + self.rect[2])
    }
}

impl HostObject {
    /// The layout this world can ask, when the page installed one.
    fn geometry(&self) -> Option<Rc<dyn GeometryHost>> {
        self.context().script.borrow().clone()?.geometry_host()
    }

    fn mint_rect(&self, rect: [f64; 4], read_only: bool) -> u64 {
        let key = self
            .context()
            .host_records
            .borrow_mut()
            .insert(HostRecord::DomRect(DomRectRecord { rect, read_only }));
        encode_host(self.slot(), key)
    }

    /// The anchored element's node, when the receiver is a node.
    fn geometry_node(&self) -> tenta_core::NodeId {
        self.anchor().1
    }

    pub(super) fn geo_bounding_client_rect(&self) -> Result<Handle<DOMRect>, JsError> {
        let node = self.geometry_node();
        let rect = self
            .geometry()
            .and_then(|host| host.bounding_rect(node))
            .unwrap_or_default();
        Ok(Handle::from_bits(self.mint_rect(
            [rect.x, rect.y, rect.width, rect.height],
            false,
        )))
    }

    pub(super) fn geo_client_rects(&self) -> Result<Handle<DOMRectList>, JsError> {
        let node = self.geometry_node();
        let rects: Vec<RectF64> = self
            .geometry()
            .map(|host| host.client_rects(node))
            .unwrap_or_default();
        let key = self
            .context()
            .host_records
            .borrow_mut()
            .insert(HostRecord::DomRectList(DomRectListRecord {
                rects: rects
                    .iter()
                    .map(|rect| [rect.x, rect.y, rect.width, rect.height])
                    .collect(),
            }));
        Ok(Handle::from_bits(encode_host(self.slot(), key)))
    }

    pub(super) fn geo_client(&self) -> crate::geometry::ClientMetrics {
        let node = self.geometry_node();
        self.geometry()
            .and_then(|host| host.client_metrics(node))
            .unwrap_or_default()
    }

    pub(super) fn geo_scroll(&self) -> crate::geometry::ScrollMetrics {
        let node = self.geometry_node();
        self.geometry()
            .and_then(|host| host.scroll_metrics(node))
            .unwrap_or_default()
    }

    pub(super) fn geo_offset(&self) -> Option<crate::geometry::OffsetMetrics> {
        let node = self.geometry_node();
        self.geometry()?.offset_metrics(node)
    }

    pub(super) fn geo_offset_parent(&self) -> Option<Handle<Element>> {
        let parent = self.geo_offset()?.parent?;
        let (doc, _) = self.anchor();
        let doc = doc.borrow();
        doc.is_live(parent)
            .then(|| Handle::from_bits(encode_node(self.slot(), doc.node_ref(parent))))
    }

    pub(super) fn geo_element_from_point(&self, x: f64, y: f64) -> Option<Handle<Element>> {
        let node = self.geometry()?.element_from_point(x, y)?;
        let (doc, _) = self.anchor();
        let doc = doc.borrow();
        doc.is_live(node)
            .then(|| Handle::from_bits(encode_node(self.slot(), doc.node_ref(node))))
    }

    pub(super) fn geo_scrolling_element(&self) -> Option<Handle<Element>> {
        let (doc, _) = self.anchor();
        let doc = doc.borrow();
        // CSSOM View: the root in standards mode, the body in quirks mode.
        let element = if doc.quirks_mode() == tenta_html::QuirksMode::Quirks {
            doc.body().or(doc.document_element())
        } else {
            doc.document_element()
        }?;
        Some(Handle::from_bits(encode_node(
            self.slot(),
            doc.node_ref(element),
        )))
    }

    fn rect_record(&self) -> Option<DomRectRecord> {
        let HostObject::Host { ctx, key } = self else {
            return None;
        };
        match ctx.host_records.borrow().get(*key)? {
            HostRecord::DomRect(record) => Some(*record),
            _ => None,
        }
    }

    fn set_rect_part(&self, index: usize, value: f64) {
        let HostObject::Host { ctx, key } = self else {
            return;
        };
        if let Some(HostRecord::DomRect(record)) = ctx.host_records.borrow_mut().get_mut(*key)
            && !record.read_only
        {
            record.rect[index] = value;
        }
    }

    fn rect_list(&self) -> Option<DomRectListRecord> {
        let HostObject::Host { ctx, key } = self else {
            return None;
        };
        match ctx.host_records.borrow().get(*key)? {
            HostRecord::DomRectList(record) => Some(record.clone()),
            _ => None,
        }
    }
}

fn rect_from_realm(realm: &Realm, rect: [f64; 4], read_only: bool) -> Result<HostObject, JsError> {
    let ctx = realm_context(realm)?;
    let key = ctx
        .host_records
        .borrow_mut()
        .insert(HostRecord::DomRect(DomRectRecord { rect, read_only }));
    Ok(HostObject::host_record(ctx, key))
}

impl DOMRectReadOnlyConstructor for HostObject {
    fn construct(
        realm: &Realm,
        x: Option<f64>,
        y: Option<f64>,
        width: Option<f64>,
        height: Option<f64>,
    ) -> Result<Self, JsError> {
        rect_from_realm(
            realm,
            [
                x.unwrap_or(0.0),
                y.unwrap_or(0.0),
                width.unwrap_or(0.0),
                height.unwrap_or(0.0),
            ],
            true,
        )
    }
}

impl DOMRectConstructor for HostObject {
    fn construct(
        realm: &Realm,
        x: Option<f64>,
        y: Option<f64>,
        width: Option<f64>,
        height: Option<f64>,
    ) -> Result<Self, JsError> {
        rect_from_realm(
            realm,
            [
                x.unwrap_or(0.0),
                y.unwrap_or(0.0),
                width.unwrap_or(0.0),
                height.unwrap_or(0.0),
            ],
            false,
        )
    }
}

impl DOMRectReadOnlyOps for HostObject {
    fn x(&self) -> f64 {
        self.rect_record().map_or(0.0, |r| r.rect[0])
    }
    fn set_x(&self, value: f64) {
        self.set_rect_part(0, value);
    }
    fn y(&self) -> f64 {
        self.rect_record().map_or(0.0, |r| r.rect[1])
    }
    fn set_y(&self, value: f64) {
        self.set_rect_part(1, value);
    }
    fn width(&self) -> f64 {
        self.rect_record().map_or(0.0, |r| r.rect[2])
    }
    fn set_width(&self, value: f64) {
        self.set_rect_part(2, value);
    }
    fn height(&self) -> f64 {
        self.rect_record().map_or(0.0, |r| r.rect[3])
    }
    fn set_height(&self, value: f64) {
        self.set_rect_part(3, value);
    }
    fn top(&self) -> f64 {
        self.rect_record().map_or(0.0, |r| r.top())
    }
    fn right(&self) -> f64 {
        self.rect_record().map_or(0.0, |r| r.right())
    }
    fn bottom(&self) -> f64 {
        self.rect_record().map_or(0.0, |r| r.bottom())
    }
    fn left(&self) -> f64 {
        self.rect_record().map_or(0.0, |r| r.left())
    }
}

impl DOMRectOps for HostObject {}

impl DOMRectListOps for HostObject {
    fn length(&self) -> u32 {
        self.rect_list().map_or(0, |list| list.rects.len() as u32)
    }

    fn item(&self, index: u32) -> Result<Option<Handle<DOMRect>>, JsError> {
        Ok(self
            .rect_list()
            .and_then(|list| list.rects.get(index as usize).copied())
            .map(|rect| Handle::from_bits(self.mint_rect(rect, false))))
    }

    fn supported_property_indices(&self) -> Result<Vec<u32>, JsError> {
        Ok((0..<Self as DOMRectListOps>::length(self)).collect())
    }

    fn is_supported_property_index(&self, index: u32) -> Result<bool, JsError> {
        Ok(index < <Self as DOMRectListOps>::length(self))
    }
}
