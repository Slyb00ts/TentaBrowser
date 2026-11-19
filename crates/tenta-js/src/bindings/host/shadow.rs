//! Shadow trees for script: `Element.attachShadow()`, `Element.shadowRoot`,
//! `ShadowRoot`, and slots (`HTMLSlotElement`, `Element.assignedSlot`).
//!
//! The tree model is `tenta-dom`'s: a shadow root is a container owned by its
//! host, outside the document tree, and slot assignment is computed from the
//! host's children when it is asked for (named assignment only). What script
//! sees here is the node tree; the flat tree is what extraction and rendering
//! read.

use tenta_dom::{NodeKind, ShadowRootMode};

use super::HostObject;
use crate::bindings::{
    AssignedNodesOptions, Element, HTMLSlotElement, HTMLSlotElementOps, Handle, JsError, Node,
    ShadowRoot, ShadowRootInit, ShadowRootOps,
};

impl HostObject {
    /// `Element.attachShadow(init)`.
    pub(super) fn attach_shadow_root(
        &self,
        init: ShadowRootInit,
    ) -> Result<Handle<ShadowRoot>, JsError> {
        const PREFIX: &str = "Failed to execute 'attachShadow' on 'Element'";
        let mode = match init.mode.as_deref() {
            Some("open") => ShadowRootMode::Open,
            Some("closed") => ShadowRootMode::Closed,
            Some(other) => {
                return Err(JsError::type_error(format!(
                    "{PREFIX}: The provided value '{other}' is not a valid enum value of type ShadowRootMode."
                )));
            }
            None => {
                return Err(JsError::type_error(format!(
                    "{PREFIX}: Failed to read the 'mode' property from 'ShadowRootInit': Required member is undefined."
                )));
            }
        };
        let (doc, id) = self.anchor();
        let mut doc = doc.borrow_mut();
        if !doc.can_host_shadow_root(id) {
            return Err(JsError::dom_exception(
                "NotSupportedError",
                &format!("{PREFIX}: This element does not support attachShadow"),
            ));
        }
        // A root the parser attached for `<template shadowrootmode>` is the
        // one a component's constructor gets, emptied.
        if let Some(root) = doc.claim_declarative_shadow_root(id, mode) {
            return Ok(Handle::from_bits(super::encode_node(
                self.slot(),
                doc.node_ref(root),
            )));
        }
        if doc.shadow_root(id).is_some() {
            return Err(JsError::dom_exception(
                "NotSupportedError",
                &format!(
                    "{PREFIX}: Shadow root cannot be created on a host which already hosts a shadow tree."
                ),
            ));
        }
        let root = doc
            .attach_shadow_root_with_mode(id, mode)
            .map_err(|error| JsError::dom_exception("NotSupportedError", &error.to_string()))?;
        Ok(Handle::from_bits(super::encode_node(
            self.slot(),
            doc.node_ref(root),
        )))
    }

    /// `Element.shadowRoot`: the element's shadow root if it is open.
    pub(super) fn open_shadow_root(&self) -> Option<Handle<ShadowRoot>> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let root = doc.shadow_root(id)?;
        (doc.shadow_root_mode(root) == Some(ShadowRootMode::Open))
            .then(|| Handle::from_bits(super::encode_node(self.slot(), doc.node_ref(root))))
    }

    /// `Element.assignedSlot`: the slot this element is assigned to, unless
    /// that slot is in a closed shadow tree.
    pub(super) fn assigned_slot_handle(&self) -> Option<Handle<HTMLSlotElement>> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let slot = doc.assigned_slot(id)?;
        let root = doc.containing_shadow_root(slot)?;
        (doc.shadow_root_mode(root) == Some(ShadowRootMode::Open))
            .then(|| Handle::from_bits(super::encode_node(self.slot(), doc.node_ref(slot))))
    }

    /// `Node.getRootNode({composed: true})`: the root after leaving every
    /// shadow tree on the way up.
    pub(super) fn composed_root_node(&self) -> Handle<Node> {
        if self.is_attr() {
            return Handle::from_bits(self.handle_bits());
        }
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let mut root = doc.root_of(id);
        for _ in 0..doc.slots() {
            match doc.shadow_host(root) {
                Some(host) => root = doc.root_of(host),
                None => break,
            }
        }
        Handle::from_bits(super::encode_node(self.slot(), doc.node_ref(root)))
    }
}

impl ShadowRootOps for HostObject {
    fn adopted_style_sheets(&self) -> Result<Vec<Handle<crate::bindings::CSSStyleSheet>>, JsError> {
        HostObject::adopted_style_sheets(self)
    }
    fn set_adopted_style_sheets(
        &self,
        value: Vec<Handle<crate::bindings::CSSStyleSheet>>,
    ) -> Result<(), JsError> {
        HostObject::set_adopted_style_sheets(self, value)
    }

    fn mode(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        Ok(match doc.borrow().shadow_root_mode(id) {
            Some(ShadowRootMode::Closed) => "closed",
            _ => "open",
        }
        .to_owned())
    }

    fn host(&self) -> Result<Handle<Element>, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let host = doc.shadow_host(id).ok_or_else(|| {
            JsError::dom_exception("InvalidStateError", "the shadow root has no host")
        })?;
        Ok(Handle::from_bits(super::encode_node(
            self.slot(),
            doc.node_ref(host),
        )))
    }

    fn delegates_focus(&self) -> bool {
        false
    }

    fn slot_assignment(&self) -> Result<String, JsError> {
        Ok("named".to_owned())
    }

    fn inner_html(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        Ok(doc.borrow().inner_html(id))
    }

    fn set_inner_html(&self, value: &str) {
        let (doc, id) = self.anchor();
        let _ = doc.borrow_mut().set_inner_html(id, value);
    }
}

impl HTMLSlotElementOps for HostObject {
    fn name(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("name"))
    }

    fn set_name(&self, value: &str) {
        self.reflect_attribute("name", value);
    }

    fn assigned_nodes(
        &self,
        _options: Option<AssignedNodesOptions>,
    ) -> Result<Vec<Handle<Node>>, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        Ok(doc
            .assigned_nodes(id)
            .into_iter()
            .map(|node| Handle::from_bits(super::encode_node(self.slot(), doc.node_ref(node))))
            .collect())
    }

    fn assigned_elements(
        &self,
        _options: Option<AssignedNodesOptions>,
    ) -> Result<Vec<Handle<Element>>, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        Ok(doc
            .assigned_nodes(id)
            .into_iter()
            .filter(|node| doc.kind(*node) == NodeKind::Element)
            .map(|node| Handle::from_bits(super::encode_node(self.slot(), doc.node_ref(node))))
            .collect())
    }
}
