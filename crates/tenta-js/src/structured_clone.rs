//! Engine-independent structured-clone graph for one agent-to-agent message.
//!
//! This first slice deliberately carries only ordinary objects, arrays,
//! primitives other than BigInt, and fixed ArrayBuffers. Unsupported kinds
//! must fail at an engine adapter rather than being approximated as objects.

use std::collections::HashMap;

use tenta_core::NativeMemoryReservation as Reservation;
#[cfg(test)]
use tenta_core::{NativeMemoryBudget as CloneBudget, NativeMemoryComponent};

// These are conservative per-entry allowances for Vec capacity growth,
// HashMap buckets, DFS frames, and property tuples. UTF-16 key data is
// charged separately at four bytes per code unit by the source adapter.
// Engine handle footprints are added with size_of::<Value>() below.
const NODE_CHARGE: usize = 512;
/// Conservative per-property workspace allowance charged by each engine adapter.
/// UTF-16 key storage is charged separately before its native allocation.
pub const PROPERTY_CHARGE: usize = 192;
const TRANSFER_ENTRY_CHARGE: usize = 128;
const CHECKPOINT_INTERVAL: usize = 256;

type NodeId = usize;

/// All keys and string values are UTF-16 code units, including lone surrogates.
pub type JsString = Vec<u16>;

#[derive(Debug, PartialEq)]
enum WireNode {
    Undefined,
    Null,
    Boolean(bool),
    /// Raw IEEE-754 bits retain NaN payloads and negative zero.
    Number(u64),
    String(JsString),
    Array {
        length: u32,
        properties: Vec<(JsString, NodeId)>,
    },
    Object {
        properties: Vec<(JsString, NodeId)>,
    },
    ArrayBuffer {
        bytes: Vec<u8>,
        transferred: bool,
    },
    /// Exists only while a listed transfer waits for the post-serialization
    /// transfer phase. A completed wire never contains this variant.
    TransferPending,
}

/// Owned, Send graph. `transfers` also includes buffers absent from `root`.
#[derive(Debug)]
pub struct WireGraph {
    root: NodeId,
    nodes: Vec<WireNode>,
    transfers: Vec<NodeId>,
    reservation: Reservation,
}

/// Native classification only. Inspecting a constructor, prototype, or
/// `Symbol.toStringTag` would execute page code and cannot be used here.
#[derive(Debug, Clone, PartialEq)]
pub enum SourceKind {
    Undefined,
    Null,
    Boolean(bool),
    Number(u64),
    String(JsString),
    Array { length: u32 },
    Object,
    ArrayBuffer,
    Unsupported,
}

/// Adapter operations run inside the engine's existing panic/poison boundary.
/// Their `Value` handles must remain rooted for the entire serialization.
pub trait CloneSource {
    /// Cloning a Value must only duplicate a compact rooted engine handle:
    /// V8 Local is copied and QuickJS JS_DupValue increments a native refcount.
    /// It must not allocate another uncharged payload.
    type Value: Clone;
    type Error;

    /// Strings are charged to `workspace` before allocating their UTF-16 copy.
    fn kind(
        &mut self,
        value: &Self::Value,
        workspace: &mut Reservation,
    ) -> Result<SourceKind, Self::Error>;
    fn identity_hash(&mut self, value: &Self::Value) -> u64;
    fn same_object(&mut self, left: &Self::Value, right: &Self::Value) -> bool;
    /// Charge at least PROPERTY_CHARGE plus four bytes per UTF-16 code unit
    /// before each native key-copy allocation. The engine's own key snapshot
    /// remains subject to its separate JS-heap limit.
    fn enumerable_own_keys(
        &mut self,
        value: &Self::Value,
        workspace: &mut Reservation,
    ) -> Result<Vec<JsString>, Self::Error>;
    fn has_own(&mut self, value: &Self::Value, key: &[u16]) -> Result<bool, Self::Error>;
    fn get(&mut self, value: &Self::Value, key: &[u16]) -> Result<Self::Value, Self::Error>;
    /// Check detached state and copy the bytes at the instant this is called.
    /// Charge at least twice the current byte length before making a copy.
    fn copy_buffer(
        &mut self,
        value: &Self::Value,
        workspace: &mut Reservation,
    ) -> Result<Vec<u8>, Self::Error>;
    /// Attempt native detachment, including a detach-key/non-detachable check.
    fn detach_buffer(&mut self, value: &Self::Value) -> Result<(), Self::Error>;
    fn data_clone_error(&mut self, reason: &'static str) -> Self::Error;
    fn budget_error(&mut self) -> Self::Error;
    /// Checks the engine watchdog and sticky native failure before more work.
    fn checkpoint(&mut self) -> Result<(), Self::Error>;
}

/// A transfer sequence built under the same reservation as the final wire.
/// Construct this before reading `options.transfer`, so reentrant clones
/// share the page ledger throughout WebIDL sequence conversion.
pub struct TransferList<V> {
    items: Vec<V>,
    reservation: Reservation,
}

impl<V: Clone> TransferList<V> {
    pub fn new(reservation: Reservation) -> Self {
        Self {
            items: Vec::new(),
            reservation,
        }
    }

    pub fn push<S: CloneSource<Value = V>>(
        &mut self,
        source: &mut S,
        value: V,
    ) -> Result<(), S::Error> {
        let handle_bytes = std::mem::size_of::<V>()
            .checked_mul(8)
            .and_then(|bytes| TRANSFER_ENTRY_CHARGE.checked_add(bytes))
            .ok_or_else(|| {
                self.reservation.fail_allocation();
                source.budget_error()
            })?;
        if !self.reservation.charge(handle_bytes) {
            return Err(source.budget_error());
        }
        self.items.try_reserve(1).map_err(|_| {
            self.reservation.fail_allocation();
            source.budget_error()
        })?;
        self.items.push(value);
        Ok(())
    }
}

/// Drives the engine's WebIDL iterator one Next at a time. `next` must
/// preserve page exceptions and perform IteratorClose where WebIDL requires it.
/// Watchdog/native-failure checks occur before the first Next and at most
/// 256 successful items apart. A sticky budget or native failure never calls
/// page-controlled `return()` during cleanup.
pub fn collect_transfer_list<S, F>(
    source: &mut S,
    reservation: Reservation,
    mut next: F,
) -> Result<TransferList<S::Value>, S::Error>
where
    S: CloneSource,
    F: FnMut(&mut S) -> Result<Option<S::Value>, S::Error>,
{
    let mut list = TransferList::new(reservation);
    loop {
        if list.items.len().is_multiple_of(CHECKPOINT_INTERVAL) {
            source.checkpoint()?;
        }
        let Some(value) = next(source)? else {
            break;
        };
        list.push(source, value)?;
    }
    Ok(list)
}

struct Frame<V> {
    value: V,
    node: NodeId,
    keys: Vec<JsString>,
    next: usize,
}

struct Builder<'a, S: CloneSource> {
    source: &'a mut S,
    transfer_list: Vec<S::Value>,
    nodes: Vec<WireNode>,
    objects: Vec<S::Value>,
    buckets: HashMap<u64, Vec<(usize, NodeId)>>,
    stack: Vec<Frame<S::Value>>,
    // Release the shared charge only after every native buffer and rooted
    // source handle has dropped, including on an early serialization error.
    reservation: Reservation,
}

impl<S: CloneSource> Builder<'_, S> {
    fn charge(&mut self, bytes: usize) -> Result<(), S::Error> {
        if self.reservation.charge(bytes) {
            Ok(())
        } else {
            Err(self.source.budget_error())
        }
    }

    fn allocation_failed(&mut self) -> S::Error {
        self.reservation.fail_allocation();
        self.source.budget_error()
    }

    fn find(&mut self, value: &S::Value) -> Option<NodeId> {
        let hash = self.source.identity_hash(value);
        self.buckets.get(&hash).and_then(|entries| {
            entries.iter().find_map(|&(slot, node)| {
                self.source
                    .same_object(&self.objects[slot], value)
                    .then_some(node)
            })
        })
    }

    fn remember(&mut self, value: &S::Value, node: NodeId) -> Result<(), S::Error> {
        let hash = self.source.identity_hash(value);
        let slot = self.objects.len();
        self.objects
            .try_reserve(1)
            .map_err(|_| self.allocation_failed())?;
        if !self.buckets.contains_key(&hash) {
            self.buckets
                .try_reserve(1)
                .map_err(|_| self.allocation_failed())?;
        }
        self.buckets
            .entry(hash)
            .or_default()
            .try_reserve(1)
            .map_err(|_| self.allocation_failed())?;
        self.objects.push(value.clone());
        self.buckets.entry(hash).or_default().push((slot, node));
        Ok(())
    }

    fn push(&mut self, node: WireNode) -> Result<NodeId, S::Error> {
        let charge = std::mem::size_of::<S::Value>()
            .checked_mul(8)
            .and_then(|bytes| NODE_CHARGE.checked_add(bytes))
            .ok_or_else(|| self.allocation_failed())?;
        self.charge(charge)?;
        self.nodes
            .try_reserve(1)
            .map_err(|_| self.allocation_failed())?;
        let id = self.nodes.len();
        self.nodes.push(node);
        Ok(id)
    }

    fn visit(&mut self, value: S::Value) -> Result<NodeId, S::Error> {
        match self.source.kind(&value, &mut self.reservation)? {
            SourceKind::Undefined => self.push(WireNode::Undefined),
            SourceKind::Null => self.push(WireNode::Null),
            SourceKind::Boolean(value) => self.push(WireNode::Boolean(value)),
            SourceKind::Number(bits) => self.push(WireNode::Number(bits)),
            SourceKind::String(value) => self.push(WireNode::String(value)),
            SourceKind::Array { length } => {
                if let Some(existing) = self.find(&value) {
                    return Ok(existing);
                }
                let node = self.push(WireNode::Array {
                    length,
                    properties: Vec::new(),
                })?;
                self.remember(&value, node)?;
                let keys = self
                    .source
                    .enumerable_own_keys(&value, &mut self.reservation)?;
                self.stack
                    .try_reserve(1)
                    .map_err(|_| self.allocation_failed())?;
                self.stack.push(Frame {
                    value,
                    node,
                    keys,
                    next: 0,
                });
                Ok(node)
            }
            SourceKind::Object => {
                if let Some(existing) = self.find(&value) {
                    return Ok(existing);
                }
                let node = self.push(WireNode::Object {
                    properties: Vec::new(),
                })?;
                self.remember(&value, node)?;
                let keys = self
                    .source
                    .enumerable_own_keys(&value, &mut self.reservation)?;
                self.stack
                    .try_reserve(1)
                    .map_err(|_| self.allocation_failed())?;
                self.stack.push(Frame {
                    value,
                    node,
                    keys,
                    next: 0,
                });
                Ok(node)
            }
            SourceKind::ArrayBuffer => {
                // A transfer placeholder is found before inspecting its
                // bytes. A getter may detach the buffer while serializing;
                // that error is observed in the later transfer phase.
                if let Some(existing) = self.find(&value) {
                    return Ok(existing);
                }
                let bytes = self.source.copy_buffer(&value, &mut self.reservation)?;
                let node = self.push(WireNode::ArrayBuffer {
                    bytes,
                    transferred: false,
                })?;
                self.remember(&value, node)?;
                Ok(node)
            }
            SourceKind::Unsupported => Err(self.source.data_clone_error("unsupported value")),
        }
    }

    fn walk(&mut self) -> Result<(), S::Error> {
        let mut steps = 0usize;
        while let Some(frame) = self.stack.last_mut() {
            if steps.is_multiple_of(CHECKPOINT_INTERVAL) {
                self.source.checkpoint()?;
            }
            steps += 1;
            if frame.next == frame.keys.len() {
                self.stack.pop();
                continue;
            }
            let key = std::mem::take(&mut frame.keys[frame.next]);
            frame.next += 1;
            let parent = frame.node;
            let value = frame.value.clone();
            if !self.source.has_own(&value, &key)? {
                continue;
            }
            // `visit` pushes a child frame. The next sibling is therefore
            // not read until the entire child has been serialized.
            let input = self.source.get(&value, &key)?;
            let child = self.visit(input)?;
            let properties = match &mut self.nodes[parent] {
                WireNode::Array { properties, .. } | WireNode::Object { properties } => properties,
                _ => unreachable!("only composite nodes have DFS frames"),
            };
            if properties.try_reserve(1).is_err() {
                self.reservation.fail_allocation();
                return Err(self.source.budget_error());
            }
            properties.push((key, child));
        }
        Ok(())
    }
}

/// HTML StructuredSerializeWithTransfer, for the supported first-slice kinds.
/// Earlier buffers may already be detached if a later transfer item fails.
pub fn serialize_with_transfer<S: CloneSource>(
    source: &mut S,
    root: S::Value,
    transfer_list: TransferList<S::Value>,
) -> Result<WireGraph, S::Error> {
    let TransferList {
        items: transfer_list,
        reservation,
    } = transfer_list;
    let mut builder = Builder {
        source,
        transfer_list,
        nodes: Vec::new(),
        objects: Vec::new(),
        buckets: HashMap::new(),
        stack: Vec::new(),
        reservation,
    };
    let mut transfers = Vec::new();
    transfers
        .try_reserve_exact(builder.transfer_list.len())
        .map_err(|_| builder.allocation_failed())?;

    // HTML §2.7.7 step 2: transferable kind/shared/duplicate, without a
    // detached-state check or irreversible operation.
    for index in 0..builder.transfer_list.len() {
        let transfer = builder.transfer_list[index].clone();
        builder.source.checkpoint()?;
        if !matches!(
            builder.source.kind(&transfer, &mut builder.reservation)?,
            SourceKind::ArrayBuffer
        ) {
            return Err(builder.source.data_clone_error("invalid transfer item"));
        }
        if builder.find(&transfer).is_some() {
            return Err(builder.source.data_clone_error("duplicate transfer item"));
        }
        let node = builder.push(WireNode::TransferPending)?;
        builder.remember(&transfer, node)?;
        transfers.push(node);
    }

    // Step 3: DFS serialization can invoke page getters and mutate transfer
    // buffers. No transfer data is captured until those getters finish.
    let root = builder.visit(root)?;
    builder.walk()?;

    // Step 5: process the original list in order. Do not roll back an earlier
    // successful detach when a later buffer is detached or non-detachable.
    for (index, &node) in transfers.iter().enumerate() {
        let transfer = builder.transfer_list[index].clone();
        builder.source.checkpoint()?;
        let bytes = builder
            .source
            .copy_buffer(&transfer, &mut builder.reservation)?;
        builder.source.detach_buffer(&transfer)?;
        builder.nodes[node] = WireNode::ArrayBuffer {
            bytes,
            transferred: true,
        };
    }

    debug_assert!(
        !builder
            .nodes
            .iter()
            .any(|node| matches!(node, WireNode::TransferPending))
    );
    // No transfer handle survives into the Send wire. Dropping them here keeps
    // the reservation in Builder live through their native finalization.
    builder.transfer_list.clear();
    Ok(WireGraph {
        root,
        nodes: builder.nodes,
        transfers,
        reservation: builder.reservation,
    })
}

/// Target-realm creation uses native own-data definitions, never ordinary
/// assignment: an inherited `__proto__` setter must not run while decoding.
pub trait CloneTarget {
    type Value: Clone;
    type Error;

    fn undefined(&mut self) -> Result<Self::Value, Self::Error>;
    fn null(&mut self) -> Result<Self::Value, Self::Error>;
    fn boolean(&mut self, value: bool) -> Result<Self::Value, Self::Error>;
    fn number(&mut self, bits: u64) -> Result<Self::Value, Self::Error>;
    fn string(&mut self, units: &[u16]) -> Result<Self::Value, Self::Error>;
    fn array(&mut self, length: u32) -> Result<Self::Value, Self::Error>;
    fn object(&mut self) -> Result<Self::Value, Self::Error>;
    fn array_buffer(&mut self, bytes: Vec<u8>) -> Result<Self::Value, Self::Error>;
    fn define_data(
        &mut self,
        owner: &Self::Value,
        key: &[u16],
        value: &Self::Value,
    ) -> Result<(), Self::Error>;
    fn invalid_wire(&mut self) -> Self::Error;
    fn budget_error(&mut self) -> Self::Error;
    fn checkpoint(&mut self) -> Result<(), Self::Error>;
}

struct DecodeFrame<V> {
    value: V,
    properties: Vec<(JsString, NodeId)>,
    next: usize,
    /// Define the parent's property only after the child's DFS frame has
    /// completed, matching StructuredDeserialize's recursive order.
    pending: Option<(JsString, NodeId)>,
}

type DecodeStep<V> = (V, Option<DecodeFrame<V>>);

struct Decoder<'a, T: CloneTarget> {
    target: &'a mut T,
    values: Vec<Option<T::Value>>,
    // WireGraph owns native nodes first and its reservation last. Keeping the
    // wire intact also covers every failure before Decoder is constructed.
    wire: WireGraph,
}

impl<T: CloneTarget> Decoder<'_, T> {
    fn create(&mut self, node: NodeId) -> Result<DecodeStep<T::Value>, T::Error> {
        if node >= self.wire.nodes.len() {
            return Err(self.target.invalid_wire());
        }
        if let Some(value) = &self.values[node] {
            return Ok((value.clone(), None));
        }
        let (value, properties) = match &mut self.wire.nodes[node] {
            WireNode::Undefined => (self.target.undefined()?, None),
            WireNode::Null => (self.target.null()?, None),
            WireNode::Boolean(value) => (self.target.boolean(*value)?, None),
            WireNode::Number(bits) => (self.target.number(*bits)?, None),
            WireNode::String(units) => (self.target.string(units)?, None),
            WireNode::Array { length, properties } => {
                let value = self.target.array(*length)?;
                (value, Some(std::mem::take(properties)))
            }
            WireNode::Object { properties } => {
                let value = self.target.object()?;
                (value, Some(std::mem::take(properties)))
            }
            WireNode::ArrayBuffer { bytes, .. } => {
                let value = self.target.array_buffer(std::mem::take(bytes))?;
                (value, None)
            }
            WireNode::TransferPending => return Err(self.target.invalid_wire()),
        };
        self.values[node] = Some(value.clone());
        Ok((
            value.clone(),
            properties.map(|properties| DecodeFrame {
                value,
                properties,
                next: 0,
                pending: None,
            }),
        ))
    }
}

/// Recreates all transfer holders first, then the root graph in a target realm.
/// The input wire is consumed so buffer bytes can move into the target engine.
pub fn deserialize<T: CloneTarget>(
    target: &mut T,
    mut wire: WireGraph,
) -> Result<T::Value, T::Error> {
    let value_bytes = wire
        .nodes
        .len()
        .checked_mul(std::mem::size_of::<Option<T::Value>>())
        .and_then(|bytes| bytes.checked_mul(2))
        .and_then(|bytes| wire.nodes.len().checked_mul(256)?.checked_add(bytes))
        .ok_or_else(|| {
            wire.reservation.fail_allocation();
            target.budget_error()
        })?;
    if !wire.reservation.charge(value_bytes) {
        return Err(target.budget_error());
    }
    let mut values = Vec::new();
    values.try_reserve_exact(wire.nodes.len()).map_err(|_| {
        wire.reservation.fail_allocation();
        target.budget_error()
    })?;
    values.resize_with(wire.nodes.len(), || None);
    let root = wire.root;
    let mut decoder = Decoder {
        target,
        values,
        wire,
    };
    for index in 0..decoder.wire.transfers.len() {
        let node = decoder.wire.transfers[index];
        decoder.target.checkpoint()?;
        if !matches!(
            decoder.wire.nodes.get(node),
            Some(WireNode::ArrayBuffer {
                transferred: true,
                ..
            })
        ) {
            return Err(decoder.target.invalid_wire());
        }
        let (_, frame) = decoder.create(node)?;
        debug_assert!(frame.is_none());
    }
    let (root_value, root_frame) = decoder.create(root)?;
    let mut stack = Vec::new();
    if let Some(frame) = root_frame {
        stack.try_reserve(1).map_err(|_| {
            decoder.wire.reservation.fail_allocation();
            decoder.target.budget_error()
        })?;
        stack.push(frame);
    }
    let mut steps = 0usize;
    while let Some(mut frame) = stack.pop() {
        if steps.is_multiple_of(CHECKPOINT_INTERVAL) {
            decoder.target.checkpoint()?;
        }
        steps += 1;
        if let Some((key, child)) = frame.pending.take() {
            let value = decoder.values[child]
                .as_ref()
                .ok_or_else(|| decoder.target.invalid_wire())?;
            decoder.target.define_data(&frame.value, &key, value)?;
        }
        if frame.next == frame.properties.len() {
            continue;
        }
        let (key, child) = &mut frame.properties[frame.next];
        let key = std::mem::take(key);
        let child = *child;
        frame.next += 1;
        let (child_value, child_frame) = decoder.create(child)?;
        if let Some(child_frame) = child_frame {
            frame.pending = Some((key, child));
            stack.try_reserve(2).map_err(|_| {
                decoder.wire.reservation.fail_allocation();
                decoder.target.budget_error()
            })?;
            stack.push(frame);
            stack.push(child_frame);
        } else {
            decoder
                .target
                .define_data(&frame.value, &key, &child_value)?;
            stack.push(frame);
        }
    }
    Ok(root_value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[derive(Clone, Debug)]
    enum Value {
        Undefined,
        Object(u64),
        Buffer(u64),
    }

    #[derive(Default)]
    struct TestSource {
        objects: HashMap<u64, Vec<(JsString, Value)>>,
        buffers: HashMap<u64, Vec<u8>>,
        detached: Vec<u64>,
        reads: Vec<JsString>,
        detach_on_get: Option<(JsString, u64)>,
        rewrite_on_get: Option<(JsString, u64, Vec<u8>)>,
        replace_property_on_get: Option<(JsString, u64, JsString, Value)>,
        remove_property_on_get: Option<(JsString, u64, JsString)>,
        hide_property_on_get: Option<(JsString, u64, JsString)>,
        nonenumerable: Vec<(u64, JsString)>,
        checkpoints: usize,
        fail_checkpoint_at: Option<usize>,
    }

    impl CloneSource for TestSource {
        type Value = Value;
        type Error = &'static str;

        fn kind(
            &mut self,
            value: &Value,
            _workspace: &mut Reservation,
        ) -> Result<SourceKind, Self::Error> {
            Ok(match value {
                Value::Undefined => SourceKind::Undefined,
                Value::Object(_) => SourceKind::Object,
                // Brand remains ArrayBuffer after detachment. The later copy
                // detects the detached state.
                Value::Buffer(_) => SourceKind::ArrayBuffer,
            })
        }

        fn identity_hash(&mut self, value: &Value) -> u64 {
            match value {
                Value::Object(id) => id * 2,
                Value::Buffer(id) => id * 2 + 1,
                Value::Undefined => unreachable!("primitive identity is never consulted"),
            }
        }

        fn same_object(&mut self, left: &Value, right: &Value) -> bool {
            matches!((left, right), (Value::Object(a), Value::Object(b)) | (Value::Buffer(a), Value::Buffer(b)) if a == b)
        }

        fn enumerable_own_keys(
            &mut self,
            value: &Value,
            workspace: &mut Reservation,
        ) -> Result<Vec<JsString>, Self::Error> {
            let Value::Object(id) = value else {
                unreachable!("only ordinary objects have property frames")
            };
            let keys: Vec<_> = self.objects[id]
                .iter()
                .filter(|(key, _)| !self.nonenumerable.contains(&(*id, key.clone())))
                .map(|(key, _)| key.clone())
                .collect();
            for key in &keys {
                if !workspace.charge(PROPERTY_CHARGE + key.len() * 4) {
                    return Err("BudgetExceeded");
                }
            }
            Ok(keys)
        }

        fn has_own(&mut self, value: &Value, key: &[u16]) -> Result<bool, Self::Error> {
            let Value::Object(id) = value else {
                unreachable!("only ordinary objects have property frames")
            };
            Ok(self.objects[id].iter().any(|(item, _)| item == key))
        }

        fn get(&mut self, value: &Value, key: &[u16]) -> Result<Value, Self::Error> {
            self.reads.push(key.to_vec());
            if let Some((trigger, id)) = &self.detach_on_get
                && trigger == key
            {
                self.detached.push(*id);
            }
            if let Some((trigger, id, bytes)) = &self.rewrite_on_get
                && trigger == key
            {
                self.buffers.insert(*id, bytes.clone());
            }
            if let Some((trigger, id, property, replacement)) = &self.replace_property_on_get
                && trigger == key
            {
                let entries = self.objects.get_mut(id).unwrap();
                entries.retain(|(item, _)| item != property);
                entries.push((property.clone(), replacement.clone()));
            }
            if let Some((trigger, id, property)) = &self.remove_property_on_get
                && trigger == key
            {
                self.objects
                    .get_mut(id)
                    .unwrap()
                    .retain(|(item, _)| item != property);
            }
            if let Some((trigger, id, property)) = &self.hide_property_on_get
                && trigger == key
            {
                self.nonenumerable.push((*id, property.clone()));
            }
            let Value::Object(id) = value else {
                unreachable!("only ordinary objects have property frames")
            };
            Ok(self.objects[id]
                .iter()
                .find(|(item, _)| item == key)
                .unwrap()
                .1
                .clone())
        }

        fn copy_buffer(
            &mut self,
            value: &Value,
            workspace: &mut Reservation,
        ) -> Result<Vec<u8>, Self::Error> {
            let Value::Buffer(id) = value else {
                unreachable!("only buffers can be copied")
            };
            if self.detached.contains(id) {
                return Err("detached");
            }
            if !workspace.charge(self.buffers[id].len() * 2) {
                return Err("BudgetExceeded");
            }
            Ok(self.buffers[id].clone())
        }

        fn detach_buffer(&mut self, value: &Value) -> Result<(), Self::Error> {
            let Value::Buffer(id) = value else {
                unreachable!("only buffers can be detached")
            };
            self.detached.push(*id);
            Ok(())
        }

        fn data_clone_error(&mut self, _: &'static str) -> Self::Error {
            "DataCloneError"
        }

        fn budget_error(&mut self) -> Self::Error {
            "BudgetExceeded"
        }

        fn checkpoint(&mut self) -> Result<(), Self::Error> {
            self.checkpoints += 1;
            if self.fail_checkpoint_at == Some(self.checkpoints) {
                Err("deadline")
            } else {
                Ok(())
            }
        }
    }

    fn key(value: &str) -> JsString {
        value.encode_utf16().collect()
    }

    fn test_budget(limit: usize) -> CloneBudget {
        CloneBudget::with_limits(limit, limit, 0, 0)
    }

    fn test_transfers(
        source: &mut TestSource,
        budget: &CloneBudget,
        items: &[Value],
    ) -> TransferList<Value> {
        let mut items = items.iter().cloned();
        collect_transfer_list(
            source,
            budget.begin(NativeMemoryComponent::StructuredClone),
            |_| Ok(items.next()),
        )
        .unwrap()
    }

    #[test]
    fn nested_getters_run_in_depth_first_order_and_share_identity() {
        let mut source = TestSource::default();
        source
            .objects
            .insert(1, vec![(key("child"), Value::Undefined)]);
        source.objects.insert(
            2,
            vec![(key("a"), Value::Object(1)), (key("b"), Value::Object(1))],
        );
        let wire = serialize_with_transfer(
            &mut source,
            Value::Object(2),
            TransferList::new(
                test_budget(1024 * 1024).begin(NativeMemoryComponent::StructuredClone),
            ),
        )
        .unwrap();
        assert_eq!(source.reads, vec![key("a"), key("child"), key("b")]);
        let WireNode::Object { properties } = &wire.nodes[wire.root] else {
            panic!("root object")
        };
        assert_eq!(properties[0].1, properties[1].1);
    }

    #[test]
    fn deleted_and_readded_key_is_read_with_its_new_value() {
        let mut source = TestSource::default();
        source.objects.insert(
            1,
            vec![(key("a"), Value::Undefined), (key("b"), Value::Undefined)],
        );
        source
            .objects
            .insert(2, vec![(key("new"), Value::Undefined)]);
        source.replace_property_on_get = Some((key("a"), 1, key("b"), Value::Object(2)));
        let wire = serialize_with_transfer(
            &mut source,
            Value::Object(1),
            TransferList::new(
                test_budget(1024 * 1024).begin(NativeMemoryComponent::StructuredClone),
            ),
        )
        .unwrap();
        assert_eq!(source.reads, vec![key("a"), key("b"), key("new")]);
        let WireNode::Object { properties } = &wire.nodes[wire.root] else {
            panic!("root object")
        };
        assert_eq!(properties.len(), 2);
        assert!(matches!(
            wire.nodes[properties[1].1],
            WireNode::Object { .. }
        ));
    }

    #[test]
    fn deleted_key_is_skipped_but_a_later_enumerability_change_is_not() {
        let mut source = TestSource::default();
        source.objects.insert(
            1,
            vec![
                (key("a"), Value::Undefined),
                (key("b"), Value::Undefined),
                (key("c"), Value::Undefined),
            ],
        );
        source.remove_property_on_get = Some((key("a"), 1, key("b")));
        source.hide_property_on_get = Some((key("a"), 1, key("c")));
        let wire = serialize_with_transfer(
            &mut source,
            Value::Object(1),
            TransferList::new(
                test_budget(1024 * 1024).begin(NativeMemoryComponent::StructuredClone),
            ),
        )
        .unwrap();
        assert_eq!(source.reads, vec![key("a"), key("c")]);
        let WireNode::Object { properties } = &wire.nodes[wire.root] else {
            panic!("root object")
        };
        assert_eq!(properties.len(), 2);
        assert_eq!(properties[1].0, key("c"));
    }

    #[test]
    fn an_unused_transfer_detaches_after_getters_and_captures_their_latest_bytes() {
        let mut source = TestSource::default();
        source
            .objects
            .insert(1, vec![(key("rewrite"), Value::Undefined)]);
        source.buffers.insert(7, vec![1, 2]);
        source.rewrite_on_get = Some((key("rewrite"), 7, vec![4, 5, 6]));
        let transfer_list =
            test_transfers(&mut source, &test_budget(1024 * 1024), &[Value::Buffer(7)]);
        let wire = serialize_with_transfer(&mut source, Value::Object(1), transfer_list).unwrap();
        assert_eq!(source.detached, vec![7]);
        assert_eq!(wire.transfers.len(), 1);
        assert_eq!(
            wire.nodes[wire.transfers[0]],
            WireNode::ArrayBuffer {
                bytes: vec![4, 5, 6],
                transferred: true,
            }
        );
    }

    #[test]
    fn a_later_detached_item_does_not_roll_back_an_earlier_transfer() {
        let mut source = TestSource::default();
        source
            .objects
            .insert(1, vec![(key("detachSecond"), Value::Undefined)]);
        source.buffers.insert(7, vec![1]);
        source.buffers.insert(8, vec![2]);
        source.detach_on_get = Some((key("detachSecond"), 8));
        let transfer_list = test_transfers(
            &mut source,
            &test_budget(1024 * 1024),
            &[Value::Buffer(7), Value::Buffer(8)],
        );
        let error =
            serialize_with_transfer(&mut source, Value::Object(1), transfer_list).unwrap_err();
        assert_eq!(error, "detached");
        assert_eq!(source.detached, vec![8, 7]);
    }

    #[test]
    fn duplicate_transfers_fail_before_graph_getters_or_detachment() {
        let mut source = TestSource::default();
        source
            .objects
            .insert(1, vec![(key("getter"), Value::Undefined)]);
        source.buffers.insert(7, vec![1]);
        let transfer_list = test_transfers(
            &mut source,
            &test_budget(1024 * 1024),
            &[Value::Buffer(7), Value::Buffer(7)],
        );
        let error =
            serialize_with_transfer(&mut source, Value::Object(1), transfer_list).unwrap_err();
        assert_eq!(error, "DataCloneError");
        assert!(source.reads.is_empty());
        assert!(source.detached.is_empty());
    }

    #[test]
    fn an_unbounded_transfer_iterator_stops_at_the_watchdog_checkpoint() {
        let budget = test_budget(1024 * 1024);
        let mut source = TestSource {
            fail_checkpoint_at: Some(2),
            ..TestSource::default()
        };
        let mut next_calls = 0usize;
        let result = collect_transfer_list(
            &mut source,
            budget.begin(NativeMemoryComponent::StructuredClone),
            |_| {
                next_calls += 1;
                Ok(Some(Value::Buffer(7)))
            },
        );
        assert_eq!(result.err(), Some("deadline"));
        assert_eq!(next_calls, CHECKPOINT_INTERVAL);
        assert_eq!(source.checkpoints, 2);
        assert_eq!(budget.used(NativeMemoryComponent::StructuredClone), 0);
        assert!(
            !budget.first_failure().is_some(),
            "a deadline is not a memory failure"
        );
    }

    #[test]
    fn a_large_transfer_iterator_hits_the_shared_cap_before_serialization() {
        let budget = test_budget(4096);
        let mut source = TestSource::default();
        let mut next_calls = 0usize;
        let result = collect_transfer_list(
            &mut source,
            budget.begin(NativeMemoryComponent::StructuredClone),
            |_| {
                next_calls += 1;
                Ok(Some(Value::Buffer(7)))
            },
        );
        assert_eq!(result.err(), Some("BudgetExceeded"));
        assert!(next_calls < CHECKPOINT_INTERVAL);
        assert!(budget.first_failure().is_some());
        assert_eq!(budget.used(NativeMemoryComponent::StructuredClone), 0);
    }

    #[test]
    fn a_reentrant_clone_from_transfer_iteration_shares_the_page_ledger() {
        let budget = test_budget(1500);
        let mut source = TestSource::default();
        let mut nested_wire = None;
        let mut next_calls = 0usize;
        let result = collect_transfer_list(
            &mut source,
            budget.begin(NativeMemoryComponent::StructuredClone),
            |source| {
                if next_calls == 0 {
                    nested_wire = Some(serialize_with_transfer(
                        source,
                        Value::Undefined,
                        TransferList::new(budget.begin(NativeMemoryComponent::StructuredClone)),
                    )?);
                }
                next_calls += 1;
                Ok(Some(Value::Buffer(7)))
            },
        );
        assert_eq!(result.err(), Some("BudgetExceeded"));
        assert!(nested_wire.is_some());
        assert!(
            budget.used(NativeMemoryComponent::StructuredClone) > 0,
            "the nested wire still owns its charge"
        );
        drop(nested_wire);
        assert_eq!(budget.used(NativeMemoryComponent::StructuredClone), 0);
    }

    #[derive(Default)]
    struct TestTarget {
        created: usize,
        properties: Vec<(usize, JsString, usize)>,
        checks: usize,
    }

    impl CloneTarget for TestTarget {
        type Value = usize;
        type Error = &'static str;

        fn undefined(&mut self) -> Result<usize, Self::Error> {
            self.created += 1;
            Ok(self.created)
        }
        fn null(&mut self) -> Result<usize, Self::Error> {
            self.undefined()
        }
        fn boolean(&mut self, _: bool) -> Result<usize, Self::Error> {
            self.undefined()
        }
        fn number(&mut self, _: u64) -> Result<usize, Self::Error> {
            self.undefined()
        }
        fn string(&mut self, _: &[u16]) -> Result<usize, Self::Error> {
            self.undefined()
        }
        fn array(&mut self, _: u32) -> Result<usize, Self::Error> {
            self.undefined()
        }
        fn object(&mut self) -> Result<usize, Self::Error> {
            self.undefined()
        }
        fn array_buffer(&mut self, _: Vec<u8>) -> Result<usize, Self::Error> {
            self.undefined()
        }
        fn define_data(
            &mut self,
            owner: &usize,
            key: &[u16],
            value: &usize,
        ) -> Result<(), Self::Error> {
            self.properties.push((*owner, key.to_vec(), *value));
            Ok(())
        }
        fn invalid_wire(&mut self) -> Self::Error {
            "invalid wire"
        }
        fn budget_error(&mut self) -> Self::Error {
            "BudgetExceeded"
        }
        fn checkpoint(&mut self) -> Result<(), Self::Error> {
            self.checks += 1;
            Ok(())
        }
    }

    #[test]
    fn a_wire_retains_its_charge_until_decode_or_drop() {
        let budget = test_budget(1024 * 1024);
        let mut source = TestSource::default();
        source
            .objects
            .insert(1, vec![(key("child"), Value::Undefined)]);
        let wire = serialize_with_transfer(
            &mut source,
            Value::Object(1),
            TransferList::new(budget.begin(NativeMemoryComponent::StructuredClone)),
        )
        .unwrap();
        assert!(budget.used(NativeMemoryComponent::StructuredClone) > 0);
        let mut target = TestTarget::default();
        deserialize(&mut target, wire).unwrap();
        assert_eq!(budget.used(NativeMemoryComponent::StructuredClone), 0);
        assert_eq!(target.properties.len(), 1);
        assert!(target.checks > 0);
    }

    #[test]
    fn decode_over_budget_is_sticky_and_releases_the_source_reservation() {
        let mut source = TestSource::default();
        source
            .objects
            .insert(1, vec![(key("child"), Value::Undefined)]);
        let generous = test_budget(1024 * 1024);
        let wire = serialize_with_transfer(
            &mut source,
            Value::Object(1),
            TransferList::new(generous.begin(NativeMemoryComponent::StructuredClone)),
        )
        .unwrap();
        let wire_bytes = generous.used(NativeMemoryComponent::StructuredClone);
        drop(wire);
        assert_eq!(generous.used(NativeMemoryComponent::StructuredClone), 0);

        let tight = test_budget(wire_bytes + 1);
        let wire = serialize_with_transfer(
            &mut source,
            Value::Object(1),
            TransferList::new(tight.begin(NativeMemoryComponent::StructuredClone)),
        )
        .unwrap();
        let mut target = TestTarget::default();
        assert_eq!(deserialize(&mut target, wire), Err("BudgetExceeded"));
        assert!(tight.first_failure().is_some());
        assert_eq!(tight.used(NativeMemoryComponent::StructuredClone), 0);
        assert_eq!(
            target.created, 0,
            "decode reserves before a target is built"
        );
    }

    #[derive(Clone)]
    struct DropLoggedValue {
        id: u64,
        budget: CloneBudget,
        all_dropped_while_charged: Rc<Cell<bool>>,
    }

    impl Drop for DropLoggedValue {
        fn drop(&mut self) {
            if self.budget.total_used() == 0 {
                self.all_dropped_while_charged.set(false);
            }
        }
    }

    struct AbruptSource;

    impl CloneSource for AbruptSource {
        type Value = DropLoggedValue;
        type Error = &'static str;

        fn kind(
            &mut self,
            value: &Self::Value,
            _: &mut Reservation,
        ) -> Result<SourceKind, Self::Error> {
            Ok(if value.id == 1 {
                SourceKind::ArrayBuffer
            } else {
                SourceKind::Object
            })
        }

        fn identity_hash(&mut self, value: &Self::Value) -> u64 {
            value.id
        }

        fn same_object(&mut self, left: &Self::Value, right: &Self::Value) -> bool {
            left.id == right.id
        }

        fn enumerable_own_keys(
            &mut self,
            _: &Self::Value,
            _: &mut Reservation,
        ) -> Result<Vec<JsString>, Self::Error> {
            Err("page getter failed")
        }

        fn has_own(&mut self, _: &Self::Value, _: &[u16]) -> Result<bool, Self::Error> {
            unreachable!()
        }

        fn get(&mut self, _: &Self::Value, _: &[u16]) -> Result<Self::Value, Self::Error> {
            unreachable!()
        }

        fn copy_buffer(
            &mut self,
            _: &Self::Value,
            _: &mut Reservation,
        ) -> Result<Vec<u8>, Self::Error> {
            unreachable!()
        }

        fn detach_buffer(&mut self, _: &Self::Value) -> Result<(), Self::Error> {
            unreachable!()
        }

        fn data_clone_error(&mut self, _: &'static str) -> Self::Error {
            "DataCloneError"
        }

        fn budget_error(&mut self) -> Self::Error {
            "BudgetExceeded"
        }

        fn checkpoint(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[test]
    fn abrupt_serialization_drops_transfer_handles_before_releasing_charge() {
        let budget = test_budget(1024 * 1024);
        let all_dropped_while_charged = Rc::new(Cell::new(true));
        let value = |id| DropLoggedValue {
            id,
            budget: budget.clone(),
            all_dropped_while_charged: Rc::clone(&all_dropped_while_charged),
        };
        let mut source = AbruptSource;
        let mut transfers = TransferList::new(budget.begin(NativeMemoryComponent::StructuredClone));
        transfers.push(&mut source, value(1)).unwrap();
        assert_eq!(
            serialize_with_transfer(&mut source, value(2), transfers).map(|_| ()),
            Err("page getter failed")
        );
        assert!(all_dropped_while_charged.get());
        assert_eq!(budget.total_used(), 0);
    }

    #[test]
    fn the_wire_can_cross_agent_threads() {
        fn require_send<T: Send>() {}
        require_send::<WireGraph>();
    }

    #[test]
    fn per_entry_allowances_cover_the_wire_and_mock_handle_footprints() {
        // Growth slack and HashMap bucket overhead still need the fixed part
        // of each charge; this guards accidental type-size expansion.
        assert!(std::mem::size_of::<WireNode>() <= NODE_CHARGE / 8);
        assert!(std::mem::size_of::<Frame<Value>>() <= NODE_CHARGE / 4);
        assert!(std::mem::size_of::<(JsString, NodeId)>() <= PROPERTY_CHARGE / 4);
        assert!(std::mem::size_of::<Value>() <= 64);
    }
}
