//! `MutationObserver` and `MutationRecord` (DOM Standard §4.3).
//!
//! The registrations, the record queues and the pending-observer set live in
//! `tenta-dom` (`document/observer_*.rs`); this module is the script-visible
//! half. A `MutationObserver` is a host record that owns its callback and one
//! [`NativeObserver`]. A `MutationRecord` is a host record that owns one
//! published native record, which pins every node it names until the record's
//! wrapper is finalized.
//!
//! # Lifetime
//!
//! An observer with registrations must keep notifying even when the page holds
//! no reference to it. While it observes, the record therefore owns a stored
//! reference to its own wrapper (`self_root`), released by `disconnect()`. An
//! observer whose targets become unreachable stays alive until it is
//! disconnected or the world is torn down; the native graph edges that would
//! let the collector break that cycle (`ObserverAgent::visit_gc_edges`) are not
//! walked yet.
//!
//! # Delivery
//!
//! A mutation inside a binding call leaves the agent's notify demand
//! `Requested`. The backend enqueues one microtask after that call returns
//! ([`mutation_observers_awaiting_microtask`] says when) and the microtask calls
//! [`notify_mutation_observers`], which is the standard's *notify mutation
//! observers*.

use tenta_dom::{
    NativeObserveError, NativeObserver, NativeRecordBatch, NotifyDemand, ObserveOptionsInput,
    ObserverId, ObserverNode, OptionsError, PublishedObserverRecord, RecordType,
};

use super::*;
use crate::bindings::{
    MutationCallback, MutationObserverConstructor, MutationObserverInit, MutationObserverOps,
    MutationRecord, MutationRecordOps, Node, NodeList,
};
use crate::dom::events::{CallbackRef, MutationDelivery};

/// The state behind one `MutationObserver`.
pub struct MutationObserverRecord {
    /// `None` only between the record's insertion and the construction of the
    /// native observer, which needs the record's own key as its identity.
    native: Option<NativeObserver>,
    /// The `MutationCallback`, one stored reference owned by this record.
    callback: u64,
    /// One stored reference to this observer's own wrapper while it observes.
    self_root: Option<u64>,
    /// Creation order within the world; the standard notifies observers in
    /// this order whatever order their records were queued in.
    serial: u64,
}

/// The most observers one world keeps alive by observation. Each observing
/// observer holds a reference to itself and pins its targets (about 2.5 KB for
/// a trivial subtree), and the collector cannot yet break that cycle, so the
/// count is bounded and the page told when it hits the bound.
pub const MAX_OBSERVING_OBSERVERS: usize = 10_000;

/// The state behind one `MutationRecord`.
pub struct MutationRecordRecord {
    published: Box<PublishedObserverRecord>,
    /// Static `NodeList` snapshots, built on first read.
    added: Option<(u16, SnapshotKey)>,
    removed: Option<(u16, SnapshotKey)>,
}

/// Native state a reclaimed record hands back so it is dropped after the slab
/// borrow ends: releasing a published record or forgetting an observer walks
/// the native registrations.
pub enum NativeRetired {
    Observer(NativeObserver),
    Record(Box<PublishedObserverRecord>),
}

impl std::fmt::Debug for NativeRetired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NativeRetired::Observer(_) => f.write_str("NativeRetired::Observer"),
            NativeRetired::Record(_) => f.write_str("NativeRetired::Record"),
        }
    }
}

impl NativeRetired {
    /// Finishes the native side of a reclaimed record. Call with no slab,
    /// agent or Document borrow held.
    pub fn finish(self) {
        match self {
            NativeRetired::Observer(observer) => drop(observer.forget()),
            NativeRetired::Record(record) => drop(record),
        }
    }
}

impl MutationObserverRecord {
    /// Moves the owned native state and stored values out of a freed record.
    pub(super) fn retire(self, out: &mut ReclaimedHost) {
        if let Some(native) = self.native {
            out.retired.push(NativeRetired::Observer(native));
        }
        out.values.push(self.callback);
        if let Some(root) = self.self_root {
            out.values.push(root);
        }
    }
}

impl MutationRecordRecord {
    pub(super) fn retire(self, out: &mut ReclaimedHost) {
        out.retired.push(NativeRetired::Record(self.published));
    }
}

impl DocumentSet {
    /// Whether any `MutationObserver` was ever constructed in this world.
    /// Until one is, a mutation's notify demand is never delivered, so the
    /// backend need not look at it after every binding call.
    pub fn mutation_observers_used(&self) -> bool {
        self.observers_used.get()
    }
}

/// Whether the world owes a mutation-observer microtask that has not been
/// enqueued yet. The backend enqueues it and then calls
/// [`mark_mutation_microtask_enqueued`].
pub fn mutation_observers_awaiting_microtask(ctx: &SharedContext) -> bool {
    ctx.documents.mutation_observers_used() && {
        let agent = ctx.documents.observer_agent.lock();
        matches!(agent.notify_demand(), NotifyDemand::Requested) && agent.has_pending_observers()
    }
}

/// Records that the backend's microtask is queued, so the demand is not
/// requested a second time. A refusal leaves the demand as it was.
pub fn mark_mutation_microtask_enqueued(ctx: &SharedContext) -> bool {
    let mut agent = ctx.documents.observer_agent.lock();
    if matches!(agent.notify_demand(), NotifyDemand::Requested) {
        return agent.mark_notify_enqueued().is_ok();
    }
    true
}

fn context_for_document(
    ctx: &SharedContext,
    document_id: u64,
    prefer: &SharedContext,
) -> Option<SharedContext> {
    if prefer.doc.borrow().document_id() == document_id {
        return Some(prefer.clone());
    }
    ctx.documents
        .all()
        .into_iter()
        .find(|candidate| candidate.doc.borrow().document_id() == document_id)
}

fn options_error(error: OptionsError) -> JsError {
    JsError::type_error(match error {
        OptionsError::NoMutationType => {
            "The options object must set at least one of 'attributes', 'characterData', or 'childList' to true."
        }
        OptionsError::AttributesDisabled => {
            "The options object may only set 'attributeOldValue' or 'attributeFilter' to true when 'attributes' is true or not present."
        }
        OptionsError::CharacterDataDisabled => {
            "The options object may only set 'characterDataOldValue' to true when 'characterData' is true or not present."
        }
    })
}

fn input_of(options: MutationObserverInit) -> ObserveOptionsInput {
    ObserveOptionsInput {
        child_list: options.child_list,
        attributes: options.attributes,
        character_data: options.character_data,
        subtree: options.subtree,
        attribute_old_value: options.attribute_old_value,
        character_data_old_value: options.character_data_old_value,
        attribute_filter: options
            .attribute_filter
            .map(|names| names.iter().map(|name| Atom::new(name)).collect()),
    }
}

impl HostObject {
    fn with_observer<R>(&self, f: impl FnOnce(&MutationObserverRecord) -> R) -> Option<R> {
        if let HostObject::Host { ctx, key } = self
            && let Some(HostRecord::MutationObserver(record)) = ctx.host_records.borrow().get(*key)
        {
            return Some(f(record));
        }
        None
    }

    fn observer_bits(&self) -> u64 {
        match self {
            HostObject::Host { ctx, key } => encode_host(ctx.slot, *key),
            _ => 0,
        }
    }

    fn with_published<R>(&self, default: R, f: impl FnOnce(&PublishedObserverRecord) -> R) -> R {
        if let HostObject::Host { ctx, key } = self
            && let Some(HostRecord::MutationRecord(record)) = ctx.host_records.borrow().get(*key)
        {
            return f(&record.published);
        }
        default
    }

    /// The handle bits of `node` as the context that owns its document sees it.
    fn observer_node_bits(&self, node: ObserverNode) -> Option<u64> {
        let ctx = self.context();
        let owner = context_for_document(ctx, node.document_id, ctx)?;
        Some(encode_node(owner.slot, node.node))
    }

    /// Builds (once) the static `NodeList` of one side of this record.
    fn record_list(&self, removed: bool) -> Result<Handle<NodeList>, JsError> {
        let HostObject::Host { ctx, key } = self else {
            return Err(JsError::type_error("the receiver is not a MutationRecord"));
        };
        let cached = match ctx.host_records.borrow().get(*key) {
            Some(HostRecord::MutationRecord(record)) => {
                if removed {
                    record.removed
                } else {
                    record.added
                }
            }
            _ => return Err(JsError::type_error("the receiver is not a MutationRecord")),
        };
        if let Some((slot, snapshot)) = cached
            && let Some(owner) = ctx.documents.get(slot)
            && owner.snapshots.borrow().get(snapshot).is_some()
        {
            return Ok(Handle::from_bits(encode_snapshot(
                slot,
                TAG_STATIC_NODE_LIST,
                snapshot,
            )));
        }
        let (document_id, members) = self
            .with_published(None, |published| {
                published.view().ok().map(|view| {
                    let nodes = if removed {
                        view.removed_nodes
                    } else {
                        view.added_nodes
                    };
                    (view.target.document_id, nodes.to_vec())
                })
            })
            .ok_or_else(|| JsError::type_error("the record was already released"))?;
        let owner = context_for_document(ctx, document_id, ctx)
            .ok_or_else(|| JsError::type_error("the record's document is gone"))?;
        let refs: Vec<NodeRef> = members.iter().map(|member| member.node).collect();
        let snapshot = owner.snapshots.borrow_mut().insert(refs)?;
        if let Some(HostRecord::MutationRecord(record)) =
            ctx.host_records.borrow_mut().get_mut(*key)
        {
            let slot = Some((owner.slot, snapshot));
            if removed {
                record.removed = slot;
            } else {
                record.added = slot;
            }
        }
        Ok(Handle::from_bits(encode_snapshot(
            owner.slot,
            TAG_STATIC_NODE_LIST,
            snapshot,
        )))
    }
}

impl MutationObserverConstructor for HostObject {
    fn construct(realm: &Realm, callback: MutationCallback) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        let key = ctx
            .host_records
            .borrow_mut()
            .insert(HostRecord::MutationObserver(MutationObserverRecord {
                native: None,
                callback: callback.bits(),
                self_root: None,
                serial: {
                    let serial = ctx.documents.observer_serial.get();
                    ctx.documents.observer_serial.set(serial + 1);
                    serial
                },
            }));
        let id = ObserverId(encode_host(ctx.slot, key));
        let native = NativeObserver::new(ctx.documents.observer_agent.clone(), id);
        match native {
            Ok(native) => {
                if let Some(HostRecord::MutationObserver(record)) =
                    ctx.host_records.borrow_mut().get_mut(key)
                {
                    record.native = Some(native);
                }
                ctx.documents.observers_used.set(true);
                Ok(HostObject::host_record(ctx, key))
            }
            Err(error) => {
                // The conversion lease still owns the callback's reference on
                // an error return, so the record must not release it too.
                if let Some(HostRecord::MutationObserver(record)) =
                    ctx.host_records.borrow_mut().remove(key)
                {
                    drop(record);
                }
                Err(mutation_error(&error))
            }
        }
    }
}

impl MutationObserverOps for HostObject {
    fn observe(
        &self,
        target: Handle<Node>,
        options: Option<MutationObserverInit>,
    ) -> Result<(), JsError> {
        let HostObject::Host { ctx, .. } = self else {
            return Err(JsError::type_error(
                "the receiver is not a MutationObserver",
            ));
        };
        let input = input_of(options.unwrap_or_default());
        let decoded = decode(target.bits());
        let outcome = self
            .with_observer(|record| {
                let native = record.native.as_ref()?;
                if decoded.tag == TAG_NODE {
                    let (doc, id) = resolve_node_handle(ctx, target.bits()).ok()?;
                    let document = doc.borrow();
                    let node = ObserverNode::new(document.document_id(), document.node_ref(id));
                    Some(native.observe(&document, node, input))
                } else {
                    let is_attr = ctx.documents.get(decoded.slot).is_some_and(|owner| {
                        matches!(
                            owner.host_records.borrow().get(decoded.host_key()),
                            Some(HostRecord::Attr(_))
                        )
                    });
                    if decoded.tag == TAG_HOST && is_attr {
                        Some(native.observe_inert(&ctx.doc.borrow(), input))
                    } else {
                        None
                    }
                }
            })
            .flatten();
        match outcome {
            None => return Err(JsError::type_error("parameter 1 is not of type 'Node'.")),
            Some(Err(NativeObserveError::Options(error))) => return Err(options_error(error)),
            Some(Err(NativeObserveError::Native(error))) => return Err(mutation_error(&error)),
            Some(Ok(())) => {}
        }
        self.root_while_observing()
    }

    fn disconnect(&self) -> Result<(), JsError> {
        let batch = self
            .with_observer(|record| record.native.as_ref().map(NativeObserver::disconnect))
            .flatten()
            .transpose()
            .map_err(|error| mutation_error(&error))?;
        // Release the queued records outside every borrow.
        drop(batch);
        self.unroot()
    }

    fn take_records(&self) -> Result<Vec<Handle<MutationRecord>>, JsError> {
        let HostObject::Host { ctx, .. } = self else {
            return Err(JsError::type_error(
                "the receiver is not a MutationObserver",
            ));
        };
        let batch = self
            .with_observer(|record| record.native.as_ref().map(NativeObserver::take_records))
            .flatten()
            .transpose()
            .map_err(|error| mutation_error(&error))?;
        let Some(mut batch) = batch else {
            return Ok(Vec::new());
        };
        let bits = publish_all(ctx, &mut batch)?;
        Ok(bits.into_iter().map(Handle::from_bits).collect())
    }
}

impl HostObject {
    /// Keeps the wrapper alive while this observer has registrations.
    fn root_while_observing(&self) -> Result<(), JsError> {
        let HostObject::Host { ctx, key } = self else {
            return Ok(());
        };
        if self.with_observer(|record| record.self_root.is_some()) != Some(false) {
            return Ok(());
        }
        if ctx.documents.observing_observers.get() >= MAX_OBSERVING_OBSERVERS {
            // Undo nothing: the native registration stays, so the observer
            // still records, but the page is told it cannot be kept alive.
            return Err(JsError::dom_exception(
                "QuotaExceededError",
                "too many MutationObservers are observing in this page; disconnect observers \
                 that are no longer needed",
            ));
        }
        let script = ctx
            .script
            .borrow()
            .clone()
            .ok_or_else(|| JsError::type_error("this world has no script host"))?;
        let Some(root) = script.retain_existing_target(self.observer_bits())? else {
            return Ok(());
        };
        let mut records = ctx.host_records.borrow_mut();
        match records.get_mut(*key) {
            Some(HostRecord::MutationObserver(record)) if record.self_root.is_none() => {
                record.self_root = Some(root);
                ctx.documents
                    .observing_observers
                    .set(ctx.documents.observing_observers.get() + 1);
            }
            _ => {
                drop(records);
                script.release_value(root);
            }
        }
        Ok(())
    }

    fn unroot(&self) -> Result<(), JsError> {
        let HostObject::Host { ctx, key } = self else {
            return Ok(());
        };
        let root = match ctx.host_records.borrow_mut().get_mut(*key) {
            Some(HostRecord::MutationObserver(record)) => record.self_root.take(),
            _ => None,
        };
        if let Some(root) = root {
            let observing = &ctx.documents.observing_observers;
            observing.set(observing.get().saturating_sub(1));
            if let Some(script) = ctx.script.borrow().clone() {
                script.release_value(root);
            }
        }
        Ok(())
    }
}

/// Publishes every record of `batch` as a `MutationRecord` host record in
/// `ctx`'s slab and returns their handle bits. On failure the records already
/// created are freed and the rest of the batch is released by its `Drop`.
fn publish_all(ctx: &SharedContext, batch: &mut NativeRecordBatch) -> Result<Vec<u64>, JsError> {
    let mut keys: Vec<HostKey> = Vec::with_capacity(batch.len());
    while !batch.is_empty() {
        let published = match batch.commit_published_front() {
            Ok(published) => published,
            Err(error) => {
                free_records(ctx, &keys);
                return Err(mutation_error(&error));
            }
        };
        keys.push(
            ctx.host_records
                .borrow_mut()
                .insert(HostRecord::MutationRecord(MutationRecordRecord {
                    published: Box::new(published),
                    added: None,
                    removed: None,
                })),
        );
    }
    Ok(keys
        .into_iter()
        .map(|key| encode_host(ctx.slot, key))
        .collect())
}

fn free_records(ctx: &SharedContext, keys: &[HostKey]) {
    for key in keys {
        drop(ctx.reclaim_host(*key));
    }
}

/// The standard's *notify mutation observers*: calls every pending observer's
/// callback with its queued records, in the order the observers were created.
///
/// Each observer's records are taken just before its own callback, so a
/// mutation an earlier callback makes lands in the later observer's single
/// batch, and the batch is complete however large it is.
///
/// Called from the backend's microtask with no host borrow held.
pub fn notify_mutation_observers(ctx: &SharedContext) {
    let begun = ctx.documents.observer_agent.lock().begin_notify();
    let Ok(mut pending) = begun else {
        return;
    };
    let mut order: Vec<(u64, ObserverId)> = Vec::new();
    while let Some(id) = pending.pop_front() {
        let serial = observer_context(ctx, id).and_then(|(owner, key)| {
            match owner.host_records.borrow().get(key) {
                Some(HostRecord::MutationObserver(record)) => Some(record.serial),
                _ => None,
            }
        });
        if let Some(serial) = serial {
            order.push((serial, id));
        }
    }
    order.sort_by_key(|(serial, _)| *serial);
    for (_, id) in order {
        deliver(ctx, id);
    }
}

fn observer_context(ctx: &SharedContext, id: ObserverId) -> Option<(SharedContext, HostKey)> {
    let decoded = decode(id.0);
    ctx.documents
        .get(decoded.slot)
        .map(|owner| (owner, decoded.host_key()))
}

/// Takes the observer's records and hands them to its callback in one call.
fn deliver(ctx: &SharedContext, id: ObserverId) {
    let Some((owner, key)) = observer_context(ctx, id) else {
        return;
    };
    let drained = match owner.host_records.borrow().get(key) {
        Some(HostRecord::MutationObserver(record)) => record
            .native
            .as_ref()
            .map(|native| (record.callback, native.drain_for_notify())),
        _ => None,
    };
    let Some((callback, Ok(mut batch))) = drained else {
        return;
    };
    if batch.is_empty() {
        return;
    }
    let Ok(records) = publish_all(&owner, &mut batch) else {
        return;
    };
    drop(batch);
    let outcome = owner
        .script
        .borrow()
        .clone()
        .map_or(MutationDelivery::Unreachable, |script| {
            script.invoke_mutation_callback(CallbackRef(callback), id.0, &records)
        });
    if outcome != MutationDelivery::Called {
        let keys: Vec<HostKey> = records
            .iter()
            .map(|bits| decode(*bits).host_key())
            .collect();
        free_records(&owner, &keys);
    }
}

fn record_type_name(kind: RecordType) -> &'static str {
    match kind {
        RecordType::ChildList => "childList",
        RecordType::Attributes => "attributes",
        RecordType::CharacterData => "characterData",
    }
}

impl MutationRecordOps for HostObject {
    fn r#type(&self) -> Result<String, JsError> {
        self.with_published(None, |published| {
            published
                .view()
                .ok()
                .map(|view| record_type_name(view.record_type).to_owned())
        })
        .ok_or_else(|| JsError::type_error("the record was already released"))
    }

    fn target(&self) -> Result<Handle<Node>, JsError> {
        let node = self
            .with_published(None, |published| {
                published.view().ok().map(|view| view.target)
            })
            .ok_or_else(|| JsError::type_error("the record was already released"))?;
        self.observer_node_bits(node)
            .map(Handle::from_bits)
            .ok_or_else(|| JsError::type_error("the record's document is gone"))
    }

    fn added_nodes(&self) -> Result<Handle<NodeList>, JsError> {
        self.record_list(false)
    }

    fn removed_nodes(&self) -> Result<Handle<NodeList>, JsError> {
        self.record_list(true)
    }

    fn previous_sibling(&self) -> Result<Option<Handle<Node>>, JsError> {
        let node = self.with_published(None, |published| {
            published.view().ok().and_then(|view| view.previous_sibling)
        });
        Ok(node
            .and_then(|node| self.observer_node_bits(node))
            .map(Handle::from_bits))
    }

    fn next_sibling(&self) -> Result<Option<Handle<Node>>, JsError> {
        let node = self.with_published(None, |published| {
            published.view().ok().and_then(|view| view.next_sibling)
        });
        Ok(node
            .and_then(|node| self.observer_node_bits(node))
            .map(Handle::from_bits))
    }

    fn attribute_name(&self) -> Result<Option<String>, JsError> {
        Ok(self.with_published(None, |published| {
            published
                .view()
                .ok()
                .and_then(|view| view.attribute_name)
                .map(|name| name.as_str().to_owned())
        }))
    }

    fn attribute_namespace(&self) -> Result<Option<String>, JsError> {
        Ok(self.with_published(None, |published| {
            published
                .view()
                .ok()
                .and_then(|view| view.attribute_namespace)
                .map(|namespace| namespace.as_str().to_owned())
        }))
    }

    fn old_value(&self) -> Result<Option<String>, JsError> {
        Ok(self.with_published(None, |published| {
            published
                .view()
                .ok()
                .and_then(|view| view.old_value.map(str::to_owned))
        }))
    }
}
