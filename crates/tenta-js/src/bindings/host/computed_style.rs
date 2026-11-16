//! `window.getComputedStyle()`: a read-only `CSSStyleProperties` whose values
//! come from the page's layout (CSSOM, "resolved values").
//!
//! The declaration is live: it keeps only the element (a generational
//! reference) and the pseudo-element, and every read asks the geometry host,
//! which brings style and layout up to date first. A page below the render tier
//! installs no host, so every property reads as the empty string, the same
//! answer an element with no style gets. Writes throw
//! `NoModificationAllowedError`.

use std::rc::Rc;

use super::*;
use crate::geometry::GeometryHost;

/// The state behind one `getComputedStyle()` result.
#[derive(Clone, Debug)]
pub struct ComputedStyleRecord {
    /// The element whose style the declaration reports.
    pub node: NodeRef,
    /// The pseudo-element argument as written, or `None`.
    pub pseudo: Option<String>,
}

impl HostObject {
    /// `window.getComputedStyle(element, pseudoElt)`.
    pub fn get_computed_style(
        &self,
        element: Handle<Element>,
        pseudo: Option<String>,
    ) -> Result<Handle<CSSStyleProperties>, JsError> {
        let HostObject::Window { ctx } = self else {
            return Err(JsError::type_error("the receiver is not Window"));
        };
        let (doc, node) = self.anchor_of(element.bits())?;
        let node_ref = doc.borrow().node_ref(node);
        // `getComputedStyle(null)` and non-elements are a TypeError at the
        // binding; a non-element node reaches here as a handle all the same.
        if !doc.borrow().is_element(node) {
            return Err(JsError::type_error(
                "Failed to execute 'getComputedStyle' on 'Window': parameter 1 is not of type 'Element'.",
            ));
        }
        let key = ctx
            .host_records
            .borrow_mut()
            .insert(HostRecord::ComputedStyle(ComputedStyleRecord {
                node: node_ref,
                pseudo,
            }));
        Ok(Handle::from_bits(encode_host(ctx.slot, key)))
    }
}

/// The record behind `host`, when it is a computed-style declaration.
pub(super) fn computed_record(host: &HostObject) -> Option<ComputedStyleRecord> {
    let HostObject::Host { ctx, key } = host else {
        return None;
    };
    match ctx.host_records.borrow().get(*key)? {
        HostRecord::ComputedStyle(record) => Some(record.clone()),
        _ => None,
    }
}

/// The geometry host of the engine behind `host`, and the live node of `record`.
fn host_and_node(
    host: &HostObject,
    record: &ComputedStyleRecord,
) -> Option<(Rc<dyn GeometryHost>, tenta_core::NodeId)> {
    let ctx = host.context();
    let geometry = ctx.script.borrow().clone()?.geometry_host()?;
    let node = ctx.doc.try_borrow().ok()?.resolve(record.node).ok()?;
    Some((geometry, node))
}

fn read_only() -> JsError {
    JsError::dom_exception(
        "NoModificationAllowedError",
        "These styles are computed, and therefore read-only.",
    )
}

impl HostObject {
    pub(super) fn computed_value_of(&self, record: &ComputedStyleRecord, property: &str) -> String {
        let Some((geometry, node)) = host_and_node(self, record) else {
            return String::new();
        };
        // Property names are case-insensitive except custom properties.
        let property = if property.starts_with("--") {
            property.to_owned()
        } else {
            property.to_ascii_lowercase()
        };
        geometry
            .computed_value(node, record.pseudo.as_deref(), &property)
            .unwrap_or_default()
    }

    pub(super) fn computed_length(&self, record: &ComputedStyleRecord) -> u32 {
        host_and_node(self, record).map_or(0, |(geometry, _)| {
            geometry.computed_property_names().len() as u32
        })
    }

    pub(super) fn computed_item(&self, record: &ComputedStyleRecord, index: u32) -> String {
        host_and_node(self, record)
            .and_then(|(geometry, _)| {
                geometry
                    .computed_property_names()
                    .get(index as usize)
                    .map(|name| (*name).to_owned())
            })
            .unwrap_or_default()
    }

    pub(super) fn computed_read_only_for(&self, _record: &ComputedStyleRecord) -> JsError {
        read_only()
    }
}
