//! The engine-independent host object that backs every DOM wrapper.
//!
//! [`HostObject`] implements the generated `*Ops` traits over a shared
//! [`tenta_dom::Document`], so the same host serves both the V8 and QuickJS
//! glue (`docs/spec/05-javascript.md` §3). It is a small enum: a `Node` host
//! stands for one node, a `NodeList` host stands for the live child collection
//! of one node. Both hold a shared handle to the context (containing the document
//! and snapshot slab), so the arena and snapshots outlive every wrapper, and a
//! generational [`tenta_core::NodeRef`] so a wrapper kept across a sweep fails
//! the handle validation instead of aliasing a recycled slot.

use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::SystemTime;

#[cfg(any(feature = "v8", feature = "quickjs"))]
pub mod computed_style;
mod crypto;
#[cfg(any(feature = "v8", feature = "quickjs"))]
pub mod cssom;
mod dataset;
#[cfg(any(feature = "v8", feature = "quickjs"))]
pub mod doc_sheets;
mod element_interface;
mod focus;
mod forms;
mod geometry;
mod media;
mod mutation_observer;
mod request;
mod shadow;
pub use element_interface::html_element_interface;
pub use request::{RequestParts, RequestRecord, RequestSignal, request_parts};
pub mod storage;
mod tree_extras;

pub use focus::{
    blur_element, fire_lost_focus, flush_autofocus, focus_element, is_focusable_area, tab_index,
};
pub use mutation_observer::{
    MutationObserverRecord, MutationRecordRecord, NativeRetired, mark_mutation_microtask_enqueued,
    mutation_observers_awaiting_microtask, notify_mutation_observers,
};

use tenta_core::atom::atoms;
use tenta_core::{
    Atom, NativeMemoryBudget, NativeMemoryComponent, NativeMemoryReservation, NodeId, NodeRef,
    Origin,
};
#[cfg(any(feature = "v8", feature = "quickjs"))]
use tenta_css::{CssDeclarationBlock, supports_condition, supports_property};
use tenta_dom::arena::AttrName;
use tenta_dom::{
    Document, LiveRangeId, Mutation, NodeHandle, NodeKind, NodeOrText, ObserverAgent,
    ObserverAttachment, RangeBoundary, ReadyState, ReplaceAllWith, SharedObserverAgent,
};
use tenta_html::xml::{Parser as XmlParser, XmlOptions};
use tenta_html::{ElementFlags, Parser as HtmlParser, ParserOptions, QualName, QuirksMode};
use tenta_net::policy::cookies::{CookieJar, SameSiteContext, Site};
use url::Url;

// The generated name of the `URLSearchParams` constructor's `init` union. It is
// spelled out of the union's own members, so the alias is the only way to read
// this code.
use super::SequenceOfSequenceOfUsvStringOrRecordOfUsvStringToUsvStringOrUsvString as ParamsInit;
use super::{
    AbortControllerConstructor, AbortControllerOps, AbortSignal, AbortSignalOps, AbstractRangeOps,
    AddEventListenerOptionsOrBoolean, AnyValue, Attr, AttrOps, Blob, BlobConstructor, BlobOps,
    BlobOrUsvString, BlobPart, BlobPropertyBag, BodyInit, CDATASection, CDATASectionOps,
    CSSStyleProperties, Callback, CharacterDataOps, Comment, CommentOps,
    CompositionEventConstructor, CompositionEventInit, CompositionEventOps, CustomEventConstructor,
    CustomEventInit, CustomEventOps, DOMImplementation, DOMImplementationOps, DOMParserConstructor,
    DOMParserOps, DOMParserSupportedType, DOMStringMap, DOMTokenList, DOMTokenListOps,
    DocumentConstructor, DocumentFragment, DocumentFragmentOps, DocumentOps, DocumentType,
    DocumentTypeOps, Element, ElementOps, EndingType, ErrorEventConstructor, ErrorEventInit,
    ErrorEventOps, Event, EventConstructor, EventInit, EventListener,
    EventListenerOptionsOrBoolean, EventOps, EventTarget, EventTargetConstructor, EventTargetOps,
    FileConstructor, FileOps, FileOrUsvString, FilePropertyBag, FocusEventConstructor,
    FocusEventInit, FocusEventOps, FocusOptions, FormDataConstructor, FormDataOps,
    HTMLAnchorElementOps, HTMLCollection, HTMLCollectionOps, HTMLDivElementOps, HTMLElement,
    HTMLElementOps, HTMLFormElement, HTMLIFrameElementOps, HTMLImageElementOps,
    HTMLLabelElementOps, HTMLLinkElementOps, HTMLParagraphElementOps, HTMLScriptElementOps,
    HTMLSpanElementOps, HTMLStyleElementOps, HTMLTemplateElementOps, Handle, Headers,
    HeadersConstructor, HeadersInit, HeadersOps, IterableEntries, JsError,
    KeyboardEventConstructor, KeyboardEventInit, KeyboardEventOps, MouseEventConstructor,
    MouseEventInit, MouseEventOps, NamedNodeMap, NamedNodeMapOps, Node, NodeList, NodeListOps,
    NodeOps, NodeOrDomString, ProcessingInstruction, ProcessingInstructionOps,
    ProgressEventConstructor, ProgressEventInit, ProgressEventOps, Promise, Range,
    RangeConstructor, RangeOps, Realm, ReferrerPolicy, Request, RequestCache, RequestConstructor,
    RequestCredentials, RequestDestination, RequestDuplex, RequestInfo, RequestInit, RequestMode,
    RequestOps, RequestRedirect, Response, ResponseConstructor, ResponseInit, ResponseOps,
    ResponseType, SVGElementOps, StaticRangeConstructor, StaticRangeInit, StaticRangeOps, Text,
    TextDecodeOptions, TextDecoderConstructor, TextDecoderOps, TextDecoderOptions,
    TextEncoderConstructor, TextEncoderOps, TextOps, UIEventConstructor, UIEventInit, UIEventOps,
    URLConstructor, URLOps, URLSearchParams, URLSearchParamsConstructor, URLSearchParamsOps,
    WheelEventConstructor, WheelEventInit, WheelEventOps, Window, WindowProxy,
    XMLHttpRequestConstructor, XMLHttpRequestEventTargetOps, XMLHttpRequestOps,
    XMLHttpRequestResponseType, XMLSerializerConstructor, XMLSerializerOps,
};
#[cfg(any(feature = "v8", feature = "quickjs"))]
use super::{CSSOps, CSSStyleDeclarationOps};
use crate::dom::events::{
    CallbackRef, DispatchTarget, EventHandlerName, EventKey, EventKind, EventRecord, EventSlab,
    GenKey, GenSlab, HostValue, ListenerId, ListenerTable, PromiseOutcome, ScriptHost, TargetKey,
    dispatch, dispatch_at, dispatch_at_from,
};
use crate::event_loop::SharedClock;
use crate::fetch::{FetchOutcome, FetchRequest, FetchStartError, FetchedResponse, header_pairs};
use crate::image::{ImageEvent, ImageHost, ImageSnapshot};
use crate::location::{LocationPart, document_location};
use crate::storage::PageStorage;
use crate::xhr::{XhrHandle, XhrRequest};

/// A document shared between the host objects of one world and the engine that
/// drives it. Every `HostObject` for the document holds one of these clones, so
/// the arena outlives every wrapper.
///
/// Re-exported from `tenta-dom`, which parses into the same handle
/// (`tenta_dom::SharedSink`): the engine and the parser must name one type, or
/// a page would have to choose between running script and building the tree.
pub use tenta_dom::SharedDocument;

/// WebIDL's `EventHandlerNonNull` — `callback EventHandlerNonNull = any (Event
/// event)` — the type every `attribute EventHandler onX` carries.
///
/// `idl/xhr/XMLHttpRequestEventTarget.webidl` is where the declarations live
/// now, so the generator emits this alias into `crate::bindings` from
/// `[LegacyTreatNonObjectAsNull] callback EventHandlerNonNull`. This spelling
/// stays for the same reason: the two are type aliases of one another, so a
/// `Option<EventHandlerNonNull>` is the same type whichever name reaches it, and
/// the host's own `on*` surface reads better against the short name.
pub type EventHandlerNonNull = Callback<(Handle<Event>,), AnyValue>;

/// A key referencing a static snapshot in the slab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotKey {
    /// Index into the slab.
    pub index: u32,
    /// Generation counter to detect reuse.
    pub generation: u16,
}

/// One slot in the snapshot slab.
struct Slot {
    generation: u16,
    entry: Option<SnapshotEntry>,
}

/// A snapshot owns each result as a native handle, even when script has never
/// asked for that result's wrapper. A detached result keeps its entire root
/// alive until the snapshot wrapper is finalized.
struct SnapshotEntry {
    nodes: Vec<NodeRef>,
    _pins: SnapshotPins,
}

/// Releasing a `NodeHandle` requires its document, not just dropping the value.
/// This guard also rolls back partial insertion if a later pin fails or unwinds.
struct SnapshotPins {
    document: SharedDocument,
    handles: Vec<NodeHandle>,
}

impl Drop for SnapshotPins {
    fn drop(&mut self) {
        if self.handles.is_empty() {
            return;
        }
        let mut document = self.document.borrow_mut();
        for handle in self.handles.drain(..) {
            document.release(handle);
        }
    }
}

/// A slab backing static snapshot collections (querySelectorAll, getElementsByTagName, etc.).
/// Reuses slots via a free list, with generation counters to detect stale snapshot keys.
///
/// An exhausted 16-bit identity is retired permanently, so a weak cached
/// snapshot key cannot alias a later collection after repeated slot reuse.
pub struct SnapshotSlab {
    document: SharedDocument,
    slots: Vec<Slot>,
    free: Vec<u32>,
}

impl SnapshotSlab {
    /// Creates a new empty snapshot slab.
    pub fn new(document: SharedDocument) -> Self {
        Self {
            document,
            slots: Vec::new(),
            free: Vec::new(),
        }
    }

    /// Pins all result nodes before publishing a new snapshot key. A failed pin
    /// releases earlier pins and leaves the slab unchanged.
    pub fn insert(&mut self, nodes: Vec<NodeRef>) -> Result<SnapshotKey, JsError> {
        let mut pins = SnapshotPins {
            document: self.document.clone(),
            handles: Vec::with_capacity(nodes.len()),
        };
        for reference in &nodes {
            let handle = {
                let mut document = self.document.borrow_mut();
                let id = document
                    .resolve(*reference)
                    .map_err(|e| mutation_error(&e))?;
                document.handle(id).map_err(|e| mutation_error(&e))?
            };
            pins.handles.push(handle);
        }
        let (index, slot) = if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            (index, slot)
        } else {
            let index = self.slots.len() as u32;
            self.slots.push(Slot {
                generation: 0,
                entry: None,
            });
            (index, &mut self.slots[index as usize])
        };
        let generation = slot.generation;
        slot.entry = Some(SnapshotEntry { nodes, _pins: pins });
        Ok(SnapshotKey { index, generation })
    }

    /// Retrieves the nodes for a snapshot key, returning None if the key is stale or out of range.
    pub fn get(&self, key: SnapshotKey) -> Option<&[NodeRef]> {
        let slot = self.slots.get(key.index as usize)?;
        if slot.generation != key.generation {
            return None;
        }
        slot.entry.as_ref().map(|entry| entry.nodes.as_slice())
    }

    /// Marks a snapshot as dead and releases its pins. The slot is reusable
    /// only while its generation can advance without repeating an old key.
    pub fn remove(&mut self, key: SnapshotKey) {
        let slot = match self.slots.get_mut(key.index as usize) {
            Some(s) => s,
            None => return,
        };
        if slot.generation != key.generation {
            return; // Already reused.
        }
        let Some(entry) = slot.entry.take() else {
            return;
        };
        if let Some(next) = slot.generation.checked_add(1) {
            slot.generation = next;
            self.free.push(key.index);
        }
        drop(entry);
    }
}

// ---------------------------------------------------------------------------
// Host records: URL, URLSearchParams, Headers, AbortController, AbortSignal,
// Response, TextDecoder, TextEncoder, XMLHttpRequest, Blob, File and FormData
// ---------------------------------------------------------------------------

/// A key into a world's [`HostSlab`]; the payload of a [`TAG_HOST`] handle.
pub type HostKey = GenKey;

/// The generational slab of live `URL`, `URLSearchParams`, `Headers`,
/// `XMLHttpRequest`, `Blob`, `File` and `FormData` records for one world.
///
/// It is a [`GenSlab`] rather than a [`SnapshotSlab`] because a host record has
/// no document row to live in and no other referent: the wrapper is its owner,
/// so the world frees the record when the wrapper is finalized, exactly as it
/// frees an event record. A snapshot, by contrast, belongs to the context.
///
/// "The wrapper is its owner" is the default and not the rule: a record a
/// second record points at — a URL's query object, a signal's controller, a
/// `Response`'s headers, a `FormData`'s entries — outlives its wrapper and is
/// freed by whichever of the two dies second
/// ([`reclaim_host_record`]).
pub type HostSlab = GenSlab<HostRecord>;

/// The state behind one JS-owned host object without an arena node slot.
pub enum HostRecord {
    /// A standalone EventTarget; listeners use its exact generational key.
    EventTarget,
    /// A `URL`.
    Url(UrlRecord),
    /// A `URLSearchParams`.
    Params(ParamsRecord),
    /// A `Headers`.
    Headers(HeadersRecord),
    /// An `AbortController`.
    Controller(ControllerRecord),
    /// An `AbortSignal`.
    Signal(SignalRecord),
    /// A `Response`.
    Response(ResponseRecord),
    /// A `Request`.
    Request(RequestRecord),
    /// A `TextDecoder`.
    TextDecoder(TextDecoderRecord),
    /// A `TextEncoder`, which has no state at all.
    TextEncoder,
    /// A `DOMParser`, whose context supplies the parsed document's URL and world.
    DomParser,
    /// An `XMLSerializer`, which has no state of its own.
    XmlSerializer,
    /// An `XMLHttpRequest`.
    Xhr(XhrRecord),
    /// A `Blob`.
    Blob(BlobRecord),
    /// A `File`, which is a `Blob` with a name and a modification time.
    File(FileRecord),
    /// A `FormData`: its entry list, and nothing else.
    FormData(FormDataRecord),
    /// A `DOMImplementation`, which has no state at all: it is a per-document
    /// object whose whole identity is the record's key, so that
    /// `document.implementation === document.implementation` holds while a
    /// second document's implementation is a different object.
    ///
    /// This record was the reason the host needed a document *slot*: the key
    /// alone cannot say which document an implementation belongs to, because
    /// two documents' slabs are independent and their first minted key is the
    /// same number.
    Implementation,
    /// One entry of an element's attribute list — an `Attr`.
    ///
    /// It is a host record rather than an arena node: the record carries the
    /// element and attribute name while its wrapper uses the Node prototype.
    Attr(AttrRecord),
    /// A `Range`.
    ///
    /// It is a host record for the reason an `Attr` is: a range names *two*
    /// nodes, and a handle's node field has room for one. It spends no tag —
    /// it shares [`TAG_HOST`] and is named by this variant — which is what
    /// keeps the handle's four tag bits available for the classes no record
    /// can multiplex (`ShadowRoot`, `MutationObserver`, `XMLDocument`).
    Range(RangeRecord),
    /// A `StaticRange`, whose four members are copied at construction and
    /// never re-derived from the tree.
    StaticRange(StaticRangeRecord),
    /// A constructed sheet, shared with live rule and declaration wrappers.
    #[cfg(any(feature = "v8", feature = "quickjs"))]
    CssSheet(Rc<cssom::CssSheetState>),
    /// A live top-level or child rule list for one constructed sheet.
    #[cfg(any(feature = "v8", feature = "quickjs"))]
    CssRuleList(cssom::CssRuleListRecord),
    /// A rule of a sheet, of whatever kind; its interface follows the kind.
    #[cfg(any(feature = "v8", feature = "quickjs"))]
    CssRule(cssom::CssRuleRecord),
    /// One keyframe of an `@keyframes` rule.
    #[cfg(any(feature = "v8", feature = "quickjs"))]
    CssKeyframe(cssom::CssKeyframeRecord),
    /// The live declaration block of a rule or keyframe, or the descriptors of
    /// an `@font-face` rule.
    #[cfg(any(feature = "v8", feature = "quickjs"))]
    CssRuleStyle(cssom::CssStyleRecord),
    /// The live media query list of an `@media` rule.
    #[cfg(any(feature = "v8", feature = "quickjs"))]
    CssMediaList(cssom::CssRuleRecord),
    /// The result of `getComputedStyle()`: a live, read-only declaration.
    #[cfg(any(feature = "v8", feature = "quickjs"))]
    ComputedStyle(computed_style::ComputedStyleRecord),
    /// `document.styleSheets`: a live list with no state of its own.
    StyleSheetList,
    /// A `DOMRect` or `DOMRectReadOnly`.
    DomRect(geometry::DomRectRecord),
    /// A `DOMRectList`.
    DomRectList(geometry::DomRectListRecord),
    /// One Window's live local or session storage view.
    Storage(storage::StorageRecord),
    /// The per-Window Web Crypto object; it has no native graph edges.
    Crypto,
    /// A `MutationObserver`: its callback and its native observer.
    MutationObserver(MutationObserverRecord),
    /// A `MutationRecord`: one published native record.
    MutationRecord(MutationRecordRecord),
}

/// The state behind one `Attr`.
///
/// An `Attr` is a Node in WebIDL but has no arena slot: an attribute lives in
/// its element's packed attribute list. The host record gives it stable wrapper
/// identity and the inherited Node surface without adding an arena node.
///
/// An attribute the page built with `createAttribute` has no element until it is
/// attached, so [`Self::value`] is the record's own state until then and the
/// element's storage is the source of truth afterwards. That is the standard's
/// own split: a detached `Attr` owns its value, an attached one reflects the
/// element's.
pub struct AttrRecord {
    /// The element the attribute is attached to, or `None` for a detached
    /// attribute.
    pub element: Option<NodeRef>,
    /// The attribute's namespace, prefix and local name.
    pub name: QualName,
    /// The value a *detached* attribute carries. It is ignored — and never
    /// written — while `element` is set.
    pub detached_value: String,
    /// Keeps an attached Attr's owner element alive while this record exists.
    owner_pin: Option<AttrOwnerPin>,
    /// The per-Attr empty child list, cached for Node.childNodes [SameObject].
    child_nodes: Option<SnapshotKey>,
}

/// An attached attribute's lookup identity ignores its prefix. A value write
/// preserves the old prefix; replacing the whole Attr detaches the old record.
type AttrIdentity = (NodeRef, Atom, Atom);

fn attr_identity(element: NodeRef, name: QualName) -> AttrIdentity {
    (element, name.ns, name.local)
}

/// A strong owner-element reference. The document's handle count, rather than
/// HAS_WRAPPER, supports simultaneous element and Attr wrappers correctly.
struct AttrOwnerPin {
    doc: SharedDocument,
    handle: Option<NodeHandle>,
}

impl AttrOwnerPin {
    fn new(doc: SharedDocument, element: NodeId) -> Result<Self, JsError> {
        let handle = doc
            .borrow_mut()
            .handle(element)
            .map_err(|e| mutation_error(&e))?;
        Ok(Self {
            doc,
            handle: Some(handle),
        })
    }
}

impl Drop for AttrOwnerPin {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.doc.borrow_mut().release(handle);
        }
    }
}

impl AttrRecord {
    /// A detached attribute with the given name and the empty value.
    pub fn detached(name: QualName) -> Self {
        Self {
            element: None,
            name,
            detached_value: String::new(),
            owner_pin: None,
            child_nodes: None,
        }
    }

    /// The attribute's qualified name: `prefix:local`, or the local name when
    /// there is no prefix (<https://dom.spec.whatwg.org/#dom-attr-name>).
    pub fn qualified_name(&self) -> String {
        qualified_attr_name(&self.name)
    }
}

impl HostRecord {
    /// The WebIDL interface this record's wrapper carries, which is the name the
    /// wrapper world instantiates a template for. The handle tag cannot say it:
    /// every one of these shares [`TAG_HOST`], because a tag is eight bits of a
    /// handle and there is no room for one per class.
    pub fn interface_name(&self) -> &'static str {
        match self {
            HostRecord::EventTarget => "EventTarget",
            HostRecord::Url(_) => "URL",
            HostRecord::Params(_) => "URLSearchParams",
            HostRecord::Headers(_) => "Headers",
            HostRecord::Controller(_) => "AbortController",
            HostRecord::Signal(_) => "AbortSignal",
            HostRecord::Response(_) => "Response",
            HostRecord::Request(_) => "Request",
            HostRecord::TextDecoder(_) => "TextDecoder",
            HostRecord::TextEncoder => "TextEncoder",
            HostRecord::DomParser => "DOMParser",
            HostRecord::XmlSerializer => "XMLSerializer",
            HostRecord::Xhr(_) => "XMLHttpRequest",
            HostRecord::Blob(_) => "Blob",
            HostRecord::File(_) => "File",
            HostRecord::FormData(_) => "FormData",
            HostRecord::Implementation => "DOMImplementation",
            HostRecord::Attr(_) => "Attr",
            HostRecord::Range(_) => "Range",
            HostRecord::StaticRange(_) => "StaticRange",
            #[cfg(any(feature = "v8", feature = "quickjs"))]
            HostRecord::CssSheet(_) => "CSSStyleSheet",
            #[cfg(any(feature = "v8", feature = "quickjs"))]
            HostRecord::CssRuleList(_) => "CSSRuleList",
            #[cfg(any(feature = "v8", feature = "quickjs"))]
            HostRecord::CssRule(rule) => rule.interface_name(),
            #[cfg(any(feature = "v8", feature = "quickjs"))]
            HostRecord::CssKeyframe(_) => "CSSKeyframeRule",
            #[cfg(any(feature = "v8", feature = "quickjs"))]
            HostRecord::CssRuleStyle(style) => style.interface_name(),
            #[cfg(any(feature = "v8", feature = "quickjs"))]
            HostRecord::CssMediaList(_) => "MediaList",
            #[cfg(any(feature = "v8", feature = "quickjs"))]
            HostRecord::ComputedStyle(_) => "CSSStyleProperties",
            HostRecord::StyleSheetList => "StyleSheetList",
            HostRecord::DomRect(record) if record.read_only => "DOMRectReadOnly",
            HostRecord::DomRect(_) => "DOMRect",
            HostRecord::DomRectList(_) => "DOMRectList",
            HostRecord::Storage(_) => "Storage",
            HostRecord::Crypto => "Crypto",
            HostRecord::MutationObserver(_) => "MutationObserver",
            HostRecord::MutationRecord(_) => "MutationRecord",
        }
    }
}

/// One boundary point of a `Range`: a node and an offset into it.
///
/// The node is held as the *encoded handle bits*, not as a `NodeRef`, and that
/// is the whole reason a range can name nodes of a document other than the one
/// its own wrapper belongs to — `dom/common.js` builds ranges over
/// `foreignDoc`'s and `xmlDoc`'s nodes, and `xmlDoc` is a document a
/// `DOMImplementation` minted into a slot of its own. The bits carry that slot,
/// so resolving a point goes through the same [`HostObject::anchor_of`] every
/// other cross-document read goes through, with the same staleness check.
///
/// The offset is a `u32` because `unsigned long` is; the standard's
/// `IndexSizeError` is raised by the callers that validate it, not here.
#[derive(Clone, Copy, Debug)]
pub struct RangePoint {
    /// The encoded `TAG_NODE` handle of the node this point names.
    pub node: u64,
    /// The offset into `node`.
    pub offset: u32,
}

/// A live range registered in its endpoint document. The document owns its
/// boundary points and counted node handles; the host owns the registration.
/// Holding the document, rather than its context, creates no slab ownership cycle.
pub struct RangeRecord {
    doc: SharedDocument,
    slot: u16,
    registration: LiveRangeId,
}

impl RangeRecord {
    fn new(ctx: &SharedContext, start: RangePoint, end: RangePoint) -> Result<Self, JsError> {
        let (doc, start_node) = resolve_node_handle(ctx, start.node)?;
        let (end_doc, end_node) = resolve_node_handle(ctx, end.node)?;
        if !Rc::ptr_eq(&doc, &end_doc) {
            return Err(wrong_document());
        }
        let registration = {
            let mut document = doc.borrow_mut();
            let start = RangeBoundary {
                node: document.node_ref(start_node),
                offset: start.offset,
            };
            let end = RangeBoundary {
                node: document.node_ref(end_node),
                offset: end.offset,
            };
            document
                .register_live_range(start, end)
                .map_err(|error| mutation_error(&error))?
        };
        Ok(Self {
            doc,
            slot: decode(start.node).slot,
            registration,
        })
    }

    fn points(&self) -> Option<(RangePoint, RangePoint)> {
        let [start, end] = self.doc.borrow().live_range_points(self.registration)?;
        let point = |point: RangeBoundary| RangePoint {
            node: encode_node(self.slot, point.node),
            offset: point.offset,
        };
        Some((point(start), point(end)))
    }
}

impl Drop for RangeRecord {
    fn drop(&mut self) {
        self.doc.borrow_mut().release_live_range(self.registration);
    }
}

/// A retained endpoint of a static range. Its document can differ from the
/// other endpoint's, and neither the node nor the offset tracks mutations.
struct StaticRangePoint {
    point: RangePoint,
    doc: SharedDocument,
    handle: Option<NodeHandle>,
}

impl StaticRangePoint {
    fn new(ctx: &SharedContext, point: RangePoint) -> Result<Self, JsError> {
        let (doc, node) = resolve_node_handle(ctx, point.node)?;
        let handle = doc
            .borrow_mut()
            .handle(node)
            .map_err(|error| mutation_error(&error))?;
        Ok(Self {
            point,
            doc,
            handle: Some(handle),
        })
    }
}

impl Drop for StaticRangePoint {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.doc.borrow_mut().release(handle);
        }
    }
}

/// Immutable endpoints, retained until the static range's wrapper is reclaimed.
pub struct StaticRangeRecord {
    start: StaticRangePoint,
    end: StaticRangePoint,
}

/// A `URL` record.
pub struct UrlRecord {
    /// The URL itself: the source of truth for every attribute, and for the
    /// query its `URLSearchParams` observes.
    pub url: url::Url,
    /// The `[[searchParams]]` slot: the key of this URL's query object once one
    /// has been created, `None` until it is. Creating it lazily and caching it
    /// here is what both halves of the spec require — `url.searchParams` returns
    /// the *same* object on every access, and a URL that is never asked for one
    /// never builds it.
    pub params: Option<HostKey>,
    /// Whether this record still has a live wrapper.
    ///
    /// A URL record outlives its own wrapper while a live query object points at
    /// it, because the object writes through to it; so "the wrapper is gone" and
    /// "the record is free" are different states, and the record is freed only
    /// once both are true.
    pub has_wrapper: bool,
}

/// A `URLSearchParams` record.
pub struct ParamsRecord {
    /// The pair list. It is this object's own state when `url` is `None`; when
    /// the object is a URL's query object, the URL's query is the source of
    /// truth instead and this stays empty — a second copy is precisely the copy
    /// that goes stale when script assigns `url.search` or `url.href` behind the
    /// object's back.
    pub list: Vec<(String, String)>,
    /// The URL this object is the query object of, when it came from
    /// `url.searchParams`. The reference is the spec's own: the query object
    /// keeps its URL alive, and every mutation writes back through it.
    pub url: Option<HostKey>,
}

/// A `Headers` record.
///
/// A `Headers` a page built points at no second record: the wrapper is its only
/// owner, so the world's finalizer sweep frees it the way it frees an `Event`,
/// without the two-way bookkeeping [`reclaim_host_record`] needs for the URL
/// pair. The `Headers` a `Response` creates is the exception, and it is the
/// `Response` record that owns it — see [`HeadersRecord::response`].
pub type HeaderList = Vec<(Vec<u8>, Vec<u8>)>;

pub struct HeadersRecord {
    /// The header list, in insertion order.
    ///
    /// Insertion order is what `getSetCookie` and every mutation work on, and
    /// what the spec's *sort and combine* is applied to when a pair iterator
    /// asks for its view (`sort_and_combine_headers`). A name is stored
    /// byte-lowercased and a value already normalized, so a stored pair is
    /// exactly what the spec's *header list* holds.
    pub list: HeaderList,
    /// The guard the list is under.
    pub guard: HeadersGuard,
    /// The `Response` or the `Request` whose header list this is, while that
    /// owner's record is alive.
    ///
    /// The `Response` constructor creates this object eagerly, because
    /// `[SameObject]` makes every read of `response.headers` answer with the same
    /// object, and the response holds the key to it. That makes the *response*
    /// the owner of the *headers*, the `ControllerRecord`/`SignalRecord` pair
    /// read the other way round: the headers record outlives its own wrapper
    /// while the response that owns it lives, and is freed when the response's
    /// wrapper is finalized unless its own wrapper is still around.
    pub response: Option<HostKey>,
    /// Whether this record still has a live wrapper. See
    /// [`UrlRecord::has_wrapper`]: a headers record outlives its own wrapper
    /// while a live response still points at it.
    pub has_wrapper: bool,
}

/// A `Response` record.
///
/// The record *is* the spec's response plus the parts of the `Body` mixin this
/// slice implements. The body is a byte sequence rather than a stream
/// (`docs/spec/18-decisions.md`), so "the body's stream is disturbed" — the
/// state `bodyUsed` and *unusable* are defined in — is modelled as the
/// [`ResponseRecord::body_used`] flag: the body is read exactly once, and a
/// second read is the spec's *unusable* case. `body()` and the stream members
/// arrive with Streams (`d-5b`), which is when the bytes have to become a
/// `ReadableStream` rather than stand in for one.
pub struct ResponseRecord {
    /// The response's *status*.
    pub status: u16,
    /// The response's *status message*, which `statusText` reads back.
    ///
    /// Kept as the `ByteString` bytes the IDL declares rather than as a `String`:
    /// the value is checked against HTTP's *reason-phrase* production byte by
    /// byte, and obs-text (0x80 to 0xFF) is not UTF-8.
    pub status_text: Vec<u8>,
    /// The response's *URL*, or `None` when it has none. The `url` getter
    /// answers `""` for `None`, and a constructor-built response always has
    /// `None` — only a fetch sets one (`d-4a-1`).
    pub url: Option<String>,
    /// The response's *type*.
    pub response_type: ResponseType,
    /// Whether the response was *redirected*.
    ///
    /// The standard defines this as "the URL list's size is greater than 1",
    /// which is one bit of the conclusion: a constructor-built response has no
    /// URL list, and a fetch appends to it as it follows redirects (`d-4a-1`).
    pub redirected: bool,
    /// The response's header list: a `Headers` record this one owns.
    pub headers: HostKey,
    /// The response's *body*, or `None` for a null body.
    pub body: Option<Vec<u8>>,
    /// Whether the body has been read — the spec's "the body's stream is
    /// disturbed". A response with no body is never disturbed.
    pub body_used: bool,
}

impl ResponseRecord {
    /// Whether a status is Fetch's *null body status*: one that a response may
    /// not carry a body with.
    ///
    /// `101` and `103` are on the list for their use elsewhere in the protocol,
    /// not because a page ever sees one (`<https://fetch.spec.whatwg.org/#null-body-status>`).
    fn is_null_body_status(status: u16) -> bool {
        matches!(status, 101 | 103 | 204 | 205 | 304)
    }

    /// Whether a byte sequence matches HTTP's *reason-phrase* production —
    /// `1*( HTAB / SP / VCHAR / obs-text )`, so every byte is a tab, a space, a
    /// visible ASCII character or obs-text
    /// (<https://httpwg.org/specs/rfc9112.html#status.line>).
    ///
    /// The empty string is allowed by *initialize a response* before this is
    /// reached, so it is not allowed here: `new Response(null, {statusText: ""})`
    /// must not throw, and the caller checks for it.
    fn is_reason_phrase(status_text: &[u8]) -> bool {
        !status_text.is_empty()
            && status_text
                .iter()
                .all(|byte| matches!(byte, b'\t' | b' ' | 0x21..=0x7E | 0x80..=0xFF))
    }

    /// Whether this response is *unusable*: its body is non-null and has already
    /// been read.
    fn is_unusable(&self) -> bool {
        self.body.is_some() && self.body_used
    }
}

/// An `XMLHttpRequest`'s *response*: the fields the response members read.
///
/// A completed request is delivered as plain data
/// ([`FetchedResponse`](crate::fetch::FetchedResponse)), and this is that data
/// sanitized the way a `Response` record's is: a name Fetch calls a *forbidden
/// response-header name*, a name that is not an HTTP field name and a value
/// holding a NUL, CR or LF are each dropped rather than faulting the whole
/// response. Two records are now built from the same driver data, which is why
/// the sanitizing is applied here rather than in the driver.
pub struct XhrResponse {
    /// The response's *status*.
    status: u16,
    /// The response's *status message*.
    ///
    /// [`http::StatusCode::canonical_reason`]'s text rather than the reason
    /// phrase the server sent: `FetchedResponse` has no field for one, so the
    /// wire's is not available at this boundary. A status with no canonical
    /// reason reads back as the empty byte sequence.
    status_text: Vec<u8>,
    /// The response's *header list*: names byte-lowercased, values normalized,
    /// forbidden names dropped.
    headers: HeaderList,
    /// The response's *body*, or `None` for a null body.
    body: Option<Vec<u8>>,
    /// The response's URL after redirects, which `responseURL` reads back
    /// without its fragment.
    url: String,
}

impl XhrResponse {
    /// The response a completed request delivers.
    fn from_fetched(data: &FetchedResponse) -> XhrResponse {
        let context = "XMLHttpRequest";
        let status_text = http::StatusCode::from_u16(data.status)
            .ok()
            .and_then(|status| status.canonical_reason())
            .map_or_else(Vec::new, |reason| reason.as_bytes().to_vec());
        let headers = data
            .headers
            .iter()
            .filter_map(|(name, value)| {
                let (name, value) = checked_header(context, name.as_bytes(), value).ok()?;
                (!is_forbidden_response_header_name(&name)).then_some((name, value))
            })
            .collect();
        // A null-body status carries no body whatever the driver reported, the
        // rule `insert_fetched_response` applies for the same reason: the
        // status is what says a response may not have one. A body the driver
        // reports as `None` for any other status is a null body too — the
        // state machine keys on the body, not on the status.
        let body = match ResponseRecord::is_null_body_status(data.status) {
            true => None,
            false => data.body.clone(),
        };
        XhrResponse {
            status: data.status,
            status_text,
            headers,
            body,
            url: data.url.clone(),
        }
    }
}

/// An `XMLHttpRequest` record.
///
/// The record *is* the standard's request object, reduced to the part this
/// product's surface implements: the five states, the request `open()` sets up,
/// the four request attributes, the send flag and author request headers
/// `send()` and `setRequestHeader()` write, and the response a completion
/// stores. The request body is not one of the record's fields: `send()` converts
/// its argument and writes the resulting bytes straight into the
/// [`XhrRequest`] it hands the driver, so nothing has to hold a body between two
/// calls.
///
/// The record is also the object's *event target key*: an `XMLHttpRequest` is an
/// `EventTarget` that is neither a node nor the window, so its listeners live
/// under [`TargetKey::Host`] of this record's key, and the state machine's
/// `readystatechange` is dispatched at it. That is why the record has to outlive
/// its own wrapper's listeners rather than being rebuilt from them.
pub struct XhrRecord {
    /// The *state*, which the five constants name: `open()` and `abort()` move it
    /// between `UNSENT` and `OPENED`, and the response machine writes
    /// `HEADERS_RECEIVED`, `LOADING` and `DONE` as a completion is delivered.
    pub ready_state: u16,
    /// The *request method*, once `open()` has validated and normalised it.
    ///
    /// `send()` builds the `http::Method` out of this rather than out of an
    /// argument it would have to be handed a second time, and a state machine
    /// that forgot what it was opened with would not be the standard's.
    pub method: Option<String>,
    /// The *request URL*: the parsed `url` argument, resolved against the
    /// document's URL.
    pub url: Option<url::Url>,
    /// The *send() flag*: whether a request started by `send()` is still in
    /// flight.
    ///
    /// `open()` resets it (the standard's step 11) and `send()` sets it (step
    /// 10); every way a request ends clears it — the *request error steps* and
    /// *handle response end-of-body* both do, which is where the standard puts
    /// it, and the timeout's own callback reports "nothing is in flight" through
    /// the same clear. Its whole purpose is to make a second `send()` on an
    /// object with a request under way an `InvalidStateError`, which is why it is
    /// set before the request is built rather than after it succeeds.
    pub send_flag: bool,
    /// The *request id*: the identity of the request `send()` most recently
    /// started, and the value a completion is checked against before it is
    /// delivered.
    ///
    /// A completion is not addressed to the object alone. `send()` leaves the
    /// object at `OPENED` until the response's headers arrive, so `open()` can
    /// run with a request still in flight and leave two of them on one object —
    /// and the response state machine, which writes `ready_state` and `response`
    /// on this record, would then interleave the two answers. The standard closes
    /// that in `open()`'s step 10 by terminating the abandoned request, and this
    /// engine *also* terminates it there — but a driver is on the other side of
    /// the seam (`crate::xhr::XhrDriver`), so an answer can already be in its
    /// table, or in flight towards the drain, when the termination arrives. The
    /// id is what refuses that answer: it is bumped by `open()` and by the
    /// `send()` that starts a request, and by the three calls that end one
    /// without an answer — `open()`'s own step 10, `abort()`, and the `timeout`
    /// that fires, all three through [`end_xhr_request`] — and it is carried to
    /// the driver in [`XhrHandle`](crate::xhr::XhrHandle), compared in
    /// [`deliver_xhr_completion`], which discards a completion whose id is no
    /// longer this record's.
    ///
    /// A counter rather than an "is a request in flight" flag, for the reason the
    /// watchdog's generation bump and the detached-node sweep's exact-`NodeRef`
    /// match exist: a bare test cannot tell the abandoned request from the live
    /// one, since both are in flight on the same object. Wrapping is unreachable
    /// — it would take 2^64 `open()`/`send()` calls on one object.
    pub request_id: u64,
    /// The *author request headers*: the header list `setRequestHeader`
    /// combines into, and the one `send()` hands to the driver.
    ///
    /// Names are byte-lowercased the way every header list in this file stores
    /// them, so Fetch's byte-case-insensitive name comparison is a plain
    /// equality; values are byte sequences because a `ByteString` may hold
    /// obs-text. `open()` empties it (step 11) and nothing else writes it.
    pub author_headers: Vec<(Vec<u8>, Vec<u8>)>,
    /// The *timeout*, an `unsigned long`.
    ///
    /// A plain stored value, and the standard's *timed out* flag with it: the
    /// setter writes whatever it is given — 0 included, which is how a page
    /// takes the deadline off a request it has not sent yet — and `send()` arms
    /// the timer once, from this value, for the request it is starting
    /// (`arm_xhr_timeout`). Nothing here reads it at the deadline: the timer is
    /// what carries it, which is what makes a value set *after* `send()`
    /// govern no request already in flight.
    pub timeout: u32,
    /// *With credentials*. `true` is refused by the setter, because the shared
    /// cookie jar decides on its own whether credentials travel.
    pub with_credentials: bool,
    /// The *response type*, which the setter narrows to `""` and `"text"`.
    pub response_type: XMLHttpRequestResponseType,
    /// The *response*: the answer the last completion delivered, or `None`
    /// while there is none.
    ///
    /// `None` is the standard's *network error*, and it is the value a fresh
    /// object and a re-opened one both hold, which is what makes the response
    /// members answer `0` / `""` / `null` in `UNSENT` and `OPENED` without a
    /// rule of their own.
    ///
    /// Stored inline rather than as an owned host record: `open()` and a later
    /// `send()` *replace* the response, and a plain drop is the whole of that
    /// replacement, where an owned key would have to be freed at every
    /// replacement point as well as at wrapper death.
    pub response: Option<XhrResponse>,
    /// The `charset` of the MIME type `overrideMimeType` stored, or `None` when
    /// it has none.
    ///
    /// Only the `charset` is kept, because it is the only parameter a text
    /// response reads — and a value that fails to parse is stored as
    /// `application/octet-stream`, whose `charset` is likewise absent, so the
    /// two are one `None` here.
    ///
    /// `open()` does not clear it: the standard says so explicitly, because
    /// `overrideMimeType()` may be called before `open()`.
    pub override_charset: Option<Vec<u8>>,
    /// The value the `response` getter last built, and what it was built for.
    ///
    /// The standard caches the *response object* of an `arraybuffer`, `blob` or
    /// `json` response so that every read is the same object; the text is kept
    /// the same way so that a read does not store a new string each time. The
    /// bits carry one stored reference, released when the value is replaced and
    /// when the record is freed.
    pub response_value: Option<XhrResponseValue>,
}

/// A built `response` value and the state it belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XhrResponseValue {
    /// The stored value.
    pub bits: u64,
    /// The request, the state and the response type the value was built for. A
    /// read under any other combination builds a new one.
    pub built_for: (u64, u16, XMLHttpRequestResponseType),
}

/// A `Blob` record: the spec's *underlying byte sequence* and its `type`.
///
/// The record *is* the `Blob`: the standard gives it no other state, because
/// the byte sequence is snapshotted at construction and `slice()` returns a new
/// `Blob` rather than a view (compare the `Response` record, whose body is
/// consumed in place). Both fields are owned, so a `Blob` is self-contained and
/// the world's finalizer frees it the way it frees a `TextEncoder` — nothing
/// else points at it.
///
/// This slice deliberately does not model the standard's *snapshot state*
/// (<https://w3c.github.io/FileAPI/#snapshot-state>): a constructed `Blob` has
/// no underlying storage to take a snapshot of, and the only source of one here
/// would be a `File` from a file on disk, which this engine cannot produce.
#[derive(Debug)]
pub struct BlobRecord {
    /// The *underlying byte sequence*.
    pub bytes: Vec<u8>,
    /// The `type`, already normalized and stored in its ASCII-lowercase form.
    pub blob_type: String,
}

/// A `File` record: a [`BlobRecord`] plus the two members `File` adds.
///
/// The `Blob` half is nested rather than flattened so that the `Blob` members —
/// `size`, `type`, `slice()` and `text()` — are served from the one field both
/// variants share (see [`HostObject::with_blob`]): a `File` is a `Blob` first,
/// and reading its bytes must not depend on which of the two spellings the page
/// constructed it with.
#[derive(Debug)]
pub struct FileRecord {
    /// The `Blob` half: the byte sequence and the normalized type.
    pub blob: BlobRecord,
    /// The `name`, an unnormalized `USVString` — the standard stores it as it
    /// was given, and only notes that an OS filename is not the same thing.
    pub name: String,
    /// The `lastModified`, in milliseconds since the Unix Epoch.
    pub last_modified: i64,
    /// Whether this record still has a live wrapper. See
    /// [`UrlRecord::has_wrapper`], and [`FileRecord::owners`] for the second
    /// half of the rule.
    pub has_wrapper: bool,
    /// The `FormData` records whose entry lists hold this `File`, by key.
    ///
    /// This is the direction HTML's *create an entry* forces on a record's
    /// lifetime. Where a host record that only its wrapper can reach is freed
    /// when that wrapper is finalized, a `File` an entry list holds must outlive
    /// the *page's* reference to it — `formData.get(name)` has to answer with
    /// the same bytes after the value it was given has been dropped — and it
    /// must be freed once nothing holds it at all. So a file record is free
    /// exactly when [`FileRecord::is_free`] says so: no live wrapper **and** no
    /// owner.
    ///
    /// A `Vec` and not an `Option<HostKey>` because one `File` object can be
    /// appended to several `FormData`s, and each of them has to be able to let
    /// go of it independently. A key appears at most once per owner, which
    /// [`FileRecord::add_owner`] guarantees, and a mutation that drops the last
    /// entry naming a key takes the owner out
    /// ([`relinquish_files`]) rather than counting entries: an entry list
    /// holding the same `File` under two names is one reference here.
    ///
    /// A stale key — one whose `FormData` record has been freed — must be
    /// ignored rather than chased, for the same reason
    /// [`SignalRecord::controller`] may be stale: the generation in the key is
    /// what makes "the record is gone" and "the slot holds something else"
    /// distinguishable, and the reading side asks the slab.
    pub owners: Vec<HostKey>,
}

impl FileRecord {
    /// Records that the `FormData` at `owner` holds this `File`, once.
    fn add_owner(&mut self, owner: HostKey) {
        if !self.owners.contains(&owner) {
            self.owners.push(owner);
        }
    }

    /// Drops every reference the `FormData` at `owner` held to this `File`.
    fn remove_owner(&mut self, owner: HostKey) {
        self.owners.retain(|held| *held != owner);
    }

    /// Whether nothing refers to this record any more: neither a live wrapper
    /// nor an entry list. See [`FileRecord::owners`].
    fn is_free(&self) -> bool {
        !self.has_wrapper && self.owners.is_empty()
    }
}

/// A `FormData` record: the standard's *entry list*, and nothing else
/// (<https://xhr.spec.whatwg.org/#concept-formdata-entry-list>).
///
/// The list is the whole of the object's state. Its *values* are not bytes but
/// keys — see [`EntryValue::File`] — because `FormDataEntryValue` is
/// `(File or USVString)` and a browser keeps the `File` *object* the page
/// passed, so two reads of one entry have to answer with one object. A copy of
/// the bytes would be a different `File`.
#[derive(Debug)]
pub struct FormDataRecord {
    /// The entries, in insertion order, duplicates and all: `append` adds an
    /// entry without looking at the names already present, and the order is
    /// observable through every iterator.
    pub list: Vec<(String, EntryValue)>,
}

/// One entry's *value*: the `FormDataEntryValue` union, stored the way the
/// host keeps each half.
///
/// The `File` half is a record key rather than a `FileRecord` so that the
/// record's identity survives: the wrapper world memoises one wrapper per
/// handle, so `formData.get('k')` twice is one JavaScript object, and the
/// `File` a page passed in stays that same object
/// (`wpt/checkout/xhr/formdata/set-blob.any.js`'s `assert_file_identity`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryValue {
    /// The `USVString` half: a scalar value string.
    String(String),
    /// The `File` half, held as the key of the `File` record it names. That
    /// key is one of the record's [`FileRecord::owners`], which is what keeps
    /// the record alive while this entry holds it.
    File(HostKey),
}

/// An `AbortController` record.
///
/// The controller is the pair's *owner*: `[SameObject]` requires every read of
/// `signal` to answer with the same object, so the signal record outlives its
/// own wrapper for as long as the controller's record does. The link runs one
/// way — the DOM Standard gives `AbortSignal` no `controller` attribute, so
/// nothing script can reach leads back — and [`SignalRecord::controller`] exists
/// only so that reclamation knows which of the two records is keeping the other
/// alive.
#[derive(Debug)]
pub struct ControllerRecord {
    /// The `AbortSignal` this controller aborts, created by the constructor and
    /// never replaced.
    pub signal: HostKey,
}

/// An `AbortSignal` record.
///
/// The abort state *is* the abort reason. The DOM Standard defines "an
/// `AbortSignal` object is aborted when its abort reason is not `undefined`"
/// (<https://dom.spec.whatwg.org/#abortsignal-aborted>), so one field answers
/// both `aborted` and `reason` and the two can never disagree.
#[derive(Debug)]
pub struct SignalRecord {
    /// The *abort reason*: the interned bits of the JavaScript value `reason`
    /// reads back, or `None` while the signal is not aborted — the reason is
    /// then `undefined`, which is the one value the prelude has no sentinel for
    /// and is produced through [`ScriptHost::build_value`].
    ///
    /// `Some(AnyValue::NULL.0)` is the aborted-with-`null` case: a value, but
    /// not a *stored* one, so freeing the record must not release it.
    pub reason: Option<u64>,
    /// The controller that aborts this signal, while that controller's record is
    /// still alive. Set by the constructor and cleared when the controller's
    /// wrapper is finalized, which is what makes the two reclamation orders
    /// converge however the pair is collected.
    pub controller: Option<HostKey>,
    /// Whether this record still has a live wrapper. See
    /// [`UrlRecord::has_wrapper`]: a signal record outlives its own wrapper
    /// while a live controller still points at it.
    pub has_wrapper: bool,
}

impl SignalRecord {
    /// The name a default abort reason carries. The DOM Standard's two producers
    /// are `"AbortError"` (`abort()`) and `"TimeoutError"` (`AbortSignal.timeout`,
    /// deferred); the message is this engine's own, since the standard states
    /// none.
    const DEFAULT_REASON_NAME: &'static str = "AbortError";

    /// The message of a default abort reason.
    const DEFAULT_REASON_MESSAGE: &'static str = "The operation was aborted.";
}

/// The Fetch Standard's *guard* for a header list.
///
/// A guard is a property of the instance, not of the list: the same header
/// bytes are mutable on a `Headers` a page built and read-only on the
/// `Headers` of a `Response`. The constructor here is the only way for script
/// to make a mutable one, which is why `None` is the variant a page reaches;
/// `Response` stamps `Response` on the list it creates; and `Request` is the
/// guard the `fetch()` initializer's `headers` are *filled* under, which is not
/// a `Headers` object at all — the request's list never becomes one until the
/// `Request` interface exists (`idl/fetch/Request.webidl`). The response's
/// other three guard values arrive with `d-4a`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeadersGuard {
    /// `"none"`: a `Headers` built by script, whose list the page owns and may
    /// therefore rewrite with `append`, `set` and `delete`.
    None,
    /// `"response"`: the guard of the header list a `Response` was constructed
    /// with. The mutators are attempted, but Fetch's *validate a header* step
    /// drops a *forbidden response-header name* — silently, which is what makes
    /// `new Response(null, {headers: {"Set-Cookie": "a=b"}}).headers.get("Set-Cookie")`
    /// `null` rather than a `TypeError`
    /// (`<https://fetch.spec.whatwg.org/#concept-headers-validate>`).
    Response,
    /// `"request"`: the guard of a request's own header list, which is what the
    /// `fetch()` initializer's `headers` are filled through
    /// (`<https://fetch.spec.whatwg.org/#dom-request>`'s "set this's headers to a
    /// new `Headers` object … whose guard is `request`"). The rule is the same
    /// shape as `"response"`'s and a different predicate: a *forbidden
    /// request-header* is dropped silently, so a page cannot name its own
    /// `Content-Length`, `Host` or `Cookie` — which is what keeps the transport
    /// the only author of the framing header
    /// (`<https://fetch.spec.whatwg.org/#forbidden-request-header>`).
    Request,
}

impl HeadersGuard {
    /// Whether a mutating member may be attempted at all under this guard.
    ///
    /// The guards that answer "yes" differ in what happens next: `"none"`
    /// allows every name, and the other two drop some
    /// ([`HeadersGuard::permits_pair`]). This is the split the `d-4a` guards
    /// need too — an `"immutable"` mutation *throws* here rather than returning —
    /// so the per-pair question is separate rather than folded in.
    fn permits_mutation(self) -> bool {
        match self {
            HeadersGuard::None | HeadersGuard::Response | HeadersGuard::Request => true,
        }
    }

    /// Whether a mutation may change the entry named `name` under this guard.
    ///
    /// This is the *name-only* half of Fetch's *validate a header*, which is the
    /// half a `delete` can ask because it has no value to test. The full
    /// question is [`HeadersGuard::permits_pair`], and the two are not the same
    /// rule for the `"request"` guard: three of that guard's forbidden names are
    /// decided by the *value*, so a name-only caller is answered with the list's
    /// name-only clause and an empty value.
    ///
    /// `name` must already be normalized, i.e. byte-lowercased: every forbidden
    /// name set is compared byte-lowercased, so the comparisons are plain
    /// equalities.
    fn permits_name(self, name: &[u8]) -> bool {
        self.permits_pair(name, b"")
    }

    /// Whether the pair `(name, value)` may be *added* under this guard.
    ///
    /// Fetch's *validate a header* for a `Headers` object answers "no" by
    /// returning `false` rather than by throwing, and its *append a header*
    /// returns without appending — so a forbidden name is dropped silently, and
    /// every caller of this predicate must drop rather than fault
    /// (`<https://fetch.spec.whatwg.org/#concept-headers-validate>`, consumed by
    /// the append step immediately below it).
    ///
    /// Both halves of the pair are normalized, so the two predicates are plain
    /// equalities over lowercase bytes.
    fn permits_pair(self, name: &[u8], value: &[u8]) -> bool {
        match self {
            HeadersGuard::None => true,
            HeadersGuard::Response => !is_forbidden_response_header_name(name),
            HeadersGuard::Request => !is_forbidden_request_header(name, value),
        }
    }
}

/// Fetch's *forbidden response-header name*: `Set-Cookie` and `Set-Cookie2`,
/// and nothing else (`<https://fetch.spec.whatwg.org/#forbidden-response-header-name>`,
/// the "A forbidden response-header name is a header name that is a
/// byte-case-insensitive match for one of: `Set-Cookie`, `Set-Cookie2`"
/// definition).
///
/// The names are compared byte-lowercased because every caller has been through
/// [`normalize_header_name`] first.
fn is_forbidden_response_header_name(name: &[u8]) -> bool {
    name == SET_COOKIE || name == b"set-cookie2"
}

/// The names Fetch forbids as a *request* header outright, byte-lowercased
/// (<https://fetch.spec.whatwg.org/#forbidden-request-header>). `Accept-Charset`
/// leads the list; `Via` ends it.
const FORBIDDEN_REQUEST_HEADER_NAMES: [&[u8]; 21] = [
    b"accept-charset",
    b"accept-encoding",
    b"access-control-request-headers",
    b"access-control-request-method",
    b"connection",
    b"content-length",
    b"cookie",
    b"cookie2",
    b"date",
    b"dnt",
    b"expect",
    b"host",
    b"keep-alive",
    b"origin",
    b"referer",
    b"set-cookie",
    b"te",
    b"trailer",
    b"transfer-encoding",
    b"upgrade",
    b"via",
];

/// The three names whose *value* decides whether they are forbidden: a page that
/// may not name a method in the request line can still try to smuggle one past a
/// proxy that inspects only the headers
/// (<https://fetch.spec.whatwg.org/#forbidden-request-header>).
const FORBIDDEN_REQUEST_HEADER_METHOD_OVERRIDES: [&[u8]; 3] = [
    b"x-http-method",
    b"x-http-method-override",
    b"x-method-override",
];

/// Fetch's *forbidden request-header*, a predicate over the **(name, value)
/// pair** rather than over the name alone
/// (<https://fetch.spec.whatwg.org/#forbidden-request-header>, the "A forbidden
/// request-header is a header name that is a byte-case-insensitive match for one
/// of: …" definition and its three clauses).
///
/// The pair is the point of clause 3: `X-HTTP-Method` is forbidden only when one
/// of its comma-separated values names a *forbidden method*, so a name-only
/// predicate would refuse `X-HTTP-Method: PATCH` as well. The method list is
/// [`FORBIDDEN_METHODS`], the same one `open()` checks, because the two are the
/// same set in the same standard.
///
/// `name` must already be normalized, i.e. byte-lowercased, exactly as
/// [`is_forbidden_response_header_name`]'s callers leave it: a normalized name
/// makes Fetch's every byte-case-insensitive match a plain equality, including
/// the `proxy-` and `sec-` prefixes.
///
/// The CORS-unsafe request-header *byte* is deliberately not part of this
/// predicate. It belongs to the *CORS-safelisted request-header* test, which is
/// about which headers a cross-origin request may carry without a preflight
/// rather than about which ones a page may set at all.
fn is_forbidden_request_header(name: &[u8], value: &[u8]) -> bool {
    if FORBIDDEN_REQUEST_HEADER_NAMES.contains(&name) {
        return true;
    }
    if name.starts_with(b"proxy-") || name.starts_with(b"sec-") {
        return true;
    }
    // Fetch splits the value the way every other reader of a header value does
    // and asks whether any part is a forbidden method, so
    // `X-HTTP-Method: PATCH, TRACE` is forbidden on its second part.
    FORBIDDEN_REQUEST_HEADER_METHOD_OVERRIDES.contains(&name)
        && split_header_value(value).iter().any(|part| {
            FORBIDDEN_METHODS
                .iter()
                .any(|method| part.eq_ignore_ascii_case(method))
        })
}

// ---------------------------------------------------------------------------
// TextDecoder / TextEncoder
// ---------------------------------------------------------------------------

/// The Encoding Standard's name for the one encoding the encoder writes.
const TEXT_ENCODER_ENCODING: &str = "utf-8";

/// The message of the `TypeError` a `"fatal"` decode throws when it meets a
/// malformed sequence. The standard states the exception and no message, so the
/// text is this engine's own.
const TEXT_DECODER_FATAL: &str =
    "Failed to execute 'decode' on 'TextDecoder': The encoded data was not valid.";

/// A `TextDecoder` record.
///
/// The state is WebIDL's `[[decoder]]`/`[[I/O queue]]`/`[[BOM seen]]`/`do not
/// flush` and the two constructor settings; there is nothing else, because the
/// decoder instance *is* the queue's state across a `stream: true` call. The
/// I/O queue itself is not stored: a call pushes the argument's bytes and
/// reads them back inside one `decode`, and the only thing that has to survive
/// the call is whatever the decoder held back.
pub struct TextDecoderRecord {
    /// `[[encoding]]`: the encoding the constructor's label selected. Every
    /// attribute and every decode reads it, and the `serialize I/O queue` rule
    /// asks whether it is UTF-8 or a UTF-16, so it is the record's own field.
    pub encoding: &'static encoding_rs::Encoding,
    /// `[[error mode]]` is `"fatal"`.
    pub fatal: bool,
    /// `ignore BOM`.
    pub ignore_bom: bool,
    /// `BOM seen`.
    pub bom_seen: bool,
    /// `[[decoder]]` — `None` until a non-`stream` decode creates it.
    pub decoder: Option<encoding_rs::Decoder>,
    /// `do not flush`.
    pub do_not_flush: bool,
}

impl TextDecoderRecord {
    /// The method's steps 1–5 for one call: create or reuse the decoder, record
    /// whether the next call starts fresh, decode `input`, and apply the
    /// *serialize I/O queue* BOM rule to what came out.
    ///
    /// Step 1 is keyed on the *previous* call's `do not flush`, and that is the
    /// whole mechanism: a decoder object is what retains a partial multi-byte
    /// sequence, so replacing it is what discards the retained state once a
    /// stream has been declared finished.
    fn decode(&mut self, input: &[u8], stream: bool) -> Result<String, JsError> {
        if !self.do_not_flush {
            self.decoder = Some(self.encoding.new_decoder_without_bom_handling());
            self.bom_seen = false;
        }
        self.do_not_flush = stream;
        let mut output = String::new();
        // Step 1 created this on the first call and `do_not_flush` is only set
        // by a call that has already been through that step, so the decoder is
        // never absent here.
        let decoder = self
            .decoder
            .as_mut()
            .expect("a decoder exists once `do not flush` has been set");
        let fatal = self.fatal;
        decode_bytes(decoder, input, !stream, fatal, &mut output)?;
        self.serialize(&mut output);
        Ok(output)
    }

    /// The Encoding Standard's *serialize I/O queue*, which is the whole BOM
    /// rule: for a decoder whose encoding is UTF-8, UTF-16LE or UTF-16BE, and
    /// whose *ignore BOM* and *BOM seen* are both false, the first scalar of
    /// the output is dropped when it is U+FEFF and *BOM seen* is set either
    /// way. The queue is otherwise appended unchanged.
    ///
    /// This never re-sniffs an encoding from a BOM; the decoder was built with
    /// `new_decoder_without_bom_handling` precisely so that the rule is the
    /// standard's own. It also drops at most one U+FEFF, as
    /// `encoding/remove-only-one-bom.html` requires.
    ///
    /// *BOM seen* is set only when a scalar was actually produced. A BOM split
    /// across two `{stream: true}` chunks is the case that needs it: the first
    /// chunk decodes to nothing while the decoder holds three partial bytes, so
    /// the U+FEFF lands at the head of the *second* chunk's output, where a
    /// flag set by the first call would have kept it.
    fn serialize(&mut self, output: &mut String) {
        if self.bom_seen || self.ignore_bom || !is_bom_encoding(self.encoding) || output.is_empty()
        {
            return;
        }
        self.bom_seen = true;
        if output.starts_with('\u{FEFF}') {
            output.drain(..'\u{FEFF}'.len_utf8());
        }
    }
}

/// Whether the decoder's encoding is one of the three the *serialize I/O
/// queue* rule names, checked against the constants rather than a name because
/// a legacy encoding's `name()` is never one of them.
fn is_bom_encoding(encoding: &'static encoding_rs::Encoding) -> bool {
    encoding == encoding_rs::UTF_8
        || encoding == encoding_rs::UTF_16LE
        || encoding == encoding_rs::UTF_16BE
}

/// Decodes one chunk of bytes into `output`, on the path the decoder's *error
/// mode* selects.
///
/// `"replacement"` (the default) appends one U+FFFD per malformed sequence and
/// carries on, exactly as WebIDL's *process an item* prescribes; `"fatal"`
/// (`fatal`) makes the first malformed sequence the `TypeError` the member's
/// `[Throws]` marker promises. The two come from one call —
/// `decode_to_string_without_replacement` — so the malformed sequence is
/// reported rather than silently replaced, and the replacement is written here.
/// The `_without_bom_handling` decoder is the other half of the same decision:
/// the BOM is [`TextDecoderRecord::serialize`]'s, not `encoding_rs`'s.
fn decode_bytes(
    decoder: &mut encoding_rs::Decoder,
    input: &[u8],
    last: bool,
    fatal: bool,
    output: &mut String,
) -> Result<(), JsError> {
    let mut rest = input;
    loop {
        // The decode stops at `OutputFull` when the destination cannot take
        // the whole chunk, and the replacement-emitting path can need more
        // space than `max_utf8_buffer_length_without_replacement` allows for,
        // so the loop runs until the input is consumed rather than trusting
        // one `reserve`.
        let capacity = decoder
            .max_utf8_buffer_length_without_replacement(rest.len())
            .unwrap_or(rest.len().saturating_mul(4))
            .max(4);
        output.reserve(capacity);
        let (result, read) = decoder.decode_to_string_without_replacement(rest, output, last);
        rest = &rest[read..];
        match result {
            encoding_rs::DecoderResult::InputEmpty => return Ok(()),
            encoding_rs::DecoderResult::OutputFull => continue,
            encoding_rs::DecoderResult::Malformed(..) => {
                if fatal {
                    return Err(JsError::type_error(TEXT_DECODER_FATAL));
                }
                output.push('\u{FFFD}');
            }
        }
    }
}

/// Every document one wrapper world spans, indexed by the handle's document
/// slot.
///
/// A world is built over exactly one document — the page the engine navigated —
/// and that document's context is slot 0. `DOMImplementation.createHTMLDocument`
/// and `createDocument` mint *further* documents, each with its own arena and
/// its own context, and each takes the next slot. The slot is what a handle's
/// `doc_slot` field carries, so a node handle from a minted document resolves in
/// the right arena and never aliases a node id of the page's.
///
/// The set is shared by every context of the world (`Rc`), so a minted
/// document's own implementation can mint the next one. The engine drains
/// minted contexts at teardown to break their ownership cycle with this set:
///
///  - Slot 0 is held **weakly**. The engine and the wrapper world own the page's
///    context strongly and outlive the set, so the set must not be a second
///    owner of it — and it cannot be, because `Rc::new_cyclic` hands the context
///    its own `Weak` before the `Rc` exists.
///  - Slots 1 and up are held **strongly**. Each minted context also owns this
///    set, so the engine must drain records and call [`DocumentSet::clear_minted`]
///    after disposing its wrapper world.
///
/// A minted context is therefore retained for the life of the world, even after
/// script drops every reference to its document. That is a bounded retention —
/// one arena per `createHTMLDocument`/`createDocument` call — and it is the only
/// owner those contexts have: freeing a slot would let a stale handle resolve to
/// whatever took the slot, which is exactly what the generation bits in a node
/// handle exist to prevent and which the slot cannot also carry.
///
/// The retention is bounded by the slot field, and a page that asks for more
/// documents than the field holds gets an error, not a panic: see
/// [`Self::next_slot`].
pub struct DocumentSet {
    root: RefCell<Weak<HostContext>>,
    minted: RefCell<Vec<SharedContext>>,
    observer_agent: SharedObserverAgent,
    observer_phase: Cell<ObserverInstallPhase>,
    /// Set when the first `MutationObserver` is constructed; until then no
    /// notify demand is ever delivered.
    observers_used: Cell<bool>,
    /// Whether the page has defined a custom element, which is when the
    /// page document starts keeping its lifecycle log.
    custom_elements_used: Cell<bool>,
    /// The next observer creation serial and the count of observers that hold
    /// themselves alive by observing.
    observer_serial: Cell<u64>,
    observing_observers: Cell<usize>,
    native_memory: NativeMemoryBudget,
    minted_charge: RefCell<NativeMemoryReservation>,
    charged_minted_capacity: Cell<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ObserverInstallPhase {
    NeverInstalled,
    Installed,
    Closed,
}

struct ObserverContextState {
    agent: SharedObserverAgent,
    attachment: Option<Rc<ObserverAttachment>>,
    phase: ObserverInstallPhase,
}

impl DocumentSet {
    fn memory_error(&self) -> JsError {
        match self.native_memory.first_failure() {
            Some(failure) => JsError::type_error(format!(
                "native page memory failure ({}, {:?})",
                failure.component.name(),
                failure.cause
            )),
            None => JsError::type_error("native observer allocation failed"),
        }
    }

    fn attachment_for(&self, document: &SharedDocument) -> Option<Rc<ObserverAttachment>> {
        if let Some(root) = self.root.borrow().upgrade()
            && Rc::ptr_eq(&root.doc, document)
        {
            return root.observer_attachment.borrow().clone();
        }
        self.minted.borrow().iter().find_map(|context| {
            Rc::ptr_eq(&context.doc, document)
                .then(|| context.observer_attachment.borrow().clone())
                .flatten()
        })
    }

    fn attach_context(&self, context: &SharedContext) -> Result<(), JsError> {
        if let Some(existing) = self.attachment_for(&context.doc) {
            *context.observer_attachment.borrow_mut() = Some(existing);
            return Ok(());
        }
        let mut document = context
            .doc
            .try_borrow_mut()
            .map_err(|_| JsError::type_error("observer Document is already borrowed"))?;
        let attached = if context.slot == 0 {
            document.attach_root_observer_agent_owned(Arc::clone(&self.observer_agent))
        } else {
            document.attach_observer_agent_owned(Arc::clone(&self.observer_agent))
        }
        .map_err(|error| mutation_error(&error))?;
        *context.observer_attachment.borrow_mut() = Some(Rc::new(attached));
        Ok(())
    }

    /// Bind the fixed root and every already-minted distinct Document before
    /// the engine publishes its script world. Direct HostContext users may
    /// mint children before this call; aliases share the first attachment.
    pub fn install_observer_world(&self) -> Result<(), JsError> {
        match self.observer_phase.get() {
            ObserverInstallPhase::Installed => return Ok(()),
            ObserverInstallPhase::Closed => {
                return Err(JsError::type_error("observer world has closed"));
            }
            ObserverInstallPhase::NeverInstalled => {}
        }
        let root = self
            .root
            .borrow()
            .upgrade()
            .ok_or_else(|| JsError::type_error("observer root context is unavailable"))?;
        let existing_capacity = self.minted.borrow().capacity();
        let existing_bytes = existing_capacity
            .checked_mul(std::mem::size_of::<SharedContext>())
            .ok_or_else(document_slots_exhausted)?;
        if !self.minted_charge.borrow_mut().charge(existing_bytes) {
            self.retire_observers();
            return Err(self.memory_error());
        }
        self.charged_minted_capacity.set(existing_capacity);
        if let Err(error) = self.attach_context(&root) {
            self.retire_observers();
            return Err(error);
        }
        let mut index = 0;
        loop {
            let next = self.minted.borrow().get(index).cloned();
            let Some(context) = next else { break };
            if let Err(error) = self.attach_context(&context) {
                self.retire_observers();
                return Err(error);
            }
            index += 1;
        }
        self.observer_phase.set(ObserverInstallPhase::Installed);
        Ok(())
    }

    /// Close native entry and retire every registration without borrowing a
    /// Document. Native pins remain on the stable nodes until a later mutable
    /// Document seam or Document destruction drains them.
    pub fn retire_observers(&self) {
        if self.observer_phase.replace(ObserverInstallPhase::Closed) == ObserverInstallPhase::Closed
        {
            return;
        }
        let teardown = self.observer_agent.lock().begin_teardown_if_needed();
        if let Some(root) = self.root.borrow().upgrade()
            && let Some(attachment) = root.observer_attachment.borrow().as_ref()
        {
            attachment.retire();
        }
        for context in self.minted.borrow().iter() {
            if let Some(attachment) = context.observer_attachment.borrow().as_ref() {
                attachment.retire();
            }
        }
        drop(teardown);
    }

    fn reserve_minted_slot(&self) -> Result<(), JsError> {
        let minted = self.minted.borrow();
        if minted.len() < minted.capacity() {
            return Ok(());
        }
        let needed = minted
            .len()
            .checked_add(1)
            .ok_or_else(document_slots_exhausted)?;
        let target = needed.max(minted.capacity().saturating_mul(2)).max(4);
        if self.observer_phase.get() == ObserverInstallPhase::Installed {
            let extra = target
                .checked_sub(self.charged_minted_capacity.get())
                .and_then(|count| count.checked_mul(std::mem::size_of::<SharedContext>()))
                .ok_or_else(document_slots_exhausted)?;
            if !self.minted_charge.borrow_mut().charge(extra) {
                return Err(self.memory_error());
            }
            self.charged_minted_capacity.set(target);
        }
        drop(minted);
        let mut minted = self.minted.borrow_mut();
        let additional = target - minted.len();
        if minted.try_reserve_exact(additional).is_err() {
            self.minted_charge.borrow().fail_allocation();
            return Err(self.memory_error());
        }
        if self.observer_phase.get() == ObserverInstallPhase::Installed
            && minted.capacity() > self.charged_minted_capacity.get()
        {
            let extra = (minted.capacity() - self.charged_minted_capacity.get())
                .checked_mul(std::mem::size_of::<SharedContext>())
                .ok_or_else(document_slots_exhausted)?;
            if !self.minted_charge.borrow_mut().charge(extra) {
                return Err(self.memory_error());
            }
            self.charged_minted_capacity.set(minted.capacity());
        }
        Ok(())
    }

    /// The context for `slot`, or `None` when no document holds it — a handle
    /// whose slot was never minted, or whose page context has been torn down.
    pub fn get(&self, slot: u16) -> Option<SharedContext> {
        match slot {
            0 => self.root.borrow().upgrade(),
            slot => self.minted.borrow().get(usize::from(slot) - 1).cloned(),
        }
    }

    /// Every live context in slot order: the page's first, then the minted ones.
    pub fn all(&self) -> Vec<SharedContext> {
        self.root
            .borrow()
            .upgrade()
            .into_iter()
            .chain(self.minted.borrow().iter().cloned())
            .collect()
    }

    /// The slot the next minted context will take, or `None` once the field is
    /// full.
    ///
    /// Exhaustion is a page's own doing and it is recoverable: a page that
    /// loops over `createHTMLDocument` without bound must not be able to abort
    /// the process, so the caller turns `None` into a `DOMException` — the
    /// engine's "the quota has been exceeded" — and the page's script sees a
    /// throw. The ceiling is far above any real page: the widest `dom/nodes`
    /// file mints a few hundred.
    fn next_slot(&self) -> Option<u16> {
        let slot = self.minted.borrow().len() + 1;
        u16::try_from(slot)
            .ok()
            .filter(|slot| u64::from(*slot) <= DOC_SLOT_MASK)
    }

    /// Registers a minted context at the slot [`Self::next_slot`] named.
    fn install(&self, ctx: SharedContext) {
        self.minted.borrow_mut().push(ctx);
    }

    /// Releases the minted contexts after their engine has drained wrappers,
    /// host records and script handles. This breaks the shared set/context
    /// ownership cycle at world teardown.
    pub fn clear_minted(&self) {
        self.retire_observers();
        let contexts = std::mem::take(&mut *self.minted.borrow_mut());
        drop(contexts);
        let fresh = self
            .native_memory
            .begin(NativeMemoryComponent::MutationObserver);
        drop(self.minted_charge.replace(fresh));
        self.charged_minted_capacity.set(0);
    }
}

/// The shared host context: document, snapshots, event state and the script
/// callback boundary for one wrapper world.
pub struct HostContext {
    /// The document all hosts point to.
    pub doc: SharedDocument,
    /// This context's document slot in its world's [`DocumentSet`]. Slot 0 is
    /// the page; every other value names a document a `DOMImplementation`
    /// minted.
    pub slot: u16,
    /// Every document context of this world, this one included. See
    /// [`DocumentSet`] for the ownership rules.
    pub documents: Rc<DocumentSet>,
    /// Shared by every same-world alias of this exact Document. Dropping one
    /// alias cannot retire the native registration while another survives.
    observer_attachment: RefCell<Option<Rc<ObserverAttachment>>>,
    /// The snapshot slab for static collections.
    pub snapshots: RefCell<SnapshotSlab>,
    /// The event listeners registered on this world's nodes.
    pub listeners: RefCell<ListenerTable>,
    /// HTML §8.1.8.1's "event handler map" — one entry per (target, handler
    /// name), for every `attribute EventHandler onX` a page has set.
    ///
    /// A handler *is* an ordinary listener: the entry in [`Self::listeners`]
    /// this map names is the registration, and the map exists only to find that
    /// listener again — to replace its callback in place when the handler is
    /// reassigned (reassignment reuses the listener, so its position does not
    /// move) and to remove it when the handler is set to null.
    pub handlers: RefCell<HashMap<(TargetKey, EventHandlerName), ListenerId>>,
    /// What is known about the event handler content attributes that have
    /// been turned into handlers, or overridden by script; see
    /// [`crate::dom::events::sync_attribute_handler`].
    pub attribute_handlers:
        RefCell<HashMap<(TargetKey, EventHandlerName), crate::dom::events::SeenHandlerAttribute>>,
    /// The `fetch()` calls that were given an `AbortSignal` and are still in
    /// flight: the promise's resolver, and the signal that aborts it, in the
    /// order the calls were made, which is the order an abort ends them in. An entry
    /// leaves when the request is answered or the signal aborts, so a
    /// resolver's bits are never found here after their promise has settled.
    pub fetch_signals: RefCell<Vec<(u64, HostKey)>>,
    /// The live event records for this world.
    pub events: RefCell<EventSlab>,
    /// The live `URL`, `URLSearchParams`, `Headers`, `AbortController`,
    /// `AbortSignal`, `TextDecoder` and `TextEncoder` records for this world.
    /// Unlike a node, a `new URL()` has no
    /// document row to live in, so the record is the whole of its state and its
    /// wrapper is its owner — the event slab's shape, not the snapshot slab's —
    /// except for the URL/query and controller/signal pairs, where one record
    /// keeps the other alive (see [`reclaim_host_record`]).
    pub host_records: RefCell<HostSlab>,
    /// Weak keys for attached Attr records. The wrapper's finalizer removes a
    /// key; the map itself does not keep the record or its owner element alive.
    attr_index: RefCell<HashMap<AttrIdentity, HostKey>>,
    /// The engine's callback boundary, installed at `Engine::build` and cleared
    /// at teardown. `None` before installation and after teardown.
    pub script: RefCell<Option<Rc<dyn ScriptHost>>>,
    /// The event loop's clock, which `Event.timeStamp` reads. It is the loop's
    /// clock and not a private origin so that a timestamp is directly
    /// comparable with `performance.now()`, as script assumes — and so that a
    /// virtual clock moves both together.
    pub clock: SharedClock,
    /// What `document.designMode` was last set to. Nothing reads it but the
    /// attribute: no document is ever edited by a user.
    pub design_mode: Cell<bool>,
    /// Elements with a synthetic `click()` in progress, so a nested `click()`
    /// on the same element during its own dispatch is a no-op (the DOM
    /// "click in progress" flag).
    pub click_in_progress: RefCell<HashSet<NodeId>>,
    /// The cookie jar of the browsing context this world belongs to.
    ///
    /// It is the *same* jar `tenta-net` sends from — a clone of one
    /// `Rc`-shared store, not a copy — so a cookie `document.cookie` writes is
    /// sent on the next request, which is the whole point of the member. The
    /// jar's policy is the context's policy, not the world's.
    pub cookies: CookieJar,
    /// The page's local and session storage view, shared with minted documents.
    pub storage: PageStorage,
    /// Page-owned native accounting, shared with every minted document context.
    pub native_memory: NativeMemoryBudget,
    /// Weak canonical record keys for this Window's local and session areas.
    /// Finalization clears the matching key; the area state lives in storage.
    storage_keys: RefCell<[Option<HostKey>; 2]>,
    /// Weak canonical key for this Window's Crypto wrapper.
    crypto_key: RefCell<Option<HostKey>>,
    /// The key of this document's `DOMImplementation` record, once script has
    /// asked for `document.implementation`.
    ///
    /// The record is created lazily and cached here, which is what
    /// `[SameObject]` requires; it is created *per context*, so a minted
    /// document's implementation is a different object from the page's, as
    /// `dom/nodes/DOMImplementation-createHTMLDocument-with-saved-implementation.html`
    /// asserts.
    pub implementation: RefCell<Option<HostKey>>,
    /// The page's image state (`complete`, `naturalWidth`, ...), installed by
    /// the page for the document that is current. `None` for a document the
    /// page does not load images for.
    pub image_host: RefCell<Option<Rc<dyn ImageHost>>>,
    /// The page's linked stylesheets (`link.sheet`, `document.styleSheets`).
    /// `None` when the page loads none.
    pub style_host: RefCell<Option<Rc<dyn crate::stylesheets::StyleHost>>>,
    /// The CSSOM sheet of each `<style>` and loaded `<link>` script has asked
    /// for, so `link.sheet === document.styleSheets[0]` holds.
    #[cfg(any(feature = "v8", feature = "quickjs"))]
    pub doc_sheets: RefCell<doc_sheets::DocSheets>,
    /// The `document.styleSheets` object, made on the first read.
    pub style_sheet_list_key: RefCell<Option<HostKey>>,
    /// The `decode()` promises still waiting for their element's load to end,
    /// as resolver bits, by element.
    image_decodes: RefCell<HashMap<NodeRef, Vec<u64>>>,
    /// Elements script touched whose load the page has not reported yet, kept
    /// alive so an unreferenced `new Image()` still loads and fires.
    image_pins: RefCell<HashMap<NodeRef, AttrOwnerPin>>,
}

impl HostContext {
    /// Builds the page's context: slot 0 of a fresh [`DocumentSet`], over `doc`,
    /// timestamping events against `clock`, with empty event state, the
    /// context's cookie jar and storage view, and no script host yet.
    ///
    /// The slot-0 context is handed to the wrapper world and the engine, and
    /// both own it; the set holds it weakly, which is why the context is built
    /// through `Rc::new_cyclic` — the set needs the context's own `Weak` before
    /// the `Rc` exists.
    pub fn new(
        doc: SharedDocument,
        clock: SharedClock,
        cookies: CookieJar,
        storage: PageStorage,
    ) -> SharedContext {
        Self::new_with_native_memory(
            doc,
            clock,
            cookies,
            storage,
            NativeMemoryBudget::for_page(tenta_core::Budgets::platform_default().page_memory_mb),
        )
    }

    /// Builds a page context using the ledger owned by its Page. Direct engine
    /// callers use [`Self::new`] for an isolated platform-default ledger.
    pub fn new_with_native_memory(
        doc: SharedDocument,
        clock: SharedClock,
        cookies: CookieJar,
        storage: PageStorage,
        native_memory: NativeMemoryBudget,
    ) -> SharedContext {
        let observer_agent = Arc::new(parking_lot::Mutex::new(ObserverAgent::new(
            native_memory.clone(),
        )));
        Self::new_with_observer_state(
            doc,
            clock,
            cookies,
            storage,
            native_memory,
            ObserverContextState {
                agent: observer_agent,
                attachment: None,
                phase: ObserverInstallPhase::NeverInstalled,
            },
        )
    }

    /// The V8 constructor receives a root registration freshly created by
    /// the Document itself while it holds its existing mutable root borrow.
    /// No script world has been published, and no root quota is charged.
    pub fn new_with_installed_observer(
        doc: SharedDocument,
        clock: SharedClock,
        cookies: CookieJar,
        storage: PageStorage,
        native_memory: NativeMemoryBudget,
    ) -> (SharedContext, NodeRef) {
        let (document_ref, observer_agent, attachment) = {
            let mut document = doc.borrow_mut();
            let (agent, attachment) = document.new_observer_world_owned(native_memory.clone());
            (
                document.node_ref(document.document_node()),
                agent,
                attachment,
            )
        };
        let context = Self::new_with_observer_state(
            doc,
            clock,
            cookies,
            storage,
            native_memory,
            ObserverContextState {
                agent: observer_agent,
                attachment: Some(Rc::new(attachment)),
                phase: ObserverInstallPhase::Installed,
            },
        );
        (context, document_ref)
    }

    fn new_with_observer_state(
        doc: SharedDocument,
        clock: SharedClock,
        cookies: CookieJar,
        storage: PageStorage,
        native_memory: NativeMemoryBudget,
        observer: ObserverContextState,
    ) -> SharedContext {
        let minted_charge = native_memory.begin(NativeMemoryComponent::MutationObserver);
        Rc::new_cyclic(|weak| Self {
            doc: doc.clone(),
            slot: 0,
            documents: Rc::new(DocumentSet {
                root: RefCell::new(weak.clone()),
                minted: RefCell::new(Vec::new()),
                observer_agent: observer.agent,
                observer_phase: Cell::new(observer.phase),
                observers_used: Cell::new(false),
                custom_elements_used: Cell::new(false),
                observer_serial: Cell::new(0),
                observing_observers: Cell::new(0),
                native_memory: native_memory.clone(),
                minted_charge: RefCell::new(minted_charge),
                charged_minted_capacity: Cell::new(0),
            }),
            observer_attachment: RefCell::new(observer.attachment),
            snapshots: RefCell::new(SnapshotSlab::new(doc)),
            listeners: RefCell::new(ListenerTable::new()),
            handlers: RefCell::new(HashMap::new()),
            attribute_handlers: RefCell::new(HashMap::new()),
            fetch_signals: RefCell::new(Vec::new()),
            events: RefCell::new(EventSlab::new()),
            host_records: RefCell::new(HostSlab::new()),
            attr_index: RefCell::new(HashMap::new()),
            script: RefCell::new(None),
            clock,
            design_mode: Cell::new(false),
            click_in_progress: RefCell::new(HashSet::new()),
            cookies,
            storage,
            native_memory,
            storage_keys: RefCell::new([None, None]),
            crypto_key: RefCell::new(None),
            implementation: RefCell::new(None),
            image_host: RefCell::new(None),
            style_host: RefCell::new(None),
            #[cfg(any(feature = "v8", feature = "quickjs"))]
            doc_sheets: RefCell::new(doc_sheets::DocSheets::default()),
            style_sheet_list_key: RefCell::new(None),
            image_decodes: RefCell::new(HashMap::new()),
            image_pins: RefCell::new(HashMap::new()),
        })
    }

    /// Builds a context for a document a `DOMImplementation` minted, in this
    /// world's [`DocumentSet`], and returns it.
    ///
    /// The child inherits the parts of a context that belong to the *world*
    /// rather than to one document: the loop's clock, the context's cookie jar,
    /// storage view, and the installed script host. The rest — the snapshot, event, listener
    /// and host-record slabs — starts empty, because those are keyed by handles
    /// whose slots name this document and would otherwise collide with the
    /// page's.
    ///
    /// The error is the field being full, which only a page that mints documents
    /// in a loop can cause: see [`DocumentSet::next_slot`].
    pub fn new_child(&self, doc: SharedDocument) -> Result<SharedContext, JsError> {
        if self.documents.observer_phase.get() == ObserverInstallPhase::Closed {
            return Err(JsError::type_error("observer world has closed"));
        }
        let slot = self
            .documents
            .next_slot()
            .ok_or_else(document_slots_exhausted)?;
        self.documents.reserve_minted_slot()?;
        let attachment = if self.documents.observer_phase.get() == ObserverInstallPhase::Installed {
            if let Some(existing) = self.documents.attachment_for(&doc) {
                Some(existing)
            } else {
                let attached = doc
                    .try_borrow_mut()
                    .map_err(|_| JsError::type_error("observer Document is already borrowed"))?
                    .attach_observer_agent_owned(Arc::clone(&self.documents.observer_agent))
                    .map_err(|error| mutation_error(&error))?;
                Some(Rc::new(attached))
            }
        } else {
            None
        };
        let ctx = Rc::new(Self {
            doc: doc.clone(),
            slot,
            documents: self.documents.clone(),
            observer_attachment: RefCell::new(attachment),
            snapshots: RefCell::new(SnapshotSlab::new(doc)),
            listeners: RefCell::new(ListenerTable::new()),
            handlers: RefCell::new(HashMap::new()),
            attribute_handlers: RefCell::new(HashMap::new()),
            fetch_signals: RefCell::new(Vec::new()),
            events: RefCell::new(EventSlab::new()),
            host_records: RefCell::new(HostSlab::new()),
            attr_index: RefCell::new(HashMap::new()),
            script: RefCell::new(self.script.borrow().clone()),
            clock: self.clock.clone(),
            design_mode: Cell::new(false),
            click_in_progress: RefCell::new(HashSet::new()),
            cookies: self.cookies.clone(),
            storage: self.storage.clone(),
            native_memory: self.native_memory.clone(),
            storage_keys: RefCell::new([None, None]),
            crypto_key: RefCell::new(None),
            implementation: RefCell::new(None),
            image_host: RefCell::new(None),
            style_host: RefCell::new(None),
            #[cfg(any(feature = "v8", feature = "quickjs"))]
            doc_sheets: RefCell::new(doc_sheets::DocSheets::default()),
            style_sheet_list_key: RefCell::new(None),
            image_decodes: RefCell::new(HashMap::new()),
            image_pins: RefCell::new(HashMap::new()),
        });
        self.documents.install(ctx.clone());
        Ok(ctx)
    }

    /// Milliseconds since the loop origin, for `Event.timeStamp` — the same
    /// reading `performance.now()` returns.
    pub fn time_stamp(&self) -> f64 {
        self.clock.now().as_millis_f64()
    }

    /// `document.lastModified`: the time the document was made, which is all
    /// that is known of it, as `MM/DD/YYYY hh:mm:ss` in UTC.
    pub fn last_modified(&self) -> String {
        let seconds = (self.clock.time_origin_ms() / 1000.0).max(0.0) as i64;
        let (days, time) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
        // The civil date of a day count from 1970-01-01 (proleptic Gregorian).
        let shifted = days + 719_468;
        let era = shifted.div_euclid(146_097);
        let day_of_era = shifted.rem_euclid(146_097);
        let year_of_era =
            (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month_index = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * month_index + 2) / 5 + 1;
        let month = if month_index < 10 {
            month_index + 3
        } else {
            month_index - 9
        };
        let year = year_of_era + era * 400 + i64::from(month <= 2);
        format!(
            "{month:02}/{day:02}/{year:04} {:02}:{:02}:{:02}",
            time / 3600,
            time % 3600 / 60,
            time % 60
        )
    }

    /// Reclaims a finalized host record and its weak Attr/CSS/Storage wrapper caches.
    /// Exact generational keys prevent delayed finalizers from forgetting a
    /// newer wrapper that reused the same slab slot.
    pub fn reclaim_host(&self, key: HostKey) -> ReclaimedHost {
        #[cfg(any(feature = "v8", feature = "quickjs"))]
        let css_state = self
            .host_records
            .borrow()
            .get(key)
            .and_then(cssom::record_state);
        let identity = match self.host_records.borrow().get(key) {
            Some(HostRecord::Attr(record)) => record
                .element
                .map(|element| attr_identity(element, record.name)),
            _ => None,
        };
        let mut reclaimed = reclaim_host_record(&mut self.host_records.borrow_mut(), key);
        for retired in std::mem::take(&mut reclaimed.retired) {
            retired.finish();
        }
        #[cfg(any(feature = "v8", feature = "quickjs"))]
        if let Some(state) = css_state
            && reclaimed.keys.contains(&key)
        {
            state.forget_key(key);
        }
        if let Some(identity) = identity
            && reclaimed.keys.contains(&key)
            && self.attr_index.borrow().get(&identity).copied() == Some(key)
        {
            self.attr_index.borrow_mut().remove(&identity);
        }
        if reclaimed.keys.contains(&key) {
            let forget_crypto = *self.crypto_key.borrow() == Some(key);
            if forget_crypto {
                *self.crypto_key.borrow_mut() = None;
            }
            for cached in self.storage_keys.borrow_mut().iter_mut() {
                if *cached == Some(key) {
                    *cached = None;
                }
            }
        }
        reclaimed
    }

    /// Applies an element attribute value write. The document retains the
    /// prefix of an existing `(namespace, local)` attribute.
    fn set_element_attribute(
        &self,
        element: NodeId,
        name: QualName,
        value: String,
    ) -> Result<(), JsError> {
        let local = name.local;
        self.doc
            .borrow_mut()
            .mutate(Mutation::SetAttribute {
                element,
                name,
                value,
            })
            .map_err(|e| mutation_error(&e))?;
        self.note_image_attribute(element, local);
        Ok(())
    }

    /// Tells the page's image loader that script changed an attribute that
    /// can select another image. Only the loader decides whether the element
    /// is one; this just keeps a detached `new Image()` from going unseen.
    fn note_image_attribute(&self, element: NodeId, local: Atom) {
        if !matches!(
            local.as_ref(),
            "src" | "srcset" | "sizes" | "loading" | "type" | "media" | "poster"
        ) {
            return;
        }
        self.note_image_element(element);
    }

    /// Tells the image host that script touched `element`, and pins the
    /// element when the host will report its load.
    fn note_image_element(&self, element: NodeId) {
        let host = self.image_host.borrow().clone();
        let Some(host) = host else {
            return;
        };
        if !host.attribute_changed(element) {
            return;
        }
        let reference = self.doc.borrow().node_ref(element);
        if self.image_pins.borrow().contains_key(&reference) {
            return;
        }
        if let Ok(pin) = AttrOwnerPin::new(self.doc.clone(), element) {
            self.image_pins.borrow_mut().insert(reference, pin);
        }
    }

    /// A clone is a new element with its source copied, and HTML runs "update
    /// the image data" for it: the host is told about the clone and every
    /// image below it.
    fn note_cloned_images(&self, clone: NodeId) {
        if self.image_host.borrow().is_none() {
            return;
        }
        let images: Vec<NodeId> = {
            let doc = self.doc.borrow();
            doc.descendants_inclusive(clone)
                .filter(|node| doc.tag(*node) == Some(atoms::IMG))
                .collect()
        };
        for image in images {
            self.note_image_element(image);
        }
    }

    /// Installs the page's image state for this document.
    pub fn set_image_host(&self, host: Rc<dyn ImageHost>) {
        *self.image_host.borrow_mut() = Some(host);
    }

    /// Installs the page's linked stylesheets for this document.
    pub fn set_style_host(&self, host: Rc<dyn crate::stylesheets::StyleHost>) {
        *self.style_host.borrow_mut() = Some(host);
    }

    /// Removes an element attribute, detaching any exposed Attr with its last
    /// value and releasing the strong owner-element handle after the mutation.
    fn remove_element_attribute(
        &self,
        element: NodeId,
        name: QualName,
    ) -> Result<Option<(QualName, String, Option<HostKey>)>, JsError> {
        let old = {
            let doc = self.doc.borrow();
            doc.attributes(element)
                .find(|(stored, _)| stored.ns == name.ns && stored.local == name.local)
                .map(|(stored, value)| (doc.node_ref(element), stored, value.to_owned()))
        };
        let Some((reference, stored_name, value)) = old else {
            return Ok(None);
        };
        self.doc
            .borrow_mut()
            .mutate(Mutation::RemoveAttribute {
                element,
                name: stored_name,
            })
            .map_err(|e| mutation_error(&e))?;
        self.note_image_attribute(element, stored_name.local);
        let key = self.detach_cached_attr(reference, stored_name, value.clone());
        Ok(Some((stored_name, value, key)))
    }

    /// Detaches one cached Attr after its packed attribute row was removed or
    /// replaced. The caller supplies the old value captured before mutation.
    fn detach_cached_attr(
        &self,
        element: NodeRef,
        name: QualName,
        value: String,
    ) -> Option<HostKey> {
        let key = self
            .attr_index
            .borrow_mut()
            .remove(&attr_identity(element, name))?;
        let pin = {
            let mut records = self.host_records.borrow_mut();
            let Some(HostRecord::Attr(record)) = records.get_mut(key) else {
                return None;
            };
            record.element = None;
            record.detached_value = value;
            record.owner_pin.take()
        };
        drop(pin);
        Some(key)
    }

    /// Drains the listener entries of every node the document's most recent
    /// sweep freed, releasing the stored callback reference each entry owned.
    ///
    /// The entries are drained by the exact [`NodeRef`] the document recorded,
    /// generation included — [`ListenerTable::drain_target_at`] — so the key
    /// matches precisely the registrations made while that node lived. A drain
    /// on the bare id would also reach a listener registered on a node that
    /// later took the freed slot; this one cannot. The generation is known here
    /// because the document recorded it before the slot was recycled.
    ///
    /// Releasing a freed node's entries is a memory reclaim, not a correctness
    /// one: their key can never match a live target again. Without it every
    /// listener registered on a swept node keeps a V8 value alive for the life
    /// of the world, because a [`CallbackRef`] is a bare handle with no `Drop`.
    ///
    /// A no-op when the last sweep freed nothing. It is also a silent no-op
    /// when no [`ScriptHost`] is installed, because there is then no
    /// `release_value` to free a reference through — so a caller must run it at
    /// a point where the world has one, which for a page engine is any entry
    /// point after `Engine::build` installed the host.
    pub fn drain_swept_listeners(&self) {
        let drained = self.take_swept_listeners();
        let script = self.script.borrow();
        if let Some(script) = script.as_ref() {
            for callback in drained {
                script.release_value(callback.0);
            }
        }
    }

    /// Detaches exact-generation swept-node registrations without releasing
    /// their owned script references. A collector can stop between releases if
    /// a native finalizer poisons the runtime; remaining roots stay reserved.
    pub fn take_swept_listeners(&self) -> Vec<CallbackRef> {
        let freed = self.doc.borrow().sweep_freed_nodes().to_vec();
        if freed.is_empty() {
            return Vec::new();
        }
        let mut listeners = self.listeners.borrow_mut();
        freed
            .iter()
            .flat_map(|node| listeners.drain_target_at(TargetKey::Node(*node)))
            .collect()
    }

    /// Removes and returns every listener registered on the host targets in
    /// `keys`, matched by the key taken exactly as given — generation included.
    ///
    /// A host record owns its listener entries the way a node does, but the two
    /// are freed by different owners: the document sweeps a dead node, and a
    /// wrapper's finalizer frees a dead host record, so this is the counterpart
    /// of [`Self::drain_swept_listeners`] for the record side. Exact matching is
    /// [`ListenerTable::drain_target_at`]'s; see it for why a generation-blind
    /// drain would be wrong.
    ///
    /// The caller owns the returned references and releases them; unlike
    /// [`Self::drain_swept_listeners`], this cannot release them itself, because
    /// its caller is already holding the world's value table to free the record
    /// in the same pass.
    pub fn drain_host_listeners(&self, keys: &[HostKey]) -> Vec<CallbackRef> {
        let mut listeners = self.listeners.borrow_mut();
        keys.iter()
            .flat_map(|key| listeners.drain_target_at(TargetKey::Host(*key)))
            .collect()
    }

    /// The JavaScript `undefined`, as the wrapper world's bits for it.
    ///
    /// A record that hands back an unset WebIDL `any` needs a *value*, and the
    /// one sentinel the prelude maps back out is `null`'s — so `undefined` is
    /// built through [`ScriptHost::build_value`], which interns it once per
    /// world and answers with the same bits every time. That stability is what
    /// makes an unset `AbortSignal.reason` read back as `undefined` on every
    /// access rather than as a fresh value.
    ///
    /// Without a script host there is no world to intern in and no script that
    /// could read the result, so the `null` sentinel stands in.
    pub fn undefined_value(&self) -> AnyValue {
        let script = self.script.borrow();
        script
            .as_ref()
            .and_then(|script| script.build_value(HostValue::Undefined))
            .map_or(AnyValue::NULL, AnyValue)
    }

    /// Sets the event handler named `name` on `target` — the setter of an event
    /// handler IDL attribute (HTML §8.1.8.1).
    ///
    /// A handler is an ordinary event listener whose callback is the page's own
    /// handler function, so the standard's three algorithms are three operations
    /// on the listener table: a first non-null assignment registers the listener
    /// and remembers its id; a reassignment replaces that listener's callback in
    /// place, so the listener keeps its position among the target's listeners;
    /// and `None` removes it. The listener's type is the handler's event type
    /// ([`EventHandlerName::event_type`]) and it is an ordinary listener in
    /// every other respect — non-capturing, not once, not passive.
    ///
    /// The registration is [`ListenerTable::add_handler_at`], not
    /// `addEventListener`: HTML gives a handler's listener an internal callback
    /// the page cannot name, so it never collides with the page's own listener
    /// for the same type and function, and `removeEventListener` cannot remove
    /// it. The handler map, not the table's duplicate rule, is what keeps a
    /// handler to one listener.
    ///
    /// # Ownership
    ///
    /// `value` carries one stored reference, exactly as an `addEventListener`
    /// callback does, and the listener entry becomes its owner: a reassignment
    /// and a deactivation hand the reference they displace back through
    /// [`ScriptHost::release_value`], and a first assignment always registers,
    /// so nothing is ever rejected and nothing is released on the way in.
    pub fn set_event_handler(
        &self,
        target: TargetKey,
        name: EventHandlerName,
        value: Option<EventHandlerNonNull>,
    ) {
        let key = (target, name);
        // A map entry can outlive the listener it names: a sweep or a host
        // reclamation drains a dead target's listeners and knows nothing about
        // this map. An id that is no longer registered is therefore treated as
        // absent, which is also what keeps a reassignment from writing the new
        // value into a listener that no longer exists and losing its reference.
        let current = self
            .handlers
            .borrow()
            .get(&key)
            .copied()
            .filter(|id| self.listeners.borrow().contains_at(target, *id));

        let Some(value) = value else {
            // Deactivate: the handler value is null, so its listener — if it has
            // one — is removed and the reference it held is given back.
            if let Some(id) = current {
                self.handlers.borrow_mut().remove(&key);
                let removed = self.listeners.borrow_mut().remove_by_id_at(target, id);
                if let Some(removed) = removed {
                    self.release_stored(removed.0);
                }
            }
            return;
        };

        let callback = CallbackRef(value.bits());
        if let Some(id) = current {
            // Reassignment reuses the listener the first non-null assignment
            // registered, so the value changes in place and the list position
            // does not move.
            let replaced = self
                .listeners
                .borrow_mut()
                .set_callback_at(target, id, callback);
            if let Some(replaced) = replaced {
                self.release_stored(replaced.0);
            }
            return;
        }

        // A first assignment registers the handler's listener. It is not
        // `addEventListener`: the DOM duplicate rule never applies to it, and
        // the entry this returns is what the map remembers.
        let id = self.listeners.borrow_mut().add_handler_at(
            target,
            name.event_type().to_owned(),
            callback,
        );
        self.handlers.borrow_mut().insert(key, id);
    }

    /// Reads the event handler named `name` on `target` — the getter of an event
    /// handler IDL attribute. `None` when no handler is set, or when the
    /// listener it named has been drained with its target.
    ///
    /// The bits are handed back as they are — non-owning. The listener entry
    /// holds the one stored reference, exactly as an `AbortSignal` owns its
    /// abort reason, so two reads return the same value and `el.onx === el.onx`
    /// holds by identity.
    pub fn event_handler(
        &self,
        target: TargetKey,
        name: EventHandlerName,
    ) -> Option<EventHandlerNonNull> {
        let id = *self.handlers.borrow().get(&(target, name))?;
        let callback = self.listeners.borrow().callback_by_id_at(target, id)?;
        Some(Callback::from_bits(callback.0))
    }

    /// Gives one stored reference back through the installed script host, if
    /// there is one. The context's counterpart of [`HostObject::release_stored`],
    /// for the handlers this context alone owns.
    fn release_stored(&self, bits: u64) {
        if let Some(script) = self.script.borrow().as_ref() {
            script.release_value(bits);
        }
    }
}

#[cfg(test)]
mod native_memory_tests {
    use super::*;
    use tenta_core::NativeMemoryComponent;

    fn context(document: SharedDocument, budget: NativeMemoryBudget) -> SharedContext {
        HostContext::new_with_native_memory(
            document,
            crate::event_loop::SharedClock::new(crate::event_loop::Clock::virtual_at(
                crate::event_loop::MonoTime::ZERO,
            )),
            CookieJar::new(),
            PageStorage::isolated(),
            budget,
        )
    }

    #[test]
    fn minted_document_shares_the_page_ledger() {
        let budget = NativeMemoryBudget::with_limits(100, 100, 100, 0);
        let root = context(Rc::new(RefCell::new(Document::new())), budget.clone());
        let child = root
            .new_child(Rc::new(RefCell::new(Document::new())))
            .expect("a second document slot is available");
        assert!(root.native_memory.same_page(&child.native_memory));
        assert!(root.native_memory.same_page(&budget));
        let mut first = root
            .native_memory
            .begin(NativeMemoryComponent::StructuredClone);
        let mut second = child
            .native_memory
            .begin(NativeMemoryComponent::MutationObserver);
        assert!(first.charge(70));
        assert!(!second.charge(40));
        assert_eq!(root.native_memory.total_used(), 70);
        root.documents.retire_observers();
        root.documents.clear_minted();
    }

    #[test]
    fn preinstalled_aliases_share_one_attachment_and_retire_without_document_borrow() {
        let budget = NativeMemoryBudget::with_limits(1 << 20, 1 << 20, 1 << 20, 0);
        let root = context(Rc::new(RefCell::new(Document::new())), budget);
        let page_alias = root.new_child(root.doc.clone()).unwrap();
        let child_document = Rc::new(RefCell::new(Document::new()));
        let first = root.new_child(child_document.clone()).unwrap();
        let second = root.new_child(child_document.clone()).unwrap();
        assert!(first.observer_attachment.borrow().is_none());
        root.documents.install_observer_world().unwrap();
        assert!(Rc::ptr_eq(
            root.observer_attachment.borrow().as_ref().unwrap(),
            page_alias.observer_attachment.borrow().as_ref().unwrap()
        ));
        let first_owner = first.observer_attachment.borrow().as_ref().unwrap().clone();
        let second_owner = second
            .observer_attachment
            .borrow()
            .as_ref()
            .unwrap()
            .clone();
        assert!(Rc::ptr_eq(&first_owner, &second_owner));
        let third = root.new_child(child_document.clone()).unwrap();
        let third_owner = third.observer_attachment.borrow().as_ref().unwrap().clone();
        assert!(Rc::ptr_eq(&first_owner, &third_owner));
        drop(first_owner);
        drop(third_owner);
        let held_document = child_document.borrow_mut();
        root.documents.retire_observers();
        assert!(!second_owner.retire());
        drop(held_document);
        root.documents.clear_minted();
        assert!(
            root.new_child(Rc::new(RefCell::new(Document::new())))
                .is_err()
        );
        drop(first);
        drop(second);
        drop(third);
    }

    #[test]
    fn independent_worlds_over_one_document_keep_distinct_attachments() {
        let document = Rc::new(RefCell::new(Document::new()));
        let first = context(
            document.clone(),
            NativeMemoryBudget::with_limits(1 << 20, 1 << 20, 1 << 20, 0),
        );
        let second = context(
            document,
            NativeMemoryBudget::with_limits(1 << 20, 1 << 20, 1 << 20, 0),
        );
        first.documents.install_observer_world().unwrap();
        second.documents.install_observer_world().unwrap();
        let first_owner = first.observer_attachment.borrow().as_ref().unwrap().clone();
        let second_owner = second
            .observer_attachment
            .borrow()
            .as_ref()
            .unwrap()
            .clone();
        assert!(!Rc::ptr_eq(&first_owner, &second_owner));
        first.documents.retire_observers();
        assert!(!first_owner.retire());
        assert!(second_owner.retire());
        second.documents.retire_observers();
    }

    #[test]
    fn child_quota_refusal_keeps_the_next_slot_unpublished() {
        let budget = NativeMemoryBudget::with_limits(0, 0, 0, 0);
        let root = context(Rc::new(RefCell::new(Document::new())), budget.clone());
        root.documents.install_observer_world().unwrap();
        assert!(
            root.new_child(Rc::new(RefCell::new(Document::new())))
                .is_err()
        );
        assert_eq!(root.documents.next_slot(), Some(1));
        assert_eq!(root.documents.minted.borrow().len(), 0);
        assert_eq!(
            budget.first_failure().unwrap().component,
            NativeMemoryComponent::MutationObserver
        );
        root.documents.retire_observers();
    }

    #[test]
    fn fresh_root_constructor_attaches_the_exact_document_before_publication() {
        let document = Rc::new(RefCell::new(Document::new()));
        let (root, document_ref) = HostContext::new_with_installed_observer(
            document.clone(),
            crate::event_loop::SharedClock::new(crate::event_loop::Clock::virtual_at(
                crate::event_loop::MonoTime::ZERO,
            )),
            CookieJar::new(),
            PageStorage::isolated(),
            NativeMemoryBudget::with_limits(1 << 20, 1 << 20, 1 << 20, 0),
        );
        let expected_ref = {
            let document = document.borrow();
            document.node_ref(document.document_node())
        };
        assert_eq!(document_ref, expected_ref);
        assert_eq!(
            root.documents.observer_phase.get(),
            ObserverInstallPhase::Installed
        );
        assert_eq!(
            root.observer_attachment
                .borrow()
                .as_ref()
                .unwrap()
                .document_id(),
            document.borrow().document_id()
        );
        root.documents.retire_observers();
    }

    #[test]
    fn failed_preinstalled_child_attach_retires_prior_nodes_and_preserves_error() {
        let budget = NativeMemoryBudget::with_limits(1 << 20, 1 << 20, 1 << 20, 0);
        let root = context(Rc::new(RefCell::new(Document::new())), budget);
        let first_document = Rc::new(RefCell::new(Document::new()));
        let first_child = root.new_child(first_document.clone()).unwrap();
        let blocked_document = Rc::new(RefCell::new(Document::new()));
        let _second_child = root.new_child(blocked_document.clone()).unwrap();
        let held = blocked_document.borrow_mut();
        let error = root.documents.install_observer_world().unwrap_err();
        assert_eq!(error.message(), "observer Document is already borrowed");
        assert_eq!(
            root.documents.observer_phase.get(),
            ObserverInstallPhase::Closed
        );
        assert!(!root.observer_attachment.borrow().as_ref().unwrap().retire());
        assert!(
            !first_child
                .observer_attachment
                .borrow()
                .as_ref()
                .unwrap()
                .retire()
        );
        drop(held);
        let independent = context(
            first_document,
            NativeMemoryBudget::with_limits(1 << 20, 1 << 20, 1 << 20, 0),
        );
        independent.documents.install_observer_world().unwrap();
        independent.documents.retire_observers();
        root.documents.clear_minted();
    }

    #[test]
    fn borrowed_post_install_child_is_not_published() {
        let root = context(
            Rc::new(RefCell::new(Document::new())),
            NativeMemoryBudget::with_limits(1 << 20, 1 << 20, 1 << 20, 0),
        );
        root.documents.install_observer_world().unwrap();
        let child_document = Rc::new(RefCell::new(Document::new()));
        let held = child_document.borrow_mut();
        let before = root.documents.next_slot();
        let error = root
            .new_child(child_document.clone())
            .err()
            .expect("a held Document borrow must refuse child publication");
        assert_eq!(error.message(), "observer Document is already borrowed");
        assert_eq!(root.documents.next_slot(), before);
        assert!(root.documents.minted.borrow().is_empty());
        drop(held);
        root.documents.retire_observers();
    }
}

/// A context shared between the host objects of one world and the engine.
pub type SharedContext = Rc<HostContext>;

/// The Rust side of a DOM wrapper.
///
/// One `HostObject` is created per live wrapper and owned by the world's host
/// map (`docs/spec/05-javascript.md` §3); the wrapper's internal field only
/// borrows a pointer to it.
pub enum HostObject {
    /// A wrapper for a single node.
    Node {
        /// The context (document and snapshots) the node lives in.
        ctx: SharedContext,
        /// A generational reference to the node this wrapper stands for.
        node: NodeRef,
    },
    /// A wrapper for the live child `NodeList` of `owner`.
    NodeList {
        /// The context the collection's owner lives in.
        ctx: SharedContext,
        /// A generational reference to the node whose children the list holds.
        owner: NodeRef,
    },
    /// A wrapper for the live element-child `HTMLCollection` of `owner`.
    HtmlCollection {
        /// The context the collection's owner lives in.
        ctx: SharedContext,
        /// A generational reference to the node whose element children the collection holds.
        owner: NodeRef,
    },
    /// A wrapper for a static snapshot of nodes (querySelectorAll result, etc.).
    StaticNodeList {
        /// The context holding the snapshot slab.
        ctx: SharedContext,
        /// The key to the snapshot in the slab.
        key: SnapshotKey,
    },
    /// A wrapper for a static snapshot of elements (getElementsByTagName result, etc.).
    StaticHtmlCollection {
        /// The context holding the snapshot slab.
        ctx: SharedContext,
        /// The key to the snapshot in the slab.
        key: SnapshotKey,
    },
    /// A wrapper for an `Event` (or a subclass), keyed into the event slab.
    Event {
        /// The context holding the event slab.
        ctx: SharedContext,
        /// The key to the event record in the slab.
        key: EventKey,
    },
    /// A wrapper for the live `DOMTokenList` (`classList`) of `owner`.
    TokenList {
        /// The context the token list's owner lives in.
        ctx: SharedContext,
        /// A generational reference to the element whose `class` the list reflects.
        owner: NodeRef,
    },
    /// A wrapper for the `DOMStringMap` (`dataset`) of `owner`: the element's
    /// `data-*` attributes under camel-cased names. Like the token list, the
    /// element is the state and the handle is derived from its [`NodeRef`].
    DataSet {
        /// The context the map's owner lives in.
        ctx: SharedContext,
        /// A generational reference to the element whose `data-*` attributes
        /// the map is.
        owner: NodeRef,
    },
    /// A wrapper for the `ValidityState` (`validity`) of the form control
    /// `owner`: a live view, read from the control on every access.
    Validity {
        /// The context the control lives in.
        ctx: SharedContext,
        /// A generational reference to the control.
        owner: NodeRef,
    },
    /// A wrapper for the live `NamedNodeMap` (`attributes`) of `owner`.
    ///
    /// It is the same shape as [`HostObject::TokenList`] — the element is the
    /// state, and the handle is derived from its [`NodeRef`] — which is what
    /// makes `element.attributes === element.attributes` hold.
    NamedNodeMap {
        /// The context the map's owner lives in.
        ctx: SharedContext,
        /// A generational reference to the element whose attributes the map is.
        owner: NodeRef,
    },
    /// A wrapper for the inline `CSSStyleProperties` (`style`) of `owner`.
    Style {
        /// The context the style declaration's owner lives in.
        ctx: SharedContext,
        /// A generational reference to the element whose `style` the block reflects.
        owner: NodeRef,
    },
    /// The window: the browsing context itself, not a node.
    ///
    /// It is the only host with no arena slot and no id, because the window is
    /// the JavaScript global object — `window`, `self`, `top` and `globalThis`
    /// are one object, so a wrapper for it cannot be a fresh instance of a
    /// template. The world resolves its handle to that global object instead of
    /// instantiating anything (see `WrapperWorld::wrapper`), which is why this
    /// variant carries nothing but the context.
    Window {
        /// The context whose browsing context this is.
        ctx: SharedContext,
    },
    /// A JS-owned object with no document row: a `URL`, a `URLSearchParams` or
    /// a `Headers`.
    ///
    /// Its whole state is the slab record, and its wrapper is its only owner, so
    /// the world reclaims the record when the wrapper is finalized — the `Event`
    /// pattern, not the snapshot one.
    Host {
        /// The context holding the host-record slab.
        ctx: SharedContext,
        /// The key to the record in the slab.
        key: HostKey,
    },
}

/// Resolves a `TAG_NODE` handle against `ctx`'s world: the document the handle's
/// slot names and the live node id inside it.
///
/// [`HostObject::anchor_of`] is this function with the receiver's context
/// supplied; it is free so that a constructor, which has a realm and not a
/// receiver, resolves handles by the same rules.
pub fn resolve_node_handle(
    ctx: &SharedContext,
    bits: u64,
) -> Result<(SharedDocument, NodeId), JsError> {
    let decoded = decode(bits);
    if decoded.tag != TAG_NODE {
        return Err(JsError::type_error("the value is not a node of this world"));
    }
    let owner = ctx
        .documents
        .get(decoded.slot)
        .ok_or_else(|| JsError::type_error("the node's document is gone"))?;
    let doc = owner.doc.clone();
    {
        let doc = doc.borrow();
        if !doc.is_live(decoded.node_id)
            || doc.node_ref(decoded.node_id).generation != u32::from(decoded.generation)
        {
            return Err(JsError::type_error("the node is stale"));
        }
    }
    Ok((doc, decoded.node_id))
}

impl HostObject {
    /// Builds a node host for `node` in `ctx`.
    pub fn node(ctx: SharedContext, node: NodeRef) -> Self {
        Self::Node { ctx, node }
    }

    /// Builds a `NodeList` host for the children of `owner` in `ctx`.
    pub fn node_list(ctx: SharedContext, owner: NodeRef) -> Self {
        Self::NodeList { ctx, owner }
    }

    /// Builds an `HTMLCollection` host for the element children of `owner` in `ctx`.
    pub fn html_collection(ctx: SharedContext, owner: NodeRef) -> Self {
        Self::HtmlCollection { ctx, owner }
    }

    /// Builds a `DOMTokenList` host for the `classList` of `owner` in `ctx`.
    pub fn token_list(ctx: SharedContext, owner: NodeRef) -> Self {
        Self::TokenList { ctx, owner }
    }

    /// Builds a `DOMStringMap` host for the `dataset` of `owner` in `ctx`.
    pub fn dataset_map(ctx: SharedContext, owner: NodeRef) -> Self {
        Self::DataSet { ctx, owner }
    }

    /// Builds a `ValidityState` host for the form control `owner` in `ctx`.
    pub fn validity_state(ctx: SharedContext, owner: NodeRef) -> Self {
        Self::Validity { ctx, owner }
    }

    /// Builds a `NamedNodeMap` host for the `attributes` of `owner` in `ctx`.
    pub fn named_node_map(ctx: SharedContext, owner: NodeRef) -> Self {
        Self::NamedNodeMap { ctx, owner }
    }

    /// Builds a `CSSStyleProperties` host for the `style` of `owner` in `ctx`.
    pub fn style(ctx: SharedContext, owner: NodeRef) -> Self {
        Self::Style { ctx, owner }
    }

    /// Builds a `NodeList` host for a static snapshot.
    pub fn static_node_list(ctx: SharedContext, key: SnapshotKey) -> Self {
        Self::StaticNodeList { ctx, key }
    }

    /// Builds an `HTMLCollection` host for a static snapshot.
    pub fn static_html_collection(ctx: SharedContext, key: SnapshotKey) -> Self {
        Self::StaticHtmlCollection { ctx, key }
    }

    /// Builds an `Event` host for the event record at `key`.
    pub fn event(ctx: SharedContext, key: EventKey) -> Self {
        Self::Event { ctx, key }
    }

    /// Builds the window host for `ctx`.
    pub fn window(ctx: SharedContext) -> Self {
        Self::Window { ctx }
    }

    /// One component of the URL `window.location` reports: the document's URL,
    /// or `about:blank` for a document without one.
    pub fn location_part(&self, part: LocationPart) -> String {
        let doc = self.context().doc.borrow();
        part.read(&document_location(doc.url()))
    }

    /// The shared steps of `history.pushState` and `history.replaceState` that
    /// concern the URL: resolves `url` against the document's base URL, checks
    /// that the document may be rewritten to it, makes it the document's URL
    /// and returns it. Without `url` the document keeps its URL, which is
    /// returned.
    pub fn rewrite_document_url(&self, url: Option<&str>) -> Result<String, JsError> {
        let mut doc = self.context().doc.borrow_mut();
        let current = document_location(doc.url());
        let Some(input) = url else {
            return Ok(current.into());
        };
        let base = doc.base_url().cloned().unwrap_or_else(|| current.clone());
        let target = crate::history::rewritten_url(&current, &base, input)
            .map_err(|refusal| JsError::dom_exception("SecurityError", refusal.message()))?;
        doc.set_url(Some(target.clone()));
        Ok(target.into())
    }

    /// `window.matchMedia(query)`: the serialised media query list and
    /// whether it matches now. The environment is the page's viewport as the
    /// layout reports it, or `fallback` (the configured viewport) for a page
    /// without one; the screen is a light, fine-pointer one, as the render
    /// tier's is.
    ///
    /// Stylo is linked only with a script engine (`tenta-css/stylo`), which is
    /// also the only time anything can call this.
    #[cfg(any(feature = "v8", feature = "quickjs"))]
    pub fn match_media(
        &self,
        query: &str,
        fallback: &crate::geometry::ViewportMetrics,
    ) -> (String, bool) {
        let viewport = self
            .script_host()
            .ok()
            .and_then(|script| script.geometry_host())
            .map_or(*fallback, |geometry| geometry.viewport());
        let env = tenta_css::cascade::StyleEnv {
            viewport: (viewport.inner_width as f32, viewport.inner_height as f32),
            device_pixel_ratio: viewport.device_pixel_ratio as f32,
            ..tenta_css::cascade::StyleEnv::default()
        };
        let result = tenta_css::cascade::match_media(query, &env);
        (result.media, result.matches)
    }

    /// Asks the page to navigate, on behalf of script in the page's document.
    /// The request is taken by the page once the running script entry returns;
    /// a document that is not the page's (one a `DOMParser` made) has no
    /// browsing context to navigate, and its request is dropped.
    pub fn request_navigation(&self, request: tenta_dom::NavigationRequest) {
        let ctx = self.context();
        if ctx.slot == 0 {
            ctx.doc.borrow_mut().request_navigation(request);
        }
    }

    /// Navigates the page's document to `target`, as `location.assign` and an
    /// activated link do. A target that differs from the document's URL only
    /// in its fragment stays in the document: the URL is rewritten here and
    /// the script world adds the session history entry and queues
    /// `hashchange` ([`ScriptHost::fragment_navigated`]). Anything else
    /// becomes a request the page takes after this script entry. A
    /// `javascript:` target is ignored.
    pub fn navigate_to(&self, target: Url, replace: bool) -> crate::location::Navigated {
        use crate::location::Navigated;
        if target.scheme() == "javascript" {
            return Navigated::Ignored;
        }
        let ctx = self.context();
        let current = document_location(ctx.doc.borrow().url());
        if crate::location::is_fragment_navigation(&current, &target) {
            let (old_url, new_url): (String, String) = (current.into(), target.clone().into());
            if ctx.slot == 0 {
                ctx.doc.borrow_mut().set_url(Some(target));
                if old_url != new_url
                    && let Some(script) = ctx.script.borrow().clone()
                {
                    script.fragment_navigated(&old_url, &new_url, replace);
                }
            }
            return Navigated::Fragment { old_url, new_url };
        }
        self.request_navigation(tenta_dom::NavigationRequest {
            url: target,
            post: None,
            replace,
        });
        Navigated::Requested
    }

    /// [`Self::navigate_to`] for a URL reference script supplied, resolved
    /// against the document's base URL: the `href` setter, `assign` and
    /// `replace`. A reference that does not parse is a `SyntaxError`.
    pub fn navigate_to_reference(
        &self,
        reference: &str,
        replace: bool,
    ) -> Result<crate::location::Navigated, JsError> {
        let (_, target) = self.resolve_navigation_url(reference)?;
        Ok(self.navigate_to(target, replace))
    }

    /// A `location` component setter other than `href`: the URL Standard's
    /// setter on a copy of the document's URL, then a navigation to the
    /// result. `hash` always navigates, even to the fragment the document
    /// already has; the others only when the URL changed.
    pub fn navigate_by_setting(
        &self,
        part: LocationPart,
        value: &str,
    ) -> crate::location::Navigated {
        let current = document_location(self.context().doc.borrow().url());
        let mut target = current.clone();
        part.write(&mut target, value);
        if part == LocationPart::Hash || target != current {
            self.navigate_to(target, false)
        } else {
            crate::location::Navigated::Ignored
        }
    }

    /// Loads the document's URL again, as `location.reload` does.
    pub fn reload_document(&self) {
        let current = document_location(self.context().doc.borrow().url());
        self.request_navigation(tenta_dom::NavigationRequest {
            url: current,
            post: None,
            replace: true,
        });
    }

    /// The document's URL (`about:blank` without one) and the URL `reference`
    /// resolves to against the document's base URL.
    pub fn resolve_navigation_url(&self, reference: &str) -> Result<(Url, Url), JsError> {
        let doc = self.context().doc.borrow();
        let current = document_location(doc.url());
        let base = doc.base_url().cloned().unwrap_or_else(|| current.clone());
        let target = base.join(reference).map_err(|_| {
            JsError::dom_exception("SyntaxError", &format!("'{reference}' is not a valid URL."))
        })?;
        Ok((current, target))
    }

    /// Makes `url`, the URL of a session history entry this document made, the
    /// document's URL again, as a same-document traversal does. A string that
    /// is not a URL the document may have is ignored.
    pub fn restore_document_url(&self, url: &str) {
        let mut doc = self.context().doc.borrow_mut();
        let current = document_location(doc.url());
        if let Ok(target) = Url::parse(url)
            && crate::history::can_rewrite(&current, &target)
        {
            doc.set_url(Some(target));
        }
    }

    /// Builds the host for the `URL`, `URLSearchParams` or `Headers` record at
    /// `key`.
    pub fn host_record(ctx: SharedContext, key: HostKey) -> Self {
        Self::Host { ctx, key }
    }

    /// The shared context this host belongs to.
    pub fn context(&self) -> &SharedContext {
        match self {
            HostObject::Node { ctx, .. }
            | HostObject::NodeList { ctx, .. }
            | HostObject::HtmlCollection { ctx, .. }
            | HostObject::StaticNodeList { ctx, .. }
            | HostObject::StaticHtmlCollection { ctx, .. }
            | HostObject::Event { ctx, .. }
            | HostObject::TokenList { ctx, .. }
            | HostObject::DataSet { ctx, .. }
            | HostObject::Validity { ctx, .. }
            | HostObject::NamedNodeMap { ctx, .. }
            | HostObject::Style { ctx, .. }
            | HostObject::Host { ctx, .. }
            | HostObject::Window { ctx } => ctx,
        }
    }

    /// This host's document slot — the slot every handle it makes must carry.
    ///
    /// It is the context's slot and not a field of the host, because a host is
    /// built *inside* one context: a node from a second document always reaches
    /// the world through that document's own context, either as the context
    /// `wrapper` was asked for or as the context the operation ran on.
    fn slot(&self) -> u16 {
        self.context().slot
    }

    /// Resolves a `TAG_NODE` handle to the document it names and the live node
    /// id inside it.
    ///
    /// This is the receiving half of the document slot: a handle produced by
    /// another document resolves in *that* document's arena through the world's
    /// [`DocumentSet`], never through this host's. A handle whose slot names no
    /// document, or whose node is stale, is a `TypeError` — the same answer a
    /// wrapper world gives when it refuses to instantiate a wrapper, because
    /// the only way to hold such a handle is to have held a wrapper for it.
    ///
    /// The work is [`resolve_node_handle`]'s, so that a caller with no host —
    /// a constructor, which receives a realm and a dictionary rather than a
    /// receiver — resolves a handle by the same rules.
    fn anchor_of(&self, bits: u64) -> Result<(SharedDocument, NodeId), JsError> {
        resolve_node_handle(self.context(), bits)
    }

    /// The id of a `Node` argument that belongs to *this* host's document.
    ///
    /// A node of another document is refused rather than indexed, because a
    /// handle carries no document id of its own beyond the slot: the id of a
    /// node in another arena means some unrelated node here — a wrong-node write
    /// — or, when it is past the end of this arena, a panic out of the arena's
    /// own accessors. That panic is what `dom/nodes` found once a page could hold
    /// two documents, and it aborted the WPT driver's whole process.
    ///
    /// The refusal is not the DOM Standard's answer to the *operation*, which is
    /// valid: the standard's insertion algorithms adopt the node into the
    /// parent's document first, and
    /// `dom/nodes/Node-mutation-adoptNode.html` asserts exactly that
    /// (`document.body.appendChild(div)` on a `createHTMLDocument` div, then
    /// `div.ownerDocument === document`). Adoption is what this engine does not
    /// have — a node's document *is* the arena it lives in, so adopting would
    /// have to move the subtree into another arena and re-key every wrapper over
    /// it — so the operations that need it report `NotSupportedError` in place
    /// of doing something arbitrary. See deviation 3 in `idl/dom/dom.webidl`.
    fn same_document_node(&self, handle: Handle<Node>) -> Result<NodeId, JsError> {
        let decoded = decode(handle.bits());
        if is_attr_handle(self.context(), handle.bits()) {
            return Err(JsError::dom_exception(
                "HierarchyRequestError",
                "an Attr cannot be inserted into a tree",
            ));
        }
        if decoded.tag != TAG_NODE {
            return Err(JsError::type_error("the argument is not a Node"));
        }
        if decoded.slot != self.slot() {
            return Err(JsError::dom_exception(
                "NotSupportedError",
                "the node belongs to another document, and this engine cannot \
                 adopt it into this one",
            ));
        }
        self.anchor_of(handle.bits()).map(|(_, id)| id)
    }

    /// The id of a `Node` argument the caller claims is a child of this node.
    ///
    /// A node of another document is not a child of this one and cannot be, so
    /// the standard's own "if child's parent is not parent, throw a
    /// `NotFoundError`" already covers it. Testing the slot first is what keeps
    /// the arena's `parent` lookup from being handed an id out of a different
    /// arena.
    fn child_argument(&self, handle: Handle<Node>) -> Result<NodeId, JsError> {
        let decoded = decode(handle.bits());
        if is_attr_handle(self.context(), handle.bits()) {
            return Err(JsError::dom_exception(
                "NotFoundError",
                "an Attr is not a child of this node",
            ));
        }
        if decoded.tag != TAG_NODE {
            return Err(JsError::type_error("the argument is not a Node"));
        }
        if decoded.slot != self.slot() {
            return Err(JsError::dom_exception(
                "NotFoundError",
                "the node is not a child of this node",
            ));
        }
        self.anchor_of(handle.bits()).map(|(_, id)| id)
    }

    /// Gives a freshly built `document` a context of its own in this world and
    /// returns the `TAG_NODE` handle bits of its document node.
    ///
    /// The context is what makes the document reachable: its slot is what every
    /// handle made from its nodes carries, so the wrapper world can find the
    /// arena a handle means. The set owns the context from here on, which is
    /// why the local clone can be dropped at the end of this call.
    fn mint_document(&self, document: Document) -> Result<u64, JsError> {
        mint_document(self.context(), document)
    }

    /// Reads the event record this host stands for, or returns `default` when
    /// the host is not an `Event` or the record has been reclaimed.
    fn with_event<R>(&self, default: R, f: impl FnOnce(&EventRecord) -> R) -> R {
        if let HostObject::Event { ctx, key } = self
            && let Some(record) = ctx.events.borrow().get(*key)
        {
            return f(record);
        }
        default
    }

    /// Runs `f` over the `AttrRecord` this host stands for, or returns
    /// `default` when the host is not an `Attr` or the record is gone.
    fn with_attr<R>(&self, default: R, f: impl FnOnce(&AttrRecord) -> R) -> R {
        if let HostObject::Host { ctx, key } = self
            && let Some(HostRecord::Attr(record)) = ctx.host_records.borrow().get(*key)
        {
            return f(record);
        }
        default
    }

    /// Attr inherits Node but has no arena id or tree position.
    fn is_attr(&self) -> bool {
        self.with_attr(false, |_| true)
    }

    /// The value this `Attr` currently reports: the element's own storage while
    /// the attribute is attached, the record's detached state otherwise.
    ///
    /// The element's document is the one this host's context owns — an `Attr`
    /// is only ever minted by an operation reached through a wrapper of that
    /// same context, so the reference cannot name another arena.
    fn attr_value(&self, record: &AttrRecord) -> String {
        let Some(element) = record.element else {
            return record.detached_value.clone();
        };
        self.context()
            .doc
            .borrow()
            .attribute_ns(element.id, record.name.ns, record.name.local)
            .unwrap_or_default()
            .to_owned()
    }

    /// Attaches the `Attr` at `attr` to the element this host stands for, the
    /// shared half of `Element.setAttributeNode` and `setAttributeNodeNS`.
    ///
    /// The two operations differ only in which name they already have — both
    /// take the attribute's own qualified name — so one implementation serves
    /// both. Returns the `Attr` the new one replaced, or `None`.
    fn attach_attribute_node(&self, attr: Handle<Attr>) -> Result<Option<Handle<Attr>>, JsError> {
        let ctx = self.context();
        let (doc, id) = self.anchor();
        let decoded = decode(attr.bits());
        if decoded.tag != TAG_HOST || decoded.slot != ctx.slot {
            return Err(JsError::dom_exception(
                "NotSupportedError",
                "an Attr from another document cannot be adopted",
            ));
        }
        let key = decoded.host_key();
        let Some(record) = attr_record(ctx, key) else {
            return Err(JsError::type_error("the argument is not an Attr"));
        };
        let node = doc.borrow().node_ref(id);
        if record.element == Some(node) {
            // Setting the exact Attr already in the list is a no-op that
            // returns that same object.
            return Ok(Some(attr));
        }
        if record.element.is_some() {
            return Err(JsError::dom_exception(
                "InUseAttributeError",
                "The attribute is already attached to an element",
            ));
        }
        let name = record.name;
        let replaced = {
            let doc = doc.borrow();
            doc.attributes(id)
                .find(|(stored, _)| stored.ns == name.ns && stored.local == name.local)
                .map(|(stored, value)| (stored, value.to_owned()))
        };
        let owner_pin = AttrOwnerPin::new(doc.clone(), id)?;
        {
            let mut doc = doc.borrow_mut();
            doc.mutate(Mutation::ReplaceAttribute {
                element: id,
                name,
                value: record.detached_value,
            })
            .map_err(|e| mutation_error(&e))?;
        }
        let old_key = replaced
            .as_ref()
            .and_then(|(old_name, value)| ctx.detach_cached_attr(node, *old_name, value.clone()));
        if let Some(HostRecord::Attr(stored)) = ctx.host_records.borrow_mut().get_mut(key) {
            stored.element = Some(node);
            stored.detached_value = String::new();
            stored.owner_pin = Some(owner_pin);
        }
        ctx.attr_index
            .borrow_mut()
            .insert(attr_identity(node, name), key);
        Ok(replaced.map(|(old_name, value)| {
            Handle::from_bits(match old_key {
                Some(old_key) => encode_host(ctx.slot, old_key),
                None => mint_detached_attr(ctx, old_name, value),
            })
        }))
    }

    /// Mutates the event record this host stands for, if it is a live `Event`.
    fn with_event_mut(&self, f: impl FnOnce(&mut EventRecord)) {
        if let HostObject::Event { ctx, key } = self
            && let Some(record) = ctx.events.borrow_mut().get_mut(*key)
        {
            f(record);
        }
    }

    /// Releases one stored value through the installed script host, if any.
    fn release_stored(&self, bits: u64) {
        if let Some(script) = self.context().script.borrow().as_ref() {
            script.release_value(bits);
        }
    }

    /// Sets the event handler IDL attribute named `name` (`onclick`, `onload`).
    ///
    /// This is the body of a generated `set_onX(&self, value:
    /// Option<EventHandlerNonNull>)`: the value is a `Callback` over an `Event`
    /// returning `any`, which is what `EventHandlerNonNull` aliases. A receiver
    /// that is not an event target — a `NodeList`, an `Event`, a `Response` — is
    /// refused with a `TypeError`, the same refusal `addEventListener` makes.
    pub fn set_event_handler(
        &self,
        name: EventHandlerName,
        value: Option<EventHandlerNonNull>,
    ) -> Result<(), JsError> {
        let target = self
            .target_key()
            .ok_or_else(|| JsError::type_error("the receiver is not an event target"))?;
        let ctx = self.context();
        ctx.set_event_handler(target, name, value);
        // The assignment stands over the element's content attribute until the
        // attribute changes, so the attribute's present value is marked seen.
        let attribute_owner = match target {
            TargetKey::Node(node) => ctx.doc.borrow().resolve(node).ok(),
            // The window's handlers are fed by `<body>`'s attributes.
            TargetKey::Window => ctx.doc.borrow().body(),
            TargetKey::Host(_) => None,
        };
        if matches!(target, TargetKey::Node(_) | TargetKey::Window) {
            let source = attribute_owner.and_then(|id| {
                ctx.doc
                    .borrow()
                    .attribute_str(id, name.as_str())
                    .map(str::to_owned)
            });
            let mut seen = ctx.attribute_handlers.borrow_mut();
            match source {
                Some(source) => {
                    seen.insert(
                        (target, name),
                        crate::dom::events::SeenHandlerAttribute {
                            source,
                            compiled: false,
                        },
                    );
                }
                None => {
                    seen.remove(&(target, name));
                }
            }
        }
        Ok(())
    }

    /// Reads the event handler IDL attribute named `name`: the value the page
    /// last assigned, or `None` when the handler is unset.
    pub fn event_handler(
        &self,
        name: EventHandlerName,
    ) -> Result<Option<EventHandlerNonNull>, JsError> {
        let target = self
            .target_key()
            .ok_or_else(|| JsError::type_error("the receiver is not an event target"))?;
        let ctx = self.context();
        // Reading the attribute is one of the two moments a content attribute
        // is compiled (`typeof element.onclick === "function"` after
        // `setAttribute("onclick", ...)` is a common feature test).
        if let Some(script) = ctx.script.borrow().clone() {
            match target {
                TargetKey::Node(node)
                    if EventHandlerName::for_event(name.event_type()) == Some(name) =>
                {
                    crate::dom::events::sync_attribute_handler(ctx, &*script, node, name);
                }
                TargetKey::Window
                    if EventHandlerName::for_window_event(name.event_type()) == Some(name) =>
                {
                    crate::dom::events::sync_window_attribute_handler(ctx, &*script, name);
                }
                _ => {}
            }
        }
        Ok(ctx.event_handler(target, name))
    }

    /// Reads the `URL` this host stands for, or returns `default` when the host
    /// is not a `URL` or its record has been reclaimed. The template's signature
    /// brand check means only a `URL` wrapper can reach a `URL` member, so
    /// `default` is unreachable in practice.
    fn with_url<R>(&self, default: R, f: impl FnOnce(&url::Url) -> R) -> R {
        if let HostObject::Host { ctx, key } = self
            && let Some(HostRecord::Url(record)) = ctx.host_records.borrow().get(*key)
        {
            return f(&record.url);
        }
        default
    }

    /// Mutates the `URL` this host stands for, if it is a live `URL`.
    fn with_url_mut(&self, f: impl FnOnce(&mut url::Url)) {
        if let HostObject::Host { ctx, key } = self
            && let Some(HostRecord::Url(record)) = ctx.host_records.borrow_mut().get_mut(*key)
        {
            f(&mut record.url);
        }
    }

    /// Reads the pair list of the `URLSearchParams` this host stands for, or
    /// returns `default` when the host is not one or its record has been
    /// reclaimed.
    fn with_params<R>(&self, default: R, f: impl FnOnce(&[(String, String)]) -> R) -> R {
        if let HostObject::Host { ctx, key } = self
            && let Some(list) = host_pairs(&ctx.host_records.borrow(), *key)
        {
            return f(&list);
        }
        default
    }

    /// Mutates the pair list of the `URLSearchParams` this host stands for,
    /// performing the URL Standard's update step. A no-op for any other host.
    fn with_params_mut(&self, f: impl FnOnce(&mut Vec<(String, String)>)) {
        if let HostObject::Host { ctx, key } = self {
            mutate_pairs(&mut ctx.host_records.borrow_mut(), *key, f);
        }
    }

    /// Reads the `Headers` this host stands for, or returns `default` when the
    /// host is not one or its record has been reclaimed.
    fn with_headers<R>(&self, default: R, f: impl FnOnce(&HeadersRecord) -> R) -> R {
        if let HostObject::Host { ctx, key } = self
            && let Some(HostRecord::Headers(record)) = ctx.host_records.borrow().get(*key)
        {
            return f(record);
        }
        default
    }

    /// Mutates the header list of the `Headers` this host stands for, with a
    /// mutation of the entry named `name`. A no-op for any other host.
    ///
    /// Both of a guard's questions are asked here, in the order Fetch asks them.
    /// [`HeadersGuard::permits_mutation`] decides whether the operation may be
    /// attempted at all; [`HeadersGuard::permits_name`] decides whether *this*
    /// name may be changed — which is what silently drops a `Set-Cookie` under
    /// the `"response"` guard. The name is checked after the syntactic checks
    /// the caller has already run, because Fetch validates the (name, value)
    /// pair first and only then applies the guard: a forbidden name paired with
    /// an invalid value is a `TypeError`, not a silent drop.
    ///
    /// A guard that forbids a mutation makes this silent rather than loud
    /// because Fetch's own steps return without touching the list in that case;
    /// the `TypeError` an `"immutable"` guard raises belongs with that guard
    /// (`d-4a`).
    fn with_header_mut(&self, name: &[u8], f: impl FnOnce(&mut HeadersRecord)) {
        if let HostObject::Host { ctx, key } = self
            && let Some(HostRecord::Headers(record)) = ctx.host_records.borrow_mut().get_mut(*key)
            && record.guard.permits_mutation()
            && record.guard.permits_name(name)
        {
            f(record);
        }
    }

    /// Reads the `Response` this host stands for, or returns `default` when the
    /// host is not one or its record has been reclaimed.
    fn with_response<R>(&self, default: R, f: impl FnOnce(&ResponseRecord) -> R) -> R {
        if let HostObject::Host { ctx, key } = self
            && let Some(HostRecord::Response(record)) = ctx.host_records.borrow().get(*key)
        {
            return f(record);
        }
        default
    }

    /// Reads the `AbortController` this host stands for, or returns `default`
    /// when the host is not one or its record has been reclaimed.
    fn with_controller<R>(&self, default: R, f: impl FnOnce(&ControllerRecord) -> R) -> R {
        if let HostObject::Host { ctx, key } = self
            && let Some(HostRecord::Controller(record)) = ctx.host_records.borrow().get(*key)
        {
            return f(record);
        }
        default
    }

    /// Reads the `AbortSignal` this host stands for, or returns `default` when
    /// the host is not one or its record has been reclaimed.
    fn with_signal<R>(&self, default: R, f: impl FnOnce(&SignalRecord) -> R) -> R {
        if let HostObject::Host { ctx, key } = self
            && let Some(HostRecord::Signal(record)) = ctx.host_records.borrow().get(*key)
        {
            return f(record);
        }
        default
    }

    /// Reads the `TextDecoder` this host stands for, or returns `default` when
    /// the host is not one or its record has been reclaimed.
    fn with_decoder<R>(&self, default: R, f: impl FnOnce(&TextDecoderRecord) -> R) -> R {
        if let HostObject::Host { ctx, key } = self
            && let Some(HostRecord::TextDecoder(record)) = ctx.host_records.borrow().get(*key)
        {
            return f(record);
        }
        default
    }

    /// Mutates the `TextDecoder` this host stands for, or returns `default`
    /// when the host is not one or its record has been reclaimed. A `decode`
    /// has to write through the record, because the decoder instance and the
    /// `do not flush`/`BOM seen` flags are what the next call reads.
    fn with_decoder_mut<R>(&self, default: R, f: impl FnOnce(&mut TextDecoderRecord) -> R) -> R {
        if let HostObject::Host { ctx, key } = self
            && let Some(HostRecord::TextDecoder(record)) =
                ctx.host_records.borrow_mut().get_mut(*key)
        {
            return f(record);
        }
        default
    }

    /// Reads the `XMLHttpRequest` this host stands for, or returns `default`
    /// when the host is not one or its record has been reclaimed.
    fn with_xhr<R>(&self, default: R, f: impl FnOnce(&XhrRecord) -> R) -> R {
        if let HostObject::Host { ctx, key } = self
            && let Some(HostRecord::Xhr(record)) = ctx.host_records.borrow().get(*key)
        {
            return f(record);
        }
        default
    }

    /// Reads the `Blob` half of the `Blob` or `File` this host stands for, or
    /// returns `default` when the host is neither or its record has been
    /// reclaimed.
    ///
    /// Both variants are read through this one method, which is what makes a
    /// `Blob`-declared member work on a `File` receiver: `File` inherits
    /// `Blob`'s members, and the generated glue reaches them through `BlobOps`
    /// on the same host type. `FileRecord` nests its `BlobRecord` precisely so
    /// this method can hand out a `&BlobRecord` without a second copy of the
    /// bytes.
    fn with_blob<R>(&self, default: R, f: impl FnOnce(&BlobRecord) -> R) -> R {
        if let HostObject::Host { ctx, key } = self
            && let Some(record) = ctx.host_records.borrow().get(*key)
        {
            match record {
                HostRecord::Blob(blob) => return f(blob),
                HostRecord::File(file) => return f(&file.blob),
                _ => {}
            }
        }
        default
    }

    /// Reads the `File` this host stands for, or returns `default` when the
    /// host is not one or its record has been reclaimed.
    fn with_file<R>(&self, default: R, f: impl FnOnce(&FileRecord) -> R) -> R {
        if let HostObject::Host { ctx, key } = self
            && let Some(HostRecord::File(record)) = ctx.host_records.borrow().get(*key)
        {
            return f(record);
        }
        default
    }

    /// Reads the `FormData` this host stands for, or returns `default` when the
    /// host is not one or its record has been reclaimed.
    fn with_form_data<R>(&self, default: R, f: impl FnOnce(&FormDataRecord) -> R) -> R {
        if let HostObject::Host { ctx, key } = self
            && let Some(HostRecord::FormData(record)) = ctx.host_records.borrow().get(*key)
        {
            return f(record);
        }
        default
    }

    /// Mutates the `XMLHttpRequest` this host stands for, or returns `default`
    /// when the host is not one or its record has been reclaimed. `open()` and
    /// the three refusing setters all write through the record, because its
    /// state is what the next member call reads.
    fn with_xhr_mut<R>(&self, default: R, f: impl FnOnce(&mut XhrRecord) -> R) -> R {
        if let HostObject::Host { ctx, key } = self
            && let Some(HostRecord::Xhr(record)) = ctx.host_records.borrow_mut().get_mut(*key)
        {
            return f(record);
        }
        default
    }

    /// Whether this host has standalone EventTarget lifetime semantics.
    /// Linked host targets such as AbortSignal need different ownership edges.
    pub fn is_standalone_event_target(&self) -> bool {
        matches!(self, HostObject::Host { ctx, key }
            if matches!(ctx.host_records.borrow().get(*key), Some(HostRecord::EventTarget)))
    }

    /// The record key of the host object this host stands for when that object
    /// is an `EventTarget`, or `None` for every other host.
    ///
    /// The tag cannot answer this: every JS-owned wrapper carries [`TAG_HOST`],
    /// so only the record knows which interface it is — and of them,
    /// standalone `EventTarget`, `AbortSignal`, `XMLHttpRequest`, and `Attr`
    /// are event targets. This is the
    /// one place the host asks, and both event-target queries below go through
    /// it, as does every refusal that has to name the receiver as a target.
    fn event_target_key(&self) -> Option<HostKey> {
        if let HostObject::Host { ctx, key } = self
            && matches!(
                ctx.host_records.borrow().get(*key),
                Some(
                    HostRecord::EventTarget
                        | HostRecord::Signal(_)
                        | HostRecord::Xhr(_)
                        | HostRecord::Attr(_)
                )
            )
        {
            return Some(*key);
        }
        None
    }

    /// The opaque handle bits this host stands for, tagged by variant so a node
    /// and its `NodeList` (and a recycled slot) never share a key.
    pub fn handle_bits(&self) -> u64 {
        let slot = self.slot();
        match self {
            HostObject::Node { node, .. } => encode_node(slot, *node),
            HostObject::NodeList { owner, .. } => encode_collection(slot, *owner),
            HostObject::HtmlCollection { owner, .. } => encode_html_collection(slot, *owner),
            HostObject::StaticNodeList { key, .. } => {
                encode_snapshot(slot, TAG_STATIC_NODE_LIST, *key)
            }
            HostObject::StaticHtmlCollection { key, .. } => {
                encode_snapshot(slot, TAG_STATIC_HTML_COLLECTION, *key)
            }
            HostObject::Event { key, .. } => encode_event(slot, *key),
            HostObject::TokenList { owner, .. } => encode_token_list(slot, *owner),
            HostObject::DataSet { owner, .. } => encode_dataset(slot, *owner),
            HostObject::Validity { owner, .. } => encode_validity(slot, *owner),
            HostObject::NamedNodeMap { owner, .. } => encode_named_node_map(slot, *owner),
            HostObject::Style { owner, .. } => encode_style(slot, *owner),
            HostObject::Host { key, .. } => encode_host(slot, *key),
            HostObject::Window { .. } => encode_window(),
        }
    }

    /// The document and the node this host is anchored to: the node itself for
    /// a `Node` host, the owner for a `NodeList` or `HtmlCollection` host.
    /// For snapshot variants (StaticNodeList/StaticHtmlCollection), returns the document
    /// and NodeId::NONE, as snapshot hosts never participate in node-navigation ops
    /// (routing by signature brand ensures only live-collection ops call navigate).
    fn anchor(&self) -> (&SharedDocument, NodeId) {
        match self {
            HostObject::Node { ctx, node } => (&ctx.doc, node.id),
            HostObject::NodeList { ctx, owner } => (&ctx.doc, owner.id),
            HostObject::HtmlCollection { ctx, owner } => (&ctx.doc, owner.id),
            HostObject::TokenList { ctx, owner } => (&ctx.doc, owner.id),
            HostObject::DataSet { ctx, owner } => (&ctx.doc, owner.id),
            HostObject::Validity { ctx, owner } => (&ctx.doc, owner.id),
            HostObject::NamedNodeMap { ctx, owner } => (&ctx.doc, owner.id),
            HostObject::Style { ctx, owner } => (&ctx.doc, owner.id),
            HostObject::StaticNodeList { ctx, .. } => (&ctx.doc, NodeId::NONE),
            HostObject::StaticHtmlCollection { ctx, .. } => (&ctx.doc, NodeId::NONE),
            HostObject::Event { ctx, .. } => (&ctx.doc, NodeId::NONE),
            // No JS-owned host object is in the tree, not even an `AbortSignal`:
            // it is an event target, and its abort event does not reach the
            // document.
            HostObject::Host { ctx, .. } => (&ctx.doc, NodeId::NONE),
            // The window is not in the tree, so there is no node to anchor to.
            // It only reaches the anchored paths if a generated operation that
            // needs a node is somehow reachable on the window, which none is:
            // the window's own members are installed natively.
            HostObject::Window { ctx } => (&ctx.doc, NodeId::NONE),
        }
    }

    /// The event target this host stands for, or `None` for a host that cannot
    /// be one.
    ///
    /// A node wrapper, the window, a standalone target, an `AbortSignal` and
    /// an `XMLHttpRequest` carry the `EventTarget` prototype chain. JS-owned
    /// host objects require [`Self::dispatch_target`] to consult the record
    /// rather than the handle tag. A `NodeList`, a snapshot, an `Event`, a
    /// `URL`, a `URLSearchParams` or a style declaration reaching here means
    /// the receiver was not what the template's signature promised.
    fn dispatch_target(&self) -> Option<DispatchTarget> {
        match self {
            HostObject::Node { node, .. } => Some(DispatchTarget::node(self.slot(), *node)),
            HostObject::Window { .. } => Some(DispatchTarget::window()),
            _ => self
                .event_target_key()
                .map(|key| DispatchTarget::host(self.slot(), key)),
        }
    }

    /// The listener-table key this host registers listeners under. See
    /// [`Self::dispatch_target`] for why the other variants have none.
    fn target_key(&self) -> Option<TargetKey> {
        match self {
            HostObject::Node { node, .. } => Some(TargetKey::Node(*node)),
            HostObject::Window { .. } => Some(TargetKey::Window),
            _ => self.event_target_key().map(TargetKey::Host),
        }
    }

    /// Resolve an omitted passive option when the listener is registered.
    /// The choice must not change if a body or document element is later moved.
    fn default_passive_value(&self, event_type: &str) -> bool {
        if !matches!(
            event_type,
            "touchstart" | "touchmove" | "wheel" | "mousewheel"
        ) {
            return false;
        }
        match self {
            HostObject::Window { .. } => true,
            HostObject::Node { ctx, node } => {
                let doc = ctx.doc.borrow();
                let Ok(id) = doc.resolve(*node) else {
                    return false;
                };
                if id == doc.document_node() || doc.document_element() == Some(id) {
                    return true;
                }
                // Document::body selects the first HTML body or frameset
                // under an HTML html document element.
                doc.body() == Some(id)
            }
            _ => false,
        }
    }

    /// Sets a reflected attribute (`id`, `class`, …) to `value` on the anchored
    /// element, ignoring any mutation error the way a fast-API reflected setter
    /// must. `name` is an already-lowercase attribute local name.
    fn reflect_attribute(&self, name: &str, value: &str) {
        let (_, id) = self.anchor();
        let qname = QualName::new(atoms::EMPTY, Atom::new(name));
        let _ = self
            .context()
            .set_element_attribute(id, qname, value.to_owned());
    }

    /// Reads a reflected DOMString attribute, returning "" when it is absent —
    /// the WHATWG default for a string-reflecting IDL getter. `name` is an
    /// already-lowercase local name.
    fn reflect_string(&self, name: &str) -> String {
        let (doc, id) = self.anchor();
        doc.borrow()
            .attribute_str(id, name)
            .unwrap_or("")
            .to_owned()
    }

    /// Reads a reflected URL-valued attribute — the HTML Standard's rule for a
    /// `USVString` attribute whose content attribute is defined to contain a
    /// URL, which is what backs `href`, `src` and `action`
    /// (`wpt/checkout/html/dom/reflection.js`, the `"url"` reflection case).
    /// An absent attribute yields the empty string (never the document URL, so
    /// `if (img.src)` stays falsy for `<img>` with no `src`); a present value
    /// is resolved against the element's node document and the serialized
    /// absolute URL returned; a value that will not parse is returned verbatim
    /// rather than throwing. `name` is an already-lowercase local name.
    fn reflect_url(&self, name: &str) -> String {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let Some(raw) = doc.attribute_str(id, name) else {
            return String::new();
        };
        match doc.resolve_url(raw) {
            Some(url) => url.to_string(),
            None => raw.to_owned(),
        }
    }

    /// Reads a reflected boolean attribute: present (any value) is `true`.
    fn reflect_bool(&self, name: &str) -> bool {
        let (doc, id) = self.anchor();
        doc.borrow().attribute_str(id, name).is_some()
    }

    /// Reads a form control's IDL `value` under the dirty value model
    /// (`docs/spec/05-javascript.md` §9a): the dirty value once one has been
    /// set, and the `attribute` content attribute until then. A control whose
    /// default comes from somewhere other than an attribute (a textarea's child
    /// text) reads the dirty value with [`Self::dirty_value`] instead.
    fn control_value(&self, attribute: &str) -> String {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        match doc.dirty_value(id) {
            Some(dirty) => dirty.to_owned(),
            None => doc.attribute_str(id, attribute).unwrap_or("").to_owned(),
        }
    }

    /// The dirty value alone, for a control whose default is not an attribute.
    fn dirty_value(&self) -> Option<String> {
        let (doc, id) = self.anchor();
        doc.borrow().dirty_value(id).map(str::to_owned)
    }

    /// Assigning to a control's `value` always stores the dirty value and
    /// raises the dirty value flag; the content attribute is left untouched,
    /// so `getAttribute("value")` keeps reporting the default.
    fn set_control_value(&self, value: &str) {
        let (doc, id) = self.anchor();
        doc.borrow_mut().set_dirty_value(id, value.to_owned());
    }

    /// Writes a reflected boolean attribute: `true` adds it with the empty-string
    /// value, `false` removes it. Mirrors `toggleAttribute` with an explicit
    /// force and, like the other reflected setters, ignores a mutation error.
    fn set_reflect_bool(&self, name: &str, value: bool) {
        let (_, id) = self.anchor();
        let qname = QualName::new(atoms::EMPTY, Atom::new(name));
        if value {
            let _ = self
                .context()
                .set_element_attribute(id, qname, String::new());
        } else {
            let _ = self.context().remove_element_attribute(id, qname);
        }
    }

    /// Detaches the anchored node from its parent (`ChildNode.remove()`). A node
    /// with no parent is a no-op; the node stays alive as long as script holds a
    /// reference to its wrapper.
    fn remove_from_parent(&self) {
        let (doc, id) = self.anchor();
        let _ = doc.borrow_mut().mutate(Mutation::Remove { child: id });
    }

    /// The anchored element's `class` attribute parsed as a DOM ordered set:
    /// ASCII-whitespace-split, empties dropped, first occurrence of each token
    /// kept. Backs the `DOMTokenList` read and mutation ops.
    fn class_tokens(&self) -> Vec<String> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let value = doc.attribute_str(id, "class").unwrap_or("");
        if value.len() >= LONG_TOKEN_LIST {
            return ordered_tokens(value)
                .into_iter()
                .map(str::to_owned)
                .collect();
        }
        // The usual handful of tokens: one pass, no intermediate list.
        let mut tokens: Vec<String> = Vec::new();
        for token in value.split_ascii_whitespace() {
            if !tokens.iter().any(|existing| existing == token) {
                tokens.push(token.to_owned());
            }
        }
        tokens
    }

    /// Reads the anchored element's class ordered set without copying its
    /// tokens: what `length`, `item` and `contains` ask on every call.
    fn with_class_tokens<R>(&self, read: impl FnOnce(&[&str]) -> R) -> R {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let value = doc.attribute_str(id, "class").unwrap_or("");
        if value.len() < LONG_TOKEN_LIST {
            return read(&ordered_tokens(value));
        }
        // Walking a list by index reads it once per token. Parsing a long
        // one each time made the walk quadratic with a large constant, so
        // the last long list parsed is kept and compared instead.
        let tokens: Vec<&str> = LONG_TOKEN_SET.with(|cached| {
            let mut cached = cached.borrow_mut();
            if cached.0 != value {
                let start = value.as_ptr() as usize;
                cached.1 = ordered_tokens(value)
                    .iter()
                    .map(|token| {
                        let offset = token.as_ptr() as usize - start;
                        (offset as u32, (offset + token.len()) as u32)
                    })
                    .collect();
                cached.0.clear();
                cached.0.push_str(value);
            }
            cached
                .1
                .iter()
                .map(|&(start, end)| &value[start as usize..end as usize])
                .collect()
        });
        read(&tokens)
    }

    /// Serialises an ordered set back into the `class` attribute (space-joined).
    fn set_class_tokens(&self, tokens: &[String]) {
        self.reflect_attribute("class", &tokens.join(" "));
    }

    /// A `CSSStyleProperties` handle for the anchored element's inline `style`.
    fn style_handle(&self) -> Result<Handle<CSSStyleProperties>, JsError> {
        let (doc, id) = self.anchor();
        let owner = doc.borrow().node_ref(id);
        Ok(Handle::from_bits(encode_style(self.slot(), owner)))
    }

    /// Parse the anchored element's inline declaration block with Stylo.
    #[cfg(any(feature = "v8", feature = "quickjs"))]
    fn style_declarations(&self) -> CssDeclarationBlock {
        let (doc, id) = self.anchor();
        let (text, base) = {
            let doc = doc.borrow();
            (
                doc.attribute_str(id, "style")
                    .unwrap_or_default()
                    .to_owned(),
                doc.base_url()
                    .or(doc.url())
                    .cloned()
                    .unwrap_or_else(about_blank),
            )
        };
        CssDeclarationBlock::parse(&text, &base)
    }

    /// Serialise Stylo's effective declarations back into the style attribute.
    #[cfg(any(feature = "v8", feature = "quickjs"))]
    fn set_style_declarations(&self, declarations: &CssDeclarationBlock) {
        self.reflect_attribute("style", &declarations.snapshot().css_text);
    }

    /// Applies a navigation step to the anchored node and wraps the result as a
    /// freshly read `Node` handle (never stale), or `None` at the tree edge.
    fn navigate(&self, step: impl Fn(&Document, NodeId) -> NodeId) -> Option<Handle<Node>> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let target = step(&doc, id);
        target
            .is_some()
            .then(|| Handle::from_bits(encode_node(self.slot(), doc.node_ref(target))))
    }
}

/// A reflected nullable DOMString attribute: null when the attribute is
/// absent, and assigning null removes it.
macro_rules! reflected_nullable_strings {
    ($($getter:ident, $setter:ident => $attribute:literal;)*) => {
        $(
            fn $getter(&self) -> Result<Option<String>, JsError> {
                let (doc, id) = self.anchor();
                Ok(doc.borrow().attribute_str(id, $attribute).map(str::to_owned))
            }
            fn $setter(&self, value: Option<&str>) -> Result<(), JsError> {
                match value {
                    Some(value) => self.reflect_attribute($attribute, value),
                    None => self.set_reflect_bool($attribute, false),
                }
                Ok(())
            }
        )*
    };
}

impl HostObject {
    /// The node `lookupPrefix()` and its two siblings start from: the element
    /// itself, a document's root element, the parent element of any other
    /// node in a tree. `None` for a node with no element to ask (a doctype, a
    /// fragment, an attribute, a detached text node).
    fn namespace_scope(&self) -> Option<(SharedDocument, NodeId)> {
        if self.is_attr() {
            return None;
        }
        let (doc, id) = self.anchor();
        let start = {
            let document = doc.borrow();
            match document.kind(id) {
                NodeKind::Element => Some(id),
                NodeKind::Document => document.document_element(),
                NodeKind::DocType | NodeKind::Fragment | NodeKind::ShadowRoot => None,
                _ => {
                    let parent = document.parent(id);
                    (parent.is_some() && document.kind(parent) == NodeKind::Element)
                        .then_some(parent)
                }
            }
        };
        start.map(|start| (Rc::clone(doc), start))
    }

    /// DOM's *locate a namespace* for `prefix` (`None` is the default
    /// namespace), from the scope element up through its ancestors.
    fn locate_namespace(&self, prefix: Option<&str>) -> Option<String> {
        match prefix {
            Some("xml") => return Some(atoms::NS_XML.as_str().to_owned()),
            Some("xmlns") => return Some(atoms::NS_XMLNS.as_str().to_owned()),
            _ => {}
        }
        let (doc, mut element) = self.namespace_scope()?;
        let doc = doc.borrow();
        loop {
            let own_prefix = doc
                .element_prefix(element)
                .filter(|own| *own != atoms::EMPTY)
                .map(|own| own.as_str());
            if let Some(namespace) = doc.namespace(element)
                && !namespace.as_str().is_empty()
                && own_prefix == prefix
            {
                return Some(namespace.as_str().to_owned());
            }
            // An `xmlns:prefix` attribute, or `xmlns` for the default one.
            for (name, value) in doc.attributes(element) {
                if name.ns != atoms::NS_XMLNS {
                    continue;
                }
                let declares = match prefix {
                    Some(prefix) => {
                        name.prefix.is_some_and(|p| p == atoms::XMLNS)
                            && name.local.as_str() == prefix
                    }
                    None => {
                        name.prefix.is_none_or(|p| p == atoms::EMPTY) && name.local == atoms::XMLNS
                    }
                };
                if declares {
                    return (!value.is_empty()).then(|| value.to_owned());
                }
            }
            let parent = doc.parent(element);
            if !parent.is_some() || doc.kind(parent) != NodeKind::Element {
                return None;
            }
            element = parent;
        }
    }

    /// DOM's *locate a namespace prefix* for `namespace`.
    fn locate_prefix(&self, namespace: &str) -> Option<String> {
        let (doc, mut element) = self.namespace_scope()?;
        let doc = doc.borrow();
        loop {
            if doc
                .namespace(element)
                .is_some_and(|own| own.as_str() == namespace)
                && let Some(prefix) = doc
                    .element_prefix(element)
                    .filter(|prefix| *prefix != atoms::EMPTY)
            {
                return Some(prefix.as_str().to_owned());
            }
            for (name, value) in doc.attributes(element) {
                if name.prefix.is_some_and(|p| p == atoms::XMLNS) && value == namespace {
                    return Some(name.local.as_str().to_owned());
                }
            }
            let parent = doc.parent(element);
            if !parent.is_some() || doc.kind(parent) != NodeKind::Element {
                return None;
            }
            element = parent;
        }
    }

    /// Whether the element is editable: what the nearest element that states
    /// `contenteditable` says, false when none does.
    fn content_editable_state(&self) -> bool {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let mut node = id;
        while node.is_some() && doc.kind(node) == NodeKind::Element {
            if let Some(value) = doc.attribute_str(node, "contenteditable") {
                match value.to_ascii_lowercase().as_str() {
                    "" | "true" | "plaintext-only" => return true,
                    "false" => return false,
                    _ => {}
                }
            }
            node = doc.parent(node);
        }
        false
    }
}

impl NodeOps for HostObject {
    fn lookup_prefix(&self, namespace: Option<&str>) -> Result<Option<String>, JsError> {
        Ok(namespace
            .filter(|namespace| !namespace.is_empty())
            .and_then(|namespace| self.locate_prefix(namespace)))
    }

    fn lookup_namespace_uri(&self, prefix: Option<&str>) -> Result<Option<String>, JsError> {
        Ok(self.locate_namespace(prefix.filter(|prefix| !prefix.is_empty())))
    }

    fn is_default_namespace(&self, namespace: Option<&str>) -> Result<bool, JsError> {
        let namespace = namespace.filter(|namespace| !namespace.is_empty());
        Ok(self.locate_namespace(None).as_deref() == namespace)
    }

    fn child_nodes(&self) -> Result<Handle<NodeList>, JsError> {
        if let HostObject::Host { ctx, key } = self
            && self.is_attr()
        {
            let cached = self.with_attr(None, |record| record.child_nodes);
            if let Some(cached) = cached
                && ctx.snapshots.borrow().get(cached).is_some()
            {
                return Ok(Handle::from_bits(encode_snapshot(
                    ctx.slot,
                    TAG_STATIC_NODE_LIST,
                    cached,
                )));
            }
            let snapshot = ctx.snapshots.borrow_mut().insert(Vec::new())?;
            if let Some(HostRecord::Attr(record)) = ctx.host_records.borrow_mut().get_mut(*key) {
                record.child_nodes = Some(snapshot);
            }
            return Ok(Handle::from_bits(encode_snapshot(
                ctx.slot,
                TAG_STATIC_NODE_LIST,
                snapshot,
            )));
        }
        let (doc, id) = self.anchor();
        let owner = doc.borrow().node_ref(id);
        Ok(Handle::from_bits(encode_collection(self.slot(), owner)))
    }

    fn first_child(&self) -> Result<Option<Handle<Node>>, JsError> {
        if self.is_attr() {
            return Ok(None);
        }
        Ok(self.navigate(|doc, id| doc.first_child(id)))
    }

    fn last_child(&self) -> Result<Option<Handle<Node>>, JsError> {
        if self.is_attr() {
            return Ok(None);
        }
        Ok(self.navigate(|doc, id| doc.last_child(id)))
    }

    fn next_sibling(&self) -> Result<Option<Handle<Node>>, JsError> {
        if self.is_attr() {
            return Ok(None);
        }
        Ok(self.navigate(|doc, id| doc.next_sibling(id)))
    }

    fn node_name(&self) -> Result<String, JsError> {
        if self.is_attr() {
            return AttrOps::name(self);
        }
        let (doc, id) = self.anchor();
        Ok(node_name(&doc.borrow(), id))
    }

    fn node_type(&self) -> u16 {
        // The DOM `nodeType` codes (https://dom.spec.whatwg.org/#dom-node-nodetype).
        // The `NodeOps` consts cover the kinds this slice exercises; the rest
        // use the fixed spec values.
        if self.is_attr() {
            return Self::ATTRIBUTE_NODE;
        }
        let (doc, id) = self.anchor();
        match doc.borrow().kind(id) {
            NodeKind::Element => <Self as NodeOps>::ELEMENT_NODE,
            NodeKind::Text => <Self as NodeOps>::TEXT_NODE,
            NodeKind::Comment => <Self as NodeOps>::COMMENT_NODE,
            NodeKind::Document => <Self as NodeOps>::DOCUMENT_NODE,
            NodeKind::CData => 4,
            NodeKind::ProcessingInstruction => 7,
            NodeKind::DocType => 10,
            NodeKind::Fragment | NodeKind::ShadowRoot => 11,
        }
    }

    fn owner_document(&self) -> Result<Option<Handle<super::Document>>, JsError> {
        // The Document node's `ownerDocument` is null; every other node in this
        // world belongs to the one document the host is anchored in. `Document`
        // is qualified because `tenta_dom::Document` shadows the binding marker.
        if self.is_attr() {
            let ctx = self.context();
            let doc = ctx.doc.borrow();
            return Ok(Some(Handle::from_bits(encode_node(
                ctx.slot,
                doc.node_ref(doc.document_node()),
            ))));
        }
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let document_node = doc.document_node();
        Ok((id != document_node)
            .then(|| Handle::from_bits(encode_node(self.slot(), doc.node_ref(document_node)))))
    }

    fn is_connected(&self) -> bool {
        if self.is_attr() {
            return false;
        }
        // Connected in the standard's sense: a node in a shadow tree is
        // connected when its host is.
        let (doc, id) = self.anchor();
        doc.borrow().is_composed_connected(id)
    }

    fn base_uri(&self) -> Result<String, JsError> {
        Ok(self.base_uri_value())
    }
    fn is_equal_node(&self, other_node: Option<Handle<Node>>) -> Result<bool, JsError> {
        Ok(self.equals_node(other_node))
    }
    fn is_same_node(&self, other_node: Option<Handle<Node>>) -> Result<bool, JsError> {
        Ok(self.is_same(other_node))
    }
    fn compare_document_position(&self, other: Handle<Node>) -> Result<u16, JsError> {
        Ok(self.document_position(other))
    }
    fn get_root_node(
        &self,
        options: Option<crate::bindings::GetRootNodeOptions>,
    ) -> Result<Handle<Node>, JsError> {
        if options.is_some_and(|options| options.composed) {
            return Ok(self.composed_root_node());
        }
        Ok(self.root_node())
    }
    fn normalize(&self) -> Result<(), JsError> {
        self.normalize_subtree()
    }

    fn has_child_nodes(&self) -> bool {
        if self.is_attr() {
            return false;
        }
        let (doc, id) = self.anchor();
        doc.borrow().first_child(id).is_some()
    }

    fn contains(&self, other: Option<Handle<Node>>) -> Result<bool, JsError> {
        // `node.contains(other)` is true when `other` is an inclusive descendant
        // of `node`. `ancestors` excludes the node itself, so the identity case
        // is checked separately. A null argument is not contained. A swept
        // `other` is not live, so `ancestors` yields nothing and the answer is
        // false. Exhausted arena slots are retired before the packed generation
        // could wrap, so a stale handle cannot become a different node.
        let Some(other) = other else {
            return Ok(false);
        };
        if self.is_attr() {
            return Ok(self.handle_bits() == other.bits());
        }
        if is_attr_handle(self.context(), other.bits()) {
            return Ok(false);
        }
        // A node of another document is not this tree's, so it is not an
        // inclusive ancestor of anything here. Its id must not reach
        // `doc.ancestors`, which walks `parent` in *this* arena.
        let decoded = decode(other.bits());
        if decoded.tag != TAG_NODE {
            return Err(JsError::type_error("the argument is not a Node"));
        }
        let (doc, self_id) = self.anchor();
        if decoded.slot != self.slot() {
            return Ok(false);
        }
        let other_id = decoded.node_id;
        let doc = doc.borrow();
        Ok(other_id == self_id || doc.ancestors(other_id).any(|ancestor| ancestor == self_id))
    }

    fn parent_element(&self) -> Result<Option<Handle<Element>>, JsError> {
        if self.is_attr() {
            return Ok(None);
        }
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let parent = doc.parent(id);
        Ok((parent.is_some() && doc.kind(parent) == NodeKind::Element)
            .then(|| Handle::from_bits(encode_node(self.slot(), doc.node_ref(parent)))))
    }

    fn parent_node(&self) -> Result<Option<Handle<Node>>, JsError> {
        if self.is_attr() {
            return Ok(None);
        }
        Ok(self.navigate(|doc, id| doc.parent(id)))
    }

    fn previous_sibling(&self) -> Result<Option<Handle<Node>>, JsError> {
        if self.is_attr() {
            return Ok(None);
        }
        Ok(self.navigate(|doc, id| doc.previous_sibling(id)))
    }

    fn text_content(&self) -> Result<Option<String>, JsError> {
        if self.is_attr() {
            return AttrOps::value(self).map(Some);
        }
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let kind = doc.kind(id);
        Ok(match kind {
            NodeKind::Document | NodeKind::DocType => None,
            // A `ProcessingInstruction` is a `CharacterData` subclass, so its
            // `textContent` is its `data`, exactly like a comment's.
            NodeKind::Text
            | NodeKind::CData
            | NodeKind::Comment
            | NodeKind::ProcessingInstruction => Some(doc.text_data(id).unwrap_or("").to_owned()),
            NodeKind::Element | NodeKind::Fragment | NodeKind::ShadowRoot => {
                let mut result = String::new();
                for child_id in doc.descendants(id) {
                    if matches!(doc.kind(child_id), NodeKind::Text | NodeKind::CData)
                        && let Some(data) = doc.text_data(child_id)
                    {
                        result.push_str(data);
                    }
                }
                Some(result)
            }
        })
    }

    fn set_text_content(&self, value: Option<&str>) -> Result<(), JsError> {
        if self.is_attr() {
            self.write_attr_value(value.unwrap_or_default());
            return Ok(());
        }
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let kind = doc.kind(id);

        match kind {
            NodeKind::Text
            | NodeKind::CData
            | NodeKind::Comment
            | NodeKind::ProcessingInstruction => {
                let text = value.unwrap_or("");
                drop(doc);
                let mut doc = self.anchor().0.borrow_mut();
                doc.mutate(Mutation::SetText {
                    node: id,
                    text: text.to_owned(),
                })
                .map_err(|e| mutation_error(&e))
            }
            NodeKind::Element | NodeKind::Fragment | NodeKind::ShadowRoot => {
                drop(doc);
                let mut doc = self.anchor().0.borrow_mut();
                let replacement = if let Some(text) = value.filter(|text| !text.is_empty()) {
                    ReplaceAllWith::Detached(doc.create_text(text))
                } else {
                    ReplaceAllWith::Empty
                };
                doc.mutate(Mutation::ReplaceAll {
                    parent: id,
                    replacement,
                })
                .map_err(|e| mutation_error(&e))
            }
            NodeKind::Document | NodeKind::DocType => {
                // No-op for Document and similar
                Ok(())
            }
        }
    }

    fn node_value(&self) -> Result<Option<String>, JsError> {
        if self.is_attr() {
            return AttrOps::value(self).map(Some);
        }
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        match doc.kind(id) {
            NodeKind::Text
            | NodeKind::CData
            | NodeKind::Comment
            | NodeKind::ProcessingInstruction => {
                Ok(Some(doc.text_data(id).unwrap_or("").to_owned()))
            }
            _ => Ok(None),
        }
    }

    fn set_node_value(&self, value: Option<&str>) -> Result<(), JsError> {
        if self.is_attr() {
            self.write_attr_value(value.unwrap_or_default());
            return Ok(());
        }
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        match doc.kind(id) {
            NodeKind::Text
            | NodeKind::CData
            | NodeKind::Comment
            | NodeKind::ProcessingInstruction => {
                let text = value.unwrap_or("");
                drop(doc);
                let mut doc = self.anchor().0.borrow_mut();
                doc.mutate(Mutation::SetText {
                    node: id,
                    text: text.to_owned(),
                })
                .map_err(|e| mutation_error(&e))
            }
            _ => {
                // No-op for other node types
                Ok(())
            }
        }
    }

    fn append_child(&self, node: Handle<Node>) -> Result<Handle<Node>, JsError> {
        if self.is_attr() {
            return Err(JsError::dom_exception(
                "HierarchyRequestError",
                "an Attr cannot have children",
            ));
        }
        let (doc, self_id) = self.anchor();
        let child_id = self.same_document_node(node)?;

        let mut doc = doc.borrow_mut();
        doc.mutate(Mutation::Append {
            parent: self_id,
            child: child_id,
        })
        .map_err(|e| mutation_error(&e))?;
        drop(doc);

        // Return the argument node's Handle (identity preserved)
        Ok(node)
    }

    fn insert_before(
        &self,
        node: Handle<Node>,
        child: Option<Handle<Node>>,
    ) -> Result<Handle<Node>, JsError> {
        if self.is_attr() {
            return Err(JsError::dom_exception(
                "HierarchyRequestError",
                "an Attr cannot have children",
            ));
        }
        let (doc, self_id) = self.anchor();
        let node_id = self.same_document_node(node)?;

        let before = child
            .map(|child| self.child_argument(child))
            .transpose()?
            .unwrap_or(NodeId::NONE);

        let mut doc = doc.borrow_mut();
        doc.mutate(Mutation::InsertBefore {
            parent: self_id,
            child: node_id,
            before,
        })
        .map_err(|e| mutation_error(&e))?;
        drop(doc);

        Ok(node)
    }

    fn remove_child(&self, child: Handle<Node>) -> Result<Handle<Node>, JsError> {
        if self.is_attr() {
            return Err(JsError::dom_exception(
                "NotFoundError",
                "an Attr has no child to remove",
            ));
        }
        let (doc, _) = self.anchor();
        let child_id = self.child_argument(child)?;

        let mut doc = doc.borrow_mut();
        doc.mutate(Mutation::Remove { child: child_id })
            .map_err(|e| mutation_error(&e))?;
        drop(doc);

        Ok(child)
    }

    fn replace_child(
        &self,
        node: Handle<Node>,
        child: Handle<Node>,
    ) -> Result<Handle<Node>, JsError> {
        if self.is_attr() {
            return Err(JsError::dom_exception(
                "HierarchyRequestError",
                "an Attr cannot have children",
            ));
        }
        let (doc, self_id) = self.anchor();
        let new_child = self.same_document_node(node)?;
        let old_child = self.child_argument(child)?;

        let mut doc = doc.borrow_mut();
        doc.mutate(Mutation::ReplaceChild {
            parent: self_id,
            new_child,
            old_child,
        })
        .map_err(|e| mutation_error(&e))?;
        drop(doc);

        // Return the old child's Handle (per spec)
        Ok(child)
    }

    fn clone_node(&self, deep: Option<bool>) -> Result<Handle<Node>, JsError> {
        if self.is_attr() {
            let record =
                self.with_attr(None, |record| Some((record.name, self.attr_value(record))));
            let Some((name, value)) = record else {
                return Err(JsError::type_error("the Attr record is gone"));
            };
            return Ok(Handle::from_bits(mint_detached_attr(
                self.context(),
                name,
                value,
            )));
        }
        let (doc, self_id) = self.anchor();
        let deep_flag = deep.unwrap_or(false);

        let new_id = {
            let mut doc = doc.borrow_mut();
            doc.clone_node(self_id, deep_flag)
                .map_err(|e| mutation_error(&e))?
        };

        self.context().note_cloned_images(new_id);
        let doc = doc.borrow();
        Ok(Handle::from_bits(encode_node(
            self.slot(),
            doc.node_ref(new_id),
        )))
    }
}

impl ElementOps for HostObject {
    reflected_nullable_strings! {
        role, set_role => "role";
        aria_atomic, set_aria_atomic => "aria-atomic";
        aria_auto_complete, set_aria_auto_complete => "aria-autocomplete";
        aria_busy, set_aria_busy => "aria-busy";
        aria_checked, set_aria_checked => "aria-checked";
        aria_col_count, set_aria_col_count => "aria-colcount";
        aria_col_index, set_aria_col_index => "aria-colindex";
        aria_col_span, set_aria_col_span => "aria-colspan";
        aria_current, set_aria_current => "aria-current";
        aria_description, set_aria_description => "aria-description";
        aria_disabled, set_aria_disabled => "aria-disabled";
        aria_expanded, set_aria_expanded => "aria-expanded";
        aria_has_popup, set_aria_has_popup => "aria-haspopup";
        aria_hidden, set_aria_hidden => "aria-hidden";
        aria_invalid, set_aria_invalid => "aria-invalid";
        aria_key_shortcuts, set_aria_key_shortcuts => "aria-keyshortcuts";
        aria_label, set_aria_label => "aria-label";
        aria_level, set_aria_level => "aria-level";
        aria_live, set_aria_live => "aria-live";
        aria_modal, set_aria_modal => "aria-modal";
        aria_multi_line, set_aria_multi_line => "aria-multiline";
        aria_multi_selectable, set_aria_multi_selectable => "aria-multiselectable";
        aria_orientation, set_aria_orientation => "aria-orientation";
        aria_placeholder, set_aria_placeholder => "aria-placeholder";
        aria_pos_in_set, set_aria_pos_in_set => "aria-posinset";
        aria_pressed, set_aria_pressed => "aria-pressed";
        aria_read_only, set_aria_read_only => "aria-readonly";
        aria_required, set_aria_required => "aria-required";
        aria_role_description, set_aria_role_description => "aria-roledescription";
        aria_row_count, set_aria_row_count => "aria-rowcount";
        aria_row_index, set_aria_row_index => "aria-rowindex";
        aria_row_span, set_aria_row_span => "aria-rowspan";
        aria_selected, set_aria_selected => "aria-selected";
        aria_set_size, set_aria_set_size => "aria-setsize";
        aria_sort, set_aria_sort => "aria-sort";
        aria_value_max, set_aria_value_max => "aria-valuemax";
        aria_value_min, set_aria_value_min => "aria-valuemin";
        aria_value_now, set_aria_value_now => "aria-valuenow";
        aria_value_text, set_aria_value_text => "aria-valuetext";
    }

    fn get_html(&self) -> Result<String, JsError> {
        ElementOps::inner_html(self)
    }

    fn set_html_unsafe(&self, html: &str) -> Result<(), JsError> {
        ElementOps::set_inner_html(self, html);
        Ok(())
    }

    fn get_bounding_client_rect(&self) -> Result<Handle<crate::bindings::DOMRect>, JsError> {
        self.geo_bounding_client_rect()
    }
    fn get_client_rects(&self) -> Result<Handle<crate::bindings::DOMRectList>, JsError> {
        self.geo_client_rects()
    }
    fn client_top(&self) -> i32 {
        self.geo_client().top as i32
    }
    fn client_left(&self) -> i32 {
        self.geo_client().left as i32
    }
    fn client_width(&self) -> i32 {
        self.geo_client().width as i32
    }
    fn client_height(&self) -> i32 {
        self.geo_client().height as i32
    }
    fn scroll_top(&self) -> f64 {
        self.geo_scroll().top
    }
    fn set_scroll_top(&self, _value: f64) {}
    fn scroll_left(&self) -> f64 {
        self.geo_scroll().left
    }
    fn set_scroll_left(&self, _value: f64) {}
    fn scroll_width(&self) -> i32 {
        self.geo_scroll().width as i32
    }
    fn scroll_height(&self) -> i32 {
        self.geo_scroll().height as i32
    }

    fn remove(&self) {
        self.remove_from_parent();
    }

    fn before(&self, nodes: Vec<NodeOrDomString>) -> Result<(), JsError> {
        self.mutate_with_nodes(nodes, Document::before_nodes)
    }

    fn after(&self, nodes: Vec<NodeOrDomString>) -> Result<(), JsError> {
        self.mutate_with_nodes(nodes, Document::after_nodes)
    }

    fn replace_with(&self, nodes: Vec<NodeOrDomString>) -> Result<(), JsError> {
        self.mutate_with_nodes(nodes, Document::replace_with_nodes)
    }

    fn child_element_count(&self) -> u32 {
        let (doc, id) = self.anchor();
        doc.borrow().element_children(id).count() as u32
    }

    fn children(&self) -> Result<Handle<HTMLCollection>, JsError> {
        let (doc, id) = self.anchor();
        let owner = doc.borrow().node_ref(id);
        Ok(Handle::from_bits(encode_html_collection(
            self.slot(),
            owner,
        )))
    }

    fn class_list(&self) -> Result<Handle<DOMTokenList>, JsError> {
        let (doc, id) = self.anchor();
        let owner = doc.borrow().node_ref(id);
        Ok(Handle::from_bits(encode_token_list(self.slot(), owner)))
    }

    fn closest(&self, selectors: &str) -> Result<Option<Handle<Element>>, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        match doc.closest(id, selectors) {
            Ok(Some(result_id)) => Ok(Some(Handle::from_bits(encode_node(
                self.slot(),
                doc.node_ref(result_id),
            )))),
            Ok(None) => Ok(None),
            Err(e) => Err(JsError::syntax_error(e.to_string())),
        }
    }

    fn first_element_child(&self) -> Result<Option<Handle<Element>>, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        Ok(doc
            .element_children(id)
            .next()
            .map(|child| Handle::from_bits(encode_node(self.slot(), doc.node_ref(child)))))
    }

    fn get_elements_by_class_name(
        &self,
        class_names: &str,
    ) -> Result<Handle<HTMLCollection>, JsError> {
        let (doc, id) = self.anchor();
        // For Element, scope the query to the element's subtree. The split is
        // the DOM's *ASCII whitespace*, not Rust's Unicode
        // `split_whitespace`: U+00A0, U+1680, U+2000-U+200A and U+3000 are all
        // whitespace to Rust and none of them is whitespace to the DOM, so the
        // Unicode split would silently drop a class name that is one of those
        // characters (`dom/nodes/getElementsByClassName-whitespace-class-names.html`).
        let tokens: Vec<&str> = class_names.split_ascii_whitespace().collect();
        let doc_ref = doc.borrow();
        let descendants: Vec<NodeRef> = doc_ref
            .descendants(id)
            .filter(|node_id| {
                // All tokens must be present on the element; an empty class
                // list (an empty or whitespace-only argument) matches nothing,
                // per the DOM `getElementsByClassName` definition.
                !tokens.is_empty()
                    && tokens
                        .iter()
                        .all(|token| doc_ref.has_class(*node_id, token))
            })
            .map(|node_id| doc_ref.node_ref(node_id))
            .collect();
        drop(doc_ref);

        // Insert snapshot and return wrapped handle
        let ctx = self.context();
        let key = ctx.snapshots.borrow_mut().insert(descendants)?;
        Ok(Handle::from_bits(encode_snapshot(
            self.slot(),
            TAG_STATIC_HTML_COLLECTION,
            key,
        )))
    }

    fn get_elements_by_tag_name(
        &self,
        qualified_name: &str,
    ) -> Result<Handle<HTMLCollection>, JsError> {
        let (doc, id) = self.anchor();
        let doc_ref = doc.borrow();

        // For elements: if tag is "*", match all elements; otherwise match by tag name
        let descendants: Vec<NodeRef> = if qualified_name == "*" {
            doc_ref
                .descendants(id)
                .filter(|node_id| doc_ref.is_element(*node_id))
                .map(|node_id| doc_ref.node_ref(node_id))
                .collect()
        } else {
            // Get the atom for the tag name; if it doesn't exist, return empty collection
            use tenta_core::atom::Atom;
            let tag_atom = match Atom::get(&qualified_name.to_ascii_lowercase()) {
                Some(atom) => atom,
                None => {
                    drop(doc_ref);
                    let ctx = self.context();
                    let key = ctx.snapshots.borrow_mut().insert(vec![])?;
                    return Ok(Handle::from_bits(encode_snapshot(
                        self.slot(),
                        TAG_STATIC_HTML_COLLECTION,
                        key,
                    )));
                }
            };

            doc_ref
                .descendants(id)
                .filter(|node_id| doc_ref.tag(*node_id) == Some(tag_atom))
                .map(|node_id| doc_ref.node_ref(node_id))
                .collect()
        };
        drop(doc_ref);

        let ctx = self.context();
        let key = ctx.snapshots.borrow_mut().insert(descendants)?;
        Ok(Handle::from_bits(encode_snapshot(
            self.slot(),
            TAG_STATIC_HTML_COLLECTION,
            key,
        )))
    }

    fn get_elements_by_tag_name_ns(
        &self,
        namespace: Option<&str>,
        local_name: &str,
    ) -> Result<Handle<HTMLCollection>, JsError> {
        // `[NewObject]`, and a snapshot rather than the standard's live
        // collection: this tree's `getElementsByTagName` is the same shape, and
        // a live one needs a document-side observer the arena does not keep.
        // `Element` scopes the query to its subtree, excluding itself, which is
        // what `dom/nodes/Element-getElementsByTagNameNS.html` asserts of the
        // context object itself.
        let (doc, id) = self.anchor();
        let snapshot = {
            let doc = doc.borrow();
            qualified_snapshot(&doc, id, namespace, local_name, false)
        };
        Ok(Handle::from_bits(encode_snapshot(
            self.slot(),
            TAG_STATIC_HTML_COLLECTION,
            self.context().snapshots.borrow_mut().insert(snapshot)?,
        )))
    }

    fn id(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        Ok(doc.borrow().element_id(id).unwrap_or("").to_owned())
    }

    fn set_id(&self, value: &str) {
        // A plain reflected setter: writing `id` sets the `id` attribute. Fast-API
        // eligible, so it returns void and cannot throw (any parse the attribute
        // needs is infallible).
        self.reflect_attribute("id", value);
    }

    fn class_name(&self) -> Result<String, JsError> {
        // `className` reflects the `class` attribute value verbatim (whitespace
        // and duplicates preserved), or "" when the element has no class
        // attribute. The DOM keeps the raw attribute string and the parsed class
        // index in lockstep, so the raw string is authoritative.
        let (doc, id) = self.anchor();
        Ok(doc
            .borrow()
            .attribute_str(id, "class")
            .unwrap_or("")
            .to_owned())
    }

    fn set_class_name(&self, value: &str) {
        self.reflect_attribute("class", value);
    }

    fn local_name(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        Ok(doc
            .borrow()
            .tag(id)
            .map(|local| local.as_str().to_owned())
            .unwrap_or_default())
    }

    fn namespace_uri(&self) -> Result<Option<String>, JsError> {
        let (doc, id) = self.anchor();
        Ok(doc
            .borrow()
            .namespace(id)
            .map(|ns| ns.as_str().to_owned())
            .filter(|uri| !uri.is_empty()))
    }

    fn prefix(&self) -> Result<Option<String>, JsError> {
        // `Element.prefix` is the *qualified name's* prefix, which the tree
        // builder stores on the element itself (`Document::element_prefix`), or
        // null when the element name has none
        // (<https://dom.spec.whatwg.org/#dom-element-prefix>). It is the sibling
        // of the `localName` and `namespaceURI` getters above, all three read
        // off the same stored qualified name.
        let (doc, id) = self.anchor();
        Ok(doc
            .borrow()
            .element_prefix(id)
            .filter(|prefix| *prefix != atoms::EMPTY)
            .map(|prefix| prefix.as_str().to_owned()))
    }

    fn last_element_child(&self) -> Result<Option<Handle<Element>>, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        Ok(doc
            .element_children(id)
            .last()
            .map(|child| Handle::from_bits(encode_node(self.slot(), doc.node_ref(child)))))
    }

    fn previous_element_sibling(&self) -> Result<Option<Handle<Element>>, JsError> {
        Ok(self.element_sibling(false))
    }
    fn next_element_sibling(&self) -> Result<Option<Handle<Element>>, JsError> {
        Ok(self.element_sibling(true))
    }
    fn insert_adjacent_element(
        &self,
        r#where: &str,
        element: Handle<Element>,
    ) -> Result<Option<Handle<Element>>, JsError> {
        self.insert_adjacent_node(r#where, element)
    }
    fn insert_adjacent_text(&self, r#where: &str, data: &str) -> Result<(), JsError> {
        self.insert_adjacent_string(r#where, data)
    }
    fn webkit_matches_selector(&self, selectors: &str) -> Result<bool, JsError> {
        let (doc, id) = self.anchor();
        doc.borrow().matches(id, selectors).map_err(|_| {
            JsError::dom_exception(
                "SyntaxError",
                &format!("'{selectors}' is not a valid selector."),
            )
        })
    }
    // Nothing scrolls.
    fn scroll_into_view(&self, _arg: Option<bool>) -> Result<(), JsError> {
        Ok(())
    }
    fn scroll_to(&self, _x: Option<f64>, _y: Option<f64>) -> Result<(), JsError> {
        Ok(())
    }
    fn scroll(&self, _x: Option<f64>, _y: Option<f64>) -> Result<(), JsError> {
        Ok(())
    }
    fn scroll_by(&self, _x: Option<f64>, _y: Option<f64>) -> Result<(), JsError> {
        Ok(())
    }

    fn matches(&self, selectors: &str) -> bool {
        // Spec: `matches` throws a `SyntaxError` on an invalid selector, but the
        // generated fast-API signature is infallible (`-> bool`) — the generator
        // has no way to mark a fast-API member fallible. Until it does, an
        // invalid selector reports `false` rather than throwing. Valid selectors
        // match per spec. (Follow-up: let `tenta-idl` force a `[Throws]` member
        // off the fast path so it returns `Result`.)
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        doc.matches(id, selectors).unwrap_or(false)
    }

    fn query_selector(&self, selectors: &str) -> Result<Option<Handle<Element>>, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        match doc.query_selector(id, selectors) {
            Ok(Some(result_id)) => Ok(Some(Handle::from_bits(encode_node(
                self.slot(),
                doc.node_ref(result_id),
            )))),
            Ok(None) => Ok(None),
            Err(e) => Err(JsError::syntax_error(e.to_string())),
        }
    }

    fn query_selector_all(&self, selectors: &str) -> Result<Handle<NodeList>, JsError> {
        let (doc, id) = self.anchor();
        let doc_ref = doc.borrow();

        // Query against this element as the root scope
        match doc_ref.query_selector_all(id, selectors) {
            Ok(result_ids) => {
                let refs: Vec<NodeRef> = result_ids
                    .iter()
                    .map(|node_id| doc_ref.node_ref(*node_id))
                    .collect();
                drop(doc_ref);

                let ctx = self.context();
                let key = ctx.snapshots.borrow_mut().insert(refs)?;
                Ok(Handle::from_bits(encode_snapshot(
                    self.slot(),
                    TAG_STATIC_NODE_LIST,
                    key,
                )))
            }
            Err(e) => Err(JsError::syntax_error(e.to_string())),
        }
    }

    fn tag_name(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        Ok(element_tag_name(&doc.borrow(), id))
    }

    fn set_attribute(&self, name: &str, value: &str) -> Result<(), JsError> {
        validate_attribute_name(name)?;
        let (doc, id) = self.anchor();
        let qname = {
            let doc = doc.borrow();
            let name = attribute_qualified_name_for_lookup(&doc, id, name);
            attribute_by_qualified_name(&doc, id, &name)
                .map(|(stored, _)| stored)
                .unwrap_or_else(|| QualName::new(atoms::EMPTY, Atom::new(&name)))
        };
        self.context()
            .set_element_attribute(id, qname, value.to_owned())
    }

    fn get_attribute(&self, name: &str) -> Result<Option<String>, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let name = attribute_qualified_name_for_lookup(&doc, id, name);
        Ok(attribute_by_qualified_name(&doc, id, &name).map(|(_, value)| value.to_owned()))
    }

    fn remove_attribute(&self, name: &str) {
        let (doc, id) = self.anchor();
        let found = {
            let doc = doc.borrow();
            let name = attribute_qualified_name_for_lookup(&doc, id, name);
            attribute_by_qualified_name(&doc, id, &name).map(|(stored, _)| stored)
        };
        if let Some(name) = found {
            let _ = self.context().remove_element_attribute(id, name);
        }
    }

    fn has_attribute(&self, name: &str) -> bool {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let name = attribute_qualified_name_for_lookup(&doc, id, name);
        attribute_by_qualified_name(&doc, id, &name).is_some()
    }

    fn has_attributes(&self) -> bool {
        let (doc, id) = self.anchor();
        doc.borrow().attributes(id).next().is_some()
    }

    fn toggle_attribute(&self, name: &str, force: Option<bool>) -> Result<bool, JsError> {
        validate_attribute_name(name)?;
        let (doc, id) = self.anchor();
        let (name, found) = {
            let doc = doc.borrow();
            let name = attribute_qualified_name_for_lookup(&doc, id, name);
            let found = attribute_by_qualified_name(&doc, id, &name).map(|(stored, _)| stored);
            (name, found)
        };
        let present = found.is_some();

        // An omitted force toggles; otherwise its Boolean value chooses the
        // final presence. A newly added attribute has an empty-string value.
        let add = match force {
            Some(force) => force,
            None => !present,
        };
        if add && !present {
            self.context().set_element_attribute(
                id,
                QualName::new(atoms::EMPTY, Atom::new(&name)),
                String::new(),
            )?;
        } else if !add && let Some(name) = found {
            self.context().remove_element_attribute(id, name)?;
        }
        Ok(add)
    }

    fn get_attribute_names(&self) -> Result<Vec<String>, JsError> {
        let (doc, id) = self.anchor();
        Ok(doc
            .borrow()
            .attributes(id)
            .map(|(name, _)| qualified_attr_name(&name))
            .collect())
    }

    fn set_attribute_ns(
        &self,
        namespace: Option<&str>,
        qualified_name: &str,
        value: &str,
    ) -> Result<(), JsError> {
        // `setAttributeNS` validates and extracts exactly as `createElementNS`
        // does — step 1 of its algorithm *is* "let (namespace, prefix,
        // localName) be the result of validating and extracting namespace and
        // qualifiedName" (<https://dom.spec.whatwg.org/#dom-element-setattributens>)
        // — so the two share one implementation and cannot drift apart on the
        // prefix/namespace rules. Nothing is ASCII-lowercased: the lowercasing
        // is `setAttribute`'s own step, which is why
        // `dom/nodes/attributes.html` reads `setAttributeNS(ns, "foo:bar", ...)`
        // back with `localName` `"bar"` and `prefix` `"foo"`.
        let (ns, prefix, local) = validate_and_extract(namespace, qualified_name)?;
        let (_, id) = self.anchor();
        let qname = QualName {
            ns,
            prefix,
            local: Atom::new(local),
        };
        self.context()
            .set_element_attribute(id, qname, value.to_owned())
    }

    fn get_attribute_ns(
        &self,
        namespace: Option<&str>,
        local_name: &str,
    ) -> Result<Option<String>, JsError> {
        // The standard's whole *get an attribute by namespace and local name*
        // is the (namespace, localName) pair; the null namespace and the empty
        // string are one case, which is why both arrive here as `None` or as
        // `""` and both are treated as no namespace.
        let (doc, id) = self.anchor();
        Ok(lookup_attribute_ns(
            &doc.borrow(),
            id,
            namespace,
            local_name,
        ))
    }

    fn remove_attribute_ns(
        &self,
        namespace: Option<&str>,
        local_name: &str,
    ) -> Result<(), JsError> {
        let (doc, id) = self.anchor();
        let Some(name) = attribute_name_ns(&doc.borrow(), id, namespace, local_name) else {
            // Removing an attribute that is not there is a silent no-op, the
            // same as `removeAttribute`'s.
            return Ok(());
        };
        let _ = self.context().remove_element_attribute(id, name.to_qual());
        Ok(())
    }

    fn has_attribute_ns(&self, namespace: Option<&str>, local_name: &str) -> Result<bool, JsError> {
        let (doc, id) = self.anchor();
        Ok(lookup_attribute_ns(&doc.borrow(), id, namespace, local_name).is_some())
    }

    fn attributes(&self) -> Result<Handle<NamedNodeMap>, JsError> {
        // `[SameObject]`: the map's handle is derived from the element's own
        // `NodeRef`, so two reads answer with one object. The interface is only
        // reachable on a wrapper that carries `Element`'s prototype, which the
        // world mints for element nodes alone.
        let (doc, id) = self.anchor();
        let node = doc.borrow().node_ref(id);
        Ok(Handle::from_bits(encode_named_node_map(self.slot(), node)))
    }

    fn get_attribute_node(&self, name: &str) -> Result<Option<Handle<Attr>>, JsError> {
        // `getAttributeNode` identifies the attribute by *qualified name*, the
        // same key `getAttribute` uses; `None` when there is none.
        let ctx = self.context();
        let (doc, id) = self.anchor();
        let (node, found) = {
            let doc = doc.borrow();
            let lookup = attribute_qualified_name_for_lookup(&doc, id, name);
            let found = attribute_by_qualified_name(&doc, id, &lookup).map(|(stored, _)| stored);
            (doc.node_ref(id), found)
        };
        found
            .map(|name| mint_attr(ctx, Some(node), name).map(Handle::from_bits))
            .transpose()
    }

    fn get_attribute_node_ns(
        &self,
        namespace: Option<&str>,
        local_name: &str,
    ) -> Result<Option<Handle<Attr>>, JsError> {
        let ctx = self.context();
        let (doc, id) = self.anchor();
        let (node, found) = {
            let doc = doc.borrow();
            let found = attribute_name_ns(&doc, id, namespace, local_name).map(AttrName::to_qual);
            (doc.node_ref(id), found)
        };
        found
            .map(|name| mint_attr(ctx, Some(node), name).map(Handle::from_bits))
            .transpose()
    }

    fn set_attribute_node(&self, attr: Handle<Attr>) -> Result<Option<Handle<Attr>>, JsError> {
        self.attach_attribute_node(attr)
    }

    fn set_attribute_node_ns(&self, attr: Handle<Attr>) -> Result<Option<Handle<Attr>>, JsError> {
        self.attach_attribute_node(attr)
    }

    fn remove_attribute_node(&self, attr: Handle<Attr>) -> Result<Handle<Attr>, JsError> {
        // The argument has to be one of *this* element's attributes: the
        // standard's step 1 is "if this's attribute list does not contain
        // attribute, throw a `NotFoundError`". A detached `Attr`, or one of
        // another element's, is exactly that case.
        let ctx = self.context();
        let decoded = decode(attr.bits());
        if decoded.tag != TAG_HOST || decoded.slot != ctx.slot {
            return Err(attr_not_found());
        }
        let key = decoded.host_key();
        let (doc, id) = self.anchor();
        let name = {
            let records = ctx.host_records.borrow();
            let Some(HostRecord::Attr(record)) = records.get(key) else {
                return Err(attr_not_found());
            };
            if record.element != Some(doc.borrow().node_ref(id)) {
                return Err(attr_not_found());
            }
            record.name
        };
        if ctx
            .attr_index
            .borrow()
            .get(&attr_identity(doc.borrow().node_ref(id), name))
            .copied()
            != Some(key)
        {
            return Err(attr_not_found());
        }
        ctx.remove_element_attribute(id, name)?
            .ok_or_else(attr_not_found)?;
        // The standard's *remove an attribute* sets the attribute's element to
        // null and keeps its value, so the record — and therefore the wrapper
        // the page already holds — becomes the detached object it returns.
        // Identity is deliberate: `removeAttributeNode` returns the *same*
        // `Attr` it was given.
        Ok(attr)
    }

    fn inner_html(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        Ok(doc.borrow().inner_html(id))
    }

    fn set_inner_html(&self, value: &str) {
        // Note: This is a fast-API eligible operation, so it must return void.
        // Errors are silently ignored to match the fast-API constraint.
        // The native replace-all preflights hierarchy, live ranges and every
        // attached observer world before removing any existing child. A
        // later native failure remains subject to the Document poison path.
        let (doc, id) = self.anchor();
        let mut doc = doc.borrow_mut();
        let _ = doc.set_inner_html(id, value);
    }

    fn attach_shadow(
        &self,
        init: crate::bindings::ShadowRootInit,
    ) -> Result<Handle<crate::bindings::ShadowRoot>, JsError> {
        self.attach_shadow_root(init)
    }

    fn shadow_root(&self) -> Result<Option<Handle<crate::bindings::ShadowRoot>>, JsError> {
        Ok(self.open_shadow_root())
    }

    fn assigned_slot(&self) -> Result<Option<Handle<crate::bindings::HTMLSlotElement>>, JsError> {
        Ok(self.assigned_slot_handle())
    }

    fn slot(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("slot"))
    }

    fn set_slot(&self, value: &str) {
        self.reflect_attribute("slot", value);
    }

    fn outer_html(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        Ok(doc.borrow().outer_html(id))
    }

    fn set_outer_html(&self, value: &str) -> Result<(), JsError> {
        let (doc, id) = self.anchor();
        let mut doc = doc.borrow_mut();
        doc.set_outer_html(id, value)
            .map_err(|e| mutation_error(&e))
    }

    fn insert_adjacent_html(&self, position: &str, text: &str) -> Result<(), JsError> {
        let (doc, id) = self.anchor();
        let mut doc = doc.borrow_mut();
        doc.insert_adjacent_html(id, position, text)
            .map_err(|e| mutation_error(&e))
    }

    fn append(&self, nodes: Vec<NodeOrDomString>) -> Result<(), JsError> {
        self.mutate_with_nodes(nodes, Document::append_nodes)
    }

    fn prepend(&self, nodes: Vec<NodeOrDomString>) -> Result<(), JsError> {
        self.mutate_with_nodes(nodes, Document::prepend_nodes)
    }

    fn replace_children(&self, nodes: Vec<NodeOrDomString>) -> Result<(), JsError> {
        self.mutate_with_nodes(nodes, Document::replace_children)
    }
}

/// The getter and setter of each event handler IDL attribute of the
/// `GlobalEventHandlers` mixin (`idl/dom/dom.webidl`), for the `Ops` trait of
/// an interface that includes it. Every one is the same two lines over
/// [`HostObject::event_handler`] and [`HostObject::set_event_handler`].
macro_rules! event_handler_attributes {
    ($($getter:ident, $setter:ident => $name:literal;)*) => {
        $(
            fn $getter(&self) -> Result<Option<EventHandlerNonNull>, JsError> {
                self.event_handler(EventHandlerName::new($name))
            }
            fn $setter(&self, value: Option<EventHandlerNonNull>) -> Result<(), JsError> {
                self.set_event_handler(EventHandlerName::new($name), value)
            }
        )*
    };
}

/// The members of the `GlobalEventHandlers` mixin, in the mixin's order.
macro_rules! global_event_handlers {
    () => {
        event_handler_attributes! {
            onabort, set_onabort => "onabort";
            onauxclick, set_onauxclick => "onauxclick";
            onbeforeinput, set_onbeforeinput => "onbeforeinput";
            onbeforematch, set_onbeforematch => "onbeforematch";
            onbeforetoggle, set_onbeforetoggle => "onbeforetoggle";
            onblur, set_onblur => "onblur";
            oncancel, set_oncancel => "oncancel";
            oncanplay, set_oncanplay => "oncanplay";
            oncanplaythrough, set_oncanplaythrough => "oncanplaythrough";
            onchange, set_onchange => "onchange";
            onclick, set_onclick => "onclick";
            onclose, set_onclose => "onclose";
            oncommand, set_oncommand => "oncommand";
            oncontextlost, set_oncontextlost => "oncontextlost";
            oncontextmenu, set_oncontextmenu => "oncontextmenu";
            oncontextrestored, set_oncontextrestored => "oncontextrestored";
            oncopy, set_oncopy => "oncopy";
            oncuechange, set_oncuechange => "oncuechange";
            oncut, set_oncut => "oncut";
            ondblclick, set_ondblclick => "ondblclick";
            ondrag, set_ondrag => "ondrag";
            ondragend, set_ondragend => "ondragend";
            ondragenter, set_ondragenter => "ondragenter";
            ondragleave, set_ondragleave => "ondragleave";
            ondragover, set_ondragover => "ondragover";
            ondragstart, set_ondragstart => "ondragstart";
            ondrop, set_ondrop => "ondrop";
            ondurationchange, set_ondurationchange => "ondurationchange";
            onemptied, set_onemptied => "onemptied";
            onended, set_onended => "onended";
            onerror, set_onerror => "onerror";
            onfocus, set_onfocus => "onfocus";
            onformdata, set_onformdata => "onformdata";
            oninput, set_oninput => "oninput";
            oninvalid, set_oninvalid => "oninvalid";
            onkeydown, set_onkeydown => "onkeydown";
            onkeypress, set_onkeypress => "onkeypress";
            onkeyup, set_onkeyup => "onkeyup";
            onload, set_onload => "onload";
            onloadeddata, set_onloadeddata => "onloadeddata";
            onloadedmetadata, set_onloadedmetadata => "onloadedmetadata";
            onloadstart, set_onloadstart => "onloadstart";
            onmousedown, set_onmousedown => "onmousedown";
            onmouseenter, set_onmouseenter => "onmouseenter";
            onmouseleave, set_onmouseleave => "onmouseleave";
            onmousemove, set_onmousemove => "onmousemove";
            onmouseout, set_onmouseout => "onmouseout";
            onmouseover, set_onmouseover => "onmouseover";
            onmouseup, set_onmouseup => "onmouseup";
            onpaste, set_onpaste => "onpaste";
            onpause, set_onpause => "onpause";
            onplay, set_onplay => "onplay";
            onplaying, set_onplaying => "onplaying";
            onprogress, set_onprogress => "onprogress";
            onratechange, set_onratechange => "onratechange";
            onreset, set_onreset => "onreset";
            onresize, set_onresize => "onresize";
            onscroll, set_onscroll => "onscroll";
            onscrollend, set_onscrollend => "onscrollend";
            onsecuritypolicyviolation, set_onsecuritypolicyviolation => "onsecuritypolicyviolation";
            onseeked, set_onseeked => "onseeked";
            onseeking, set_onseeking => "onseeking";
            onselect, set_onselect => "onselect";
            onslotchange, set_onslotchange => "onslotchange";
            onstalled, set_onstalled => "onstalled";
            onsubmit, set_onsubmit => "onsubmit";
            onsuspend, set_onsuspend => "onsuspend";
            ontimeupdate, set_ontimeupdate => "ontimeupdate";
            ontoggle, set_ontoggle => "ontoggle";
            onvolumechange, set_onvolumechange => "onvolumechange";
            onwaiting, set_onwaiting => "onwaiting";
            onwebkitanimationend, set_onwebkitanimationend => "onwebkitanimationend";
            onwebkitanimationiteration, set_onwebkitanimationiteration => "onwebkitanimationiteration";
            onwebkitanimationstart, set_onwebkitanimationstart => "onwebkitanimationstart";
            onwebkittransitionend, set_onwebkittransitionend => "onwebkittransitionend";
            onwheel, set_onwheel => "onwheel";
            onanimationstart, set_onanimationstart => "onanimationstart";
            onanimationiteration, set_onanimationiteration => "onanimationiteration";
            onanimationend, set_onanimationend => "onanimationend";
            onanimationcancel, set_onanimationcancel => "onanimationcancel";
            ontransitionrun, set_ontransitionrun => "ontransitionrun";
            ontransitionstart, set_ontransitionstart => "ontransitionstart";
            ontransitionend, set_ontransitionend => "ontransitionend";
            ontransitioncancel, set_ontransitioncancel => "ontransitioncancel";
            ongotpointercapture, set_ongotpointercapture => "ongotpointercapture";
            onlostpointercapture, set_onlostpointercapture => "onlostpointercapture";
            onpointerdown, set_onpointerdown => "onpointerdown";
            onpointermove, set_onpointermove => "onpointermove";
            onpointerup, set_onpointerup => "onpointerup";
            onpointercancel, set_onpointercancel => "onpointercancel";
            onpointerover, set_onpointerover => "onpointerover";
            onpointerout, set_onpointerout => "onpointerout";
            onpointerenter, set_onpointerenter => "onpointerenter";
            onpointerleave, set_onpointerleave => "onpointerleave";
        }
    };
}

impl DocumentOps for HostObject {
    fn dir(&self) -> Result<String, JsError> {
        let (doc, _) = self.anchor();
        let doc = doc.borrow();
        let given = doc
            .document_element()
            .and_then(|root| doc.attribute_str(root, "dir"))
            .unwrap_or("")
            .to_ascii_lowercase();
        Ok(match given.as_str() {
            "ltr" | "rtl" | "auto" => given,
            _ => String::new(),
        })
    }

    fn set_dir(&self, value: &str) {
        let root = {
            let (doc, _) = self.anchor();
            doc.borrow().document_element()
        };
        if let Some(root) = root {
            let qname = QualName::new(atoms::EMPTY, Atom::new("dir"));
            let _ = self
                .context()
                .set_element_attribute(root, qname, value.to_owned());
        }
    }

    fn design_mode(&self) -> Result<String, JsError> {
        Ok(if self.context().design_mode.get() {
            "on".to_owned()
        } else {
            "off".to_owned()
        })
    }

    fn set_design_mode(&self, value: &str) {
        // Only the two keywords change it; nothing becomes editable.
        if value.eq_ignore_ascii_case("on") {
            self.context().design_mode.set(true);
        } else if value.eq_ignore_ascii_case("off") {
            self.context().design_mode.set(false);
        }
    }

    fn exec_command(
        &self,
        _command_id: &str,
        _show_ui: Option<bool>,
        _value: Option<&str>,
    ) -> Result<bool, JsError> {
        // No command is supported, which is the answer a page tests for.
        Ok(false)
    }

    fn query_command_supported(&self, _command_id: &str) -> bool {
        false
    }

    fn query_command_enabled(&self, _command_id: &str) -> bool {
        false
    }

    fn last_modified(&self) -> Result<String, JsError> {
        Ok(self.context().last_modified())
    }

    fn fullscreen_element(&self) -> Result<Option<Handle<Element>>, JsError> {
        Ok(None)
    }

    fn fullscreen_enabled(&self) -> bool {
        false
    }

    fn picture_in_picture_enabled(&self) -> bool {
        false
    }

    fn pointer_lock_element(&self) -> Result<Option<Handle<Element>>, JsError> {
        Ok(None)
    }

    global_event_handlers!();

    fn visibility_state(&self) -> Result<String, JsError> {
        Ok("visible".to_owned())
    }
    fn hidden(&self) -> bool {
        false
    }
    fn forms(&self) -> Result<Handle<HTMLCollection>, JsError> {
        self.document_elements(|doc, node| doc.tag(node) == Some(atoms::FORM))
    }
    fn images(&self) -> Result<Handle<HTMLCollection>, JsError> {
        self.document_elements(|doc, node| doc.tag(node) == Some(atoms::IMG))
    }
    fn links(&self) -> Result<Handle<HTMLCollection>, JsError> {
        self.document_elements(tree_extras::is_link)
    }
    fn scripts(&self) -> Result<Handle<HTMLCollection>, JsError> {
        self.document_elements(|doc, node| doc.tag(node) == Some(atoms::SCRIPT))
    }
    fn embeds(&self) -> Result<Handle<HTMLCollection>, JsError> {
        self.document_elements(|doc, node| doc.tag(node) == Some(atoms::EMBED))
    }
    fn anchors(&self) -> Result<Handle<HTMLCollection>, JsError> {
        self.document_elements(|doc, node| {
            doc.tag(node) == Some(atoms::A) && doc.attribute_str(node, "name").is_some()
        })
    }
    fn get_elements_by_name(&self, element_name: &str) -> Result<Handle<NodeList>, JsError> {
        self.elements_named(element_name)
    }
    fn import_node(&self, node: Handle<Node>, deep: Option<bool>) -> Result<Handle<Node>, JsError> {
        self.import(node, deep.unwrap_or(false))
    }
    fn adopt_node(&self, node: Handle<Node>) -> Result<Handle<Node>, JsError> {
        self.adopt(node)
    }

    fn current_script(&self) -> Result<Option<Handle<Element>>, JsError> {
        let (doc, _) = self.anchor();
        let doc = doc.borrow();
        Ok(doc
            .current_script()
            .map(|script| Handle::from_bits(encode_node(self.slot(), doc.node_ref(script)))))
    }

    fn element_from_point(&self, x: f64, y: f64) -> Result<Option<Handle<Element>>, JsError> {
        Ok(self.geo_element_from_point(x, y))
    }
    fn elements_from_point(&self, x: f64, y: f64) -> Result<Vec<Handle<Element>>, JsError> {
        Ok(self.geo_element_from_point(x, y).into_iter().collect())
    }
    fn scrolling_element(&self) -> Result<Option<Handle<Element>>, JsError> {
        Ok(self.geo_scrolling_element())
    }
    fn style_sheets(&self) -> Result<Handle<crate::bindings::StyleSheetList>, JsError> {
        self.document_style_sheets()
    }
    fn adopted_style_sheets(&self) -> Result<Vec<Handle<crate::bindings::CSSStyleSheet>>, JsError> {
        HostObject::adopted_style_sheets(self)
    }
    fn set_adopted_style_sheets(
        &self,
        value: Vec<Handle<crate::bindings::CSSStyleSheet>>,
    ) -> Result<(), JsError> {
        HostObject::set_adopted_style_sheets(self, value)
    }

    fn child_element_count(&self) -> u32 {
        let (doc, id) = self.anchor();
        doc.borrow().element_children(id).count() as u32
    }

    fn children(&self) -> Result<Handle<HTMLCollection>, JsError> {
        let (doc, id) = self.anchor();
        let owner = doc.borrow().node_ref(id);
        Ok(Handle::from_bits(encode_html_collection(
            self.slot(),
            owner,
        )))
    }

    fn create_comment(&self, data: &str) -> Result<Handle<Comment>, JsError> {
        let (doc, _) = self.anchor();
        let mut doc = doc.borrow_mut();
        let id = doc.create_comment(data);
        Ok(Handle::from_bits(encode_node(
            self.slot(),
            doc.node_ref(id),
        )))
    }

    fn create_attribute(&self, local_name: &str) -> Result<Handle<Attr>, JsError> {
        // `createAttribute(localName)` mints a *detached* `Attr` in no
        // namespace with no prefix, lowercasing the name in an HTML document —
        // step 2 of its algorithm, the same conditional step `createElement`
        // has (<https://dom.spec.whatwg.org/#dom-document-createattribute>).
        validate_attribute_name(local_name)?;
        let (doc, _) = self.anchor();
        let local = if doc.borrow().is_html_document() {
            local_name.to_ascii_lowercase()
        } else {
            local_name.to_owned()
        };
        let name = QualName::new(atoms::EMPTY, Atom::new(&local));
        Ok(Handle::from_bits(mint_attr(self.context(), None, name)?))
    }

    fn create_attribute_ns(
        &self,
        namespace: Option<&str>,
        qualified_name: &str,
    ) -> Result<Handle<Attr>, JsError> {
        // The same *validate and extract* as `createElementNS`, and no
        // lowercasing: the namespace-aware factory takes the qualified name as
        // it is given.
        let (ns, prefix, local) = validate_and_extract(namespace, qualified_name)?;
        let name = QualName {
            ns,
            prefix,
            local: Atom::new(local),
        };
        Ok(Handle::from_bits(mint_attr(self.context(), None, name)?))
    }

    fn create_event(&self, interface: &str) -> Result<Handle<Event>, JsError> {
        // Legacy `createEvent` interface aliases (https://dom.spec.whatwg.org/#dom-document-createevent).
        // The Event/CustomEvent families arrived in 3a/3c; UIEvent/MouseEvent
        // in 3d. Each returns a record whose `kind` picks the wrapper prototype.
        //
        // The table is closed and short: the standard lists no `progressevent`,
        // so ProgressEvent is reachable only through its constructor, and
        // `createEvent('ProgressEvent')` throws here as it does in a browser.
        let kind = match interface.to_ascii_lowercase().as_str() {
            "event" | "events" | "htmlevents" | "svgevents" => EventKind::Plain,
            "customevent" => EventKind::Custom { detail: None },
            "uievent" | "uievents" => EventKind::Ui {
                view: None,
                detail: 0,
                which: 0,
            },
            "compositionevent" => EventKind::Composition {
                view: None,
                detail: 0,
                which: 0,
                data: String::new(),
            },
            "focusevent" => EventKind::Focus {
                view: None,
                detail: 0,
                which: 0,
                related_target: None,
                related_value: None,
            },
            "keyboardevent" => EventKind::Keyboard {
                view: None,
                detail: 0,
                which: 0,
                key: String::new(),
                code: String::new(),
                location: 0,
                modifiers: 0,
                repeat: false,
                is_composing: false,
                char_code: 0,
                key_code: 0,
            },
            "mouseevent" | "mouseevents" => EventKind::Mouse {
                view: None,
                detail: 0,
                which: 0,
                screen_x: 0,
                screen_y: 0,
                client_x: 0,
                client_y: 0,
                modifiers: 0,
                button: 0,
                buttons: 0,
                related_target: None,
                related_value: None,
                wheel: None,
            },
            _ => {
                return Err(JsError::dom_exception(
                    "NotSupportedError",
                    "createEvent does not support this interface yet",
                ));
            }
        };
        let ctx = self.context();
        let record = EventRecord::new(String::new(), kind, ctx.time_stamp());
        let key = ctx.events.borrow_mut().insert(record);
        Ok(Handle::from_bits(encode_event(self.slot(), key)))
    }

    fn create_element(&self, local_name: &str) -> Result<Handle<Element>, JsError> {
        // `createElement`'s name is its own local name, so the whole string is
        // the one whose first character has to match `NameStartChar`. (An XML
        // document's own stricter production is checked again by the arena; an
        // HTML document's is not, which is why this boundary rule is the one
        // `dom/nodes/Document-createElement.html` measures.)
        validate_name(local_name, local_name)?;
        let (doc, _) = self.anchor();
        let mut doc = doc.borrow_mut();
        // "If this is an HTML document, set localName to localName in ASCII
        // lowercase" — the lowercasing is conditional on the document, so an
        // XML document a `DOMImplementation` minted keeps the case it was
        // given (`dom/nodes/DOMImplementation-createDocument.html` reads
        // `doc.createElement("DIV").localName` back as `"DIV"`). The namespace
        // is the HTML namespace either way, which is what the same step says.
        let atom = if doc.is_html_document() {
            Atom::new(&local_name.to_ascii_lowercase())
        } else {
            Atom::new(local_name)
        };
        // A `<template>` owns a separate contents fragment; the create path must
        // request it so a script-created template matches a parsed one and its
        // `.content` resolves.
        let flags = ElementFlags {
            template: atom == atoms::TEMPLATE,
            ..ElementFlags::default()
        };
        let id = doc
            .create_element(QualName::new(atoms::NS_HTML, atom), Vec::new(), flags)
            .map_err(|e| mutation_error(&e))?;
        Ok(Handle::from_bits(encode_node(
            self.slot(),
            doc.node_ref(id),
        )))
    }

    fn create_document_fragment(&self) -> Result<Handle<DocumentFragment>, JsError> {
        let (doc, _) = self.anchor();
        let mut doc = doc.borrow_mut();
        let id = doc.create_fragment();
        Ok(Handle::from_bits(encode_node(
            self.slot(),
            doc.node_ref(id),
        )))
    }

    fn create_element_ns(
        &self,
        namespace: Option<&str>,
        qualified_name: &str,
    ) -> Result<Handle<Element>, JsError> {
        let (ns, prefix, local) = validate_and_extract(namespace, qualified_name)?;
        let (doc, _) = self.anchor();
        let mut doc = doc.borrow_mut();
        let name = QualName {
            ns,
            prefix,
            local: Atom::new(local),
        };
        let flags = ElementFlags {
            template: ns == atoms::NS_HTML && local == atoms::TEMPLATE.as_str(),
            ..ElementFlags::default()
        };
        let id = doc
            .create_element(name, Vec::new(), flags)
            .map_err(|e| mutation_error(&e))?;
        Ok(Handle::from_bits(encode_node(
            self.slot(),
            doc.node_ref(id),
        )))
    }

    fn create_text_node(&self, data: &str) -> Result<Handle<Text>, JsError> {
        let (doc, _) = self.anchor();
        let mut doc = doc.borrow_mut();
        let id = doc.create_text(data);
        Ok(Handle::from_bits(encode_node(
            self.slot(),
            doc.node_ref(id),
        )))
    }

    fn document_element(&self) -> Result<Option<Handle<Element>>, JsError> {
        let (doc, _) = self.anchor();
        let doc = doc.borrow();
        Ok(doc
            .document_element()
            .map(|id| Handle::from_bits(encode_node(self.slot(), doc.node_ref(id)))))
    }

    fn doctype(&self) -> Result<Option<Handle<DocumentType>>, JsError> {
        // The document's *first child* doctype, not any descendant: the DOM
        // Standard's `doctype` is "the child of this that is a doctype", so a
        // doctype nested anywhere else is invisible. The arena already refuses
        // to insert one anywhere but the document
        // (`Document::validate_insert`), so this is the tree-order read.
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let doctype = doc
            .children(id)
            .find(|child| doc.kind(*child) == NodeKind::DocType);
        Ok(doctype.map(|id| Handle::from_bits(encode_node(self.slot(), doc.node_ref(id)))))
    }

    fn implementation(&self) -> Result<Handle<DOMImplementation>, JsError> {
        let ctx = self.context();
        Ok(Handle::from_bits(encode_host(
            ctx.slot,
            implementation_key(ctx),
        )))
    }

    fn default_view(&self) -> Result<Option<Handle<WindowProxy>>, JsError> {
        // The window of the browsing context this document is the active
        // document of, or null when it is not any context's active document.
        //
        // Slot 0 is exactly that distinction in this tree: the wrapper world is
        // built over the one document the engine navigated, so the page's
        // document is the active document of the world's browsing context and
        // every document a `DOMImplementation` minted is a detached, never-
        // navigated document with no context at all. When a second browsing
        // context (an `<iframe>`, a Worker's owner) arrives, it will have to
        // arrive with its own world and this test will have to become "is this
        // context's document the one a window was installed over".
        let ctx = self.context();
        Ok((ctx.slot == 0).then(|| Handle::from_bits(encode_window())))
    }

    fn charset(&self) -> Result<String, JsError> {
        Ok(self.context().doc.borrow().encoding().to_owned())
    }

    fn input_encoding(&self) -> Result<String, JsError> {
        Ok(self.context().doc.borrow().encoding().to_owned())
    }

    fn content_type(&self) -> Result<String, JsError> {
        Ok(self.context().doc.borrow().content_type().to_owned())
    }

    fn create_processing_instruction(
        &self,
        target: &str,
        data: &str,
    ) -> Result<Handle<ProcessingInstruction>, JsError> {
        // The two refusals are the DOM Standard's, and neither is a stubbing:
        // both rules are enforced here because the arena's infallible factory
        // is also the parsers' entry, and the parsers have already applied
        // them. See `tenta_dom::validate_processing_instruction_target` for
        // why the target rule is XML's `Name` and not the DOM's own regex.
        tenta_dom::validate_processing_instruction_target(target)
            .map_err(|error| invalid_character_error(&error))?;
        tenta_dom::validate_processing_instruction_data(data)
            .map_err(|error| invalid_character_error(&error))?;
        let (doc, _) = self.anchor();
        let mut doc = doc.borrow_mut();
        let id = doc.create_processing_instruction(target, data);
        Ok(Handle::from_bits(encode_node(
            self.slot(),
            doc.node_ref(id),
        )))
    }

    fn create_cdata_section(&self, data: &str) -> Result<Handle<CDATASection>, JsError> {
        // Both refusals are the DOM Standard's
        // (<https://dom.spec.whatwg.org/#dom-document-createcdatasection>): an
        // HTML document has no CDATA section to give, and `]]>` cannot appear
        // inside one. The second is the same `InvalidCharacterError` the two
        // parsers raise for the same input, because this factory is theirs too.
        if self.anchor().0.borrow().is_html_document() {
            return Err(JsError::dom_exception(
                "NotSupportedError",
                "an HTML document has no CDATA sections",
            ));
        }
        if data.contains("]]>") {
            return Err(JsError::dom_exception(
                "InvalidCharacterError",
                "the data contains the CDATA section terminator ]]>",
            ));
        }
        let (doc, _) = self.anchor();
        let mut doc = doc.borrow_mut();
        let id = doc.create_cdata(data);
        Ok(Handle::from_bits(encode_node(
            self.slot(),
            doc.node_ref(id),
        )))
    }

    fn create_range(&self) -> Result<Handle<Range>, JsError> {
        // A new range's start and end are both `(this document, 0)`, so it comes
        // out collapsed — which is what `Range-constructor.html` and
        // `Range-detach.html` read before they move it.
        let (doc, _) = self.anchor();
        let point = {
            let doc = doc.borrow();
            RangePoint {
                node: encode_node(self.slot(), doc.node_ref(doc.document_node())),
                offset: 0,
            }
        };
        Ok(Handle::from_bits(mint_range(self.context(), point, point)?))
    }

    fn append(&self, nodes: Vec<NodeOrDomString>) -> Result<(), JsError> {
        self.mutate_with_nodes(nodes, Document::append_nodes)
    }

    fn prepend(&self, nodes: Vec<NodeOrDomString>) -> Result<(), JsError> {
        self.mutate_with_nodes(nodes, Document::prepend_nodes)
    }

    fn replace_children(&self, nodes: Vec<NodeOrDomString>) -> Result<(), JsError> {
        self.mutate_with_nodes(nodes, Document::replace_children)
    }

    fn body(&self) -> Result<Option<Handle<HTMLElement>>, JsError> {
        // Only an HTML-namespace body/frameset is `document.body`, so the
        // `HTMLElement` handle is always backed by an HTML element (the wrapper
        // interface is picked by namespace at wrap time).
        let (doc, _) = self.anchor();
        let doc = doc.borrow();
        Ok(doc
            .body()
            .map(|id| Handle::from_bits(encode_node(self.slot(), doc.node_ref(id)))))
    }

    fn head(&self) -> Result<Option<Handle<HTMLElement>>, JsError> {
        let (doc, _) = self.anchor();
        let doc = doc.borrow();
        Ok(doc
            .head()
            .filter(|id| doc.namespace(*id) == Some(atoms::NS_HTML))
            .map(|id| Handle::from_bits(encode_node(self.slot(), doc.node_ref(id)))))
    }

    fn ready_state(&self) -> Result<String, JsError> {
        // No incremental load/parse model yet: a document handed to the engine is
        // already fully built, so it reports "complete" (the settle machine that
        // drives "loading"/"interactive" arrives with the page pipeline).
        Ok("complete".to_owned())
    }

    fn active_element(&self) -> Result<Option<Handle<Element>>, JsError> {
        // The focused element, or the fallback the standard names: the body, or
        // the document element when there is no body.
        let (doc, _) = self.anchor();
        let doc = doc.borrow();
        let active = doc
            .focused_element()
            .or_else(|| doc.body())
            .or_else(|| doc.document_element());
        Ok(active.map(|id| Handle::from_bits(encode_node(self.slot(), doc.node_ref(id)))))
    }

    fn has_focus(&self) -> bool {
        // The only document that can hold system focus is the page's own; a
        // document a `DOMImplementation` minted has no browsing context.
        self.slot() == 0
    }

    fn title(&self) -> Result<String, JsError> {
        // Not a reflected attribute. The HTML Standard's getter
        // (<https://html.spec.whatwg.org/multipage/dom.html#dom-document-title>)
        // reads the *title element* — the first `title` element in the document
        // in tree order, wherever it sits, not necessarily in the head — and
        // returns its child text content with ASCII whitespace stripped and
        // collapsed. No title element reads as the empty string. The
        // SVG-rooted-document branch of the algorithm is unreachable here:
        // TentaBrowser builds HTML documents only.
        let (doc, _) = self.anchor();
        let doc = doc.borrow();
        let Some(title) = document_title_element(&doc) else {
            return Ok(String::new());
        };
        let mut value = String::new();
        for child in doc.children(title) {
            if matches!(doc.kind(child), NodeKind::Text | NodeKind::CData)
                && let Some(data) = doc.text_data(child)
            {
                value.push_str(data);
            }
        }
        Ok(strip_and_collapse_ascii_whitespace(&value))
    }

    fn set_title(&self, value: &str) {
        // The HTML-namespace branch of the setter
        // (<https://html.spec.whatwg.org/multipage/dom.html#dom-document-title>):
        // a write with neither a title element nor a head element is ignored;
        // otherwise it targets the existing title element, or a freshly created
        // `title` appended to the head. The new value is written by "string
        // replace all", so an empty value leaves the element with *no* children
        // rather than an empty text node.
        let (doc, _) = self.anchor();
        let mut doc = doc.borrow_mut();
        let Some(root) = doc.document_element() else {
            return;
        };
        // Any other document element (the SVG `svg` branch, or "otherwise: do
        // nothing") is out of reach for an HTML document.
        if doc.namespace(root) != Some(atoms::NS_HTML) {
            return;
        }
        let title = match document_title_element(&doc) {
            Some(title) => title,
            None => {
                let Some(head) = doc.head() else {
                    return;
                };
                let created = doc.create_html_element(atoms::TITLE);
                if doc
                    .mutate(Mutation::Append {
                        parent: head,
                        child: created,
                    })
                    .is_err()
                {
                    return;
                }
                created
            }
        };
        let replacement = if value.is_empty() {
            ReplaceAllWith::Empty
        } else {
            ReplaceAllWith::Detached(doc.create_text(value))
        };
        let _ = doc.mutate(Mutation::ReplaceAll {
            parent: title,
            replacement,
        });
    }

    fn url(&self) -> Result<String, JsError> {
        // https://dom.spec.whatwg.org/#dom-document-url: the document's URL,
        // serialized. The engine keeps it as an `Option`, so a document built
        // without one reports the empty string rather than the standard's
        // default `about:blank`, which the tree does not model.
        let (doc, _) = self.anchor();
        let doc = doc.borrow();
        Ok(doc.url().map(ToString::to_string).unwrap_or_default())
    }

    fn domain(&self) -> Result<String, JsError> {
        // https://html.spec.whatwg.org/#dom-document-domain: the host of the
        // document's origin, serialized, or the empty string when the origin
        // is opaque, which is what a document without a URL has here.
        let (doc, _) = self.anchor();
        let doc = doc.borrow();
        Ok(doc
            .url()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_default())
    }

    fn set_domain(&self, value: &str) -> Result<(), JsError> {
        if value == DocumentOps::domain(self)? && !value.is_empty() {
            return Ok(());
        }
        Err(JsError::dom_exception(
            "SecurityError",
            "Failed to set the 'domain' property on 'Document': relaxing the same-origin policy is not supported.",
        ))
    }

    fn document_uri(&self) -> Result<String, JsError> {
        // The same getter as `URL`
        // (https://dom.spec.whatwg.org/#dom-document-documenturi).
        DocumentOps::url(self)
    }

    fn character_set(&self) -> Result<String, JsError> {
        // https://dom.spec.whatwg.org/#dom-document-characterset: the encoding's
        // name. The encoding is stored on `tenta_dom::Document`, defaulted to
        // "UTF-8" and set from the sniffer by the page pipeline once the parse
        // finishes (`tenta-page`'s `page.rs`).
        let (doc, _) = self.anchor();
        Ok(doc.borrow().encoding().to_owned())
    }

    fn compat_mode(&self) -> Result<String, JsError> {
        // https://dom.spec.whatwg.org/#dom-document-compatmode: "BackCompat" in
        // quirks mode, "CSS1Compat" otherwise; limited-quirks is *not* quirks
        // mode. The tree really does record the mode — the HTML parser sets it
        // from the doctype — so a page with no doctype reports "BackCompat".
        let (doc, _) = self.anchor();
        Ok(match doc.borrow().quirks_mode() {
            QuirksMode::Quirks => "BackCompat",
            QuirksMode::NoQuirks | QuirksMode::LimitedQuirks => "CSS1Compat",
        }
        .to_owned())
    }

    fn referrer(&self) -> Result<String, JsError> {
        // The engine records no referrer for a navigation. `PageOptions::referer`
        // only feeds the outgoing `Referer` header on a *request* and is never
        // stored on the `Document`, which has no referrer accessor at all. The
        // HTML Standard's `document.referrer` is the URL of the document the
        // user navigated from, so the empty string is what a direct navigation
        // must report — but it is also what this returns for every navigation,
        // which cannot be distinguished from a wired one until the navigation
        // side records the referrer on the document.
        Ok(String::new())
    }

    fn cookie(&self) -> Result<String, JsError> {
        // https://html.spec.whatwg.org/multipage/dom.html#dom-document-cookie:
        // the getter is RFC 6265bis's "cookie-string" for the document's URL,
        // computed with the non-HTTP API flag set. `cookie_header` is exactly
        // that — `cookies_for` (RFC 6265 §5.4 order: longer path first, then
        // earlier creation time) joined as `name=value` with `"; "`. Nothing of
        // the join is re-implemented here.
        let ctx = self.context();
        let doc = ctx.doc.borrow();
        // A cookie-averse `Document` returns the empty string; see
        // [`is_cookie_averse_scheme`] for the rule.
        let Some(url) = doc.url() else {
            return Ok(String::new());
        };
        if is_cookie_averse_scheme(url) {
            return Ok(String::new());
        }
        // `is_http = false` is the security boundary of this member: it makes
        // `cookies_for`'s `is_http || !cookie.http_only` filter hide every
        // `HttpOnly` cookie from script. Never pass `true` here.
        //
        // The request is trivially same-site, because the site for cookies *is*
        // the document's own site: `SameSiteContext::same_site()` is the value
        // the standard's computation yields for a non-HTTP API acting on the
        // document's URL. Passing `Site::from_url(url)` as the first party also
        // selects the document's own partition (`cookies_for`'s partition
        // filter), which is right — the document is the top-level site.
        Ok(ctx
            .cookies
            .cookie_header(
                url,
                Site::from_url(url).as_ref(),
                false,
                SameSiteContext::same_site(),
                SystemTime::now(),
            )
            .unwrap_or_default())
    }

    fn set_cookie(&self, value: &str) {
        // https://html.spec.whatwg.org/multipage/dom.html#dom-document-cookie:
        // the setter is RFC 6265bis's "receive a set-cookie-string" with the
        // non-HTTP API flag, which is exactly `set_cookie`'s contract — the
        // whole assignment is delegated, including its parser
        // (`parse_set_cookie_in`, which splits at the first `;` into the
        // name-value pair and the attribute list). `document.cookie =
        // 'a=b; Path=/sub'` therefore stores `a` with path `/sub`; a naive
        // split at the first `=` and take-the-rest-as-value would not.
        //
        // Attribute syntax (`Path`, `Domain`, `Max-Age`, ...) *is* honoured
        // from script. What script cannot do is create or overwrite an
        // `HttpOnly` cookie: `is_http = false` makes `CookieJar::store` return
        // `RejectedHttpOnly` (RFC 6265bis §5.7 steps 15 and 21).
        let ctx = self.context();
        let url = {
            let doc = ctx.doc.borrow();
            doc.url().cloned()
        };
        // A cookie-averse `Document` stores nothing — the standard's setter is a
        // silent no-op there, and there is no jar key to compute without a URL.
        let Some(url) = url else { return };
        if is_cookie_averse_scheme(&url) {
            return;
        }
        // The outcome is deliberately discarded: every refusal path of the
        // storage algorithm — no `=`, a `__Secure-`/`__Host-` prefix violation,
        // `SameSite=None` without `Secure`, `RejectedHttpOnly` — is silent in
        // the standard, so surfacing it as a `JsError` would invent an error the
        // page cannot get from a real browser.
        let _ = ctx
            .cookies
            .set_cookie(value.as_bytes(), &url, false, SystemTime::now());
    }

    fn first_element_child(&self) -> Result<Option<Handle<Element>>, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        Ok(doc
            .element_children(id)
            .next()
            .map(|child| Handle::from_bits(encode_node(self.slot(), doc.node_ref(child)))))
    }

    fn get_element_by_id(&self, element_id: &str) -> Result<Option<Handle<Element>>, JsError> {
        let (doc, _) = self.anchor();
        let doc = doc.borrow();
        Ok(doc
            .get_element_by_id(element_id)
            .map(|id| Handle::from_bits(encode_node(self.slot(), doc.node_ref(id)))))
    }

    fn get_elements_by_class_name(
        &self,
        class_names: &str,
    ) -> Result<Handle<HTMLCollection>, JsError> {
        let (doc, _) = self.anchor();
        // ASCII whitespace only; see the `Element` version for why the Unicode
        // split is wrong.
        let tokens: Vec<&str> = class_names.split_ascii_whitespace().collect();
        let doc_ref = doc.borrow();
        let doc_node = doc_ref.document_node();
        let descendants: Vec<NodeRef> = doc_ref
            .descendants(doc_node)
            .filter(|node_id| {
                // All tokens must be present on the element; an empty class
                // list (an empty or whitespace-only argument) matches nothing,
                // per the DOM `getElementsByClassName` definition.
                !tokens.is_empty()
                    && tokens
                        .iter()
                        .all(|token| doc_ref.has_class(*node_id, token))
            })
            .map(|node_id| doc_ref.node_ref(node_id))
            .collect();
        drop(doc_ref);

        let ctx = self.context();
        let key = ctx.snapshots.borrow_mut().insert(descendants)?;
        Ok(Handle::from_bits(encode_snapshot(
            self.slot(),
            TAG_STATIC_HTML_COLLECTION,
            key,
        )))
    }

    fn get_elements_by_tag_name(
        &self,
        qualified_name: &str,
    ) -> Result<Handle<HTMLCollection>, JsError> {
        let (doc, _) = self.anchor();
        let doc_ref = doc.borrow();
        let doc_node = doc_ref.document_node();

        let descendants: Vec<NodeRef> = if qualified_name == "*" {
            doc_ref
                .descendants(doc_node)
                .filter(|node_id| doc_ref.is_element(*node_id))
                .map(|node_id| doc_ref.node_ref(node_id))
                .collect()
        } else {
            use tenta_core::atom::Atom;
            let tag_atom = match Atom::get(&qualified_name.to_ascii_lowercase()) {
                Some(atom) => atom,
                None => {
                    drop(doc_ref);
                    let ctx = self.context();
                    let key = ctx.snapshots.borrow_mut().insert(vec![])?;
                    return Ok(Handle::from_bits(encode_snapshot(
                        self.slot(),
                        TAG_STATIC_HTML_COLLECTION,
                        key,
                    )));
                }
            };

            doc_ref
                .descendants(doc_node)
                .filter(|node_id| doc_ref.tag(*node_id) == Some(tag_atom))
                .map(|node_id| doc_ref.node_ref(node_id))
                .collect()
        };
        drop(doc_ref);

        let ctx = self.context();
        let key = ctx.snapshots.borrow_mut().insert(descendants)?;
        Ok(Handle::from_bits(encode_snapshot(
            self.slot(),
            TAG_STATIC_HTML_COLLECTION,
            key,
        )))
    }

    fn get_elements_by_tag_name_ns(
        &self,
        namespace: Option<&str>,
        local_name: &str,
    ) -> Result<Handle<HTMLCollection>, JsError> {
        // The document-level query is the whole tree in document order, which
        // is `descendants` of the document node.
        let (doc, _) = self.anchor();
        let snapshot = {
            let doc_ref = doc.borrow();
            qualified_snapshot(
                &doc_ref,
                doc_ref.document_node(),
                namespace,
                local_name,
                false,
            )
        };
        Ok(Handle::from_bits(encode_snapshot(
            self.slot(),
            TAG_STATIC_HTML_COLLECTION,
            self.context().snapshots.borrow_mut().insert(snapshot)?,
        )))
    }

    fn last_element_child(&self) -> Result<Option<Handle<Element>>, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        Ok(doc
            .element_children(id)
            .last()
            .map(|child| Handle::from_bits(encode_node(self.slot(), doc.node_ref(child)))))
    }

    fn query_selector(&self, selectors: &str) -> Result<Option<Handle<Element>>, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        match doc.query_selector(id, selectors) {
            Ok(Some(result_id)) => Ok(Some(Handle::from_bits(encode_node(
                self.slot(),
                doc.node_ref(result_id),
            )))),
            Ok(None) => Ok(None),
            Err(e) => Err(JsError::syntax_error(e.to_string())),
        }
    }

    fn query_selector_all(&self, selectors: &str) -> Result<Handle<NodeList>, JsError> {
        let (doc, id) = self.anchor();
        let doc_ref = doc.borrow();

        match doc_ref.query_selector_all(id, selectors) {
            Ok(result_ids) => {
                let refs: Vec<NodeRef> = result_ids
                    .iter()
                    .map(|node_id| doc_ref.node_ref(*node_id))
                    .collect();
                drop(doc_ref);

                let ctx = self.context();
                let key = ctx.snapshots.borrow_mut().insert(refs)?;
                Ok(Handle::from_bits(encode_snapshot(
                    self.slot(),
                    TAG_STATIC_NODE_LIST,
                    key,
                )))
            }
            Err(e) => Err(JsError::syntax_error(e.to_string())),
        }
    }
}

impl DocumentTypeOps for HostObject {
    fn remove(&self) {
        self.remove_from_parent();
    }

    fn before(&self, nodes: Vec<NodeOrDomString>) -> Result<(), JsError> {
        self.mutate_with_nodes(nodes, Document::before_nodes)
    }

    fn after(&self, nodes: Vec<NodeOrDomString>) -> Result<(), JsError> {
        self.mutate_with_nodes(nodes, Document::after_nodes)
    }

    fn replace_with(&self, nodes: Vec<NodeOrDomString>) -> Result<(), JsError> {
        self.mutate_with_nodes(nodes, Document::replace_with_nodes)
    }

    fn name(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        Ok(doc
            .borrow()
            .doctype(id)
            .map(|d| d.name.clone())
            .unwrap_or_default())
    }

    fn public_id(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        Ok(doc
            .borrow()
            .doctype(id)
            .map(|d| d.public_id.clone())
            .unwrap_or_default())
    }

    fn system_id(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        Ok(doc
            .borrow()
            .doctype(id)
            .map(|d| d.system_id.clone())
            .unwrap_or_default())
    }
}

impl ProcessingInstructionOps for HostObject {
    fn target(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        Ok(doc
            .borrow()
            .processing_instruction(id)
            .map(|(target, _)| target.to_owned())
            .unwrap_or_default())
    }
}

impl DOMImplementationOps for HostObject {
    /// The DOM Standard retired feature detection: `hasFeature` "must return
    /// true", whatever it is asked. `dom/nodes/DOMImplementation-hasFeature.html`
    /// asserts exactly that for 130 argument lists, including the empty ones and
    /// the deliberately bogus features, so the method declares no parameters at
    /// all and has nothing to inspect.
    fn has_feature(&self) -> bool {
        true
    }

    fn create_document_type(
        &self,
        qualified_name: &str,
        public_id: &str,
        system_id: &str,
    ) -> Result<Handle<DocumentType>, JsError> {
        tenta_dom::validate_doctype_name(qualified_name)
            .map_err(|error| invalid_character_error(&error))?;
        // The doctype is created in the *implementation's own document*, which
        // is the document the implementation came from: `document.implementation`
        // makes a node the page's arena owns, so appending it to `document`
        // later is a plain tree mutation with no cross-arena move.
        let (doc, _) = self.anchor();
        let mut doc = doc.borrow_mut();
        let id = doc.create_doctype(qualified_name, public_id, system_id);
        Ok(Handle::from_bits(encode_node(
            self.slot(),
            doc.node_ref(id),
        )))
    }

    fn create_document(
        &self,
        namespace: Option<&str>,
        qualified_name: Option<&str>,
        doctype: Option<Option<Handle<DocumentType>>>,
    ) -> Result<Handle<super::Document>, JsError> {
        // The root element name is validated exactly as `createElementNS`
        // validates it, which is what the standard's "validate and extract"
        // step says and what `name-validation.html` pins: `createDocument`
        // must throw the same `InvalidCharacterError`s.
        //
        // A `null` qualified name means "no root element", the same as the empty
        // string: `DOMImplementation-createDocument.html` decides with
        // `qualifiedName === null || String(qualifiedName) === ""`. The
        // parameter is nullable for that reason — the standard spells it
        // `[LegacyNullToEmptyString] DOMString`, which the generator does not
        // implement, and a nullable parameter is observationally the same here
        // (see `idl/dom/dom.webidl`).
        let name = match qualified_name {
            None => None,
            Some("") => None,
            Some(qualified_name) => Some(validate_and_extract(namespace, qualified_name)?),
        };

        // The doctype is *reproduced*, not adopted. The DOM Standard appends the
        // argument node into the new document, which moves it out of wherever it
        // was and keeps its identity; a node in this tree belongs to one arena
        // and its handle names that arena's slot, so moving it would have to
        // rewrite a handle script already holds. The copy keeps every attribute
        // (`name`, `publicId`, `systemId`) and the
        // `doc.doctype.ownerDocument === doc` invariant, and loses only the
        // object identity — the third deviation recorded in the IDL header.
        //
        // A doctype that already has a parent is reproduced too, rather than
        // refused: the standard's step is an append, which adopts, and it is not
        // fallible, so `DOMImplementation-createDocument.html` expects no throw
        // for a doctype that is already associated with a document.
        let doctype_data = match doctype.flatten() {
            Some(handle) => {
                let (doc, id) = self.anchor_of(handle.bits())?;
                let doc = doc.borrow();
                let Some(data) = doc.doctype(id) else {
                    return Err(JsError::type_error(
                        "createDocument: the doctype is not a DocumentType of this world",
                    ));
                };
                Some(data.clone())
            }
            None => None,
        };

        let mut document = tenta_dom::Document::new();
        document.set_url(Some(about_blank()));
        document.set_content_type(content_type_for_namespace(namespace));
        if let Some(data) = doctype_data {
            let copy = document.create_doctype(&data.name, &data.public_id, &data.system_id);
            document
                .mutate(Mutation::Append {
                    parent: document.document_node(),
                    child: copy,
                })
                .map_err(|error| mutation_error(&error))?;
        }
        if let Some((ns, prefix, local)) = name {
            let element = document.create_element(
                QualName {
                    ns,
                    prefix,
                    local: Atom::new(local),
                },
                Vec::new(),
                ElementFlags::default(),
            );
            let element = element.map_err(|error| mutation_error(&error))?;
            document
                .mutate(Mutation::Append {
                    parent: document.document_node(),
                    child: element,
                })
                .map_err(|error| mutation_error(&error))?;
        }

        Ok(Handle::from_bits(self.mint_document(document)?))
    }

    fn create_html_document(
        &self,
        title: Option<&str>,
    ) -> Result<Handle<super::Document>, JsError> {
        // The HTML Standard's "create an HTML document" algorithm: a document
        // with the HTML tree `<!doctype html><html><head>[<title>]</head><body>`,
        // whose URL is `about:blank`, whose encoding is the UTF-8 default (not
        // the calling document's — the WPT file that pins this is served as
        // windows-1252 on purpose), and whose title is the argument.
        let mut document = tenta_dom::Document::new();
        document.set_url(Some(about_blank()));

        let doctype = document.create_doctype("html", "", "");
        let html = document.create_html_element(atoms::HTML);
        let head = document.create_html_element(atoms::HEAD);
        let body = document.create_html_element(atoms::BODY);
        let parent = document.document_node();
        for child in [doctype, html] {
            document
                .mutate(Mutation::Append { parent, child })
                .map_err(|error| mutation_error(&error))?;
        }
        for child in [head, body] {
            document
                .mutate(Mutation::Append {
                    parent: html,
                    child,
                })
                .map_err(|error| mutation_error(&error))?;
        }
        if let Some(title) = title {
            // The title is the text child of a `<title>` element; the argument
            // itself is not normalized (the "strip and collapse" step applies to
            // the *getter*), which is what `DOMImplementation-createHTMLDocument.js`
            // checks: `"foo  bar baz"` reads back verbatim through `firstChild.data`.
            let title_element = document.create_html_element(atoms::TITLE);
            let text = document.create_text(title);
            document
                .mutate(Mutation::Append {
                    parent: head,
                    child: title_element,
                })
                .map_err(|error| mutation_error(&error))?;
            document
                .mutate(Mutation::Append {
                    parent: title_element,
                    child: text,
                })
                .map_err(|error| mutation_error(&error))?;
        }

        Ok(Handle::from_bits(self.mint_document(document)?))
    }
}

// ---------------------------------------------------------------------------
// DOMParser / XMLSerializer
// ---------------------------------------------------------------------------

/// A parsed document inherits the parser's page URL but has no browsing
/// context. Both parsers receive an already-decoded DOMString, so declarations
/// inside the source cannot change its UTF-8 encoding.
fn inert_parsed_document(ctx: &SharedContext, content_type: &str) -> Document {
    let mut document = Document::new();
    document.set_url(ctx.doc.borrow().url().cloned());
    document.set_content_type(content_type);
    document.set_scripting_enabled(false);
    document
}

impl DOMParserConstructor for HostObject {
    fn construct(realm: &Realm) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        let key = ctx.host_records.borrow_mut().insert(HostRecord::DomParser);
        Ok(HostObject::host_record(ctx, key))
    }
}

impl DOMParserOps for HostObject {
    fn parse_from_string(
        &self,
        source: &str,
        content_type: DOMParserSupportedType,
    ) -> Result<Handle<super::Document>, JsError> {
        let mime = content_type.as_str();
        let document = inert_parsed_document(self.context(), mime);
        let mut parsed = if mime == "text/html" {
            let mut parser = HtmlParser::new(
                document,
                ParserOptions {
                    scripting_enabled: false,
                    ..ParserOptions::default()
                },
            );
            let _ = parser.feed_str(source);
            parser.finish()
        } else {
            let mut parser = XmlParser::new(document, XmlOptions::default());
            parser.feed_str(source);
            match parser.finish() {
                Ok(parsed) => parsed,
                Err(error) => {
                    // The XML parser may have written part of the tree before
                    // it found the error. Discard that arena and return a new
                    // document with only the standard parsererror root.
                    let mut error_document = inert_parsed_document(self.context(), mime);
                    let root = error_document
                        .create_element(
                            QualName {
                                ns: Atom::new(
                                    "http://www.mozilla.org/newlayout/xml/parsererror.xml",
                                ),
                                prefix: None,
                                local: Atom::new("parsererror"),
                            },
                            Vec::new(),
                            ElementFlags::default(),
                        )
                        .map_err(|error| mutation_error(&error))?;
                    let message = error_document.create_text(&error.to_string());
                    error_document
                        .mutate(Mutation::Append {
                            parent: error_document.document_node(),
                            child: root,
                        })
                        .map_err(|error| mutation_error(&error))?;
                    error_document
                        .mutate(Mutation::Append {
                            parent: root,
                            child: message,
                        })
                        .map_err(|error| mutation_error(&error))?;
                    error_document
                }
            }
        };
        parsed.set_ready_state(ReadyState::Complete);
        Ok(Handle::from_bits(self.mint_document(parsed)?))
    }
}

impl XMLSerializerConstructor for HostObject {
    fn construct(realm: &Realm) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        let key = ctx
            .host_records
            .borrow_mut()
            .insert(HostRecord::XmlSerializer);
        Ok(HostObject::host_record(ctx, key))
    }
}

impl XMLSerializerOps for HostObject {
    fn serialize_to_string(&self, root: Handle<Node>) -> Result<String, JsError> {
        if is_attr_handle(self.context(), root.bits()) {
            return Ok(String::new());
        }
        let (document, root) = self.anchor_of(root.bits())?;
        Ok(document.borrow().to_xml(root))
    }
}

impl NodeListOps for HostObject {
    fn supported_property_indices(&self) -> Result<Vec<u32>, JsError> {
        Ok((0..NodeListOps::length(self)).collect())
    }

    fn is_supported_property_index(&self, index: u32) -> Result<bool, JsError> {
        Ok(index < NodeListOps::length(self))
    }

    fn item(&self, index: u32) -> Result<Option<Handle<Node>>, JsError> {
        match self {
            HostObject::StaticNodeList { ctx, key } => {
                // Static snapshot: look up in slab
                let snapshots = ctx.snapshots.borrow();
                let refs = match snapshots.get(*key) {
                    Some(r) => r,
                    None => return Ok(None),
                };
                if let Some(node_ref) = refs.get(index as usize) {
                    let doc = ctx.doc.borrow();
                    if let Ok(id) = doc.resolve(*node_ref) {
                        return Ok(Some(Handle::from_bits(encode_node(
                            self.slot(),
                            doc.node_ref(id),
                        ))));
                    }
                }
                Ok(None)
            }
            _ => {
                // Live collection: traverse from anchor
                let (doc, id) = self.anchor();
                let doc = doc.borrow();
                Ok(doc
                    .children(id)
                    .nth(index as usize)
                    .map(|child| Handle::from_bits(encode_node(self.slot(), doc.node_ref(child)))))
            }
        }
    }

    fn length(&self) -> u32 {
        match self {
            HostObject::StaticNodeList { ctx, key } => {
                let snapshots = ctx.snapshots.borrow();
                snapshots.get(*key).map(|r| r.len() as u32).unwrap_or(0)
            }
            _ => {
                let (doc, id) = self.anchor();
                doc.borrow().children(id).count() as u32
            }
        }
    }
}

impl CharacterDataOps for HostObject {
    fn remove(&self) {
        self.remove_from_parent();
    }
    fn previous_element_sibling(&self) -> Result<Option<Handle<Element>>, JsError> {
        Ok(self.element_sibling(false))
    }
    fn next_element_sibling(&self) -> Result<Option<Handle<Element>>, JsError> {
        Ok(self.element_sibling(true))
    }

    fn before(&self, nodes: Vec<NodeOrDomString>) -> Result<(), JsError> {
        self.mutate_with_nodes(nodes, Document::before_nodes)
    }

    fn after(&self, nodes: Vec<NodeOrDomString>) -> Result<(), JsError> {
        self.mutate_with_nodes(nodes, Document::after_nodes)
    }

    fn replace_with(&self, nodes: Vec<NodeOrDomString>) -> Result<(), JsError> {
        self.mutate_with_nodes(nodes, Document::replace_with_nodes)
    }

    fn data(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        Ok(doc.borrow().text_data(id).unwrap_or("").to_owned())
    }

    fn set_data(&self, value: &str) {
        let (doc, id) = self.anchor();
        let mut doc = doc.borrow_mut();
        // SetText should not fail for CharacterData nodes (Text, Comment, CDATA).
        // If it does, we ignore the error since the setter is infallible.
        let _ = doc.mutate(Mutation::SetText {
            node: id,
            text: value.to_owned(),
        });
    }

    fn length(&self) -> u32 {
        let (doc, id) = self.anchor();
        doc.borrow()
            .text_data(id)
            .map(|s| s.encode_utf16().count() as u32)
            .unwrap_or(0)
    }

    fn substring_data(&self, offset: u32, count: u32) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        doc.borrow()
            .substring_data(id, offset, count)
            .map_err(|e| mutation_error(&e))
    }

    fn append_data(&self, data: &str) {
        let (doc, id) = self.anchor();
        let mut doc = doc.borrow_mut();
        // appendData cannot fail on a character-data node; the void signature
        // has no way to report an error regardless.
        let _ = doc.append_data(id, data);
    }

    fn insert_data(&self, offset: u32, data: &str) -> Result<(), JsError> {
        let (doc, id) = self.anchor();
        let mut doc = doc.borrow_mut();
        doc.replace_data(id, offset, 0, data)
            .map_err(|e| mutation_error(&e))
    }

    fn delete_data(&self, offset: u32, count: u32) -> Result<(), JsError> {
        let (doc, id) = self.anchor();
        let mut doc = doc.borrow_mut();
        doc.replace_data(id, offset, count, "")
            .map_err(|e| mutation_error(&e))
    }

    fn replace_data(&self, offset: u32, count: u32, data: &str) -> Result<(), JsError> {
        let (doc, id) = self.anchor();
        let mut doc = doc.borrow_mut();
        doc.replace_data(id, offset, count, data)
            .map_err(|e| mutation_error(&e))
    }
}

impl TextOps for HostObject {
    fn split_text(&self, offset: u32) -> Result<Handle<crate::bindings::Text>, JsError> {
        let (doc, id) = self.anchor();
        let mut doc = doc.borrow_mut();
        let tail = doc
            .split_text(id, offset)
            .map_err(|error| mutation_error(&error))?;
        Ok(Handle::from_bits(encode_node(
            self.slot(),
            doc.node_ref(tail),
        )))
    }

    fn whole_text(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let is_text = |node: NodeId| matches!(doc.kind(node), NodeKind::Text | NodeKind::CData);
        // The contiguous Text nodes: back to the first of the run, then forward.
        let mut first = id;
        loop {
            let previous = doc.previous_sibling(first);
            if !previous.is_some() || !is_text(previous) {
                break;
            }
            first = previous;
        }
        let mut whole = String::new();
        let mut node = first;
        while node.is_some() && is_text(node) {
            whole.push_str(doc.text_data(node).unwrap_or_default());
            node = doc.next_sibling(node);
        }
        Ok(whole)
    }
}

impl CommentOps for HostObject {}

/// Names are collected in tree order, with each element's ID preceding its
/// HTML-namespace `name` attribute. Later duplicates are omitted.
fn html_collection_supported_names(
    doc: &Document,
    elements: impl Iterator<Item = NodeId>,
) -> Vec<String> {
    let mut names = Vec::new();
    let mut seen = HashSet::new();
    for element in elements {
        if let Some(id) = doc.element_id(element)
            && seen.insert(id)
        {
            names.push(id.to_owned());
        }
        if doc.namespace(element) == Some(atoms::NS_HTML)
            && let Some(name) = doc.attribute_str(element, "name")
            && !name.is_empty()
            && seen.insert(name)
        {
            names.push(name.to_owned());
        }
    }
    names
}

fn html_collection_matches_name(doc: &Document, element: NodeId, name: &str) -> bool {
    doc.element_id(element) == Some(name)
        || (doc.namespace(element) == Some(atoms::NS_HTML)
            && doc.attribute_str(element, "name") == Some(name))
}

impl HTMLCollectionOps for HostObject {
    fn supported_property_indices(&self) -> Result<Vec<u32>, JsError> {
        Ok((0..HTMLCollectionOps::length(self)).collect())
    }

    fn is_supported_property_index(&self, index: u32) -> Result<bool, JsError> {
        Ok(index < HTMLCollectionOps::length(self))
    }

    fn supported_property_names(&self) -> Result<Vec<String>, JsError> {
        match self {
            HostObject::StaticHtmlCollection { ctx, key } => {
                let snapshots = ctx.snapshots.borrow();
                let Some(refs) = snapshots.get(*key) else {
                    return Ok(Vec::new());
                };
                let doc = ctx.doc.borrow();
                Ok(html_collection_supported_names(
                    &doc,
                    refs.iter()
                        .filter_map(|reference| doc.resolve(*reference).ok()),
                ))
            }
            _ => {
                let (doc, root) = self.anchor();
                let doc = doc.borrow();
                Ok(html_collection_supported_names(
                    &doc,
                    doc.element_children(root),
                ))
            }
        }
    }

    fn is_supported_property_name(&self, name: &str) -> Result<bool, JsError> {
        Ok(HTMLCollectionOps::named_item(self, name)?.is_some())
    }

    fn item(&self, index: u32) -> Result<Option<Handle<Element>>, JsError> {
        match self {
            HostObject::StaticHtmlCollection { ctx, key } => {
                // Static snapshot: look up in slab
                let snapshots = ctx.snapshots.borrow();
                let refs = match snapshots.get(*key) {
                    Some(r) => r,
                    None => return Ok(None),
                };
                if let Some(node_ref) = refs.get(index as usize) {
                    let doc = ctx.doc.borrow();
                    if let Ok(id) = doc.resolve(*node_ref) {
                        return Ok(Some(Handle::from_bits(encode_node(
                            self.slot(),
                            doc.node_ref(id),
                        ))));
                    }
                }
                Ok(None)
            }
            _ => {
                // Live collection: traverse from anchor
                let (doc, id) = self.anchor();
                let doc = doc.borrow();
                Ok(doc
                    .element_children(id)
                    .nth(index as usize)
                    .map(|child| Handle::from_bits(encode_node(self.slot(), doc.node_ref(child)))))
            }
        }
    }

    fn length(&self) -> u32 {
        match self {
            HostObject::StaticHtmlCollection { ctx, key } => {
                let snapshots = ctx.snapshots.borrow();
                snapshots.get(*key).map(|r| r.len() as u32).unwrap_or(0)
            }
            _ => {
                let (doc, id) = self.anchor();
                doc.borrow().element_children(id).count() as u32
            }
        }
    }

    fn named_item(&self, name: &str) -> Result<Option<Handle<Element>>, JsError> {
        if name.is_empty() {
            return Ok(None);
        }
        match self {
            HostObject::StaticHtmlCollection { ctx, key } => {
                let snapshots = ctx.snapshots.borrow();
                let refs = match snapshots.get(*key) {
                    Some(r) => r,
                    None => return Ok(None),
                };
                let doc = ctx.doc.borrow();
                for node_ref in refs {
                    if let Ok(id) = doc.resolve(*node_ref)
                        && html_collection_matches_name(&doc, id, name)
                    {
                        return Ok(Some(Handle::from_bits(encode_node(
                            self.slot(),
                            doc.node_ref(id),
                        ))));
                    }
                }
                Ok(None)
            }
            _ => {
                // Live collection: traverse from anchor
                let (doc, id) = self.anchor();
                let doc = doc.borrow();
                Ok(doc
                    .element_children(id)
                    .find(|child_id| html_collection_matches_name(&doc, *child_id, name))
                    .map(|child| Handle::from_bits(encode_node(self.slot(), doc.node_ref(child)))))
            }
        }
    }
}

impl EventTargetConstructor for HostObject {
    fn construct(realm: &Realm) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        let key = ctx
            .host_records
            .borrow_mut()
            .insert(HostRecord::EventTarget);
        Ok(HostObject::host_record(ctx, key))
    }
}

impl EventTargetOps for HostObject {
    fn add_event_listener(
        &self,
        r#type: &str,
        callback: Option<EventListener>,
        options: Option<AddEventListenerOptionsOrBoolean>,
    ) -> Result<(), JsError> {
        let Some(callback) = callback else {
            return Ok(());
        };
        let (capture, once, passive) = match options {
            None => (false, false, None),
            Some(AddEventListenerOptionsOrBoolean::Boolean(capture)) => (capture, false, None),
            Some(AddEventListenerOptionsOrBoolean::AddEventListenerOptions(o)) => {
                (o.capture, o.once, o.passive)
            }
        };
        let target = self
            .target_key()
            .ok_or_else(|| JsError::type_error("the receiver is not an event target"))?;
        let passive = passive.unwrap_or_else(|| self.default_passive_value(r#type));
        let cb = CallbackRef(callback.bits());
        let added = self.context().listeners.borrow_mut().add_at(
            target,
            r#type.to_owned(),
            cb,
            capture,
            once,
            passive,
        );
        // The host owns the callback's stored reference: it keeps that reference
        // when the listener is added, and releases it when the entry is a
        // duplicate (the DOM "add an event listener" no-op).
        if added.is_none() {
            self.release_stored(cb.0);
        }
        Ok(())
    }

    fn remove_event_listener(
        &self,
        r#type: &str,
        callback: Option<EventListener>,
        options: Option<EventListenerOptionsOrBoolean>,
    ) -> Result<(), JsError> {
        let Some(callback) = callback else {
            return Ok(());
        };
        let capture = match options {
            None => false,
            Some(EventListenerOptionsOrBoolean::Boolean(capture)) => capture,
            Some(EventListenerOptionsOrBoolean::EventListenerOptions(o)) => o.capture,
        };
        let target = self
            .target_key()
            .ok_or_else(|| JsError::type_error("the receiver is not an event target"))?;
        let cb = CallbackRef(callback.bits());
        let removed = self
            .context()
            .listeners
            .borrow_mut()
            .remove_at(target, r#type, cb, capture);
        // Release the argument's stored reference, plus the removed listener's.
        self.release_stored(cb.0);
        if let Some(removed) = removed {
            self.release_stored(removed.0);
        }
        Ok(())
    }

    fn dispatch_event(&self, event: Handle<Event>) -> Result<bool, JsError> {
        let decoded = decode(event.bits());
        if decoded.tag != TAG_EVENT {
            return Err(JsError::type_error("dispatchEvent requires an Event"));
        }
        let key = decoded.event_key();
        let target = self
            .dispatch_target()
            .ok_or_else(|| JsError::type_error("the receiver is not an event target"))?;
        let context = self.context();
        let event_context = context.documents.get(decoded.slot).ok_or_else(|| {
            JsError::dom_exception(
                "InvalidStateError",
                "the event's document has been reclaimed",
            )
        })?;
        dispatch_at_from(context, &event_context, target, key, false)
    }
}

impl EventOps for HostObject {
    fn r#type(&self) -> Result<String, JsError> {
        Ok(self.with_event(String::new(), |r| r.type_.clone()))
    }

    fn target(&self) -> Result<Option<Handle<EventTarget>>, JsError> {
        Ok(self.with_event(None, |r| r.target.map(Handle::from_bits)))
    }

    fn current_target(&self) -> Result<Option<Handle<EventTarget>>, JsError> {
        Ok(self.with_event(None, |r| r.current_target.map(Handle::from_bits)))
    }

    fn event_phase(&self) -> u16 {
        self.with_event(0, |r| r.phase)
    }

    fn stop_propagation(&self) {
        self.with_event_mut(|r| r.flags.stop_propagation = true);
    }

    fn stop_immediate_propagation(&self) {
        self.with_event_mut(|r| {
            r.flags.stop_propagation = true;
            r.flags.stop_immediate = true;
        });
    }

    fn bubbles(&self) -> bool {
        self.with_event(false, |r| r.bubbles)
    }

    fn cancelable(&self) -> bool {
        self.with_event(false, |r| r.cancelable)
    }

    fn prevent_default(&self) {
        self.with_event_mut(|r| {
            if r.cancelable && !r.flags.in_passive_listener {
                r.flags.canceled = true;
            }
        });
    }

    fn default_prevented(&self) -> bool {
        self.with_event(false, |r| r.flags.canceled)
    }

    fn composed(&self) -> bool {
        self.with_event(false, |r| r.composed)
    }

    fn is_trusted(&self) -> bool {
        self.with_event(false, |r| r.is_trusted)
    }

    fn time_stamp(&self) -> f64 {
        self.with_event(0.0, |r| r.time_stamp)
    }

    fn src_element(&self) -> Result<Option<Handle<EventTarget>>, JsError> {
        // Legacy alias of `target`.
        Ok(self.with_event(None, |r| r.target.map(Handle::from_bits)))
    }

    fn composed_path(&self) -> Result<Vec<Handle<EventTarget>>, JsError> {
        // The propagation path, populated during dispatch and cleared after, so
        // this returns an empty array when the event is not being dispatched.
        Ok(self.with_event(Vec::new(), |r| {
            r.path.iter().copied().map(Handle::from_bits).collect()
        }))
    }

    fn cancel_bubble(&self) -> bool {
        // Legacy alias of the "stop propagation" flag.
        self.with_event(false, |r| r.flags.stop_propagation)
    }

    fn set_cancel_bubble(&self, value: bool) {
        // Setting true stops propagation; setting false is a no-op (per the
        // legacy attribute's definition).
        if value {
            self.with_event_mut(|r| r.flags.stop_propagation = true);
        }
    }

    fn return_value(&self) -> bool {
        // Legacy inverse of `defaultPrevented`: false once the event is canceled.
        self.with_event(true, |r| !r.flags.canceled)
    }

    fn set_return_value(&self, value: bool) {
        // Setting false cancels the event (as `preventDefault`); true is a no-op.
        if !value {
            self.with_event_mut(|r| {
                if r.cancelable && !r.flags.in_passive_listener {
                    r.flags.canceled = true;
                }
            });
        }
    }

    fn init_event(
        &self,
        r#type: &str,
        bubbles: Option<bool>,
        cancelable: Option<bool>,
    ) -> Result<(), JsError> {
        self.with_event_mut(|r| {
            // Ignored while the event is being dispatched.
            if r.flags.dispatch {
                return;
            }
            reset_for_init(
                r,
                r#type,
                bubbles.unwrap_or(false),
                cancelable.unwrap_or(false),
            );
        });
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Event and CustomEvent constructors (`new Event(...)`, `new CustomEvent(...)`)
// ---------------------------------------------------------------------------

/// Downcasts a [`Realm`] back to the [`SharedContext`] it actually is. The
/// wrapper world hands its own context as the realm, so this only fails if a
/// foreign world is installed — a wiring bug surfaced as a `TypeError` rather
/// than a panic.
///
/// Public because a backend that installs its own host members outside the
/// generated glue — the window's `addEventListener` and friends, which cannot
/// be generated while the window is not an IDL interface — needs the same
/// realm-to-context step the generated callbacks perform.
pub fn realm_context(realm: &Realm) -> Result<SharedContext, JsError> {
    realm
        .clone()
        .downcast::<HostContext>()
        .map_err(|_| JsError::type_error("event constructor: the realm is not a HostContext"))
}

/// Resets an event record to its post-`initEvent` state: initialized, every
/// propagation/cancel flag cleared, untrusted, no target, with the given type,
/// `bubbles` and `cancelable`. Shared by `initEvent` and `initCustomEvent`.
fn reset_for_init(record: &mut EventRecord, r#type: &str, bubbles: bool, cancelable: bool) {
    record.flags.initialized = true;
    record.flags.stop_propagation = false;
    record.flags.stop_immediate = false;
    record.flags.canceled = false;
    record.is_trusted = false;
    record.target = None;
    record.current_target = None;
    record.type_ = r#type.to_owned();
    record.bubbles = bubbles;
    record.cancelable = cancelable;
}

/// Retains the exact wrapper converted from a script-provided EventTarget.
/// `Handle` conversion has already rooted the source JS value for this callback.
fn retain_related_target(ctx: &SharedContext, target: Option<u64>) -> Result<Option<u64>, JsError> {
    let Some(target) = target else {
        return Ok(None);
    };
    let decoded = decode(target);
    let live = match decoded.tag {
        TAG_WINDOW => target == encode_window(),
        TAG_NODE => ctx.documents.get(decoded.slot).is_some_and(|owner| {
            let doc = owner.doc.borrow();
            doc.is_live(decoded.node_id)
                && doc.node_ref(decoded.node_id).generation == u32::from(decoded.generation)
        }),
        TAG_HOST => ctx.documents.get(decoded.slot).is_some_and(|owner| {
            matches!(
                owner.host_records.borrow().get(decoded.host_key()),
                Some(
                    HostRecord::EventTarget
                        | HostRecord::Signal(_)
                        | HostRecord::Xhr(_)
                        | HostRecord::Attr(_)
                )
            )
        }),
        _ => false,
    };
    if !live {
        return Err(JsError::type_error("relatedTarget native handle is stale"));
    }
    let script = ctx
        .script
        .borrow()
        .as_ref()
        .cloned()
        .ok_or_else(|| JsError::type_error("relatedTarget has no active script world"))?;
    script.retain_existing_target(target)
}

/// Builds an initialized, untrusted event record of `kind` and inserts it into
/// `ctx`'s slab, returning the host that stands for it. Shared by the `Event`
/// and `CustomEvent` constructors.
fn new_event(
    ctx: &SharedContext,
    r#type: &str,
    bubbles: bool,
    cancelable: bool,
    composed: bool,
    kind: EventKind,
) -> HostObject {
    let mut record = EventRecord::new(r#type.to_owned(), kind, ctx.time_stamp());
    record.bubbles = bubbles;
    record.cancelable = cancelable;
    record.composed = composed;
    // A script-constructed event is initialized but never trusted.
    record.flags.initialized = true;
    record.is_trusted = false;
    let key = ctx.events.borrow_mut().insert(record);
    HostObject::event(ctx.clone(), key)
}

impl EventConstructor for HostObject {
    fn construct(
        realm: &Realm,
        r#type: &str,
        event_init_dict: Option<EventInit>,
    ) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        let init = event_init_dict.unwrap_or_default();
        Ok(new_event(
            &ctx,
            r#type,
            init.bubbles,
            init.cancelable,
            init.composed,
            EventKind::Plain,
        ))
    }
}

impl CustomEventConstructor for HostObject {
    fn construct(
        realm: &Realm,
        r#type: &str,
        event_init_dict: Option<CustomEventInit>,
    ) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        let init = event_init_dict.unwrap_or_default();
        // `detail` is an `any` the glue already interned into the value store
        // (+1 ref) during dictionary conversion; the record owns that reference
        // until it is reclaimed (the world's dead-event sweep releases it). An
        // absent member and an explicit `null` both read back as `null`.
        let detail = init.detail.map(|value| value.0);
        Ok(new_event(
            &ctx,
            r#type,
            init.bubbles,
            init.cancelable,
            init.composed,
            EventKind::Custom { detail },
        ))
    }
}

impl CustomEventOps for HostObject {
    fn detail(&self) -> Result<AnyValue, JsError> {
        Ok(self.with_event(AnyValue::NULL, |r| match &r.kind {
            EventKind::Custom { detail: Some(bits) } => AnyValue(*bits),
            _ => AnyValue::NULL,
        }))
    }

    fn init_custom_event(
        &self,
        r#type: &str,
        bubbles: Option<bool>,
        cancelable: Option<bool>,
        detail: Option<AnyValue>,
    ) -> Result<(), JsError> {
        let HostObject::Event { ctx, key } = self else {
            return Ok(());
        };
        let new_detail = detail.map(|value| value.0);

        // Mutate the record under a short borrow, deciding which stale detail
        // (if any) to release afterwards. The release must happen outside the
        // slab borrow, since `release_stored` borrows the script host.
        let released = {
            let mut events = ctx.events.borrow_mut();
            match events.get_mut(*key) {
                // Ignored while dispatching, or if the record is gone: keep any
                // existing detail and drop the new argument's stored reference.
                Some(record) if record.flags.dispatch => new_detail,
                None => new_detail,
                Some(record) => {
                    reset_for_init(
                        record,
                        r#type,
                        bubbles.unwrap_or(false),
                        cancelable.unwrap_or(false),
                    );
                    let old = match record.kind {
                        EventKind::Custom { detail } => detail,
                        _ => None,
                    };
                    record.kind = EventKind::Custom { detail: new_detail };
                    old
                }
            }
        };
        if let Some(bits) = released {
            self.release_stored(bits);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// UIEvent / MouseEvent (constructors, getters) and HTMLElement.click()
// ---------------------------------------------------------------------------

impl UIEventConstructor for HostObject {
    fn construct(
        realm: &Realm,
        r#type: &str,
        event_init_dict: Option<UIEventInit>,
    ) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        let init = event_init_dict.unwrap_or_default();
        Ok(new_event(
            &ctx,
            r#type,
            init.bubbles,
            init.cancelable,
            init.composed,
            EventKind::Ui {
                view: init.view.map(|handle| handle.bits()),
                detail: init.detail,
                which: init.which,
            },
        ))
    }
}

impl UIEventOps for HostObject {
    fn view(&self) -> Result<Option<Handle<Window>>, JsError> {
        Ok(self.with_event(None, |r| match r.kind {
            EventKind::Ui { view, .. }
            | EventKind::Focus { view, .. }
            | EventKind::Composition { view, .. }
            | EventKind::Keyboard { view, .. }
            | EventKind::Mouse { view, .. } => view.map(Handle::from_bits),
            _ => None,
        }))
    }

    fn detail(&self) -> i32 {
        self.with_event(0, |r| match r.kind {
            EventKind::Ui { detail, .. }
            | EventKind::Focus { detail, .. }
            | EventKind::Composition { detail, .. }
            | EventKind::Keyboard { detail, .. }
            | EventKind::Mouse { detail, .. } => detail,
            _ => 0,
        })
    }

    fn which(&self) -> u32 {
        self.with_event(0, |r| match r.kind {
            EventKind::Ui { which, .. }
            | EventKind::Focus { which, .. }
            | EventKind::Composition { which, .. }
            | EventKind::Keyboard { which, .. }
            | EventKind::Mouse { which, .. } => which,
            _ => 0,
        })
    }

    fn init_ui_event(
        &self,
        type_arg: &str,
        bubbles_arg: Option<bool>,
        cancelable_arg: Option<bool>,
        view_arg: Option<Option<Handle<Window>>>,
        detail_arg: Option<i32>,
    ) -> Result<(), JsError> {
        let new_view = view_arg.flatten().map(|handle| handle.bits());
        self.with_event_mut(|record| {
            if record.flags.dispatch {
                return;
            }
            reset_for_init(
                record,
                type_arg,
                bubbles_arg.unwrap_or(false),
                cancelable_arg.unwrap_or(false),
            );
            match &mut record.kind {
                EventKind::Ui {
                    view,
                    detail,
                    which,
                }
                | EventKind::Focus {
                    view,
                    detail,
                    which,
                    ..
                }
                | EventKind::Composition {
                    view,
                    detail,
                    which,
                    ..
                }
                | EventKind::Keyboard {
                    view,
                    detail,
                    which,
                    ..
                }
                | EventKind::Mouse {
                    view,
                    detail,
                    which,
                    ..
                } => {
                    *view = new_view;
                    *detail = detail_arg.unwrap_or(0);
                    *which = 0;
                }
                _ => {}
            }
        });
        Ok(())
    }
}

impl FocusEventConstructor for HostObject {
    fn construct(
        realm: &Realm,
        r#type: &str,
        event_init_dict: Option<FocusEventInit>,
    ) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        let init = event_init_dict.unwrap_or_default();
        let related_target = init.related_target.map(|handle| handle.bits());
        let related_value = retain_related_target(&ctx, related_target)?;
        Ok(new_event(
            &ctx,
            r#type,
            init.bubbles,
            init.cancelable,
            init.composed,
            EventKind::Focus {
                view: init.view.map(|handle| handle.bits()),
                detail: init.detail,
                which: init.which,
                related_target,
                related_value,
            },
        ))
    }
}

impl FocusEventOps for HostObject {
    fn related_target(&self) -> Result<Option<Handle<EventTarget>>, JsError> {
        Ok(self.with_event(None, |record| match record.kind {
            EventKind::Focus { related_target, .. } => related_target.map(Handle::from_bits),
            _ => None,
        }))
    }
}

impl CompositionEventConstructor for HostObject {
    fn construct(
        realm: &Realm,
        r#type: &str,
        event_init_dict: Option<CompositionEventInit>,
    ) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        let init = event_init_dict.unwrap_or_default();
        Ok(new_event(
            &ctx,
            r#type,
            init.bubbles,
            init.cancelable,
            init.composed,
            EventKind::Composition {
                view: init.view.map(|handle| handle.bits()),
                detail: init.detail,
                which: init.which,
                data: init.data,
            },
        ))
    }
}

impl CompositionEventOps for HostObject {
    fn data(&self) -> Result<String, JsError> {
        Ok(self.with_event(String::new(), |record| match &record.kind {
            EventKind::Composition { data, .. } => data.clone(),
            _ => String::new(),
        }))
    }

    fn init_composition_event(
        &self,
        type_arg: &str,
        bubbles_arg: Option<bool>,
        cancelable_arg: Option<bool>,
        view_arg: Option<Option<Handle<WindowProxy>>>,
        data_arg: Option<&str>,
    ) -> Result<(), JsError> {
        let new_view = view_arg.flatten().map(|handle| handle.bits());
        self.with_event_mut(|record| {
            if record.flags.dispatch {
                return;
            }
            reset_for_init(
                record,
                type_arg,
                bubbles_arg.unwrap_or(false),
                cancelable_arg.unwrap_or(false),
            );
            if let EventKind::Composition {
                view,
                detail,
                which,
                data,
            } = &mut record.kind
            {
                *view = new_view;
                // The legacy initializer has no detail argument. The UI
                // Events initializer path resets it to the UIEvent default.
                *detail = 0;
                *which = 0;
                *data = data_arg.unwrap_or("").to_owned();
            }
        });
        Ok(())
    }
}

const KEY_CTRL: u16 = 1 << 0;
const KEY_SHIFT: u16 = 1 << 1;
const KEY_ALT: u16 = 1 << 2;
const KEY_META: u16 = 1 << 3;
const KEY_ALT_GRAPH: u16 = 1 << 4;
const KEY_CAPS_LOCK: u16 = 1 << 5;
const KEY_FN: u16 = 1 << 6;
const KEY_FN_LOCK: u16 = 1 << 7;
const KEY_HYPER: u16 = 1 << 8;
const KEY_NUM_LOCK: u16 = 1 << 9;
const KEY_SCROLL_LOCK: u16 = 1 << 10;
const KEY_SUPER: u16 = 1 << 11;
const KEY_SYMBOL: u16 = 1 << 12;
const KEY_SYMBOL_LOCK: u16 = 1 << 13;

macro_rules! event_modifier_bits {
    ($init:expr) => {{
        let init = &$init;
        (u16::from(init.ctrl_key) * KEY_CTRL)
            | (u16::from(init.shift_key) * KEY_SHIFT)
            | (u16::from(init.alt_key) * KEY_ALT)
            | (u16::from(init.meta_key) * KEY_META)
            | (u16::from(init.modifier_alt_graph) * KEY_ALT_GRAPH)
            | (u16::from(init.modifier_caps_lock) * KEY_CAPS_LOCK)
            | (u16::from(init.modifier_fn) * KEY_FN)
            | (u16::from(init.modifier_fn_lock) * KEY_FN_LOCK)
            | (u16::from(init.modifier_hyper) * KEY_HYPER)
            | (u16::from(init.modifier_num_lock) * KEY_NUM_LOCK)
            | (u16::from(init.modifier_scroll_lock) * KEY_SCROLL_LOCK)
            | (u16::from(init.modifier_super) * KEY_SUPER)
            | (u16::from(init.modifier_symbol) * KEY_SYMBOL)
            | (u16::from(init.modifier_symbol_lock) * KEY_SYMBOL_LOCK)
    }};
}

fn modifier_bit(key: &str) -> u16 {
    match key {
        "Control" => KEY_CTRL,
        "Shift" => KEY_SHIFT,
        "Alt" => KEY_ALT,
        "Meta" => KEY_META,
        "AltGraph" => KEY_ALT_GRAPH,
        "CapsLock" => KEY_CAPS_LOCK,
        "Fn" => KEY_FN,
        "FnLock" => KEY_FN_LOCK,
        "Hyper" => KEY_HYPER,
        "NumLock" => KEY_NUM_LOCK,
        "ScrollLock" => KEY_SCROLL_LOCK,
        "Super" => KEY_SUPER,
        "Symbol" => KEY_SYMBOL,
        "SymbolLock" => KEY_SYMBOL_LOCK,
        _ => 0,
    }
}

impl HostObject {
    fn event_modifier(&self, bit: u16) -> bool {
        self.with_event(false, |record| match record.kind {
            EventKind::Keyboard { modifiers, .. } | EventKind::Mouse { modifiers, .. } => {
                modifiers & bit != 0
            }
            _ => false,
        })
    }
}

impl KeyboardEventConstructor for HostObject {
    fn construct(
        realm: &Realm,
        r#type: &str,
        event_init_dict: Option<KeyboardEventInit>,
    ) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        let init = event_init_dict.unwrap_or_default();
        let modifiers = event_modifier_bits!(init);
        Ok(new_event(
            &ctx,
            r#type,
            init.bubbles,
            init.cancelable,
            init.composed,
            EventKind::Keyboard {
                view: init.view.map(|handle| handle.bits()),
                detail: init.detail,
                which: init.which,
                key: init.key,
                code: init.code,
                location: init.location,
                modifiers,
                repeat: init.repeat,
                is_composing: init.is_composing,
                char_code: init.char_code,
                key_code: init.key_code,
            },
        ))
    }
}

impl KeyboardEventOps for HostObject {
    fn key(&self) -> Result<String, JsError> {
        Ok(self.with_event(String::new(), |record| match &record.kind {
            EventKind::Keyboard { key, .. } => key.clone(),
            _ => String::new(),
        }))
    }

    fn code(&self) -> Result<String, JsError> {
        Ok(self.with_event(String::new(), |record| match &record.kind {
            EventKind::Keyboard { code, .. } => code.clone(),
            _ => String::new(),
        }))
    }

    fn location(&self) -> u32 {
        self.with_event(0, |record| match record.kind {
            EventKind::Keyboard { location, .. } => location,
            _ => 0,
        })
    }

    fn ctrl_key(&self) -> bool {
        self.event_modifier(KEY_CTRL)
    }
    fn shift_key(&self) -> bool {
        self.event_modifier(KEY_SHIFT)
    }
    fn alt_key(&self) -> bool {
        self.event_modifier(KEY_ALT)
    }
    fn meta_key(&self) -> bool {
        self.event_modifier(KEY_META)
    }

    fn repeat(&self) -> bool {
        self.with_event(false, |record| match record.kind {
            EventKind::Keyboard { repeat, .. } => repeat,
            _ => false,
        })
    }

    fn is_composing(&self) -> bool {
        self.with_event(false, |record| match record.kind {
            EventKind::Keyboard { is_composing, .. } => is_composing,
            _ => false,
        })
    }

    fn get_modifier_state(&self, key_arg: &str) -> bool {
        self.event_modifier(modifier_bit(key_arg))
    }

    fn char_code(&self) -> u32 {
        self.with_event(0, |record| match record.kind {
            EventKind::Keyboard { char_code, .. } => char_code,
            _ => 0,
        })
    }

    fn key_code(&self) -> u32 {
        self.with_event(0, |record| match record.kind {
            EventKind::Keyboard { key_code, .. } => key_code,
            _ => 0,
        })
    }

    fn init_keyboard_event(
        &self,
        type_arg: &str,
        bubbles_arg: Option<bool>,
        cancelable_arg: Option<bool>,
        view_arg: Option<Option<Handle<Window>>>,
        key_arg: Option<&str>,
        location_arg: Option<u32>,
        ctrl_key: Option<bool>,
        alt_key: Option<bool>,
        shift_key: Option<bool>,
        meta_key: Option<bool>,
    ) -> Result<(), JsError> {
        let new_view = view_arg.flatten().map(|handle| handle.bits());
        self.with_event_mut(|record| {
            if record.flags.dispatch {
                return;
            }
            reset_for_init(
                record,
                type_arg,
                bubbles_arg.unwrap_or(false),
                cancelable_arg.unwrap_or(false),
            );
            if let EventKind::Keyboard {
                view,
                detail,
                which,
                key,
                code,
                location,
                modifiers,
                repeat,
                is_composing,
                char_code,
                key_code,
            } = &mut record.kind
            {
                *view = new_view;
                *detail = 0;
                *which = 0;
                *key = key_arg.unwrap_or("").to_owned();
                code.clear();
                *location = location_arg.unwrap_or(0);
                *modifiers = (u16::from(ctrl_key.unwrap_or(false)) * KEY_CTRL)
                    | (u16::from(alt_key.unwrap_or(false)) * KEY_ALT)
                    | (u16::from(shift_key.unwrap_or(false)) * KEY_SHIFT)
                    | (u16::from(meta_key.unwrap_or(false)) * KEY_META);
                *repeat = false;
                *is_composing = false;
                *char_code = 0;
                *key_code = 0;
            }
        });
        Ok(())
    }
}

impl MouseEventConstructor for HostObject {
    fn construct(
        realm: &Realm,
        r#type: &str,
        event_init_dict: Option<MouseEventInit>,
    ) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        let init = event_init_dict.unwrap_or_default();
        // Dictionary conversion keeps the original wrapper rooted through this
        // host call. The record takes one stored reference to that same object;
        // the raw handle alone would not retain a detached relatedTarget.
        let modifiers = event_modifier_bits!(init);
        let related_target = init.related_target.flatten().map(|handle| handle.bits());
        let related_value = retain_related_target(&ctx, related_target)?;
        Ok(new_event(
            &ctx,
            r#type,
            init.bubbles,
            init.cancelable,
            init.composed,
            EventKind::Mouse {
                view: init.view.map(|handle| handle.bits()),
                detail: init.detail,
                which: init.which,
                screen_x: init.screen_x,
                screen_y: init.screen_y,
                client_x: init.client_x,
                client_y: init.client_y,
                modifiers,
                button: init.button,
                buttons: init.buttons,
                related_target,
                related_value,
                wheel: None,
            },
        ))
    }
}

impl MouseEventOps for HostObject {
    fn get_modifier_state(&self, key_arg: &str) -> bool {
        self.event_modifier(modifier_bit(key_arg))
    }

    fn init_mouse_event(
        &self,
        type_arg: &str,
        bubbles_arg: Option<bool>,
        cancelable_arg: Option<bool>,
        view_arg: Option<Option<Handle<Window>>>,
        detail_arg: Option<i32>,
        screen_x_arg: Option<i32>,
        screen_y_arg: Option<i32>,
        client_x_arg: Option<i32>,
        client_y_arg: Option<i32>,
        ctrl_key_arg: Option<bool>,
        alt_key_arg: Option<bool>,
        shift_key_arg: Option<bool>,
        meta_key_arg: Option<bool>,
        button_arg: Option<i16>,
        related_target_arg: Option<Option<Handle<EventTarget>>>,
    ) -> Result<(), JsError> {
        let HostObject::Event { ctx, key } = self else {
            return Ok(());
        };
        // WebIDL conversion already happened. A dispatching or dead record
        // ignores this initializer without any new fallible native acquisition.
        if ctx
            .events
            .borrow()
            .get(*key)
            .is_none_or(|record| record.flags.dispatch)
        {
            return Ok(());
        }
        let new_view = view_arg.flatten().map(|handle| handle.bits());
        let new_related_target = related_target_arg.flatten().map(|handle| handle.bits());
        let new_related_value = retain_related_target(ctx, new_related_target)?;
        let mut adopted = false;
        let old_related_value = {
            let mut events = ctx.events.borrow_mut();
            if let Some(record) = events.get_mut(*key) {
                if record.flags.dispatch {
                    None
                } else {
                    reset_for_init(
                        record,
                        type_arg,
                        bubbles_arg.unwrap_or(false),
                        cancelable_arg.unwrap_or(false),
                    );
                    if let EventKind::Mouse {
                        view,
                        detail,
                        which,
                        screen_x,
                        screen_y,
                        client_x,
                        client_y,
                        modifiers,
                        button,
                        buttons,
                        related_target,
                        related_value,
                        ..
                    } = &mut record.kind
                    {
                        *view = new_view;
                        *detail = detail_arg.unwrap_or(0);
                        *which = 0;
                        *screen_x = screen_x_arg.unwrap_or(0);
                        *screen_y = screen_y_arg.unwrap_or(0);
                        *client_x = client_x_arg.unwrap_or(0);
                        *client_y = client_y_arg.unwrap_or(0);
                        *modifiers = (u16::from(ctrl_key_arg.unwrap_or(false)) * KEY_CTRL)
                            | (u16::from(shift_key_arg.unwrap_or(false)) * KEY_SHIFT)
                            | (u16::from(alt_key_arg.unwrap_or(false)) * KEY_ALT)
                            | (u16::from(meta_key_arg.unwrap_or(false)) * KEY_META);
                        *button = button_arg.unwrap_or(0);
                        // The deprecated initializer has no `buttons` parameter.
                        *buttons = 0;
                        *related_target = new_related_target;
                        adopted = true;
                        std::mem::replace(related_value, new_related_value)
                    } else {
                        None
                    }
                }
            } else {
                None
            }
        };
        if adopted {
            if let Some(bits) = old_related_value {
                self.release_stored(bits);
            }
        } else if let Some(bits) = new_related_value {
            // Native reentrancy invalidated the record after acquisition.
            // Discard only this newly acquired reference; the old edge stayed.
            self.release_stored(bits);
        }
        Ok(())
    }

    fn screen_x(&self) -> i32 {
        self.with_event(0, |r| match r.kind {
            EventKind::Mouse { screen_x, .. } => screen_x,
            _ => 0,
        })
    }

    fn screen_y(&self) -> i32 {
        self.with_event(0, |r| match r.kind {
            EventKind::Mouse { screen_y, .. } => screen_y,
            _ => 0,
        })
    }

    fn client_x(&self) -> i32 {
        self.with_event(0, |r| match r.kind {
            EventKind::Mouse { client_x, .. } => client_x,
            _ => 0,
        })
    }

    fn client_y(&self) -> i32 {
        self.with_event(0, |r| match r.kind {
            EventKind::Mouse { client_y, .. } => client_y,
            _ => 0,
        })
    }

    fn ctrl_key(&self) -> bool {
        self.event_modifier(KEY_CTRL)
    }

    fn shift_key(&self) -> bool {
        self.event_modifier(KEY_SHIFT)
    }

    fn alt_key(&self) -> bool {
        self.event_modifier(KEY_ALT)
    }

    fn meta_key(&self) -> bool {
        self.event_modifier(KEY_META)
    }

    fn button(&self) -> i16 {
        self.with_event(0, |r| match r.kind {
            EventKind::Mouse { button, .. } => button,
            _ => 0,
        })
    }

    fn buttons(&self) -> u16 {
        self.with_event(0, |r| match r.kind {
            EventKind::Mouse { buttons, .. } => buttons,
            _ => 0,
        })
    }

    fn related_target(&self) -> Result<Option<Handle<EventTarget>>, JsError> {
        Ok(self.with_event(None, |r| match r.kind {
            EventKind::Mouse { related_target, .. } => related_target.map(Handle::from_bits),
            _ => None,
        }))
    }
}

impl WheelEventConstructor for HostObject {
    fn construct(
        realm: &Realm,
        r#type: &str,
        event_init_dict: Option<WheelEventInit>,
    ) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        let init = event_init_dict.unwrap_or_default();
        let modifiers = event_modifier_bits!(init);
        let related_target = init.related_target.flatten().map(|handle| handle.bits());
        let related_value = retain_related_target(&ctx, related_target)?;
        Ok(new_event(
            &ctx,
            r#type,
            init.bubbles,
            init.cancelable,
            init.composed,
            EventKind::Mouse {
                view: init.view.map(|handle| handle.bits()),
                detail: init.detail,
                which: init.which,
                screen_x: init.screen_x,
                screen_y: init.screen_y,
                client_x: init.client_x,
                client_y: init.client_y,
                modifiers,
                button: init.button,
                buttons: init.buttons,
                related_target,
                related_value,
                wheel: Some(Box::new(crate::dom::events::WheelDelta {
                    x: init.delta_x,
                    y: init.delta_y,
                    z: init.delta_z,
                    mode: init.delta_mode,
                })),
            },
        ))
    }
}

impl WheelEventOps for HostObject {
    fn delta_x(&self) -> f64 {
        self.with_event(0.0, |record| match &record.kind {
            EventKind::Mouse {
                wheel: Some(wheel), ..
            } => wheel.x,
            _ => 0.0,
        })
    }

    fn delta_y(&self) -> f64 {
        self.with_event(0.0, |record| match &record.kind {
            EventKind::Mouse {
                wheel: Some(wheel), ..
            } => wheel.y,
            _ => 0.0,
        })
    }

    fn delta_z(&self) -> f64 {
        self.with_event(0.0, |record| match &record.kind {
            EventKind::Mouse {
                wheel: Some(wheel), ..
            } => wheel.z,
            _ => 0.0,
        })
    }

    fn delta_mode(&self) -> u32 {
        self.with_event(0, |record| match &record.kind {
            EventKind::Mouse {
                wheel: Some(wheel), ..
            } => wheel.mode,
            _ => 0,
        })
    }
}

// ---------------------------------------------------------------------------
// ProgressEvent (constructor, getters)
// ---------------------------------------------------------------------------

impl ProgressEventConstructor for HostObject {
    fn construct(
        realm: &Realm,
        r#type: &str,
        event_init_dict: Option<ProgressEventInit>,
    ) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        let init = event_init_dict.unwrap_or_default();
        // The three members are stored as initialized. `lengthComputable` is
        // *not* derived from `total`: the XHR getter steps return the values
        // the record was constructed with, so a non-zero `total` under the init
        // default still answers `false`.
        Ok(new_event(
            &ctx,
            r#type,
            init.bubbles,
            init.cancelable,
            init.composed,
            EventKind::Progress {
                length_computable: init.length_computable,
                loaded: init.loaded,
                total: init.total,
            },
        ))
    }
}

// ---------------------------------------------------------------------------
// ErrorEvent (constructor, getters)
// ---------------------------------------------------------------------------

impl ErrorEventConstructor for HostObject {
    fn construct(
        realm: &Realm,
        r#type: &str,
        event_init_dict: Option<ErrorEventInit>,
    ) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        let init = event_init_dict.unwrap_or_default();
        // `error` is an `any` the glue interned during dictionary conversion
        // (+1 ref); the record owns that reference until it is reclaimed, as a
        // `CustomEvent` owns its `detail`.
        let error = init.error.map(|value| value.0);
        Ok(new_event(
            &ctx,
            r#type,
            init.bubbles,
            init.cancelable,
            init.composed,
            EventKind::Error {
                message: init.message,
                filename: init.filename,
                lineno: init.lineno,
                colno: init.colno,
                error,
            },
        ))
    }
}

impl ErrorEventOps for HostObject {
    fn message(&self) -> Result<String, JsError> {
        Ok(self.with_event(String::new(), |r| match &r.kind {
            EventKind::Error { message, .. } => message.clone(),
            _ => String::new(),
        }))
    }

    fn filename(&self) -> Result<String, JsError> {
        Ok(self.with_event(String::new(), |r| match &r.kind {
            EventKind::Error { filename, .. } => filename.clone(),
            _ => String::new(),
        }))
    }

    fn lineno(&self) -> u32 {
        self.with_event(0, |r| match r.kind {
            EventKind::Error { lineno, .. } => lineno,
            _ => 0,
        })
    }

    fn colno(&self) -> u32 {
        self.with_event(0, |r| match r.kind {
            EventKind::Error { colno, .. } => colno,
            _ => 0,
        })
    }

    fn error(&self) -> Result<AnyValue, JsError> {
        // An event made without an `error` reads back as `undefined`, the
        // member's default in the standard.
        let stored = self.with_event(None, |r| match r.kind {
            EventKind::Error { error, .. } => error,
            _ => None,
        });
        Ok(match stored {
            Some(bits) => AnyValue(bits),
            None => self.context().undefined_value(),
        })
    }
}

impl ProgressEventOps for HostObject {
    fn length_computable(&self) -> bool {
        self.with_event(false, |r| match r.kind {
            EventKind::Progress {
                length_computable, ..
            } => length_computable,
            _ => false,
        })
    }

    fn loaded(&self) -> f64 {
        self.with_event(0.0, |r| match r.kind {
            EventKind::Progress { loaded, .. } => loaded,
            _ => 0.0,
        })
    }

    fn total(&self) -> f64 {
        self.with_event(0.0, |r| match r.kind {
            EventKind::Progress { total, .. } => total,
            _ => 0.0,
        })
    }
}

impl HTMLElementOps for HostObject {
    global_event_handlers!();

    fn draggable(&self) -> bool {
        self.reflect_string("draggable")
            .eq_ignore_ascii_case("true")
    }
    fn set_draggable(&self, value: bool) {
        self.reflect_attribute("draggable", if value { "true" } else { "false" });
    }
    fn spellcheck(&self) -> bool {
        !self
            .reflect_string("spellcheck")
            .eq_ignore_ascii_case("false")
    }
    fn set_spellcheck(&self, value: bool) {
        self.reflect_attribute("spellcheck", if value { "true" } else { "false" });
    }
    fn translate(&self) -> bool {
        !self.reflect_string("translate").eq_ignore_ascii_case("no")
    }
    fn set_translate(&self, value: bool) {
        self.reflect_attribute("translate", if value { "yes" } else { "no" });
    }
    fn content_editable(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        Ok(match doc.attribute_str(id, "contenteditable") {
            Some(value) => match value.to_ascii_lowercase().as_str() {
                "" | "true" => "true",
                "false" => "false",
                "plaintext-only" => "plaintext-only",
                _ => "inherit",
            },
            None => "inherit",
        }
        .to_owned())
    }
    fn set_content_editable(&self, value: &str) -> Result<(), JsError> {
        let keyword = value.to_ascii_lowercase();
        match keyword.as_str() {
            "inherit" => self.set_reflect_bool("contenteditable", false),
            "true" | "false" | "plaintext-only" => {
                self.reflect_attribute("contenteditable", &keyword);
            }
            _ => {
                return Err(JsError::dom_exception(
                    "SyntaxError",
                    &format!(
                        "The value provided ('{value}') is not one of 'true', 'false', 'plaintext-only', or 'inherit'."
                    ),
                ));
            }
        }
        Ok(())
    }
    fn is_content_editable(&self) -> bool {
        self.content_editable_state()
    }
    fn input_mode(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("inputmode").to_ascii_lowercase())
    }
    fn set_input_mode(&self, value: &str) {
        self.reflect_attribute("inputmode", value);
    }
    fn enter_key_hint(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("enterkeyhint").to_ascii_lowercase())
    }
    fn set_enter_key_hint(&self, value: &str) {
        self.reflect_attribute("enterkeyhint", value);
    }
    fn autocapitalize(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("autocapitalize").to_ascii_lowercase())
    }
    fn set_autocapitalize(&self, value: &str) {
        self.reflect_attribute("autocapitalize", value);
    }
    fn autofocus(&self) -> bool {
        self.reflect_bool("autofocus")
    }
    fn set_autofocus(&self, value: bool) {
        self.set_reflect_bool("autofocus", value);
    }
    fn nonce(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("nonce"))
    }
    fn set_nonce(&self, value: &str) {
        self.reflect_attribute("nonce", value);
    }
    reflected_nullable_strings! {
        popover, set_popover => "popover";
    }
    fn outer_text(&self) -> Result<String, JsError> {
        HTMLElementOps::inner_text(self)
    }

    fn title(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("title"))
    }
    fn set_title(&self, value: &str) {
        self.reflect_attribute("title", value);
    }
    fn lang(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("lang"))
    }
    fn set_lang(&self, value: &str) {
        self.reflect_attribute("lang", value);
    }
    fn dir(&self) -> Result<String, JsError> {
        // Limited to known values: anything else reads as the empty string.
        let given = self.reflect_string("dir").to_ascii_lowercase();
        Ok(match given.as_str() {
            "ltr" | "rtl" | "auto" => given,
            _ => String::new(),
        })
    }
    fn set_dir(&self, value: &str) {
        self.reflect_attribute("dir", value);
    }
    fn hidden(&self) -> bool {
        self.reflect_bool("hidden")
    }
    fn set_hidden(&self, value: bool) {
        self.set_reflect_bool("hidden", value);
    }
    fn inert(&self) -> bool {
        self.reflect_bool("inert")
    }
    fn set_inert(&self, value: bool) {
        self.set_reflect_bool("inert", value);
    }
    fn access_key(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("accesskey"))
    }
    fn set_access_key(&self, value: &str) {
        self.reflect_attribute("accesskey", value);
    }

    fn inner_text(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        // An element that is not rendered reads as its text content; one in
        // the document reads as the text extractor lays it out.
        if doc.is_composed_connected(id) {
            // Line breaks at either end are not part of the rendered text.
            Ok(tenta_dom::extract::text(&doc, id)
                .trim_matches('\n')
                .to_owned())
        } else {
            Ok(doc.text_content(id))
        }
    }

    fn set_inner_text(&self, value: &str) -> Result<(), JsError> {
        let (doc, id) = self.anchor();
        let mut doc = doc.borrow_mut();
        doc.mutate(Mutation::ReplaceAll {
            parent: id,
            replacement: ReplaceAllWith::Empty,
        })
        .map_err(|e| mutation_error(&e))?;
        // Each line break becomes a `<br>`; the text between them, text nodes.
        let normalised = value.replace("\r\n", "\n").replace('\r', "\n");
        for (index, line) in normalised.split('\n').enumerate() {
            if index > 0 {
                let br = doc.create_html_element(atoms::BR);
                doc.mutate(Mutation::Append {
                    parent: id,
                    child: br,
                })
                .map_err(|e| mutation_error(&e))?;
            }
            if !line.is_empty() {
                let text = doc.create_text(line);
                doc.mutate(Mutation::Append {
                    parent: id,
                    child: text,
                })
                .map_err(|e| mutation_error(&e))?;
            }
        }
        Ok(())
    }

    fn offset_parent(&self) -> Result<Option<Handle<Element>>, JsError> {
        Ok(self.geo_offset_parent())
    }
    fn offset_top(&self) -> i32 {
        self.geo_offset().map_or(0, |offset| offset.top as i32)
    }
    fn offset_left(&self) -> i32 {
        self.geo_offset().map_or(0, |offset| offset.left as i32)
    }
    fn offset_width(&self) -> i32 {
        self.geo_offset().map_or(0, |offset| offset.width as i32)
    }
    fn offset_height(&self) -> i32 {
        self.geo_offset().map_or(0, |offset| offset.height as i32)
    }

    fn click(&self) -> Result<(), JsError> {
        let (doc, id) = self.anchor();
        // The DOM "click in progress" flag: a nested `click()` on the same
        // element during its own dispatch is a no-op, so a handler that calls
        // `click()` again cannot recurse.
        if !self.context().click_in_progress.borrow_mut().insert(id) {
            return Ok(());
        }
        let result = self.dispatch_synthetic_click(doc, id);
        self.context().click_in_progress.borrow_mut().remove(&id);
        // A click nobody cancelled activates the element.
        if result? {
            self.activate_after_click()?;
        }
        Ok(())
    }

    fn style(&self) -> Result<Handle<CSSStyleProperties>, JsError> {
        self.style_handle()
    }

    fn focus(&self, options: Option<FocusOptions>) -> Result<(), JsError> {
        self.focus_steps(options)
    }

    fn blur(&self) -> Result<(), JsError> {
        self.unfocus_steps()
    }

    fn dataset(&self) -> Result<Handle<DOMStringMap>, JsError> {
        Ok(self.dataset_handle())
    }

    fn tab_index(&self) -> i32 {
        self.tab_index_value()
    }

    fn set_tab_index(&self, value: i32) {
        self.reflect_attribute("tabindex", &value.to_string());
    }
}

// `HTMLIFrameElement` is an interface shell for now: it adds no members of its
// own, so its `Ops` trait is empty and the host satisfies it through its
// `HTMLElementOps`/`ElementOps` impls. `SVGElement` carries `style` via the
// `ElementCSSInlineStyle` mixin.
impl HTMLIFrameElementOps for HostObject {}

// Phase 4d per-tag interface shells. The block/inline elements carry no members
// of their own; the form controls reflect only the safe attributes (`name`,
// `placeholder`, `disabled`, `htmlFor`). d-2c adds the URL-valued `href`/`src`/
// `action` through `reflect_url`; the dirty value model and the enumerated
// attributes stay deferred.
impl HTMLDivElementOps for HostObject {}
impl HTMLSpanElementOps for HostObject {}
impl HTMLParagraphElementOps for HostObject {}

// EL1: the element interfaces declared without members.
impl super::HTMLHtmlElementOps for HostObject {}
impl super::HTMLHeadElementOps for HostObject {}
impl super::HTMLTitleElementOps for HostObject {}
impl super::HTMLBaseElementOps for HostObject {}
impl super::HTMLBodyElementOps for HostObject {}
impl super::HTMLHeadingElementOps for HostObject {}
impl super::HTMLHRElementOps for HostObject {}
impl super::HTMLPreElementOps for HostObject {}
impl super::HTMLQuoteElementOps for HostObject {}
impl super::HTMLOListElementOps for HostObject {}
impl super::HTMLUListElementOps for HostObject {}
impl super::HTMLMenuElementOps for HostObject {}
impl super::HTMLLIElementOps for HostObject {}
impl super::HTMLDListElementOps for HostObject {}
impl super::HTMLDataElementOps for HostObject {}
impl super::HTMLTimeElementOps for HostObject {}
impl super::HTMLBRElementOps for HostObject {}
impl super::HTMLModElementOps for HostObject {}
impl super::HTMLPictureElementOps for HostObject {}
impl super::HTMLEmbedElementOps for HostObject {}
impl super::HTMLObjectElementOps for HostObject {}
impl super::HTMLTrackElementOps for HostObject {}
impl super::HTMLMapElementOps for HostObject {}
/// HTML's `HTMLHyperlinkElementUtils`: the element's `href` content attribute,
/// resolved against the document and read or written one URL component at a
/// time. The URL is not kept: HTML's "reinitialize url" runs before every
/// getter and setter, so parsing the attribute each time is the same thing.
impl HostObject {
    /// The element's URL, or `None` when `href` is missing or does not parse.
    fn hyperlink_url(&self) -> Option<url::Url> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        doc.resolve_url(doc.attribute_str(id, "href")?)
    }

    /// One component of the element's URL. An element with no URL answers the
    /// empty string for every component but `protocol`, which is ":".
    fn hyperlink_part(&self, part: LocationPart) -> String {
        match self.hyperlink_url() {
            Some(url) => part.read(&url),
            None if part == LocationPart::Protocol => ":".to_owned(),
            None => String::new(),
        }
    }

    /// Changes the element's URL and writes it back to `href`. An element
    /// with no URL is left alone, and so is `href` when `edit` changed
    /// nothing, which is what a value the URL setter refuses comes to.
    fn update_hyperlink(&self, edit: impl FnOnce(&mut url::Url)) {
        let Some(mut url) = self.hyperlink_url() else {
            return;
        };
        let before = url.clone();
        edit(&mut url);
        if url != before {
            self.reflect_attribute("href", url.as_str());
        }
    }
}

/// The members `HTMLHyperlinkElementUtils` gives each interface that includes
/// it; the generated traits declare them once per interface.
macro_rules! hyperlink_utils {
    () => {
        fn href(&self) -> Result<String, JsError> {
            Ok(self.reflect_url("href"))
        }
        fn set_href(&self, value: &str) {
            self.reflect_attribute("href", value);
        }
        fn origin(&self) -> Result<String, JsError> {
            Ok(self.hyperlink_part(LocationPart::Origin))
        }
        fn protocol(&self) -> Result<String, JsError> {
            Ok(self.hyperlink_part(LocationPart::Protocol))
        }
        fn set_protocol(&self, value: &str) {
            self.update_hyperlink(|url| LocationPart::Protocol.write(url, value));
        }
        fn username(&self) -> Result<String, JsError> {
            Ok(self
                .hyperlink_url()
                .map_or(String::new(), |url| url.username().to_owned()))
        }
        fn set_username(&self, value: &str) {
            self.update_hyperlink(|url| {
                let _ = url.set_username(value);
            });
        }
        fn password(&self) -> Result<String, JsError> {
            Ok(self
                .hyperlink_url()
                .map_or(String::new(), |url| url.password().unwrap_or("").to_owned()))
        }
        fn set_password(&self, value: &str) {
            self.update_hyperlink(|url| {
                let _ = url.set_password(Some(value));
            });
        }
        fn host(&self) -> Result<String, JsError> {
            Ok(self.hyperlink_part(LocationPart::Host))
        }
        fn set_host(&self, value: &str) {
            self.update_hyperlink(|url| LocationPart::Host.write(url, value));
        }
        fn hostname(&self) -> Result<String, JsError> {
            Ok(self.hyperlink_part(LocationPart::Hostname))
        }
        fn set_hostname(&self, value: &str) {
            self.update_hyperlink(|url| LocationPart::Hostname.write(url, value));
        }
        fn port(&self) -> Result<String, JsError> {
            Ok(self.hyperlink_part(LocationPart::Port))
        }
        fn set_port(&self, value: &str) {
            self.update_hyperlink(|url| LocationPart::Port.write(url, value));
        }
        fn pathname(&self) -> Result<String, JsError> {
            Ok(self.hyperlink_part(LocationPart::Pathname))
        }
        fn set_pathname(&self, value: &str) {
            self.update_hyperlink(|url| LocationPart::Pathname.write(url, value));
        }
        fn search(&self) -> Result<String, JsError> {
            Ok(self.hyperlink_part(LocationPart::Search))
        }
        fn set_search(&self, value: &str) {
            self.update_hyperlink(|url| LocationPart::Search.write(url, value));
        }
        fn hash(&self) -> Result<String, JsError> {
            Ok(self.hyperlink_part(LocationPart::Hash))
        }
        fn set_hash(&self, value: &str) {
            self.update_hyperlink(|url| LocationPart::Hash.write(url, value));
        }
        fn to_string(&self) -> Result<String, JsError> {
            Ok(self.reflect_url("href"))
        }
    };
}

impl super::HTMLAreaElementOps for HostObject {
    hyperlink_utils!();
}
impl super::HTMLTableElementOps for HostObject {}
impl super::HTMLTableCaptionElementOps for HostObject {}
impl super::HTMLTableColElementOps for HostObject {}
impl super::HTMLTableSectionElementOps for HostObject {}
impl super::HTMLTableRowElementOps for HostObject {}
impl super::HTMLTableCellElementOps for HostObject {}
impl super::HTMLFieldSetElementOps for HostObject {}
impl super::HTMLLegendElementOps for HostObject {}
impl super::HTMLOptGroupElementOps for HostObject {}
impl super::HTMLDataListElementOps for HostObject {}
impl super::HTMLOutputElementOps for HostObject {}
impl super::HTMLProgressElementOps for HostObject {}
impl super::HTMLMeterElementOps for HostObject {}
impl super::HTMLDetailsElementOps for HostObject {}
impl super::HTMLParamElementOps for HostObject {}
impl super::HTMLFontElementOps for HostObject {}
impl super::HTMLDirectoryElementOps for HostObject {}
impl super::HTMLFrameElementOps for HostObject {}
impl super::HTMLFrameSetElementOps for HostObject {}
impl super::HTMLMarqueeElementOps for HostObject {}
impl super::HTMLUnknownElementOps for HostObject {}

impl HTMLAnchorElementOps for HostObject {
    hyperlink_utils!();

    fn target(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("target"))
    }
    fn set_target(&self, value: &str) {
        self.reflect_attribute("target", value);
    }
    fn download(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("download"))
    }
    fn set_download(&self, value: &str) {
        self.reflect_attribute("download", value);
    }
    fn rel(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("rel"))
    }
    fn set_rel(&self, value: &str) {
        self.reflect_attribute("rel", value);
    }
    fn hreflang(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("hreflang"))
    }
    fn set_hreflang(&self, value: &str) {
        self.reflect_attribute("hreflang", value);
    }
    fn r#type(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("type"))
    }
    fn set_type(&self, value: &str) {
        self.reflect_attribute("type", value);
    }
    fn text(&self) -> Result<String, JsError> {
        Ok(NodeOps::text_content(self)?.unwrap_or_default())
    }
    fn set_text(&self, value: &str) {
        // Replacing an element's children with one text node cannot fail.
        let _ = NodeOps::set_text_content(self, Some(value));
    }
}

impl HTMLImageElementOps for HostObject {
    fn src(&self) -> Result<String, JsError> {
        Ok(self.reflect_url("src"))
    }
    fn set_src(&self, value: &str) {
        self.reflect_attribute("src", value);
    }
    fn srcset(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("srcset"))
    }
    fn set_srcset(&self, value: &str) {
        self.reflect_attribute("srcset", value);
    }
    fn sizes(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("sizes"))
    }
    fn set_sizes(&self, value: &str) {
        self.reflect_attribute("sizes", value);
    }
    fn alt(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("alt"))
    }
    fn set_alt(&self, value: &str) {
        self.reflect_attribute("alt", value);
    }
    fn loading(&self) -> Result<String, JsError> {
        // An enumerated attribute limited to known values: `lazy`, else the
        // `eager` default.
        let lazy = self.reflect_string("loading").eq_ignore_ascii_case("lazy");
        Ok(if lazy { "lazy" } else { "eager" }.to_owned())
    }
    fn set_loading(&self, value: &str) {
        self.reflect_attribute("loading", value);
    }
    fn decoding(&self) -> Result<String, JsError> {
        let value = self.reflect_string("decoding").to_ascii_lowercase();
        Ok(match value.as_str() {
            "sync" | "async" => value,
            _ => "auto".to_owned(),
        })
    }
    fn set_decoding(&self, value: &str) {
        self.reflect_attribute("decoding", value);
    }
    fn width(&self) -> u32 {
        self.image_dimension("width", |snapshot| snapshot.natural_width)
    }
    fn set_width(&self, value: u32) {
        self.reflect_attribute("width", &value.to_string());
    }
    fn height(&self) -> u32 {
        self.image_dimension("height", |snapshot| snapshot.natural_height)
    }
    fn set_height(&self, value: u32) {
        self.reflect_attribute("height", &value.to_string());
    }
    fn natural_width(&self) -> u32 {
        self.image_snapshot().natural_width
    }
    fn natural_height(&self) -> u32 {
        self.image_snapshot().natural_height
    }
    fn complete(&self) -> bool {
        self.image_snapshot().complete
    }
    fn current_src(&self) -> Result<String, JsError> {
        Ok(self.image_snapshot().current_src)
    }

    fn decode(&self) -> Result<Promise<()>, JsError> {
        // https://html.spec.whatwg.org/multipage/embedded-content.html#dom-img-decode
        // The page decodes an image as part of loading it, so an image that is
        // completely available is already decoded: the promise settles with
        // the load. One that is still loading is parked until its `load` or
        // `error` is delivered (`deliver_image_events`).
        let script = self.script_host()?;
        let promise = script
            .new_promise()
            .ok_or_else(|| JsError::type_error("decode: the promise could not be created"))?;
        let snapshot = self.image_snapshot();
        let rejected = snapshot.broken || (snapshot.complete && snapshot.current_src.is_empty());
        if rejected {
            script.settle_promise(
                promise.resolver,
                PromiseOutcome::Reject(HostValue::DomException {
                    name: "EncodingError",
                    message: "The source image cannot be decoded.",
                }),
            );
        } else if snapshot.complete {
            script.settle_promise(
                promise.resolver,
                PromiseOutcome::Resolve(HostValue::Undefined),
            );
        } else if let HostObject::Node { ctx, node } = self {
            ctx.image_decodes
                .borrow_mut()
                .entry(*node)
                .or_default()
                .push(promise.resolver);
        }
        Ok(Promise::from_bits(promise.promise))
    }
}

impl HostObject {
    /// What the page's image loader reports for the anchored element. Without
    /// a loader the element is complete exactly when it has nothing to load.
    fn image_snapshot(&self) -> ImageSnapshot {
        let (doc, id) = self.anchor();
        let host = self.context().image_host.borrow().clone();
        match host {
            Some(host) => host.snapshot(id),
            // No host: the page loads no images, so there is nothing pending.
            // The element is complete, and `decode()` rejects as it does for an
            // image with no usable source.
            None => ImageSnapshot {
                complete: true,
                ..ImageSnapshot::default()
            },
        }
    }

    /// `width` or `height`: the content attribute when it holds a non-negative
    /// integer (HTML's rules for parsing one), else the natural size
    /// `natural` picks from the snapshot. No layout is consulted.
    fn image_dimension(&self, name: &str, natural: impl FnOnce(&ImageSnapshot) -> u32) -> u32 {
        let attribute = self.reflect_string(name);
        let digits = attribute
            .trim_start()
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        if digits > 0
            && let Ok(value) = attribute.trim_start()[..digits].parse::<u32>()
        {
            return value;
        }
        natural(&self.image_snapshot())
    }
}

impl HTMLLabelElementOps for HostObject {
    fn html_for(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("for"))
    }
    fn set_html_for(&self, value: &str) {
        self.reflect_attribute("for", value);
    }
}

// d-2c: `<script>` content-attribute reflection. `async`, `defer` and
// `noModule` are present-or-absent booleans here; whether a script actually
// defers or runs asynchronously is decided when it is prepared, not by this
// surface.
impl HTMLScriptElementOps for HostObject {
    fn src(&self) -> Result<String, JsError> {
        Ok(self.reflect_url("src"))
    }
    fn set_src(&self, value: &str) {
        self.reflect_attribute("src", value);
    }
    fn r#type(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("type"))
    }
    fn set_type(&self, value: &str) {
        self.reflect_attribute("type", value);
    }
    fn charset(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("charset"))
    }
    fn set_charset(&self, value: &str) {
        self.reflect_attribute("charset", value);
    }
    fn cross_origin(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("crossorigin"))
    }
    fn set_cross_origin(&self, value: &str) {
        self.reflect_attribute("crossorigin", value);
    }
    fn integrity(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("integrity"))
    }
    fn set_integrity(&self, value: &str) {
        self.reflect_attribute("integrity", value);
    }
    // HTML: a script element that script created is "force async" until its
    // `async` is assigned, and reads as async for as long as it is. Assigning
    // false is how a loader asks for its scripts to run in insertion order.
    fn r#async(&self) -> bool {
        let (doc, id) = self.anchor();
        doc.borrow().script_force_async(id) || self.reflect_bool("async")
    }
    fn set_async(&self, value: bool) {
        let (doc, id) = self.anchor();
        doc.borrow_mut().set_script_force_async(id, false);
        self.set_reflect_bool("async", value);
    }
    fn text(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        Ok(doc
            .children(id)
            .filter(|child| matches!(doc.kind(*child), NodeKind::Text | NodeKind::CData))
            .filter_map(|child| doc.text_data(child))
            .collect())
    }
    fn set_text(&self, value: &str) {
        // Replacing the children of an element cannot fail.
        let _ = <HostObject as NodeOps>::set_text_content(self, Some(value));
    }
    fn defer(&self) -> bool {
        self.reflect_bool("defer")
    }
    fn set_defer(&self, value: bool) {
        self.set_reflect_bool("defer", value);
    }
    fn no_module(&self) -> bool {
        self.reflect_bool("nomodule")
    }
    fn set_no_module(&self, value: bool) {
        self.set_reflect_bool("nomodule", value);
    }
}

#[cfg(not(any(feature = "v8", feature = "quickjs")))]
impl HostObject {
    fn element_sheet(&self) -> Option<Handle<crate::bindings::CSSStyleSheet>> {
        None
    }

    fn document_style_sheets(&self) -> Result<Handle<crate::bindings::StyleSheetList>, JsError> {
        Err(JsError::type_error("this build has no CSSOM"))
    }

    pub(super) fn adopted_style_sheets(
        &self,
    ) -> Result<Vec<Handle<crate::bindings::CSSStyleSheet>>, JsError> {
        Ok(Vec::new())
    }

    pub(super) fn set_adopted_style_sheets(
        &self,
        _sheets: Vec<Handle<crate::bindings::CSSStyleSheet>>,
    ) -> Result<(), JsError> {
        Err(JsError::type_error("this build has no CSSOM"))
    }
}

// CSS1: `<link>` and `<style>` reflect the attributes the page's stylesheet
// loader reads. The loader fires `load` and `error` at a `<link>`; the handler
// properties are the same `EventHandler` attributes an image has.
impl HTMLLinkElementOps for HostObject {
    fn href(&self) -> Result<String, JsError> {
        Ok(self.reflect_url("href"))
    }
    fn set_href(&self, value: &str) {
        self.reflect_attribute("href", value);
    }
    fn rel(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("rel"))
    }
    fn set_rel(&self, value: &str) {
        self.reflect_attribute("rel", value);
    }
    fn r#as(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("as"))
    }
    fn set_as(&self, value: &str) {
        self.reflect_attribute("as", value);
    }
    fn media(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("media"))
    }
    fn set_media(&self, value: &str) {
        self.reflect_attribute("media", value);
    }
    fn r#type(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("type"))
    }
    fn set_type(&self, value: &str) {
        self.reflect_attribute("type", value);
    }
    fn cross_origin(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("crossorigin"))
    }
    fn set_cross_origin(&self, value: &str) {
        self.reflect_attribute("crossorigin", value);
    }
    fn hreflang(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("hreflang"))
    }
    fn set_hreflang(&self, value: &str) {
        self.reflect_attribute("hreflang", value);
    }
    fn disabled(&self) -> bool {
        self.reflect_bool("disabled")
    }
    fn set_disabled(&self, value: bool) {
        self.set_reflect_bool("disabled", value);
    }
    fn sheet(&self) -> Result<Option<Handle<crate::bindings::CSSStyleSheet>>, JsError> {
        Ok(self.element_sheet())
    }
}

impl HTMLStyleElementOps for HostObject {
    fn media(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("media"))
    }
    fn set_media(&self, value: &str) {
        self.reflect_attribute("media", value);
    }
    fn r#type(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("type"))
    }
    fn set_type(&self, value: &str) {
        self.reflect_attribute("type", value);
    }
    fn disabled(&self) -> bool {
        self.reflect_bool("disabled")
    }
    fn set_disabled(&self, value: bool) {
        self.set_reflect_bool("disabled", value);
    }
    fn sheet(&self) -> Result<Option<Handle<crate::bindings::CSSStyleSheet>>, JsError> {
        Ok(self.element_sheet())
    }
}

impl SVGElementOps for HostObject {
    global_event_handlers!();

    fn style(&self) -> Result<Handle<CSSStyleProperties>, JsError> {
        self.style_handle()
    }

    fn focus(&self, options: Option<FocusOptions>) -> Result<(), JsError> {
        self.focus_steps(options)
    }

    fn blur(&self) -> Result<(), JsError> {
        self.unfocus_steps()
    }

    fn dataset(&self) -> Result<Handle<DOMStringMap>, JsError> {
        Ok(self.dataset_handle())
    }

    fn tab_index(&self) -> i32 {
        self.tab_index_value()
    }

    fn set_tab_index(&self, value: i32) {
        self.reflect_attribute("tabindex", &value.to_string());
    }
}
impl DocumentFragmentOps for HostObject {
    // The queries are the element's, which scope themselves to the subtree of
    // the node they are asked of.
    fn first_element_child(&self) -> Result<Option<Handle<Element>>, JsError> {
        <Self as ElementOps>::first_element_child(self)
    }
    fn last_element_child(&self) -> Result<Option<Handle<Element>>, JsError> {
        <Self as ElementOps>::last_element_child(self)
    }
    fn child_element_count(&self) -> u32 {
        <Self as ElementOps>::child_element_count(self)
    }
    fn children(&self) -> Result<Handle<HTMLCollection>, JsError> {
        <Self as ElementOps>::children(self)
    }
    fn query_selector(&self, selectors: &str) -> Result<Option<Handle<Element>>, JsError> {
        <Self as ElementOps>::query_selector(self, selectors)
    }
    fn query_selector_all(&self, selectors: &str) -> Result<Handle<NodeList>, JsError> {
        <Self as ElementOps>::query_selector_all(self, selectors)
    }
    fn get_element_by_id(&self, element_id: &str) -> Result<Option<Handle<Element>>, JsError> {
        // A fragment is not in the document, so the document's id index does
        // not know its elements: the first match in tree order is found here.
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        Ok(doc
            .descendants(id)
            .find(|node| doc.element_id(*node) == Some(element_id))
            .map(|node| Handle::from_bits(encode_node(self.slot(), doc.node_ref(node)))))
    }

    fn append(&self, nodes: Vec<NodeOrDomString>) -> Result<(), JsError> {
        self.mutate_with_nodes(nodes, Document::append_nodes)
    }

    fn prepend(&self, nodes: Vec<NodeOrDomString>) -> Result<(), JsError> {
        self.mutate_with_nodes(nodes, Document::prepend_nodes)
    }

    fn replace_children(&self, nodes: Vec<NodeOrDomString>) -> Result<(), JsError> {
        self.mutate_with_nodes(nodes, Document::replace_children)
    }
}

impl HTMLTemplateElementOps for HostObject {
    fn content(&self) -> Result<Handle<DocumentFragment>, JsError> {
        // The contents fragment is created with the template element and owned
        // by it, so an HTML `<template>` always has one.
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let contents = doc.template_contents(id).ok_or_else(|| {
            JsError::dom_exception("InvalidStateError", "template has no contents fragment")
        })?;
        Ok(Handle::from_bits(encode_node(
            self.slot(),
            doc.node_ref(contents),
        )))
    }
}

/// The length of a token list attribute from which its parse is kept between
/// reads (`HostObject::with_class_tokens`). Shorter ones, which is nearly
/// every one, are parsed each time: that is cheaper than keeping them.
const LONG_TOKEN_LIST: usize = 512;

thread_local! {
    /// The last long token list parsed on this thread: the attribute value
    /// and the byte range of each token of its ordered set.
    static LONG_TOKEN_SET: RefCell<(String, Vec<(u32, u32)>)> =
        const { RefCell::new((String::new(), Vec::new())) };
}

/// `value` parsed as a DOM ordered set: split on ASCII whitespace, empties
/// dropped, the first occurrence of each token kept.
fn ordered_tokens(value: &str) -> Vec<&str> {
    /// Below this many tokens a search of the list is cheaper than a set.
    const SEARCHED: usize = 16;
    let mut tokens: Vec<&str> = Vec::new();
    let mut seen: Option<HashSet<&str>> = None;
    for token in value.split_ascii_whitespace() {
        let new = if tokens.len() < SEARCHED {
            !tokens.contains(&token)
        } else {
            // A list this long is not searched once per token, which made a
            // read of an n-token list cost n squared comparisons.
            seen.get_or_insert_with(|| tokens.iter().copied().collect())
                .insert(token)
        };
        if new {
            tokens.push(token);
        }
    }
    tokens
}

impl DOMTokenListOps for HostObject {
    fn supported_property_indices(&self) -> Result<Vec<u32>, JsError> {
        Ok((0..DOMTokenListOps::length(self)).collect())
    }

    fn is_supported_property_index(&self, index: u32) -> Result<bool, JsError> {
        Ok(index < DOMTokenListOps::length(self))
    }

    fn length(&self) -> u32 {
        self.with_class_tokens(|tokens| tokens.len() as u32)
    }

    fn item(&self, index: u32) -> Result<Option<String>, JsError> {
        Ok(self
            .with_class_tokens(|tokens| tokens.get(index as usize).map(|&token| token.to_owned())))
    }

    fn contains(&self, token: &str) -> bool {
        // Membership does not need the set: a repeated token is the same one.
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        doc.attribute_str(id, "class").is_some_and(|value| {
            value
                .split_ascii_whitespace()
                .any(|existing| existing == token)
        })
    }

    fn add(&self, tokens: Vec<String>) -> Result<(), JsError> {
        for token in &tokens {
            validate_token(token)?;
        }
        let mut set = self.class_tokens();
        for token in tokens {
            if !set.contains(&token) {
                set.push(token);
            }
        }
        self.set_class_tokens(&set);
        Ok(())
    }

    fn remove(&self, tokens: Vec<String>) -> Result<(), JsError> {
        for token in &tokens {
            validate_token(token)?;
        }
        let mut set = self.class_tokens();
        set.retain(|existing| !tokens.iter().any(|token| token == existing));
        self.set_class_tokens(&set);
        Ok(())
    }

    fn toggle(&self, token: &str, force: Option<bool>) -> Result<bool, JsError> {
        validate_token(token)?;
        let mut set = self.class_tokens();
        let present = set.iter().any(|existing| existing == token);
        match force {
            // `force` pins the outcome: present-if-true, absent-if-false.
            Some(true) => {
                if !present {
                    set.push(token.to_owned());
                    self.set_class_tokens(&set);
                }
                Ok(true)
            }
            Some(false) => {
                if present {
                    set.retain(|existing| existing != token);
                    self.set_class_tokens(&set);
                }
                Ok(false)
            }
            None if present => {
                set.retain(|existing| existing != token);
                self.set_class_tokens(&set);
                Ok(false)
            }
            None => {
                set.push(token.to_owned());
                self.set_class_tokens(&set);
                Ok(true)
            }
        }
    }

    fn replace(&self, token: &str, new_token: &str) -> Result<bool, JsError> {
        validate_token(token)?;
        validate_token(new_token)?;
        let mut set = self.class_tokens();
        let Some(position) = set.iter().position(|existing| existing == token) else {
            return Ok(false);
        };
        set[position] = new_token.to_owned();
        // Replacing may collide with a later token; keep the ordered set unique.
        let mut seen: Vec<String> = Vec::new();
        set.retain(|existing| {
            if seen.iter().any(|kept| kept == existing) {
                false
            } else {
                seen.push(existing.clone());
                true
            }
        });
        self.set_class_tokens(&set);
        Ok(true)
    }

    fn value(&self) -> Result<String, JsError> {
        // The `value` getter returns the underlying attribute verbatim, not the
        // re-serialised ordered set.
        let (doc, id) = self.anchor();
        Ok(doc
            .borrow()
            .attribute_str(id, "class")
            .unwrap_or("")
            .to_owned())
    }

    fn set_value(&self, value: &str) {
        self.reflect_attribute("class", value);
    }
}

/// `Element.attributes`. Every member reads the element's attribute list, so
/// the map is live without any state of its own to keep in step: the element is
/// the state, exactly as it is for `classList`.
impl NamedNodeMapOps for HostObject {
    fn supported_property_indices(&self) -> Result<Vec<u32>, JsError> {
        Ok((0..NamedNodeMapOps::length(self)).collect())
    }

    fn is_supported_property_index(&self, index: u32) -> Result<bool, JsError> {
        Ok(index < NamedNodeMapOps::length(self))
    }

    fn supported_property_names(&self) -> Result<Vec<String>, JsError> {
        let (doc, element) = self.anchor();
        let doc = doc.borrow();
        let filter_uppercase =
            doc.is_html_document() && doc.namespace(element) == Some(atoms::NS_HTML);
        let mut names = Vec::new();
        let mut seen = HashSet::new();
        for (attribute, _) in doc.attributes(element) {
            let qualified = qualified_attr_name(&attribute);
            if filter_uppercase && qualified.bytes().any(|byte| byte.is_ascii_uppercase()) {
                continue;
            }
            if seen.insert(qualified.clone()) {
                names.push(qualified);
            }
        }
        Ok(names)
    }

    fn is_supported_property_name(&self, name: &str) -> Result<bool, JsError> {
        let (doc, element) = self.anchor();
        let doc = doc.borrow();
        if doc.is_html_document()
            && doc.namespace(element) == Some(atoms::NS_HTML)
            && name.bytes().any(|byte| byte.is_ascii_uppercase())
        {
            return Ok(false);
        }
        Ok(attribute_by_qualified_name(&doc, element, name).is_some())
    }

    fn length(&self) -> u32 {
        let (doc, id) = self.anchor();
        doc.borrow().attribute_count(id) as u32
    }

    fn item(&self, index: u32) -> Result<Option<Handle<Attr>>, JsError> {
        let ctx = self.context();
        let (doc, id) = self.anchor();
        let (node, found) = {
            let doc = doc.borrow();
            let found = doc.attributes(id).nth(index as usize).map(|(name, _)| name);
            (doc.node_ref(id), found)
        };
        found
            .map(|name| mint_attr(ctx, Some(node), name).map(Handle::from_bits))
            .transpose()
    }

    fn get_named_item(&self, name: &str) -> Result<Option<Handle<Attr>>, JsError> {
        let ctx = self.context();
        let (doc, id) = self.anchor();
        let (node, found) = {
            let doc = doc.borrow();
            let lookup = attribute_qualified_name_for_lookup(&doc, id, name);
            let found = attribute_by_qualified_name(&doc, id, &lookup).map(|(stored, _)| stored);
            (doc.node_ref(id), found)
        };
        found
            .map(|name| mint_attr(ctx, Some(node), name).map(Handle::from_bits))
            .transpose()
    }

    fn get_named_item_ns(
        &self,
        namespace: Option<&str>,
        local_name: &str,
    ) -> Result<Option<Handle<Attr>>, JsError> {
        let ctx = self.context();
        let (doc, id) = self.anchor();
        let (node, found) = {
            let doc = doc.borrow();
            let found = attribute_name_ns(&doc, id, namespace, local_name).map(AttrName::to_qual);
            (doc.node_ref(id), found)
        };
        found
            .map(|name| mint_attr(ctx, Some(node), name).map(Handle::from_bits))
            .transpose()
    }

    fn set_named_item(&self, attr: Handle<Attr>) -> Result<Option<Handle<Attr>>, JsError> {
        // The map *is* the element, so attaching through it is
        // `setAttributeNode` reached from the other side — the standard defines
        // the two in terms of one *set an attribute*.
        self.attach_attribute_node(attr)
    }

    fn set_named_item_ns(&self, attr: Handle<Attr>) -> Result<Option<Handle<Attr>>, JsError> {
        self.attach_attribute_node(attr)
    }

    fn remove_named_item(&self, name: &str) -> Result<Handle<Attr>, JsError> {
        let ctx = self.context();
        let (doc, id) = self.anchor();
        let found = {
            let doc = doc.borrow();
            let lookup = attribute_qualified_name_for_lookup(&doc, id, name);
            attribute_by_qualified_name(&doc, id, &lookup).map(|(stored, _)| stored)
        };
        let Some(name) = found else {
            return Err(attr_not_found());
        };
        self.detach_attribute(ctx, doc, id, name)
    }

    fn remove_named_item_ns(
        &self,
        namespace: Option<&str>,
        local_name: &str,
    ) -> Result<Handle<Attr>, JsError> {
        let ctx = self.context();
        let (doc, id) = self.anchor();
        let found = attribute_name_ns(&doc.borrow(), id, namespace, local_name);
        let Some(name) = found else {
            return Err(attr_not_found());
        };
        self.detach_attribute(ctx, doc, id, name.to_qual())
    }
}

impl HostObject {
    /// Removes the attribute `name` from element `id` and returns an `Attr`
    /// wrapper for what was removed — the shared half of `removeNamedItem` and
    /// `removeNamedItemNS`.
    ///
    /// The returned object is *detached* and carries the value the attribute
    /// had, which is the standard's own outcome: *remove an attribute* sets the
    /// removed attribute's element to null and leaves its value alone.
    fn detach_attribute(
        &self,
        ctx: &HostContext,
        _doc: &SharedDocument,
        id: NodeId,
        name: QualName,
    ) -> Result<Handle<Attr>, JsError> {
        let (stored_name, value, cached) = ctx
            .remove_element_attribute(id, name)?
            .ok_or_else(attr_not_found)?;
        Ok(Handle::from_bits(match cached {
            Some(key) => encode_host(ctx.slot, key),
            None => mint_detached_attr(ctx, stored_name, value),
        }))
    }
}

/// Attr-specific members; inherited Node behavior is in `NodeOps` above.
impl AttrOps for HostObject {
    fn namespace_uri(&self) -> Result<Option<String>, JsError> {
        Ok(self
            .with_attr(None, |record| Some(record.name.ns.as_str().to_owned()))
            .filter(|ns| !ns.is_empty()))
    }

    fn prefix(&self) -> Result<Option<String>, JsError> {
        Ok(self.with_attr(None, |record| {
            record
                .name
                .prefix
                .filter(|prefix| *prefix != atoms::EMPTY)
                .map(|prefix| prefix.as_str().to_owned())
        }))
    }

    fn local_name(&self) -> Result<String, JsError> {
        Ok(self.with_attr(String::new(), |record| {
            record.name.local.as_str().to_owned()
        }))
    }

    fn name(&self) -> Result<String, JsError> {
        Ok(self.with_attr(String::new(), AttrRecord::qualified_name))
    }

    fn value(&self) -> Result<String, JsError> {
        Ok(self.with_attr(String::new(), |record| self.attr_value(record)))
    }

    fn set_value(&self, value: &str) {
        self.write_attr_value(value);
    }

    fn owner_element(&self) -> Result<Option<Handle<Element>>, JsError> {
        let ctx = self.context();
        let element = self.with_attr(None, |record| record.element);
        Ok(element.map(|node| {
            let doc = ctx.doc.borrow();
            Handle::from_bits(encode_node(ctx.slot, doc.node_ref(node.id)))
        }))
    }

    fn specified(&self) -> bool {
        // Every attribute in this tree came from a start tag, from a script
        // write or from parsing: none is a DTD default, which is the state
        // `specified` distinguishes.
        true
    }
}

impl HostObject {
    /// Writes `value` through the `Attr` this host stands for.
    ///
    /// The write goes to the element's storage while the attribute is attached
    /// — which is what makes a held `Attr` reflect a later `setAttribute` — and
    /// to the record's own state while it is detached.
    fn write_attr_value(&self, value: &str) {
        let HostObject::Host { ctx, key } = self else {
            return;
        };
        let target = {
            let records = ctx.host_records.borrow();
            match records.get(*key) {
                Some(HostRecord::Attr(record)) => (record.element, record.name.clone()),
                _ => return,
            }
        };
        match target {
            (Some(element), name) => {
                let _ = ctx.set_element_attribute(element.id, name, value.to_owned());
            }
            (None, _) => {
                if let Some(HostRecord::Attr(record)) = ctx.host_records.borrow_mut().get_mut(*key)
                {
                    record.detached_value = value.to_owned();
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// `AbstractRange`, `Range`, `StaticRange`
// ---------------------------------------------------------------------------
//
// A range spends no handle tag. It is a `HostRecord` under `TAG_HOST`, like
// `Attr` and for the same structural reason: a range names *two* nodes and a
// handle's node field has room for one. `HostRecord::Range` says this at length,
// and the five tags that are still free stay free.

/// One boundary point of a range, decoded and resolved against the live tree.
#[derive(Clone, Copy)]
struct ResolvedPoint {
    /// The node's id, in the document `anchor_of` resolved it in.
    node: NodeId,
    /// The offset into that node, in UTF-16 code units.
    offset: u32,
}

impl ResolvedPoint {
    /// The point as `tenta-dom`'s range operations take it.
    fn pair(&self) -> (NodeId, u32) {
        (self.node, self.offset)
    }
}

/// A range's two endpoints, each resolved in the document its own handle names.
struct ResolvedRange {
    /// The document the *start* endpoint resolves in. It is the one every
    /// tree-order question about a range is asked of, because the standard
    /// defines a range's root as the root of its start node
    /// (<https://dom.spec.whatwg.org/#concept-range-root>).
    doc: SharedDocument,
    /// The document slot the start endpoint's handle carries, and so the slot
    /// every handle this range hands back has to carry.
    slot: u16,
    start: ResolvedPoint,
    end: ResolvedPoint,
    /// Whether both endpoints resolved in the same document.
    ///
    /// A live `Range` can never be in the other state: `set_boundary` drags the
    /// far endpoint onto the near one whenever a write crosses a document, so
    /// the two always agree afterwards. A `StaticRange` can, because its
    /// constructor is the one range entry point the standard does not validate
    /// and `StaticRange-constructor.html` asserts that endpoints in two
    /// disconnected *documents* are allowed. Comparing the two documents once
    /// here is what keeps a foreign `NodeId` out of this arena's tree walk.
    same_document: bool,
}

/// *length* of a node (<https://dom.spec.whatwg.org/#concept-node-length>): `0`
/// for a doctype, the number of UTF-16 code units of the data for a
/// character-data node, and the child count otherwise.
///
/// The UTF-16 count is what `CharacterData.length` is and what the range tests
/// measure offsets in (their own `nodeLength`), so `IndexSizeError` and the
/// expected offsets agree with the standard only if this does.
fn dom_node_length(doc: &Document, id: NodeId) -> u32 {
    match doc.kind(id) {
        NodeKind::DocType => 0,
        kind if kind.is_character_data_subclass() => doc
            .text_data(id)
            .map_or(0, |data| data.encode_utf16().count() as u32),
        _ => doc.children(id).count() as u32,
    }
}

/// The index of `node` among its parent's children, or `None` when it has no
/// parent.
fn child_index(doc: &Document, node: NodeId) -> Option<u32> {
    let parent = doc.parent(node);
    if !parent.is_some() {
        return None;
    }
    doc.children(parent)
        .position(|child| child == node)
        .map(|index| index as u32)
}

/// The index, among `ancestor`'s children, of the one that leads to
/// `descendant`.
///
/// The callers establish that `ancestor` is an inclusive ancestor of
/// `descendant`, so the walk always finds the child; the `0` fallbacks are what
/// keep the comparison below total when it does not.
fn child_index_toward(doc: &Document, ancestor: NodeId, descendant: NodeId) -> u32 {
    let mut node = descendant;
    loop {
        if node == ancestor {
            return 0;
        }
        let parent = doc.parent(node);
        if parent == ancestor {
            return child_index(doc, node).unwrap_or(0);
        }
        if !parent.is_some() {
            return 0;
        }
        node = parent;
    }
}

/// The standard's *position of one boundary point relative to another*
/// (<https://dom.spec.whatwg.org/#concept-range-bp-position>), as an `Ordering`.
///
/// This is a *strict* order over boundary points: `Equal` comes out only when the
/// two points name the same node and the same offset. In particular
/// `(parent, 0)` is before `(parent.firstChild, 0)` and not equal to it, which is
/// why the two containment arms compare with `>` and `<` rather than `>=` and
/// `<=`. `dom/common.js`'s `getPosition` is the same algorithm written out in
/// JavaScript, and it is the expectation every range test in the directory is
/// derived from.
///
/// Both points must resolve in one document: a `NodeId` means a node of one
/// arena, and `tenta-dom`'s document-order table is that arena's.
fn boundary_position(doc: &Document, a: ResolvedPoint, b: ResolvedPoint) -> Ordering {
    if a.node == b.node {
        return a.offset.cmp(&b.offset);
    }
    if doc.is_inclusive_ancestor(a.node, b.node) {
        // `a` is an ancestor of `b`, so `a` is after `b` only when its offset is
        // past the child that leads to `b`.
        return if a.offset > child_index_toward(doc, a.node, b.node) {
            Ordering::Greater
        } else {
            Ordering::Less
        };
    }
    if doc.is_inclusive_ancestor(b.node, a.node) {
        // The mirror: `b` is an ancestor of `a`, so `a` is before `b` only when
        // `b`'s offset is past the child that leads to `a`.
        return if child_index_toward(doc, b.node, a.node) < b.offset {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    if doc.precedes(a.node, b.node) {
        Ordering::Less
    } else {
        Ordering::Greater
    }
}

/// The UTF-16 substring of `data` between two code-unit offsets, clamped to its
/// length.
///
/// Range offsets are UTF-16 code units, so a byte slice would split a surrogate
/// pair. An offset pair that lands inside a surrogate pair — which JavaScript
/// `substring` would answer with a lone surrogate — yields the shorter of the two
/// answers instead, because a Rust `String` cannot hold half a pair.
fn utf16_slice(data: &str, start: u32, end: u32) -> String {
    let mut unit = 0u32;
    let mut from: Option<usize> = None;
    let mut to: Option<usize> = None;
    for (byte, ch) in data.char_indices() {
        let next = unit + ch.len_utf16() as u32;
        if from.is_none() && unit >= start {
            from = Some(byte);
        }
        if to.is_none() && next > end {
            to = Some(byte);
        }
        unit = next;
        if from.is_some() && to.is_some() {
            break;
        }
    }
    let from = from.unwrap_or(data.len());
    let to = to.unwrap_or(data.len()).max(from);
    data[from..to].to_owned()
}

/// The `WrongDocumentError` three range members share.
fn wrong_document() -> JsError {
    JsError::dom_exception("WrongDocumentError", "the node is not in the range's tree")
}

/// The `InvalidNodeTypeError` a doctype container, a doctype/`Attr` static range
/// endpoint or a parentless node produces.
fn invalid_node_type(message: &str) -> JsError {
    JsError::dom_exception("InvalidNodeTypeError", message)
}

/// The `IndexSizeError` an out-of-range offset produces.
fn index_size() -> JsError {
    JsError::dom_exception(
        "IndexSizeError",
        "the offset is greater than the node's length",
    )
}

/// Gives a freshly built `document` a context of its own in `ctx`'s world and
/// returns the `TAG_NODE` handle bits of its document node.
///
/// [`HostObject::mint_document`] supplies the receiver's context; it is free so
/// that `new Document()` — a constructor, with a realm and no receiver — mints
/// a document the same way `DOMImplementation.createDocument` does.
fn mint_document(ctx: &SharedContext, document: Document) -> Result<u64, JsError> {
    let shared: SharedDocument = Rc::new(RefCell::new(document));
    let child = ctx.new_child(shared)?;
    let node = {
        let doc = child.doc.borrow();
        doc.node_ref(doc.document_node())
    };
    Ok(encode_node(child.slot, node))
}

/// Stores a `Range` record in `ctx`'s slab and returns its key.
///
/// Both the constructor and `Document.createRange` need the key: the former
/// hands a [`HostObject`] to the generated glue, the latter hands
/// `AbstractRange` handle bits to the page, and the bits are the key encoded.
fn insert_range(
    ctx: &SharedContext,
    start: RangePoint,
    end: RangePoint,
) -> Result<HostKey, JsError> {
    let record = RangeRecord::new(ctx, start, end)?;
    Ok(ctx
        .host_records
        .borrow_mut()
        .insert(HostRecord::Range(record)))
}

/// Stores a `StaticRange` record in `ctx`'s slab and returns its key.
fn insert_static_range(
    ctx: &SharedContext,
    start: RangePoint,
    end: RangePoint,
) -> Result<HostKey, JsError> {
    let start = StaticRangePoint::new(ctx, start)?;
    let end = StaticRangePoint::new(ctx, end)?;
    Ok(ctx
        .host_records
        .borrow_mut()
        .insert(HostRecord::StaticRange(StaticRangeRecord { start, end })))
}

/// Mints a `Range` record in `ctx`'s slab and returns its handle bits.
///
/// The bits carry no range tag: a range is a `TAG_HOST` object whose class the
/// record names, exactly as a `URL` or an `Attr` is. Two ranges with equal
/// endpoints get different keys and so different wrappers, which is what
/// `cloneRange`'s `[NewObject]` requires.
fn mint_range(ctx: &SharedContext, start: RangePoint, end: RangePoint) -> Result<u64, JsError> {
    Ok(encode_host(ctx.slot, insert_range(ctx, start, end)?))
}

impl HostObject {
    /// The two boundary points of the `Range` or `StaticRange` this host stands
    /// for, still encoded, or `None` when the host is neither.
    fn range_points(&self) -> Option<(RangePoint, RangePoint)> {
        let HostObject::Host { ctx, key } = self else {
            return None;
        };
        let records = ctx.host_records.borrow();
        match records.get(*key)? {
            HostRecord::Range(record) => record.points(),
            HostRecord::StaticRange(record) => Some((record.start.point, record.end.point)),
            _ => None,
        }
    }

    /// The two boundary points as a `Result`, for the members that cannot answer
    /// without them.
    fn range_record(&self) -> Result<(RangePoint, RangePoint), JsError> {
        self.range_points()
            .ok_or_else(|| JsError::type_error("the object is not a Range"))
    }

    /// Checks that both endpoints of this range are in one document, and returns
    /// them resolved.
    ///
    /// The `RangeOps` members that compare two points all go through here. A
    /// live `Range` is always in the state this demands — see
    /// [`ResolvedRange::same_document`] — so the refusal is only reachable
    /// through a `StaticRange`, which does not carry these members.
    fn live_range(&self) -> Result<ResolvedRange, JsError> {
        let range = self.resolved_range()?;
        if !range.same_document {
            return Err(JsError::type_error(
                "the range's endpoints are in two documents",
            ));
        }
        Ok(range)
    }

    /// Resolves both endpoints of this range against the live tree.
    fn resolved_range(&self) -> Result<ResolvedRange, JsError> {
        let (start, end) = self.range_record()?;
        let (doc, start_node) = self.anchor_of(start.node)?;
        let (end_doc, end_node) = self.anchor_of(end.node)?;
        let same_document = Rc::ptr_eq(&doc, &end_doc);
        Ok(ResolvedRange {
            doc,
            slot: decode(start.node).slot,
            start: ResolvedPoint {
                node: start_node,
                offset: start.offset,
            },
            end: ResolvedPoint {
                node: end_node,
                offset: end.offset,
            },
            same_document,
        })
    }

    /// The `TAG_NODE` handle of one encoded boundary point.
    ///
    /// The slot comes from the point's own handle and not from this host: a range
    /// may name nodes of a document other than the one its record lives in, and
    /// the handle it hands back has to carry *that* document's slot, or the
    /// wrapper world looks the node up in the wrong arena.
    fn point_handle(&self, point: RangePoint) -> Result<Handle<Node>, JsError> {
        let (doc, id) = self.anchor_of(point.node)?;
        let doc = doc.borrow();
        Ok(Handle::from_bits(encode_node(
            decode(point.node).slot,
            doc.node_ref(id),
        )))
    }

    /// The endpoints of the `Range` a handle names, resolved in the document the
    /// handle carries.
    ///
    /// The handle is resolved through the world's `DocumentSet`, not through this
    /// host's own record slab: the argument may be a range of another document,
    /// and `Range-compareBoundaryPoints.html` builds exactly that pair — a page
    /// range and a `foreignDoc` range — and expects `WrongDocumentError`.
    /// Checking the record variant also protects Rust callers that construct a
    /// typed handle directly, independently of the generated WebIDL brand check.
    fn range_of(&self, handle: Handle<Range>) -> Result<ResolvedRange, JsError> {
        let decoded = decode(handle.bits());
        if decoded.tag != TAG_HOST {
            return Err(JsError::type_error("the value is not a Range"));
        }
        let ctx = self
            .context()
            .documents
            .get(decoded.slot)
            .ok_or_else(|| JsError::type_error("the range's document is gone"))?;
        if !matches!(
            ctx.host_records.borrow().get(decoded.host_key()),
            Some(HostRecord::Range(_))
        ) {
            return Err(JsError::type_error("the value is not a Range"));
        }
        HostObject::host_record(ctx, decoded.host_key()).resolved_range()
    }

    /// The node a handle names, if it is in the range's *tree*: its document has
    /// to be the range's document and its root the range's root.
    ///
    /// `None` is the standard's "different root" answer, which the three members
    /// built on this spell differently — `WrongDocumentError` for `comparePoint`
    /// and `false` for `isPointInRange` and `intersectsNode`. Both halves have to
    /// be checked: a `NodeId` means a node of one arena, so a node of another
    /// document cannot even be ordered against this range's endpoints, and within
    /// one arena a detached node has a root of its own.
    fn same_tree(
        &self,
        range: &ResolvedRange,
        handle: Handle<Node>,
    ) -> Result<Option<(SharedDocument, NodeId)>, JsError> {
        let (doc, id) = self.anchor_of(handle.bits())?;
        if !Rc::ptr_eq(&doc, &range.doc) {
            return Ok(None);
        }
        if doc.borrow().root_of(id) != doc.borrow().root_of(range.start.node) {
            return Ok(None);
        }
        Ok(Some((doc, id)))
    }

    /// Writes both boundary points of the `Range` this host stands for.
    /// Sets both of this range's points to `point` of the range's document,
    /// which is where the standard leaves a range whose contents are gone.
    fn collapse_range_to(
        &self,
        range: &ResolvedRange,
        point: (NodeId, u32),
    ) -> Result<(), JsError> {
        let node = encode_node(range.slot, range.doc.borrow().node_ref(point.0));
        let point = RangePoint {
            node,
            offset: point.1,
        };
        self.write_points(point, point)
    }

    /// The handle of a fragment a range operation made, with the images in
    /// its copies told to the page as any clone's are.
    fn range_fragment(
        &self,
        range: &ResolvedRange,
        fragment: NodeId,
    ) -> Handle<crate::bindings::DocumentFragment> {
        self.context().note_cloned_images(fragment);
        Handle::from_bits(encode_node(
            range.slot,
            range.doc.borrow().node_ref(fragment),
        ))
    }

    /// A node argument of a range operation, which has to be of the range's
    /// own document: this engine does not adopt nodes across documents.
    fn range_node_argument(
        &self,
        range: &ResolvedRange,
        handle: Handle<Node>,
    ) -> Result<NodeId, JsError> {
        let decoded = decode(handle.bits());
        if is_attr_handle(self.context(), handle.bits()) {
            return Err(JsError::dom_exception(
                "HierarchyRequestError",
                "an Attr cannot be inserted into a tree",
            ));
        }
        if decoded.tag != TAG_NODE {
            return Err(JsError::type_error("the argument is not a Node"));
        }
        let (doc, id) = self.anchor_of(handle.bits())?;
        if decoded.slot != range.slot || !Rc::ptr_eq(&doc, &range.doc) {
            return Err(JsError::dom_exception(
                "NotSupportedError",
                "the node belongs to another document, and this engine cannot \
                 adopt it into this one",
            ));
        }
        Ok(id)
    }

    /// `insertNode()`: the node goes in at the range's start, and a range
    /// that was collapsed is extended over it.
    fn insert_into_range(&self, range: &ResolvedRange, node: NodeId) -> Result<(), JsError> {
        let collapsed = range.start.pair() == range.end.pair();
        let after = range
            .doc
            .borrow_mut()
            .insert_node_at_range(range.start.pair(), node)
            .map_err(|error| mutation_error(&error))?;
        if !collapsed {
            return Ok(());
        }
        // The insertion moved the range's own points with every other
        // range's; only the end is set, to just after what went in.
        let (start, _) = self.range_record()?;
        let end = RangePoint {
            node: encode_node(range.slot, range.doc.borrow().node_ref(after.0)),
            offset: after.1,
        };
        self.write_points(start, end)
    }

    fn write_points(&self, start: RangePoint, end: RangePoint) -> Result<(), JsError> {
        let HostObject::Host { ctx, key } = self else {
            return Err(JsError::type_error("the object is not a Range"));
        };
        let replacement = RangeRecord::new(ctx, start, end)?;
        let mut records = ctx.host_records.borrow_mut();
        match records.get_mut(*key) {
            Some(HostRecord::Range(record)) => {
                *record = replacement;
                Ok(())
            }
            // A `StaticRange` has no mutators and its template carries none of
            // the members that reach here, so any other record is a wiring bug
            // and not a page's mistake.
            _ => Err(JsError::type_error("the object is not a Range")),
        }
    }

    /// The boundary point *before* a node — `(node.parentNode, node's index)` —
    /// or after it when `after` is set.
    ///
    /// This is what the four node-relative setters are defined in terms of. A node
    /// with no parent has no such point, which is the standard's own
    /// `InvalidNodeTypeError`.
    fn relative_point(&self, handle: Handle<Node>, after: bool) -> Result<RangePoint, JsError> {
        let (doc, id) = self.anchor_of(handle.bits())?;
        let slot = decode(handle.bits()).slot;
        let doc = doc.borrow();
        let parent = doc.parent(id);
        if !parent.is_some() {
            return Err(invalid_node_type("the node has no parent"));
        }
        let index = child_index(&doc, id).unwrap_or(0);
        Ok(RangePoint {
            node: encode_node(slot, doc.node_ref(parent)),
            offset: if after { index + 1 } else { index },
        })
    }

    /// The standard's *set the start of a range to a boundary point*, and its
    /// mirror for the end, with the caller's point already validated.
    ///
    /// Moving either boundary into another root collapses the range there,
    /// including two detached roots in the same document arena. Within one
    /// root, a start after the end (or end before the start) also collapses it.
    fn apply_boundary(&self, is_start: bool, point: RangePoint) -> Result<(), JsError> {
        let (start_raw, end_raw) = self.range_record()?;
        let other_raw = if is_start { end_raw } else { start_raw };
        let (point_doc, point_node) = self.anchor_of(point.node)?;
        let (other_doc, other_node) = self.anchor_of(other_raw.node)?;
        let drags = if Rc::ptr_eq(&point_doc, &other_doc) {
            let doc = point_doc.borrow();
            if doc.root_of(point_node) != doc.root_of(other_node) {
                true
            } else {
                boundary_position(
                    &doc,
                    ResolvedPoint {
                        node: point_node,
                        offset: point.offset,
                    },
                    ResolvedPoint {
                        node: other_node,
                        offset: other_raw.offset,
                    },
                ) == if is_start {
                    Ordering::Greater
                } else {
                    Ordering::Less
                }
            }
        } else {
            true
        };
        let (start, end) = if is_start {
            (point, if drags { point } else { end_raw })
        } else {
            (if drags { point } else { start_raw }, point)
        };
        self.write_points(start, end)
    }

    /// The standard's own two steps for `setStart` and `setEnd`: validate the
    /// node and the offset, then apply the point.
    fn set_boundary(&self, is_start: bool, node: Handle<Node>, offset: u32) -> Result<(), JsError> {
        self.apply_boundary(is_start, self.checked_point(node, offset)?)
    }

    /// A boundary point the standard accepts as `setStart`/`setEnd`'s argument:
    /// a live node that is not a doctype, at an offset inside the node's length.
    fn checked_point(&self, node: Handle<Node>, offset: u32) -> Result<RangePoint, JsError> {
        let (doc, id) = self.anchor_of(node.bits())?;
        let slot = decode(node.bits()).slot;
        let doc = doc.borrow();
        if doc.kind(id) == NodeKind::DocType {
            return Err(invalid_node_type(
                "a DocumentType cannot be a range boundary point",
            ));
        }
        if offset > dom_node_length(&doc, id) {
            return Err(index_size());
        }
        Ok(RangePoint {
            node: encode_node(slot, doc.node_ref(id)),
            offset,
        })
    }

    /// Runs one of the `ParentNode`/`ChildNode` convenience mutations on the
    /// anchored node with its `(Node or DOMString)...` arguments
    /// (<https://dom.spec.whatwg.org/#converting-nodes-into-a-node>).
    ///
    /// Every node argument is resolved first: the resolution reads the
    /// document, and the mutation holds it mutably. Each string argument
    /// becomes a separate Text node, and adjacent Text nodes are preserved.
    fn mutate_with_nodes(
        &self,
        nodes: Vec<NodeOrDomString>,
        apply: fn(&mut Document, NodeId, Vec<NodeOrText>) -> tenta_core::Result<()>,
    ) -> Result<(), JsError> {
        let (doc, id) = self.anchor();
        let items = nodes
            .into_iter()
            .map(|item| match item {
                NodeOrDomString::Node(handle) => {
                    self.same_document_node(handle).map(NodeOrText::Node)
                }
                NodeOrDomString::DomString(text) => Ok(NodeOrText::Text(text)),
            })
            .collect::<Result<Vec<_>, JsError>>()?;
        apply(&mut doc.borrow_mut(), id, items).map_err(|error| mutation_error(&error))
    }
}

impl AbstractRangeOps for HostObject {
    fn start_container(&self) -> Result<Handle<Node>, JsError> {
        let (start, _) = self.range_record()?;
        self.point_handle(start)
    }

    fn start_offset(&self) -> u32 {
        self.range_points().map_or(0, |(start, _)| start.offset)
    }

    fn end_container(&self) -> Result<Handle<Node>, JsError> {
        let (_, end) = self.range_record()?;
        self.point_handle(end)
    }

    fn end_offset(&self) -> u32 {
        self.range_points().map_or(0, |(_, end)| end.offset)
    }

    fn collapsed(&self) -> bool {
        // "This's start and end are the same", where two boundary points are the
        // same when their nodes are the same and their offsets are equal. The
        // encoded handles answer that without touching the tree: a handle carries
        // the node's id *and* its generation, so a swept node whose id was reused
        // does not compare equal to the handle a range still holds. A host that
        // is not a range cannot reach this — the template's signature — and a
        // fresh range is collapsed, so `true` is the harmless answer.
        match self.range_points() {
            Some((start, end)) => start.node == end.node && start.offset == end.offset,
            None => true,
        }
    }
}

impl StaticRangeOps for HostObject {}

impl CDATASectionOps for HostObject {}

/// A node the constructor of its interface makes: one of the window's
/// document, detached, as the document's own `create*` method makes it.
fn construct_node(
    realm: &Realm,
    create: impl FnOnce(&mut Document) -> NodeId,
) -> Result<HostObject, JsError> {
    let ctx = realm_context(realm)?;
    let node = {
        let mut doc = ctx.doc.borrow_mut();
        let id = create(&mut doc);
        doc.node_ref(id)
    };
    Ok(HostObject::node(ctx, node))
}

impl super::TextConstructor for HostObject {
    fn construct(realm: &Realm, data: Option<&str>) -> Result<Self, JsError> {
        construct_node(realm, |doc| doc.create_text(data.unwrap_or("")))
    }
}

impl super::CommentConstructor for HostObject {
    fn construct(realm: &Realm, data: Option<&str>) -> Result<Self, JsError> {
        construct_node(realm, |doc| doc.create_comment(data.unwrap_or("")))
    }
}

impl super::DocumentFragmentConstructor for HostObject {
    fn construct(realm: &Realm) -> Result<Self, JsError> {
        construct_node(realm, Document::create_fragment)
    }
}

impl DocumentConstructor for HostObject {
    /// `new Document()`: "a new `Document`, whose content type is
    /// `application/xml`, URL is `about:blank`, and origin is the current global
    /// object's associated `Document`'s origin"
    /// (<https://dom.spec.whatwg.org/#dom-document-document>).
    ///
    /// Both of the two observable properties are set, because the repository's
    /// `Document::new` defaults are the *HTML* ones: it reports `text/html`, and
    /// an HTML document is one that has no CDATA sections and lowercases tag
    /// names. `dom/common.js` builds the XML tree its ranges are tested over with
    /// this constructor and then calls `createCDATASection` on it, which an HTML
    /// document refuses.
    ///
    /// A document minted here is a document of its own slot, exactly as one
    /// `DOMImplementation.createDocument` produced is — so `document` and
    /// `new Document()` are two arenas, and a node of one cannot be inserted into
    /// the other. That is this engine's missing adoption, and it is why
    /// `dom/common.js` still stops one line below this call (see the deviation in
    /// `idl/dom/dom.webidl`).
    fn construct(realm: &Realm) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        let mut document = Document::new();
        document.set_url(Some(about_blank()));
        document.set_content_type("application/xml");
        let shared: SharedDocument = Rc::new(RefCell::new(document));
        let child = ctx.new_child(shared)?;
        let node = {
            let doc = child.doc.borrow();
            doc.node_ref(doc.document_node())
        };
        Ok(HostObject::node(child, node))
    }
}

impl RangeOps for HostObject {
    fn common_ancestor_container(&self) -> Result<Handle<Node>, JsError> {
        let range = self.resolved_range()?;
        let doc = range.doc.borrow();
        let mut container = range.start.node;
        // The standard's walk up from the start container. When the two endpoints
        // are in different documents there is no common ancestor to find and the
        // start container is the only container either tree can offer; the
        // `parent` guard is the arena's `NONE`, which bounds the walk at the root
        // for the same reason.
        if range.same_document && !doc.is_inclusive_ancestor(container, range.end.node) {
            loop {
                let parent = doc.parent(container);
                if !parent.is_some() {
                    break;
                }
                container = parent;
                if doc.is_inclusive_ancestor(container, range.end.node) {
                    break;
                }
            }
        }
        Ok(Handle::from_bits(encode_node(
            range.slot,
            doc.node_ref(container),
        )))
    }

    fn delete_contents(&self) -> Result<(), JsError> {
        let range = self.resolved_range()?;
        let collapsed = range
            .doc
            .borrow_mut()
            .delete_range_contents(range.start.pair(), range.end.pair())
            .map_err(|error| mutation_error(&error))?;
        self.collapse_range_to(&range, collapsed)
    }

    fn extract_contents(&self) -> Result<Handle<crate::bindings::DocumentFragment>, JsError> {
        let range = self.resolved_range()?;
        let (fragment, collapsed) = range
            .doc
            .borrow_mut()
            .extract_range_contents(range.start.pair(), range.end.pair())
            .map_err(|error| mutation_error(&error))?;
        self.collapse_range_to(&range, collapsed)?;
        Ok(self.range_fragment(&range, fragment))
    }

    fn clone_contents(&self) -> Result<Handle<crate::bindings::DocumentFragment>, JsError> {
        let range = self.resolved_range()?;
        let fragment = range
            .doc
            .borrow_mut()
            .clone_range_contents(range.start.pair(), range.end.pair())
            .map_err(|error| mutation_error(&error))?;
        Ok(self.range_fragment(&range, fragment))
    }

    fn create_contextual_fragment(
        &self,
        html: &str,
    ) -> Result<Handle<crate::bindings::DocumentFragment>, JsError> {
        let range = self.resolved_range()?;
        let fragment = range
            .doc
            .borrow_mut()
            .create_contextual_fragment(range.start.node, html)
            .map_err(|error| mutation_error(&error))?;
        Ok(self.range_fragment(&range, fragment))
    }

    fn insert_node(&self, node: Handle<Node>) -> Result<(), JsError> {
        let range = self.resolved_range()?;
        let node = self.range_node_argument(&range, node)?;
        self.insert_into_range(&range, node)
    }

    fn surround_contents(&self, new_parent: Handle<Node>) -> Result<(), JsError> {
        let range = self.resolved_range()?;
        let parent = self.range_node_argument(&range, new_parent)?;
        {
            let doc = range.doc.borrow();
            if doc.range_splits_a_non_text_node(range.start.pair(), range.end.pair()) {
                return Err(JsError::dom_exception(
                    "InvalidStateError",
                    "the range partially contains a node that is not a Text node",
                ));
            }
            if matches!(
                doc.kind(parent),
                NodeKind::Document | NodeKind::DocType | NodeKind::Fragment | NodeKind::ShadowRoot
            ) {
                return Err(invalid_node_type(
                    "a document, a doctype or a fragment cannot surround a range",
                ));
            }
        }
        let (fragment, collapsed) = range
            .doc
            .borrow_mut()
            .extract_range_contents(range.start.pair(), range.end.pair())
            .map_err(|error| mutation_error(&error))?;
        self.collapse_range_to(&range, collapsed)?;
        // What the new parent held goes; it is then put where the contents
        // were, takes them, and becomes what the range selects.
        {
            let mut doc = range.doc.borrow_mut();
            let children: Vec<NodeId> = doc.children(parent).collect();
            for child in children {
                doc.mutate(Mutation::Remove { child })
                    .map_err(|error| mutation_error(&error))?;
            }
        }
        let range = self.resolved_range()?;
        self.insert_into_range(&range, parent)?;
        let mut doc = range.doc.borrow_mut();
        doc.mutate(Mutation::Append {
            parent,
            child: fragment,
        })
        .map_err(|error| mutation_error(&error))?;
        let container = doc.parent(parent);
        if !container.is_some() {
            return Err(invalid_node_type("the node has no parent"));
        }
        let index = doc
            .children(container)
            .position(|child| child == parent)
            .unwrap_or(0) as u32;
        let container = encode_node(range.slot, doc.node_ref(container));
        drop(doc);
        self.write_points(
            RangePoint {
                node: container,
                offset: index,
            },
            RangePoint {
                node: container,
                offset: index + 1,
            },
        )
    }

    fn set_start(&self, node: Handle<Node>, offset: u32) -> Result<(), JsError> {
        self.set_boundary(true, node, offset)
    }

    fn set_end(&self, node: Handle<Node>, offset: u32) -> Result<(), JsError> {
        self.set_boundary(false, node, offset)
    }

    fn set_start_before(&self, node: Handle<Node>) -> Result<(), JsError> {
        let point = self.relative_point(node, false)?;
        self.apply_boundary(true, point)
    }

    fn set_start_after(&self, node: Handle<Node>) -> Result<(), JsError> {
        let point = self.relative_point(node, true)?;
        self.apply_boundary(true, point)
    }

    fn set_end_before(&self, node: Handle<Node>) -> Result<(), JsError> {
        let point = self.relative_point(node, false)?;
        self.apply_boundary(false, point)
    }

    fn set_end_after(&self, node: Handle<Node>) -> Result<(), JsError> {
        let point = self.relative_point(node, true)?;
        self.apply_boundary(false, point)
    }

    fn collapse(&self, to_start: Option<bool>) -> Result<(), JsError> {
        let (start, end) = self.range_record()?;
        // The argument defaults to false, which moves the start onto the end.
        if to_start.unwrap_or(false) {
            self.write_points(start, start)
        } else {
            self.write_points(end, end)
        }
    }

    fn select_node(&self, node: Handle<Node>) -> Result<(), JsError> {
        // The standard sets the start to `(parent, index)` and then the end to
        // `(parent, index + 1)`. Written straight through, rather than through
        // `apply_boundary`, because that is the state the two steps leave the
        // range in either way: the second point is after the first in tree order,
        // so neither step's fixup fires.
        let before = self.relative_point(node, false)?;
        let after = self.relative_point(node, true)?;
        self.write_points(before, after)
    }

    fn select_node_contents(&self, node: Handle<Node>) -> Result<(), JsError> {
        let (doc, id) = self.anchor_of(node.bits())?;
        let slot = decode(node.bits()).slot;
        let (point, length) = {
            let doc = doc.borrow();
            if doc.kind(id) == NodeKind::DocType {
                return Err(invalid_node_type("a DocumentType has no contents"));
            }
            (
                RangePoint {
                    node: encode_node(slot, doc.node_ref(id)),
                    offset: 0,
                },
                dom_node_length(&doc, id),
            )
        };
        self.write_points(
            point,
            RangePoint {
                node: point.node,
                offset: length,
            },
        )
    }

    fn compare_boundary_points(
        &self,
        how: u16,
        source_range: Handle<Range>,
    ) -> Result<i16, JsError> {
        // The standard's table, read off the expectation
        // `Range-compareBoundaryPoints.html` computes for itself:
        //   0 START_TO_START: this start vs source start
        //   1 START_TO_END:   this end   vs source start
        //   2 END_TO_END:     this end   vs source end
        //   3 END_TO_START:   this start vs source end
        let (this_is_start, source_is_start) = match how {
            0 => (true, true),
            1 => (false, true),
            2 => (false, false),
            3 => (true, false),
            _ => {
                return Err(JsError::dom_exception(
                    "NotSupportedError",
                    "the comparison method is not one of START_TO_START, START_TO_END, \
                     END_TO_END or END_TO_START",
                ));
            }
        };
        let this = self.live_range()?;
        let source = self.range_of(source_range)?;
        if !Rc::ptr_eq(&this.doc, &source.doc) {
            return Err(wrong_document());
        }
        let doc = this.doc.borrow();
        if doc.root_of(this.start.node) != doc.root_of(source.start.node) {
            return Err(wrong_document());
        }
        let this_point = if this_is_start { this.start } else { this.end };
        let source_point = if source_is_start {
            source.start
        } else {
            source.end
        };
        Ok(match boundary_position(&doc, this_point, source_point) {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        })
    }

    fn clone_range(&self) -> Result<Handle<Range>, JsError> {
        let (start, end) = self.range_record()?;
        Ok(Handle::from_bits(mint_range(self.context(), start, end)?))
    }

    /// The standard's own note: "The detach() method must do nothing. Its
    /// functionality (removing a Range from the processing it used to be part of)
    /// is now a no-op, because the processing is no longer performed."
    fn detach(&self) {}

    fn is_point_in_range(&self, node: Handle<Node>, offset: u32) -> Result<bool, JsError> {
        let range = self.live_range()?;
        let Some((doc, id)) = self.same_tree(&range, node)? else {
            return Ok(false);
        };
        let doc = doc.borrow();
        if doc.kind(id) == NodeKind::DocType {
            return Err(invalid_node_type(
                "a DocumentType cannot be a range boundary point",
            ));
        }
        if offset > dom_node_length(&doc, id) {
            return Err(index_size());
        }
        let point = ResolvedPoint { node: id, offset };
        Ok(
            boundary_position(&doc, point, range.start) != Ordering::Less
                && boundary_position(&doc, point, range.end) != Ordering::Greater,
        )
    }

    fn compare_point(&self, node: Handle<Node>, offset: u32) -> Result<i16, JsError> {
        let range = self.live_range()?;
        let Some((doc, id)) = self.same_tree(&range, node)? else {
            return Err(wrong_document());
        };
        let doc = doc.borrow();
        if doc.kind(id) == NodeKind::DocType {
            return Err(invalid_node_type(
                "a DocumentType cannot be a range boundary point",
            ));
        }
        if offset > dom_node_length(&doc, id) {
            return Err(index_size());
        }
        let point = ResolvedPoint { node: id, offset };
        if boundary_position(&doc, point, range.start) == Ordering::Less {
            return Ok(-1);
        }
        if boundary_position(&doc, point, range.end) == Ordering::Greater {
            return Ok(1);
        }
        Ok(0)
    }

    fn intersects_node(&self, node: Handle<Node>) -> Result<bool, JsError> {
        let range = self.live_range()?;
        let Some((doc, id)) = self.same_tree(&range, node)? else {
            return Ok(false);
        };
        let doc = doc.borrow();
        // A node that is an inclusive ancestor of either endpoint shares that
        // endpoint's position or is above it, so the range cannot miss it — and
        // that covers `node` being the start node or the end node itself. This
        // shortcut includes the root (whose null parent makes it intersect)
        // and ancestors that satisfy the boundary-point comparisons.
        if doc.is_inclusive_ancestor(id, range.start.node)
            || doc.is_inclusive_ancestor(id, range.end.node)
        {
            return Ok(true);
        }
        let length = dom_node_length(&doc, id);
        Ok(boundary_position(
            &doc,
            ResolvedPoint {
                node: id,
                offset: 0,
            },
            range.end,
        ) == Ordering::Less
            && boundary_position(
                &doc,
                ResolvedPoint {
                    node: id,
                    offset: length,
                },
                range.start,
            ) == Ordering::Greater)
    }

    fn to_string(&self) -> Result<String, JsError> {
        let range = self.live_range()?;
        let doc = range.doc.borrow();
        let is_text = |id: NodeId| {
            matches!(doc.kind(id), NodeKind::Text | NodeKind::CData) && doc.text_data(id).is_some()
        };
        // The standard's stringifier, step by step: the start node's own tail, the
        // data of every text node *contained* in the range, and the end node's own
        // head. The one case those steps get wrong on their own is a range whose
        // two endpoints are the same text node — the head and the tail would both
        // be emitted, giving the whole string — so that case is the substring
        // between the two offsets instead. `Range-stringifier.html`'s "Text node
        // with offsets" is exactly it. An element container is *not* this case:
        // `selectNodeContents(div)` has one node for both endpoints too, and there
        // the general walk below is the answer.
        if range.start.node == range.end.node && is_text(range.start.node) {
            let data = doc.text_data(range.start.node).unwrap_or_default();
            return Ok(utf16_slice(data, range.start.offset, range.end.offset));
        }
        let mut out = String::new();
        if is_text(range.start.node) {
            let data = doc.text_data(range.start.node).unwrap_or_default();
            out.push_str(&utf16_slice(data, range.start.offset, u32::MAX));
        }
        // "A node is contained in a range if its root is the range's root, and
        // (node, 0) is after the range's start, and (node, node's length) is
        // before the range's end." Walking the root's descendants in tree order
        // and asking the question of every text node is the definition, applied
        // directly; the range's own endpoints fall out of it because their own
        // points are never strictly inside.
        let root = doc.root_of(range.start.node);
        let contained: Vec<NodeId> = doc
            .descendants_inclusive(root)
            .filter(|id| is_text(*id))
            .filter(|id| {
                let length = dom_node_length(&doc, *id);
                boundary_position(
                    &doc,
                    ResolvedPoint {
                        node: *id,
                        offset: 0,
                    },
                    range.start,
                ) == Ordering::Greater
                    && boundary_position(
                        &doc,
                        ResolvedPoint {
                            node: *id,
                            offset: length,
                        },
                        range.end,
                    ) == Ordering::Less
            })
            .collect();
        for id in contained {
            out.push_str(doc.text_data(id).unwrap_or_default());
        }
        if is_text(range.end.node) {
            let data = doc.text_data(range.end.node).unwrap_or_default();
            out.push_str(&utf16_slice(data, 0, range.end.offset));
        }
        Ok(out)
    }
}

impl RangeConstructor for HostObject {
    /// `new Range()`, whose start and end are the *current global object's*
    /// associated `Document` at offset `0`
    /// (<https://dom.spec.whatwg.org/#dom-range-range>).
    ///
    /// The document is taken from the realm and not from `Document.createRange`'s
    /// receiver, and the record is minted in the realm's own slot, so a range the
    /// page makes with `new Range()` and one it makes with
    /// `document.createRange()` are the same kind of object in the same place.
    fn construct(realm: &Realm) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        let point = {
            let doc = ctx.doc.borrow();
            RangePoint {
                node: encode_node(ctx.slot, doc.node_ref(doc.document_node())),
                offset: 0,
            }
        };
        Ok(HostObject::host_record(
            ctx.clone(),
            insert_range(&ctx, point, point)?,
        ))
    }
}

impl StaticRangeConstructor for HostObject {
    /// `new StaticRange(init)`: the four members are *copied*, and nothing is
    /// validated against the tree.
    ///
    /// Two things are refused, both because the standard's own definition of a
    /// static range says its containers are nodes a range could hold, and
    /// `StaticRange-constructor.html`'s last block asserts `InvalidNodeTypeError`
    /// for both:
    ///
    /// - a `DocumentType`, which is a node of this tree and would resolve;
    /// - an `Attr`, which inherits `Node` but uses a host record rather than
    ///   an arena slot and so arrives as a `TAG_HOST` handle. Resolving it like
    ///   an arena node would answer `TypeError` instead of the required
    ///   `InvalidNodeTypeError`.
    fn construct(realm: &Realm, init: StaticRangeInit) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        for handle in [init.start_container, init.end_container] {
            let decoded = decode(handle.bits());
            if decoded.tag == TAG_HOST {
                let is_attr = ctx.documents.get(decoded.slot).is_some_and(|context| {
                    matches!(
                        context.host_records.borrow().get(decoded.host_key()),
                        Some(HostRecord::Attr(_))
                    )
                });
                return Err(if is_attr {
                    invalid_node_type("an Attr cannot be a StaticRange endpoint")
                } else {
                    JsError::type_error("a StaticRange endpoint must be a Node")
                });
            }
            let (doc, id) = resolve_node_handle(&ctx, handle.bits())?;
            if doc.borrow().kind(id) == NodeKind::DocType {
                return Err(invalid_node_type(
                    "a DocumentType cannot be a StaticRange endpoint",
                ));
            }
        }
        let start = RangePoint {
            node: init.start_container.bits(),
            offset: init.start_offset,
        };
        let end = RangePoint {
            node: init.end_container.bits(),
            offset: init.end_offset,
        };
        Ok(HostObject::host_record(
            ctx.clone(),
            insert_static_range(&ctx, start, end)?,
        ))
    }
}

#[cfg(any(feature = "v8", feature = "quickjs"))]
impl CSSOps for HostObject {
    fn escape(_realm: &Realm, ident: &str) -> Result<String, JsError> {
        Ok(tenta_css::escape_identifier(ident))
    }

    fn supports_1(_realm: &Realm, condition_text: &str) -> Result<bool, JsError> {
        Ok(supports_condition(condition_text, &about_blank()))
    }

    fn supports_2(_realm: &Realm, property: &str, value: &str) -> Result<bool, JsError> {
        Ok(supports_property(property, value, &about_blank()))
    }
}

// ---------------------------------------------------------------------------
// URL and URLSearchParams
// ---------------------------------------------------------------------------

/// The `TypeError` the URL Standard raises for a string the URL parser rejects.
fn invalid_url(property: &str) -> JsError {
    JsError::type_error(format!(
        "Failed to set the '{property}' property on 'URL': Invalid URL"
    ))
}

/// Splits a `host[:port]` value the way the URL Standard's host state does.
///
/// `url::Url::set_host` takes a host alone and silently drops everything after a
/// `:`, so leaving the split to it would drop the port of `url.host =
/// 'example.com:8080'`. Brackets protect the colons of an IPv6 literal, which is
/// why the closing bracket is searched for first.
fn split_host_port(value: &str) -> (&str, Option<&str>) {
    let host_end = if value.starts_with('[') {
        match value.find(']') {
            Some(index) => index + 1,
            None => return (value, None),
        }
    } else {
        value.find(':').unwrap_or(value.len())
    };
    match value[host_end..].strip_prefix(':') {
        Some(port) => (&value[..host_end], Some(port)),
        None => (value, None),
    }
}

/// The URL Standard's sort order: UTF-16 code units.
///
/// It differs from comparing Rust strings (UTF-8 bytes, so scalar values) only
/// where a non-BMP character meets one in `U+E000..=U+FFFF`, and sorting a query
/// string differently from every other browser is not worth the saved iterator.
fn cmp_code_units(left: &str, right: &str) -> std::cmp::Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

/// Parses an `application/x-www-form-urlencoded` byte string into a pair list.
///
/// This is the URL Standard's parser itself: `+` is a space, a `%XX` escape is
/// decoded, and anything undecodable is replaced.
fn parse_pairs(input: &str) -> Vec<(String, String)> {
    url::form_urlencoded::parse(input.as_bytes())
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect()
}

/// Serializes a pair list as `application/x-www-form-urlencoded` — the inverse
/// of [`parse_pairs`], and the serializer the spec names, so the surviving
/// character set (`A-Za-z0-9*-._`, with a space as `+`) is not a choice made
/// here.
fn serialize_pairs(list: &[(String, String)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (name, value) in list {
        serializer.append_pair(name, value);
    }
    serializer.finish()
}

/// The key of `ctx`'s `DOMImplementation` record, creating it on first use.
///
/// Caching the key on the context is what `[SameObject]` requires of
/// `document.implementation`. The cache is re-checked against the slab because
/// a record is freed when its wrapper is collected: the next read after a
/// collection has to mint a fresh record rather than answer with a stale key,
/// and a stale key would resolve to whatever took the slot.
fn implementation_key(ctx: &HostContext) -> HostKey {
    if let Some(key) = *ctx.implementation.borrow()
        && ctx.host_records.borrow().get(key).is_some()
    {
        return key;
    }
    let key = ctx
        .host_records
        .borrow_mut()
        .insert(HostRecord::Implementation);
    *ctx.implementation.borrow_mut() = Some(key);
    key
}

/// The URL every `DOMImplementation`-minted document has, until it is navigated
/// (which nothing in this tree can do to one).
fn about_blank() -> Url {
    "about:blank"
        .parse()
        .expect("`about:blank` is a valid URL in every URL Standard")
}

/// The `contentType` of a document `createDocument` mints: the MIME type the
/// namespace names, which is also what makes the document an XML document for
/// `Document::is_html_document` (every one of the three contains "xml" except
/// the SVG one, which is deliberately not an HTML document either).
fn content_type_for_namespace(namespace: Option<&str>) -> &'static str {
    match namespace {
        Some("http://www.w3.org/1999/xhtml") => "application/xhtml+xml",
        Some("http://www.w3.org/2000/svg") => "image/svg+xml",
        _ => "application/xml",
    }
}

/// Maps a `tenta_dom` validation failure onto the `InvalidCharacterError` the
/// DOM Standard reports for a name it refuses.
fn invalid_character_error(error: &tenta_core::Error) -> JsError {
    JsError::dom_exception("InvalidCharacterError", &error.to_string())
}

/// The error a `DOMImplementation` factory reports when the world's document
/// slot field is full.
///
/// The DOM Standard has no name for "this engine cannot hold another document",
/// so this borrows the one DOMException name that means "a resource this engine
/// accounts for is exhausted". It is a plain, catchable JavaScript exception and
/// never an abort: a page that mints documents in a loop loses the documents it
/// asked for past the ceiling, and nothing else.
fn document_slots_exhausted() -> JsError {
    JsError::dom_exception(
        "QuotaExceededError",
        "the engine cannot hold another document in this browsing context",
    )
}

/// The pair list of the `URLSearchParams` at `key`, or `None` when the key names
/// no such record.
///
/// A query object derives its pairs from its URL's *current* query on every
/// read, because the URL is the shared source of truth: an assignment to
/// `url.search` or `url.href` must be visible through a `URLSearchParams` that
/// was obtained before it, and a stored copy would not see it. Deriving on read
/// is also what keeps a read from rewriting the URL: the pair list of `?a` is
/// `[('a', '')]`, whose serialization is `a=`, so only a *mutation* may perform
/// the spec's update — the URL must not gain an `=` merely because something
/// read its params.
fn host_pairs(slab: &HostSlab, key: HostKey) -> Option<Vec<(String, String)>> {
    let params = match slab.get(key)? {
        HostRecord::Params(params) => params,
        HostRecord::EventTarget | HostRecord::Url(_) | HostRecord::Headers(_) => return None,
        // Neither abort record holds a pair list, so neither owns pairs.
        HostRecord::Controller(_) | HostRecord::Signal(_) => return None,
        // A response and a request hold no pair list of their own; their
        // headers do.
        HostRecord::Response(_) | HostRecord::Request(_) => return None,
        // Neither encoding record holds one either, and neither does an
        // `XMLHttpRequest`, a `Blob`, a `File` or a `FormData` — whose entry
        // list is a list of *entries*, not a pair list the URL Standard's form
        // would read.
        HostRecord::TextDecoder(_)
        | HostRecord::TextEncoder
        | HostRecord::DomParser
        | HostRecord::XmlSerializer
        | HostRecord::Xhr(_)
        | HostRecord::Blob(_)
        | HostRecord::File(_)
        | HostRecord::Implementation
        | HostRecord::Attr(_)
        | HostRecord::FormData(_)
        | HostRecord::Storage(_)
        | HostRecord::StyleSheetList
        | HostRecord::DomRect(_)
        | HostRecord::DomRectList(_)
        | HostRecord::Crypto
        | HostRecord::MutationObserver(_)
        | HostRecord::MutationRecord(_) => return None,
        // A range holds two node handles and no pair list either.
        HostRecord::Range(_) | HostRecord::StaticRange(_) => return None,
        #[cfg(any(feature = "v8", feature = "quickjs"))]
        HostRecord::CssSheet(_)
        | HostRecord::CssRuleList(_)
        | HostRecord::CssRule(_)
        | HostRecord::CssKeyframe(_)
        | HostRecord::CssRuleStyle(_)
        | HostRecord::CssMediaList(_)
        | HostRecord::ComputedStyle(_) => return None,
    };
    match params.url {
        Some(owner) => match slab.get(owner)? {
            HostRecord::Url(record) => Some(parse_pairs(record.url.query().unwrap_or(""))),
            HostRecord::EventTarget
            | HostRecord::Params(_)
            | HostRecord::Headers(_)
            | HostRecord::Controller(_)
            | HostRecord::Signal(_)
            | HostRecord::Response(_)
            | HostRecord::Request(_)
            | HostRecord::TextDecoder(_)
            | HostRecord::TextEncoder
            | HostRecord::DomParser
            | HostRecord::XmlSerializer
            | HostRecord::Xhr(_)
            | HostRecord::Blob(_)
            | HostRecord::File(_)
            | HostRecord::Attr(_)
            | HostRecord::FormData(_)
            | HostRecord::Range(_)
            | HostRecord::StaticRange(_)
            | HostRecord::Implementation
            | HostRecord::Storage(_)
            | HostRecord::StyleSheetList
            | HostRecord::DomRect(_)
            | HostRecord::DomRectList(_)
            | HostRecord::Crypto
            | HostRecord::MutationObserver(_)
            | HostRecord::MutationRecord(_) => None,
            #[cfg(any(feature = "v8", feature = "quickjs"))]
            HostRecord::CssSheet(_)
            | HostRecord::CssRuleList(_)
            | HostRecord::CssRule(_)
            | HostRecord::CssKeyframe(_)
            | HostRecord::CssRuleStyle(_)
            | HostRecord::CssMediaList(_)
            | HostRecord::ComputedStyle(_) => None,
        },
        None => Some(params.list.clone()),
    }
}

/// Applies `f` to the pair list of the `URLSearchParams` at `key`, then performs
/// the URL Standard's *update* step: the serialized list becomes the query of
/// the owning URL, or the stored list of a standalone object.
fn mutate_pairs(slab: &mut HostSlab, key: HostKey, f: impl FnOnce(&mut Vec<(String, String)>)) {
    let owner = match slab.get(key) {
        Some(HostRecord::Params(params)) => params.url,
        _ => return,
    };
    let Some(mut list) = host_pairs(slab, key) else {
        return;
    };
    f(&mut list);
    match owner {
        Some(owner) => {
            // `set_query` takes the query without its `?`. The empty list
            // serializes to the empty string, which is the query `''` and not
            // `null` — `url.searchParams.delete(name)` leaves a trailing `?`,
            // as the spec's update step requires.
            let serialized = serialize_pairs(&list);
            if let Some(HostRecord::Url(record)) = slab.get_mut(owner) {
                record.url.set_query(Some(&serialized));
            }
        }
        None => {
            if let Some(HostRecord::Params(params)) = slab.get_mut(key) {
                params.list = list;
            }
        }
    }
}

/// The query object of the `URL` at `key`, creating and caching it on first
/// access — the `[[searchParams]]` slot of the URL Standard.
///
/// The returned handle's bits are stable once the object exists, and a wrapper
/// world keeps one wrapper per handle, so `url.searchParams` hands back the
/// *same* JavaScript object on every read, as `[SameObject]` requires.
fn url_search_params(ctx: &HostContext, key: HostKey) -> Result<Handle<URLSearchParams>, JsError> {
    let mut slab = ctx.host_records.borrow_mut();
    let cached = match slab.get(key) {
        Some(HostRecord::Url(record)) => record.params,
        _ => {
            return Err(JsError::type_error(
                "searchParams: the receiver is not a URL",
            ));
        }
    };
    let params_key = match cached {
        Some(params_key) => params_key,
        None => {
            // The record starts with an empty list: a query object reads its
            // pairs from its URL every time, so there is nothing to seed.
            let params_key = slab.insert(HostRecord::Params(ParamsRecord {
                list: Vec::new(),
                url: Some(key),
            }));
            if let Some(HostRecord::Url(record)) = slab.get_mut(key) {
                record.params = Some(params_key);
            }
            params_key
        }
    };
    Ok(Handle::from_bits(encode_host(ctx.slot, params_key)))
}

/// The link a host record holds to its counterpart, read out before either is
/// touched: a `match` on `slab.get` borrows the slab for the whole match, so the
/// two steps cannot be interleaved.
enum HostLink {
    /// A standalone target has no native incoming owner.
    EventTarget,
    /// The record is a URL holding this `[[searchParams]]` key, if any.
    Url(Option<HostKey>),
    /// The record is a query object holding the key of its URL, if any.
    Params(Option<HostKey>),
    /// The record is a `Headers` holding the key of the `Response` that owns it,
    /// if one does.
    Headers(Option<HostKey>),
    /// The record is an `AbortController` holding its signal's key, always.
    Controller(HostKey),
    /// The record is a `Request` holding its headers' key, always, and the key
    /// of the signal it owns, when it owns one.
    Request(HostKey, Option<HostKey>),
    /// The record is an `AbortSignal` holding its controller's key, while that
    /// controller's record lives.
    Signal(Option<HostKey>),
    /// The record is a `Response` holding its headers' key, always.
    Response(HostKey),
    /// The record is a `TextDecoder`, which links to nothing.
    TextDecoder,
    /// The record is a `TextEncoder`, which links to nothing.
    TextEncoder,
    /// The record is a `DOMParser`, which links to nothing.
    DomParser,
    /// The record is an `XMLSerializer`, which links to nothing.
    XmlSerializer,
    /// The record is an `XMLHttpRequest`, which links to nothing.
    Xhr,
    /// The record is a `Blob`, which links to nothing.
    Blob,
    /// The record is a `File` holding the keys of the `FormData` records whose
    /// entry lists hold it — the reverse of [`HostLink::FormData`], and the
    /// reason a file record is not freed by its wrapper's death alone.
    File(Vec<HostKey>),
    /// The record is a `FormData` holding the distinct keys of the `File`
    /// records its entry list names, which it owns.
    FormData(Vec<HostKey>),
    /// The record is a `DOMImplementation`, which links to nothing.
    Implementation,
    /// The record is an `Attr`, which links to nothing: it names an element by
    /// node reference, and that reference is not a slab key.
    Attr,
    /// A `Range` owns a document registration, released when its record drops.
    Range,
    /// A `StaticRange` owns node handles, released when its record drops.
    StaticRange,
    /// A Storage view links to no host record; the page's native lease owns it.
    Storage,
    /// A rectangle, a rectangle list and the style sheet list are plain data.
    DomRect,
    DomRectList,
    StyleSheetList,
    /// The Window Crypto object has no native graph edge.
    Crypto,
    /// A `MutationObserver` links to no other host record.
    MutationObserver,
    /// A `MutationRecord` links to no other host record.
    MutationRecord,
    /// Constructed CSS records are independently owned by their wrappers.
    #[cfg(any(feature = "v8", feature = "quickjs"))]
    CssSheet,
    #[cfg(any(feature = "v8", feature = "quickjs"))]
    CssRuleList,
    #[cfg(any(feature = "v8", feature = "quickjs"))]
    CssRule,
    #[cfg(any(feature = "v8", feature = "quickjs"))]
    CssKeyframe,
    #[cfg(any(feature = "v8", feature = "quickjs"))]
    CssRuleStyle,
    #[cfg(any(feature = "v8", feature = "quickjs"))]
    CssMediaList,
    /// A computed-style declaration names a node by generational reference.
    #[cfg(any(feature = "v8", feature = "quickjs"))]
    ComputedStyle,
}

/// What freeing host records took out of the world: the record slots that left
/// the slab, and the interned values those records owned.
///
/// The caller needs both. A freed signal's listener list is keyed under the
/// record's own key ([`TargetKey::Host`]), so it has to leave the listener table
/// with it; and the abort reason it interned has to be released, or the page's
/// `reason` would keep a V8 value alive for the life of the world. Neither can
/// be done here — one needs the listener table and the other the world's value
/// table — so this reports what to free and leaves the freeing to the caller.
#[derive(Debug, Default)]
pub struct ReclaimedHost {
    /// The record keys whose slots were freed, in the order they were freed.
    pub keys: Vec<HostKey>,
    /// The interned value bits those records owned and no longer reference.
    pub values: Vec<u64>,
    /// Native observer state the caller drops once no slab borrow is held.
    pub retired: Vec<NativeRetired>,
}

/// Removes the record at `key` and reports what its removal released.
///
/// A `GenSlab::remove` that finds a stale key frees nothing and reports nothing,
/// which is what makes a second reclamation of the same key a no-op rather than
/// a double release.
fn free_record(slab: &mut HostSlab, key: HostKey, out: &mut ReclaimedHost) {
    let Some(record) = slab.remove(key) else {
        return;
    };
    out.keys.push(key);
    // A signal owns its abort reason and a mutation observer its callback; a
    // signal's is only a *stored* one: `null` and `undefined` are sentinels the
    // wrapper world never handed a slot to.
    match record {
        HostRecord::Signal(signal) => {
            if let Some(bits) = signal.reason
                && bits != AnyValue::NULL.0
            {
                out.values.push(bits);
            }
        }
        HostRecord::MutationObserver(observer) => observer.retire(out),
        HostRecord::MutationRecord(record) => record.retire(out),
        // A request holds one reference to the wrapper of the signal it was
        // given.
        HostRecord::Request(request) => {
            if let Some(bits) = request.stored_value() {
                out.values.push(bits);
            }
        }
        // The `response` value an `XMLHttpRequest` last built is its own.
        HostRecord::Xhr(xhr) => {
            if let Some(value) = xhr.response_value {
                out.values.push(value.bits);
            }
        }
        _ => {}
    }
}

/// Frees the host record of a wrapper that has just been finalized, along with
/// the record that wrapper alone was keeping alive, and reports what the caller
/// still has to give back.
///
/// A `Headers` a page built owns nothing else, so it is freed outright, and the
/// two encoding records are the same: a `TextDecoder` owns only its own decoder
/// state and a `TextEncoder` owns nothing at all. An `XMLHttpRequest` is freed
/// outright too — nothing points at its record but its own wrapper — though it is
/// an event target, so its listener entries leave with the key this reports.
/// Two pairs point at each other. A URL and its query object: the object writes
/// every mutation through to its URL, and the URL caches the object's key so
/// that `url.searchParams` can return the same object twice. An `AbortController`
/// and its signal: `[SameObject]` makes every read of `controller.signal` answer
/// with the object the constructor made, so the signal record has to outlive its
/// own wrapper for as long as the controller's record does. A `Response` and the
/// `Headers` it created are the same shape read the other way round: the response
/// *owns* the headers, which every read of `response.headers` answers with.
/// A `FormData` and the `File` records its entries name are the same shape with
/// more than one file and more than one owner: the entry list owns every file it
/// holds, and each file lists the entry lists that hold it.
///
/// Either wrapper of a pair may be collected first, so this clears the link that
/// pointed at the dying record and frees a record only once nothing else refers
/// to it:
///
///  - the query object's wrapper dies: the object is freed, and its URL goes
///    with it when that URL's own wrapper is already gone;
///  - the URL's wrapper dies: the URL keeps its cached key while the object
///    lives on, and is freed at once when no object was ever created;
///  - the signal's wrapper dies: the signal stays while a live controller still
///    points at it, and is freed at once otherwise;
///  - the controller's wrapper dies: the controller's record goes (nothing else
///    can reach it), and the signal goes with it unless the signal's own wrapper
///    is still alive;
///  - the headers' wrapper dies: the headers stay while the response that owns
///    them lives, and are freed at once for a headers a page built;
///  - the response's wrapper dies: the response's record goes, and its headers
///    go with them unless the headers' own wrapper is still alive;
///  - a `File`'s wrapper dies: the file stays while a `FormData` entry list
///    still holds its key, and is freed at once otherwise;
///  - the `FormData`'s wrapper dies: the entry list goes, and every file it
///    held loses this owner and is freed unless another owner holds it.
///
/// The `File`/`FormData` pair is the one where *both* records can be reachable
/// by script at once and neither points at the other exclusively: the entry
/// list holds the file's key, and the file holds the keys of the lists that
/// hold it, so each side's reclamation finishes the other's.
///
/// Freeing a record that is still pointed at would be observable — the object
/// would write into a recycled slot, or a second `abort()` would reach a signal
/// whose state is gone — while clearing a cached key without freeing the record
/// would make the next read a *different* object, which is the one thing
/// `[SameObject]` forbids. Whichever of the two wrappers dies first, the other's
/// reclamation finishes the job, so the two orders converge.
pub fn reclaim_host_record(slab: &mut HostSlab, key: HostKey) -> ReclaimedHost {
    let mut freed = ReclaimedHost::default();
    let link = match slab.get(key) {
        Some(HostRecord::EventTarget) => HostLink::EventTarget,
        Some(HostRecord::Url(record)) => HostLink::Url(record.params),
        Some(HostRecord::Params(record)) => HostLink::Params(record.url),
        Some(HostRecord::Headers(record)) => HostLink::Headers(record.response),
        Some(HostRecord::Controller(record)) => HostLink::Controller(record.signal),
        Some(HostRecord::Signal(record)) => HostLink::Signal(record.controller),
        Some(HostRecord::Response(record)) => HostLink::Response(record.headers),
        Some(HostRecord::Request(record)) => HostLink::Request(record.headers, record.own_signal()),
        Some(HostRecord::TextDecoder(_)) => HostLink::TextDecoder,
        Some(HostRecord::TextEncoder) => HostLink::TextEncoder,
        Some(HostRecord::DomParser) => HostLink::DomParser,
        Some(HostRecord::XmlSerializer) => HostLink::XmlSerializer,
        Some(HostRecord::Xhr(_)) => HostLink::Xhr,
        Some(HostRecord::Blob(_)) => HostLink::Blob,
        Some(HostRecord::File(record)) => HostLink::File(record.owners.clone()),
        Some(HostRecord::FormData(record)) => HostLink::FormData(file_keys(&record.list)),
        Some(HostRecord::Implementation) => HostLink::Implementation,
        Some(HostRecord::Attr(_)) => HostLink::Attr,
        Some(HostRecord::Range(_)) => HostLink::Range,
        Some(HostRecord::StaticRange(_)) => HostLink::StaticRange,
        Some(HostRecord::Storage(_)) => HostLink::Storage,
        Some(HostRecord::StyleSheetList) => HostLink::StyleSheetList,
        Some(HostRecord::DomRect(_)) => HostLink::DomRect,
        Some(HostRecord::DomRectList(_)) => HostLink::DomRectList,
        Some(HostRecord::Crypto) => HostLink::Crypto,
        Some(HostRecord::MutationObserver(_)) => HostLink::MutationObserver,
        Some(HostRecord::MutationRecord(_)) => HostLink::MutationRecord,
        #[cfg(any(feature = "v8", feature = "quickjs"))]
        Some(HostRecord::CssSheet(_)) => HostLink::CssSheet,
        #[cfg(any(feature = "v8", feature = "quickjs"))]
        Some(HostRecord::CssRuleList(_)) => HostLink::CssRuleList,
        #[cfg(any(feature = "v8", feature = "quickjs"))]
        Some(HostRecord::CssRule(_)) => HostLink::CssRule,
        #[cfg(any(feature = "v8", feature = "quickjs"))]
        Some(HostRecord::CssKeyframe(_)) => HostLink::CssKeyframe,
        #[cfg(any(feature = "v8", feature = "quickjs"))]
        Some(HostRecord::CssRuleStyle(_)) => HostLink::CssRuleStyle,
        #[cfg(any(feature = "v8", feature = "quickjs"))]
        Some(HostRecord::CssMediaList(_)) => HostLink::CssMediaList,
        #[cfg(any(feature = "v8", feature = "quickjs"))]
        Some(HostRecord::ComputedStyle(_)) => HostLink::ComputedStyle,
        None => return freed,
    };
    match link {
        // The two encoding records hold no key and own no stored value, so a
        // wrapper's death is the record's death — freed here and nowhere else,
        // exactly once, because the world calls this once per finalized wrapper
        // and `GenSlab::remove` bumps the generation so a second call for the
        // same key finds nothing. An `XMLHttpRequest` is the same shape: nothing
        // else points at its record, and its own state (no stored value, not
        // even an interned one) frees with it. Its *listeners* are keyed by the
        // record's key, so the caller drains them from the freed key. A `Blob`
        // is the same shape again: nothing but its own wrapper can reach it, and
        // it owns no second record.
        HostLink::EventTarget
        | HostLink::TextDecoder
        | HostLink::TextEncoder
        | HostLink::DomParser
        | HostLink::XmlSerializer
        | HostLink::Xhr
        | HostLink::Blob
        | HostLink::Implementation
        | HostLink::Attr
        | HostLink::Range
        | HostLink::StaticRange => {
            free_record(slab, key, &mut freed);
        }
        HostLink::Storage
        | HostLink::DomRect
        | HostLink::DomRectList
        | HostLink::StyleSheetList
        | HostLink::Crypto
        | HostLink::MutationObserver
        | HostLink::MutationRecord => {
            free_record(slab, key, &mut freed);
        }
        #[cfg(any(feature = "v8", feature = "quickjs"))]
        HostLink::CssSheet
        | HostLink::CssRuleList
        | HostLink::CssRule
        | HostLink::CssKeyframe
        | HostLink::CssRuleStyle
        | HostLink::CssMediaList
        | HostLink::ComputedStyle => {
            free_record(slab, key, &mut freed);
        }
        // A file record is the shape a `FormData` entry list makes two-way: its
        // own wrapper's death is not enough to free it while a live `FormData`
        // still holds its key, because `formData.get(name)` has to keep
        // answering with it — the same record, hence the same object. So the
        // flag is cleared here and the record is freed only if no entry list
        // holds it either. The owner keys are read as *live*: a key whose
        // `FormData` record is gone is stale, which is what the generation in
        // the key is for, and `slab.get` answers `None` for it.
        HostLink::File(owners) => {
            if let Some(HostRecord::File(record)) = slab.get_mut(key) {
                record.has_wrapper = false;
            }
            let still_owned = owners.iter().any(|owner| slab.get(*owner).is_some());
            if !still_owned {
                free_record(slab, key, &mut freed);
            }
        }
        // A `FormData` record has no second referent — nothing script can reach
        // leads to it but its own wrapper — so its wrapper's death is its
        // record's death. The `File` records its entry list held are the other
        // half of the pair above: it gives up its ownership of each, and a
        // record that no other entry list and no wrapper holds any more is free
        // with it. The keys are deduplicated because an entry list may name one
        // `File` under several names, and it is one owner either way.
        HostLink::FormData(files) => {
            free_record(slab, key, &mut freed);
            for file in files {
                if let Some(HostRecord::File(record)) = slab.get_mut(file) {
                    record.remove_owner(key);
                }
                if matches!(slab.get(file), Some(HostRecord::File(record)) if record.is_free()) {
                    free_record(slab, file, &mut freed);
                }
            }
        }
        // The headers record a response owns outlives its own wrapper, because
        // `[SameObject]` requires the next read of `response.headers` to answer
        // with the same object: the record goes only when the response that owns
        // it is gone too. A standalone `Headers` has no owner and is freed here.
        HostLink::Headers(response) => {
            if let Some(HostRecord::Headers(record)) = slab.get_mut(key) {
                record.has_wrapper = false;
            }
            let still_owned = response.is_some_and(|response| slab.get(response).is_some());
            if !still_owned {
                free_record(slab, key, &mut freed);
            }
        }
        // The response's record has no second referent — nothing script can
        // reach leads to it but its own wrapper — so its wrapper's death is its
        // record's death. The headers it created become free with it, unless
        // their own wrapper is still alive, in which case script may still hold
        // that object and `headers.get` has to keep working.
        HostLink::Response(headers) => {
            free_record(slab, key, &mut freed);
            if let Some(HostRecord::Headers(record)) = slab.get_mut(headers) {
                record.response = None;
            }
            if matches!(slab.get(headers), Some(HostRecord::Headers(record)) if !record.has_wrapper)
            {
                free_record(slab, headers, &mut freed);
            }
        }
        // A request's record has no second referent either, so its wrapper's
        // death is its record's death, and it owns two records the way a
        // response owns its headers and a controller its signal: each becomes
        // free with it unless its own wrapper is still alive.
        HostLink::Request(headers, signal) => {
            free_record(slab, key, &mut freed);
            if let Some(HostRecord::Headers(record)) = slab.get_mut(headers) {
                record.response = None;
            }
            if matches!(slab.get(headers), Some(HostRecord::Headers(record)) if !record.has_wrapper)
            {
                free_record(slab, headers, &mut freed);
            }
            if let Some(signal) = signal {
                let free_signal = match slab.get_mut(signal) {
                    Some(HostRecord::Signal(record)) => {
                        record.controller = None;
                        !record.has_wrapper
                    }
                    _ => false,
                };
                if free_signal {
                    free_record(slab, signal, &mut freed);
                }
            }
        }
        HostLink::Url(params) => {
            if let Some(HostRecord::Url(record)) = slab.get_mut(key) {
                record.has_wrapper = false;
            }
            // The record is free unless a live query object still writes
            // through it.
            let still_referenced = params.is_some_and(|params| slab.get(params).is_some());
            if !still_referenced {
                free_record(slab, key, &mut freed);
            }
        }
        HostLink::Params(owner) => {
            free_record(slab, key, &mut freed);
            let Some(owner) = owner else {
                return freed;
            };
            if let Some(HostRecord::Url(record)) = slab.get_mut(owner) {
                // The cached key is cleared so a later `url.searchParams`
                // creates a fresh object rather than a handle to a record that
                // is gone.
                record.params = None;
            }
            if matches!(slab.get(owner), Some(HostRecord::Url(record)) if !record.has_wrapper) {
                free_record(slab, owner, &mut freed);
            }
        }
        HostLink::Controller(signal) => {
            // The controller's record has no second referent — the DOM Standard
            // gives `AbortSignal` no `controller` attribute, so nothing script
            // can reach leads back — so its wrapper's death is its record's
            // death.
            free_record(slab, key, &mut freed);
            // The signal it was keeping alive becomes free, unless the signal's
            // own wrapper is still around: then script may still hold the
            // object, and a second `abort()` is no longer reachable through a
            // controller but `aborted`, `reason` and `throwIfAborted()` are.
            let free_signal = match slab.get_mut(signal) {
                Some(HostRecord::Signal(record)) => {
                    record.controller = None;
                    !record.has_wrapper
                }
                _ => false,
            };
            if free_signal {
                free_record(slab, signal, &mut freed);
            }
        }
        HostLink::Signal(controller) => {
            // The signal's wrapper died. Whether the record goes with it is
            // decided by the controller, not by the signal: while the
            // controller's record is alive, `controller.signal` can be read
            // again, and it has to answer with the path's state — a fresh signal
            // would come back un-aborted.
            let free_signal = match slab.get_mut(key) {
                Some(HostRecord::Signal(record)) => {
                    record.has_wrapper = false;
                    // A cleared key is a dead controller: the record's `None` case
                    // and a key whose slot no longer holds that record say the
                    // same thing, which is why the generation in the key is
                    // enough and this does not have to chase the controller
                    // first.
                    controller.is_none_or(|controller| slab.get(controller).is_none())
                }
                _ => false,
            };
            if free_signal {
                free_record(slab, key, &mut freed);
            }
        }
    }
    freed
}

impl URLOps for HostObject {
    fn parse(
        realm: &Realm,
        url: &str,
        base: Option<&str>,
    ) -> Result<Option<Handle<super::URL>>, JsError> {
        // What the constructor throws for, this answers with null.
        let Some(parsed) = parse_api_url(url, base) else {
            return Ok(None);
        };
        let ctx = realm_context(realm)?;
        let key = insert_url(&ctx, parsed);
        Ok(Some(Handle::from_bits(encode_host(ctx.slot, key))))
    }

    fn can_parse(_realm: &Realm, url: &str, base: Option<&str>) -> Result<bool, JsError> {
        Ok(parse_api_url(url, base).is_some())
    }

    fn href(&self) -> Result<String, JsError> {
        Ok(self.with_url(String::new(), |url| url.as_str().to_owned()))
    }

    fn set_href(&self, value: &str) -> Result<(), JsError> {
        // The URL Standard makes this the one setter that throws: the value is
        // parsed, and a parse failure leaves the URL untouched and raises a
        // `TypeError`. That is why `href` carries `[Throws]` in the IDL and the
        // generated glue takes the fallible path for it alone.
        let parsed = url::Url::parse(value).map_err(|_| invalid_url("href"))?;
        self.with_url_mut(|url| *url = parsed);
        Ok(())
    }

    fn origin(&self) -> Result<String, JsError> {
        // An opaque origin serializes as `"null"`, which is what the `url`
        // crate's own serialization returns; a `blob:` URL is the common case.
        Ok(self.with_url(String::from("null"), |url| LocationPart::Origin.read(url)))
    }

    fn protocol(&self) -> Result<String, JsError> {
        Ok(self.with_url(String::new(), |url| LocationPart::Protocol.read(url)))
    }

    fn set_protocol(&self, value: &str) {
        // A silent no-op on failure, as the spec's "return" is: the parser
        // refuses a switch between the special and non-special schemes, a
        // remaining `:` is a failure too (so one trailing `:` is stripped first),
        // and `file:` is refused on any URL that carries an authority.
        let scheme = value.strip_suffix(':').unwrap_or(value);
        self.with_url_mut(|url| {
            let _ = url.set_scheme(scheme);
        });
    }

    fn username(&self) -> Result<String, JsError> {
        Ok(self.with_url(String::new(), |url| url.username().to_owned()))
    }

    fn set_username(&self, value: &str) {
        // The parser percent-encodes with the userinfo set, and refuses a URL
        // that cannot carry credentials at all.
        self.with_url_mut(|url| {
            let _ = url.set_username(value);
        });
    }

    fn password(&self) -> Result<String, JsError> {
        Ok(self.with_url(String::new(), |url| url.password().unwrap_or("").to_owned()))
    }

    fn set_password(&self, value: &str) {
        self.with_url_mut(|url| {
            let _ = url.set_password(Some(value));
        });
    }

    fn host(&self) -> Result<String, JsError> {
        Ok(self.with_url(String::new(), |url| LocationPart::Host.read(url)))
    }

    fn set_host(&self, value: &str) {
        let (host, port) = split_host_port(value);
        // The spec's `host` setter sets the host *and* the port, so a value with
        // no port clears an existing one. A port the parser cannot read fails
        // the whole parse, so the host is not written either — which is why the
        // port is resolved before anything is touched.
        let Some(port) = parse_port(port.unwrap_or("")) else {
            return;
        };
        self.with_url_mut(|url| {
            if url.set_host(Some(host)).is_err() {
                return;
            }
            let _ = url.set_port(port);
        });
    }

    fn hostname(&self) -> Result<String, JsError> {
        Ok(self.with_url(String::new(), |url| LocationPart::Hostname.read(url)))
    }

    fn set_hostname(&self, value: &str) {
        self.with_url_mut(|url| {
            let _ = url.set_host(Some(value));
        });
    }

    fn port(&self) -> Result<String, JsError> {
        Ok(self.with_url(String::new(), |url| LocationPart::Port.read(url)))
    }

    fn set_port(&self, value: &str) {
        self.with_url_mut(|url| set_port(url, value));
    }

    fn pathname(&self) -> Result<String, JsError> {
        Ok(self.with_url(String::new(), |url| LocationPart::Pathname.read(url)))
    }

    fn set_pathname(&self, value: &str) {
        self.with_url_mut(|url| url.set_path(value));
    }

    fn search(&self) -> Result<String, JsError> {
        Ok(self.with_url(String::new(), |url| LocationPart::Search.read(url)))
    }

    fn set_search(&self, value: &str) {
        // Only the *whole* value being empty clears the query; `'?'` on its own
        // leaves a bare one. That asymmetry is the spec's: its setter tests the
        // given value first and then strips one leading `?` from what is left,
        // so `'?'` becomes the empty query rather than a null one.
        if value.is_empty() {
            self.with_url_mut(|url| url.set_query(None));
            return;
        }
        let query = value.strip_prefix('?').unwrap_or(value);
        self.with_url_mut(|url| url.set_query(Some(query)));
    }

    fn search_params(&self) -> Result<Handle<URLSearchParams>, JsError> {
        match self {
            HostObject::Host { ctx, key } => url_search_params(ctx, *key),
            _ => Err(JsError::type_error(
                "searchParams: the receiver is not a URL",
            )),
        }
    }

    fn hash(&self) -> Result<String, JsError> {
        Ok(self.with_url(String::new(), |url| LocationPart::Hash.read(url)))
    }

    fn set_hash(&self, value: &str) {
        // The same asymmetry as `search`: an empty value removes the fragment
        // and its `#`, while `'#'` alone leaves a bare one.
        if value.is_empty() {
            self.with_url_mut(|url| url.set_fragment(None));
            return;
        }
        let fragment = value.strip_prefix('#').unwrap_or(value);
        self.with_url_mut(|url| url.set_fragment(Some(fragment)));
    }

    fn to_string(&self) -> Result<String, JsError> {
        // The derived `toString` of `stringifier attribute USVString href` is
        // the attribute's getter, which is `href` itself — named in full because
        // the element traits declare an `href` of their own.
        <Self as URLOps>::href(self)
    }
}

/// Sets a URL's port from the string a setter was given, following the URL
/// Standard's *port* state.
fn set_port(url: &mut url::Url, value: &str) {
    if let Some(port) = parse_port(value) {
        // `set_port` also drops a port that is the scheme's default, which is
        // the spec's own rule.
        let _ = url.set_port(port);
    }
}

/// The port the URL Standard's *port* state reads out of `value`: `Some(None)`
/// for a value that clears the port, `Some(Some(port))` for one that sets it,
/// and `None` for a value the state rejects.
///
/// The state reads the leading ASCII digits and ignores what follows, so
/// `'8080abc'` sets port 8080 rather than failing; a value whose first character
/// is not a digit fails the state, and the empty string clears the port. A port
/// above `u16::MAX` is the state's own "greater than 2^16 - 1" failure.
fn parse_port(value: &str) -> Option<Option<u16>> {
    if value.is_empty() {
        return Some(None);
    }
    let digits: String = value.chars().take_while(char::is_ascii_digit).collect();
    digits.parse::<u16>().ok().map(Some)
}

/// The URL Standard's *API URL parser* as `new URL()`, `URL.parse()` and
/// `URL.canParse()` share it: `url` against `base` when one is given.
///
/// A relative URL is resolved against the parsed base, so a base that does
/// not itself parse fails. With no base the URL has to be absolute.
fn parse_api_url(url: &str, base: Option<&str>) -> Option<url::Url> {
    match base {
        Some(base) => url::Url::parse(base).and_then(|base| base.join(url)).ok(),
        None => url::Url::parse(url).ok(),
    }
}

/// Makes the record of a `URL` object for `url`.
fn insert_url(ctx: &SharedContext, url: url::Url) -> HostKey {
    ctx.host_records
        .borrow_mut()
        .insert(HostRecord::Url(UrlRecord {
            url,
            params: None,
            has_wrapper: false,
        }))
}

impl URLConstructor for HostObject {
    fn construct(realm: &Realm, url: &str, base: Option<&str>) -> Result<Self, JsError> {
        let parsed = parse_api_url(url, base)
            .ok_or_else(|| JsError::type_error("Failed to construct 'URL': Invalid URL"))?;
        let ctx = realm_context(realm)?;
        let key = insert_url(&ctx, parsed);
        Ok(HostObject::host_record(ctx, key))
    }
}

impl URLSearchParamsOps for HostObject {
    fn size(&self) -> Result<u32, JsError> {
        Ok(self.with_params(0, |list| list.len() as u32))
    }

    fn append(&self, name: &str, value: &str) -> Result<(), JsError> {
        self.with_params_mut(|list| list.push((name.to_owned(), value.to_owned())));
        Ok(())
    }

    fn delete(&self, name: &str, value: Option<&str>) -> Result<(), JsError> {
        // `delete(name)` removes every pair with that name; `delete(name,
        // value)` only the pairs matching both halves.
        self.with_params_mut(|list| match value {
            Some(value) => {
                list.retain(|(pair_name, pair_value)| pair_name != name || pair_value != value)
            }
            None => list.retain(|(pair_name, _)| pair_name != name),
        });
        Ok(())
    }

    fn get(&self, name: &str) -> Result<Option<String>, JsError> {
        Ok(self.with_params(None, |list| {
            list.iter()
                .find(|(pair_name, _)| pair_name == name)
                .map(|(_, value)| value.clone())
        }))
    }

    fn get_all(&self, name: &str) -> Result<Vec<String>, JsError> {
        Ok(self.with_params(Vec::new(), |list| {
            list.iter()
                .filter(|(pair_name, _)| pair_name == name)
                .map(|(_, value)| value.clone())
                .collect()
        }))
    }

    fn has(&self, name: &str, value: Option<&str>) -> Result<bool, JsError> {
        Ok(self.with_params(false, |list| {
            list.iter().any(|(pair_name, pair_value)| {
                pair_name == name && value.is_none_or(|value| pair_value == value)
            })
        }))
    }

    fn set(&self, name: &str, value: &str) -> Result<(), JsError> {
        self.with_params_mut(|list| {
            // The first matching pair keeps its position and takes the new
            // value; the rest are removed, and a name that matched nothing is
            // appended.
            let mut replaced = false;
            list.retain_mut(|(pair_name, pair_value)| {
                if pair_name != name {
                    return true;
                }
                if replaced {
                    return false;
                }
                replaced = true;
                *pair_value = value.to_owned();
                true
            });
            if !replaced {
                list.push((name.to_owned(), value.to_owned()));
            }
        });
        Ok(())
    }

    fn sort(&self) {
        // `sort_by` is stable, which is what makes the spec's "pairs with the
        // same name keep their relative order" hold.
        self.with_params_mut(|list| {
            list.sort_by(|(left, _), (right, _)| cmp_code_units(left, right));
        });
    }

    fn iterable_entries(&self) -> Result<Vec<(String, String)>, JsError> {
        // Re-derived on every iteration, so a `for...of` over
        // `url.searchParams` sees the URL's query as it stands when the
        // iteration begins.
        Ok(self.with_params(Vec::new(), |list| list.to_vec()))
    }

    fn to_string(&self) -> Result<String, JsError> {
        Ok(self.with_params(String::new(), serialize_pairs))
    }
}

impl URLSearchParamsConstructor for HostObject {
    fn construct(realm: &Realm, init: Option<ParamsInit>) -> Result<Self, JsError> {
        let list = match init {
            None => Vec::new(),
            Some(ParamsInit::SequenceOfSequenceOfUsvString(pairs)) => {
                let mut list = Vec::with_capacity(pairs.len());
                for pair in pairs {
                    // The URL Standard throws when an inner sequence does not
                    // hold exactly two items. The generated union converter
                    // collects any length, so the check belongs here.
                    let [name, value] = pair.as_slice() else {
                        return Err(JsError::type_error(
                            "Failed to construct 'URLSearchParams': each item must be a pair",
                        ));
                    };
                    list.push((name.clone(), value.clone()));
                }
                list
            }
            Some(ParamsInit::RecordOfUsvStringToUsvString(pairs)) => pairs,
            // A string is parsed; the record arm is the plain list of pairs the
            // converter already produced.
            Some(ParamsInit::UsvString(init)) => parse_pairs(&init),
        };
        let ctx = realm_context(realm)?;
        let key = ctx
            .host_records
            .borrow_mut()
            .insert(HostRecord::Params(ParamsRecord { list, url: None }));
        Ok(HostObject::host_record(ctx, key))
    }
}

// ---------------------------------------------------------------------------
// Headers
// ---------------------------------------------------------------------------

/// The Fetch Standard's *HTTP whitespace* bytes: exactly the ones its
/// *normalize* step strips from the ends of a header value, and the only bytes
/// that may not sit at either end of one.
fn is_http_whitespace(byte: u8) -> bool {
    matches!(byte, b'\t' | b'\n' | b'\r' | b' ')
}

/// Fetch's *normalize*: the value without its leading and trailing HTTP
/// whitespace.
///
/// A form feed or a no-break space is *not* HTTP whitespace and survives, which
/// is why this cannot be `str::trim` — that would strip a `\x0C` the standard
/// keeps, and would strip by `char` where the standard strips by byte.
fn strip_http_whitespace(value: &[u8]) -> &[u8] {
    let start = value
        .iter()
        .position(|byte| !is_http_whitespace(*byte))
        .unwrap_or(value.len());
    let end = value
        .iter()
        .rposition(|byte| !is_http_whitespace(*byte))
        .map_or(start, |index| index + 1);
    &value[start..end]
}

/// The bytes Fetch's *header value* definition forbids anywhere in a value:
/// NUL and the two HTTP newline bytes, CR and LF.
///
/// Nothing else is out of bounds once the value has been normalized, so this is
/// deliberately not `http::HeaderValue::from_bytes`: that function also rejects
/// every other control byte and `0x7F`, all of which a page is allowed to send
/// and a WPT test asserts survive. Reusing it would turn a legal header into a
/// `TypeError`.
fn is_forbidden_header_value_byte(byte: u8) -> bool {
    matches!(byte, 0x00 | b'\n' | b'\r')
}

/// Validates a name as Fetch's *header name* and byte-lowercases it.
///
/// `http::HeaderName::from_bytes` is exactly RFC 9110's field-name `token`
/// production plus the byte-lowercasing Fetch asks for, so the character table
/// is not restated here. Everything that is Fetch's and not HTTP's is done
/// around it: the error is mapped to a `TypeError`, because that is the only
/// exception the standard raises and the `http` crate's own error type must
/// never reach script. `tenta-net` already depends on this crate, so the
/// validation costs the graph nothing.
fn normalize_header_name(context: &str, name: &[u8]) -> Result<Vec<u8>, JsError> {
    match http::HeaderName::from_bytes(name) {
        Ok(name) => Ok(name.as_str().as_bytes().to_vec()),
        Err(_) => Err(JsError::type_error(format!(
            "{context}: Invalid header name"
        ))),
    }
}

/// Fetch's *header value* check, applied to the value Fetch's *normalize* step
/// has already produced: the leading and trailing HTTP whitespace is stripped,
/// and what remains may not hold a NUL, CR or LF.
fn normalize_header_value(context: &str, value: &[u8]) -> Result<Vec<u8>, JsError> {
    let value = strip_http_whitespace(value);
    if value
        .iter()
        .any(|byte| is_forbidden_header_value_byte(*byte))
    {
        return Err(JsError::type_error(format!(
            "{context}: Invalid header value"
        )));
    }
    Ok(value.to_vec())
}

/// Normalizes and validates the two halves of one header, in the order Fetch
/// checks them: the name first, so a bad name is reported even when the value is
/// bad too.
///
/// Nothing is written to a header list until both halves have passed, which is
/// what keeps a rejected pair from partially mutating anything — a `set` whose
/// value holds an LF leaves the pair it was about to replace exactly as it was.
fn checked_header(context: &str, name: &[u8], value: &[u8]) -> Result<(Vec<u8>, Vec<u8>), JsError> {
    let name = normalize_header_name(context, name)?;
    let value = normalize_header_value(context, value)?;
    Ok((name, value))
}

/// The name `getSetCookie` reads, the one name the iteration view does not
/// combine, and one of the two *forbidden response-header names*
/// ([`is_forbidden_response_header_name`]).
const SET_COOKIE: &[u8] = b"set-cookie";

/// Fetch's *get a header name*: the values of every header of `list` whose name
/// is `name`, joined with `", "`, or `None` when the list holds no such header.
///
/// The comparison is a plain equality rather than a case-insensitive one
/// because both sides are byte-lowercase by the time either reaches here:
/// `name` has been through [`normalize_header_name`], and so has every stored
/// name, which is what makes Fetch's *byte-case-insensitive match* the same
/// thing as `==`.
fn combined_header_value(list: &[(Vec<u8>, Vec<u8>)], name: &[u8]) -> Option<Vec<u8>> {
    let mut combined: Option<Vec<u8>> = None;
    for (pair_name, value) in list {
        if pair_name.as_slice() != name {
            continue;
        }
        match &mut combined {
            Some(combined) => {
                combined.extend_from_slice(b", ");
                combined.extend_from_slice(value);
            }
            None => combined = Some(value.clone()),
        }
    }
    combined
}

/// Fetch's *sort and combine a header list*: the view the pair iterator walks.
///
/// The list itself stays in insertion order — that is the order `getSetCookie`
/// and every mutation work on — so this view is built on demand from it, and
/// rebuilt on each iteration, so a `for...of` sees the list as it stands when
/// the iteration begins. Names are stored byte-lowercased, so Fetch's
/// *sorted-lowercase set* is a plain `sort` and `dedup`, and the sort is the
/// byte order the standard's code-unit order is for an ASCII name.
///
/// `set-cookie` is the one name the standard does not combine: each of its
/// values becomes its own pair, because a value formed by joining two
/// `Set-Cookie` fields is a cookie header no client could parse back. Fetch's
/// `getSetCookie` and this carve-out are two halves of the same decision.
fn sort_and_combine_headers(list: &[(Vec<u8>, Vec<u8>)]) -> HeaderList {
    let mut names: Vec<&[u8]> = list.iter().map(|(name, _)| name.as_slice()).collect();
    names.sort_unstable();
    names.dedup();

    let mut combined = Vec::with_capacity(names.len());
    for name in names {
        if name == SET_COOKIE {
            combined.extend(
                list.iter()
                    .filter(|(pair_name, _)| pair_name.as_slice() == name)
                    .map(|(_, value)| (name.to_vec(), value.clone())),
            );
            continue;
        }
        // `name` came out of `names`, so a header in `list` carries it and a
        // combined value exists.
        if let Some(value) = combined_header_value(list, name) {
            combined.push((name.to_vec(), value));
        }
    }
    combined
}

impl HeadersOps for HostObject {
    fn append(&self, name: &[u8], value: &[u8]) -> Result<(), JsError> {
        let context = "Failed to execute 'append' on 'Headers'";
        let (name, value) = checked_header(context, name, value)?;
        // The pair is moved into the list rather than handed to a closure, which
        // is why the guard checks are spelled out here: a `FnOnce` capturing an
        // owned name could not also be borrowing it as the lookup key.
        if let HostObject::Host { ctx, key } = self
            && let Some(HostRecord::Headers(record)) = ctx.host_records.borrow_mut().get_mut(*key)
            && record.guard.permits_mutation()
            && record.guard.permits_name(&name)
        {
            record.list.push((name, value));
        }
        Ok(())
    }

    fn delete(&self, name: &[u8]) -> Result<(), JsError> {
        let context = "Failed to execute 'delete' on 'Headers'";
        // There is no value to check, so the name check stands alone — and it
        // must, or `headers.delete({})` would be the one member that silently
        // accepted a value WebIDL's `ByteString` conversion let through.
        let name = normalize_header_name(context, name)?;
        self.with_header_mut(&name, |record| {
            record.list.retain(|(pair_name, _)| pair_name != &name);
        });
        Ok(())
    }

    fn get(&self, name: &[u8]) -> Result<Option<Vec<u8>>, JsError> {
        let context = "Failed to execute 'get' on 'Headers'";
        let name = normalize_header_name(context, name)?;
        Ok(self.with_headers(None, |record| combined_header_value(&record.list, &name)))
    }

    fn get_set_cookie(&self) -> Result<Vec<Vec<u8>>, JsError> {
        // The cookie accessor is the one read that is not combined: it reports
        // each `Set-Cookie` field separately, in list order, because a page
        // that reads cookies needs them one per field and `get` would hand back
        // the joined string Fetch forbids.
        Ok(self.with_headers(Vec::new(), |record| {
            record
                .list
                .iter()
                .filter(|(name, _)| name.as_slice() == SET_COOKIE)
                .map(|(_, value)| value.clone())
                .collect()
        }))
    }

    fn has(&self, name: &[u8]) -> Result<bool, JsError> {
        let context = "Failed to execute 'has' on 'Headers'";
        let name = normalize_header_name(context, name)?;
        Ok(self.with_headers(false, |record| {
            record.list.iter().any(|(pair_name, _)| pair_name == &name)
        }))
    }

    fn set(&self, name: &[u8], value: &[u8]) -> Result<(), JsError> {
        let context = "Failed to execute 'set' on 'Headers'";
        let (name, value) = checked_header(context, name, value)?;
        self.with_header_mut(&name, |record| {
            // The first pair with the name takes the new value where it stands;
            // the rest are removed, and a name that matched nothing is appended.
            // Insertion order therefore survives a `set`, which is what the
            // list's order means: the sorted view is produced only for
            // iteration.
            let mut replaced = false;
            record.list.retain_mut(|(pair_name, pair_value)| {
                if pair_name != &name {
                    return true;
                }
                if replaced {
                    return false;
                }
                replaced = true;
                *pair_value = value.clone();
                true
            });
            if !replaced {
                record.list.push((name.clone(), value.clone()));
            }
        });
        Ok(())
    }

    fn iterable_entries(&self) -> Result<HeaderList, JsError> {
        // Every pair view the standard defines is produced here: the generated
        // glue builds `keys`, `values`, `entries`, the spread operator and
        // `@@iterator` from this one hook, and `@@iterator` is the very same
        // function object as `entries`, so the identity holds with no second
        // code path to keep in step.
        Ok(self.with_headers(Vec::new(), |record| sort_and_combine_headers(&record.list)))
    }
}

/// Fetch's *fill a Headers object*: the list `init` builds, under `guard`.
///
/// Every caller of the standard's fill uses this one helper. `Headers`'s
/// constructor fills a list under guard `"none"`, the `Response` constructor's
/// *initialize a response* step 5 is literally "fill response's headers with
/// `init["headers"]`", under guard `"response"`, and the `fetch()` initializer
/// fills the request's own list under guard `"request"` — see
/// [`request_from_init`]. The standard's fill appends *to the Headers object* —
/// the guarded append, not a bare list append — so the guard's per-pair rule
/// applies on every path, and a second copy of the loop is exactly where the
/// paths would drift.
///
/// The list is built locally and returned only once every pair has passed,
/// which is what makes a throw leave nothing half-built behind: the caller
/// installs it in a record afterwards.
///
/// `context` names the caller in the error message, because the same failure
/// (`each item must be a pair`) is reported against whichever interface script
/// called.
fn fill_header_list(
    context: &str,
    init: HeadersInit,
    guard: HeadersGuard,
) -> Result<HeaderList, JsError> {
    let mut list = Vec::new();
    match init {
        HeadersInit::SequenceOfSequenceOfByteString(pairs) => {
            for pair in pairs {
                // Fetch throws when an inner sequence does not hold exactly
                // two items. The generated union converter collects any
                // length, so the check belongs here.
                let [name, value] = pair.as_slice() else {
                    return Err(JsError::type_error(format!(
                        "{context}: each item must be a pair"
                    )));
                };
                let (name, value) = checked_header(context, name, value)?;
                if guard.permits_pair(&name, &value) {
                    list.push((name, value));
                }
            }
        }
        // The record arm arrives with its keys already read through the
        // declared `ByteString` key type, which is what turns a key above
        // `0xFF` into a `TypeError` before this code is reached.
        HeadersInit::RecordOfByteStringToByteString(pairs) => {
            for (name, value) in pairs {
                let (name, value) = checked_header(context, &name, &value)?;
                if guard.permits_pair(&name, &value) {
                    list.push((name, value));
                }
            }
        }
    }
    Ok(list)
}

impl HeadersConstructor for HostObject {
    fn construct(realm: &Realm, init: Option<HeadersInit>) -> Result<Self, JsError> {
        let context = "Failed to construct 'Headers'";
        let list = match init {
            Some(init) => fill_header_list(context, init, HeadersGuard::None)?,
            None => Vec::new(),
        };
        let ctx = realm_context(realm)?;
        let key = ctx
            .host_records
            .borrow_mut()
            .insert(HostRecord::Headers(HeadersRecord {
                list,
                guard: HeadersGuard::None,
                response: None,
                has_wrapper: false,
            }));
        Ok(HostObject::host_record(ctx, key))
    }
}

// ---------------------------------------------------------------------------
// AbortController / AbortSignal
// ---------------------------------------------------------------------------

impl AbortControllerConstructor for HostObject {
    fn construct(realm: &Realm) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        // The constructor makes the signal and links the pair eagerly — the
        // spec's own steps ("let signal be a new `AbortSignal` object", "set
        // this's signal to signal") — so `signal` is a read of a key that
        // already exists rather than a lazy creation, and `[SameObject]` holds
        // from the first read.
        let mut records = ctx.host_records.borrow_mut();
        let signal = records.insert(HostRecord::Signal(SignalRecord {
            reason: None,
            controller: None,
            has_wrapper: false,
        }));
        let controller = records.insert(HostRecord::Controller(ControllerRecord { signal }));
        if let Some(HostRecord::Signal(record)) = records.get_mut(signal) {
            record.controller = Some(controller);
        }
        drop(records);
        Ok(HostObject::host_record(ctx, controller))
    }
}

impl AbortControllerOps for HostObject {
    fn signal(&self) -> Result<Handle<AbortSignal>, JsError> {
        self.with_controller(None, |record| Some(record.signal))
            .map(|signal| Handle::from_bits(encode_host(self.slot(), signal)))
            // The template's signature brand check makes this unreachable: only a
            // controller wrapper can reach the getter, and the record it was
            // built over lives exactly as long as that wrapper does.
            .ok_or_else(|| JsError::type_error("signal: the receiver is not an AbortController"))
    }

    fn abort(&self, reason: Option<AnyValue>) -> Result<(), JsError> {
        let HostObject::Host { ctx, key } = self else {
            return Ok(());
        };
        let controller = *key;
        // The signal this controller aborts. Read under one short borrow, which
        // ends before anything below can fire an event, so a listener reached
        // through `dispatch_at` never runs under a live borrow of the slab.
        let signal = {
            let records = ctx.host_records.borrow();
            match records.get(controller) {
                Some(HostRecord::Controller(record)) => record.signal,
                _ => return Ok(()),
            }
        };

        // Gives back the reference to the argument's stored value. A reason the
        // page passed arrived as bits the glue already stored (+1 reference), so
        // the call owns it; `null` is the prelude's sentinel and holds none.
        let give_back = |bits: u64| {
            if bits != AnyValue::NULL.0 {
                self.release_stored(bits);
            }
        };

        // Step 1 of the DOM Standard's *signal abort*: "If signal is aborted,
        // then return." This is what makes the first abort the only one that
        // counts, and it is deliberately read *before* the default reason is
        // built: step 2 runs the `DOMException` constructor, so returning first
        // is the difference between a no-op second `abort()` and one that calls
        // that constructor and throws the result away.
        let already_aborted = matches!(
            ctx.host_records.borrow().get(signal),
            Some(HostRecord::Signal(record)) if record.reason.is_some()
        );
        if already_aborted {
            if let Some(value) = reason {
                give_back(value.0);
            }
            return Ok(());
        }

        // Step 2: "Set signal's abort reason to reason if it is given; otherwise
        // to a new `AbortError` `DOMException`."
        //
        // `undefined` never reaches here as a reason: the generated glue maps an
        // absent *and* an explicitly `undefined` argument to `None`, which is
        // what makes `abort(undefined)` produce an `AbortError` as the WPT test
        // requires. `null` does arrive, as the prelude's own sentinel.
        let reason = match reason {
            Some(value) => Some(value.0),
            None => ctx
                .script
                .borrow()
                .as_ref()
                .and_then(|script| {
                    script.build_value(HostValue::DomException {
                        name: SignalRecord::DEFAULT_REASON_NAME,
                        message: SignalRecord::DEFAULT_REASON_MESSAGE,
                    })
                })
                // No script host means no context to build a `DOMException` in,
                // and none to have called `abort()` either; the signal is still
                // marked aborted, with the `null` sentinel standing in for the
                // reason that could not be built.
                .or(Some(AnyValue::NULL.0)),
        };

        // Storing the reason *is* aborting the signal, because the standard
        // defines the two as the same fact. The test of step 1 is repeated here,
        // atomically with the store: the default reason above was built by
        // running the `DOMException` constructor, which is script, so a page that
        // replaced that constructor with one that re-enters `abort()` is the one
        // caller this second test exists for.
        let stored = {
            let mut records = ctx.host_records.borrow_mut();
            match records.get_mut(signal) {
                Some(HostRecord::Signal(record)) if record.reason.is_none() => {
                    record.reason = reason;
                    true
                }
                _ => false,
            }
        };
        if !stored {
            // Lost the race, or the signal's record went with a collected
            // wrapper. Either way nothing below runs and the reference the
            // argument came with is nobody's.
            if let Some(bits) = reason {
                give_back(bits);
            }
            return Ok(());
        }

        // The abort algorithms `fetch()` added: each request that was given
        // this signal has its promise rejected with the reason and its
        // transport ended. They run before the event, as the abort steps
        // order them, so an `abort` listener already sees the rejection queued.
        let aborted: Vec<u64> = {
            let mut fetches = ctx.fetch_signals.borrow_mut();
            let resolvers: Vec<u64> = fetches
                .iter()
                .filter(|(_, key)| *key == signal)
                .map(|(resolver, _)| *resolver)
                .collect();
            fetches.retain(|(_, key)| *key != signal);
            resolvers
        };
        if !aborted.is_empty() {
            let script = ctx.script.borrow().as_ref().map(Rc::clone);
            if let (Some(script), Some(bits)) = (script, reason) {
                for resolver in aborted {
                    script.abort_fetch(resolver);
                    script.reject_promise_with_stored(resolver, bits);
                }
            }
        }

        // Steps 3 and 4: `dependentSignalsToAbort`. `AbortSignal.any` is the
        // only producer of a dependent signal and it is deferred, so the list is
        // always empty here — the steps are not reordered around it, they simply
        // have nothing to iterate.

        // Step 5: "Run the abort steps for signal": run the abort algorithms
        // (none are registered yet), empty them, and fire an event named `abort`
        // at signal.
        //
        // The event is UA-created, so `isTrusted` is true; "fire an event named
        // e at target" gives it no bubbles, no cancelable and no default action,
        // and the signal is already aborted while a listener runs. It is fired
        // here, on this stack, which is what makes a listener added before the
        // call run before `abort()` returns.
        let mut record = EventRecord::new("abort".to_owned(), EventKind::Plain, ctx.time_stamp());
        record.flags.initialized = true;
        record.is_trusted = true;
        let event_key = ctx.events.borrow_mut().insert(record);
        let dispatched = dispatch_at(ctx, DispatchTarget::host(ctx.slot, signal), event_key, true);
        // Remove the transient record unless a listener made a wrapper for it
        // (in which case the world's dead-event sweep owns its removal).
        let orphaned = ctx
            .events
            .borrow()
            .get(event_key)
            .is_some_and(|record| !record.has_wrapper);
        if orphaned {
            ctx.events.borrow_mut().remove(event_key);
        }
        dispatched.map(|_| ())
    }
}

impl AbortSignalOps for HostObject {
    fn abort(realm: &Realm, reason: Option<AnyValue>) -> Result<Handle<AbortSignal>, JsError> {
        let ctx = realm_context(realm)?;
        // "Set signal's abort reason to reason if it is given; otherwise to a
        // new `AbortError` `DOMException`", as `AbortController.abort()` does.
        // A reason the page passed arrived stored, and the record takes that
        // reference over.
        let reason = match reason {
            Some(value) => value.0,
            None => ctx
                .script
                .borrow()
                .as_ref()
                .and_then(|script| {
                    script.build_value(HostValue::DomException {
                        name: SignalRecord::DEFAULT_REASON_NAME,
                        message: SignalRecord::DEFAULT_REASON_MESSAGE,
                    })
                })
                .unwrap_or(AnyValue::NULL.0),
        };
        // The signal has no controller, so nothing can abort it again and no
        // event is owed: it was never not aborted.
        let key = ctx
            .host_records
            .borrow_mut()
            .insert(HostRecord::Signal(SignalRecord {
                reason: Some(reason),
                controller: None,
                has_wrapper: false,
            }));
        Ok(Handle::from_bits(encode_host(ctx.slot, key)))
    }

    fn aborted(&self) -> bool {
        // "An `AbortSignal` object is aborted when its abort reason is not
        // `undefined`" — so this and `reason` read one field and cannot
        // disagree.
        self.with_signal(false, |record| record.reason.is_some())
    }

    fn reason(&self) -> Result<AnyValue, JsError> {
        // The stored bits are returned as they are: `value_to_v8` loads the same
        // stored value every time, so `signal.reason === signal.reason` holds by
        // identity for an object reason and not merely by value.
        Ok(match self.with_signal(None, |record| record.reason) {
            Some(bits) => AnyValue(bits),
            None => self.context().undefined_value(),
        })
    }

    fn throw_if_aborted(&self) -> Result<(), JsError> {
        let Some(bits) = self.with_signal(None, |record| record.reason) else {
            return Ok(());
        };
        // The reason is thrown as the very value it is, not as a copy and not as
        // a `JsError`: `assert_throws_exactly` compares the thrown value with
        // `===` against `signal.reason`, so a default `AbortError` has to be the
        // same `DOMException` object `reason` returns.
        if let Some(script) = self.context().script.borrow().as_ref() {
            script.throw_value(bits);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Response
// ---------------------------------------------------------------------------

/// The header name *initialize a response* looks for before appending the type
/// of a body taken from a `USVString`.
const CONTENT_TYPE: &[u8] = b"content-type";

/// The `TypeError` message a `Body` reader rejects with when its object is
/// *unusable*. The standard states the exception and no message, so this text is
/// the engine's own.
const BODY_UNUSABLE: &str = "The body has already been read";

/// The type *extract a body with type* gives a `USVString`
/// (`<https://fetch.spec.whatwg.org/#concept-bodyinit-extract>`): the UTF-8
/// encoding of the string, with this type. A buffer source contributes its bytes
/// and no type at all, which is why only the string arm reaches
/// [`CONTENT_TYPE`].
const TEXT_PLAIN_UTF8: &str = "text/plain;charset=UTF-8";

/// The type *extract a body with type* gives a `URLSearchParams`.
const FORM_URLENCODED_UTF8: &str = "application/x-www-form-urlencoded;charset=UTF-8";

/// Fetch's *extract a body with type*, over the union this engine's `BodyInit`
/// holds: the bytes, and the `Content-Type` the arm infers, or `None` for an
/// arm that infers none
/// (`<https://fetch.spec.whatwg.org/#concept-bodyinit-extract>`).
///
/// This is the whole of the rule, so it is **one function and not one per
/// caller**. Its three callers are the `Response` constructor's step 4, the
/// `fetch()` initializer's body step and `send()`'s; each then writes the type
/// into its own header list when it is non-null and that list has no
/// `Content-Type`, which is the second half of the standard's step and belongs
/// to each caller's own list ([`set_inferred_content_type`]).
///
/// Per arm:
///
///  - a `Blob` is its bytes, with its `type` when that is not the empty string;
///  - a buffer source is a copy of its bytes and no type;
///  - a `FormData` is the `multipart/form-data` encoding of its entry list with
///    a boundary made for this body, and the type names that boundary;
///  - a `URLSearchParams` is its `application/x-www-form-urlencoded`
///    serialization;
///  - a string is its UTF-8 encoding, as `text/plain`.
///
/// A handle that names no record of its interface cannot come out of the
/// generated conversion, which brand-checks each interface arm, so the error is
/// defensive.
fn extract_body(ctx: &SharedContext, body: BodyInit) -> Result<(Vec<u8>, Option<String>), JsError> {
    let not_a = |interface: &str| {
        JsError::type_error(format!(
            "the body is not the {interface} it was converted as"
        ))
    };
    match body {
        BodyInit::Blob(handle) => {
            let (bytes, blob_type) = blob_of(ctx, handle).ok_or_else(|| not_a("Blob"))?;
            Ok((bytes, (!blob_type.is_empty()).then_some(blob_type)))
        }
        BodyInit::BufferSource(bytes) => Ok((bytes, None)),
        BodyInit::FormData(handle) => {
            let boundary = forms::multipart_boundary();
            let records = ctx.host_records.borrow();
            let Some(HostRecord::FormData(record)) = host_record_of(&records, handle.bits()) else {
                return Err(not_a("FormData"));
            };
            let bytes = multipart_form_data(&records, &record.list, &boundary);
            Ok((
                bytes,
                Some(format!("multipart/form-data; boundary={boundary}")),
            ))
        }
        BodyInit::URLSearchParams(handle) => {
            let records = ctx.host_records.borrow();
            let Some(HostRecord::Params(record)) = host_record_of(&records, handle.bits()) else {
                return Err(not_a("URLSearchParams"));
            };
            Ok((
                serialize_pairs(&record.list).into_bytes(),
                Some(FORM_URLENCODED_UTF8.to_owned()),
            ))
        }
        BodyInit::UsvString(text) => Ok((text.into_bytes(), Some(TEXT_PLAIN_UTF8.to_owned()))),
    }
}

/// Whether `response` names a `Response` that has a body, which is what
/// decides between a stream and `null` for `Response.prototype.body`
/// (`crate::streams`). `None` for a handle that names no `Response`.
pub fn response_has_body(ctx: &SharedContext, response: Handle<Response>) -> Option<bool> {
    match host_record_of(&ctx.host_records.borrow(), response.bits()) {
        Some(HostRecord::Response(record)) => Some(record.body.is_some()),
        _ => None,
    }
}

/// The record handle `bits` name, when they name a host record at all.
///
/// The tag is checked before the key is read: a handle that is not a host
/// handle has no key, and reading one would ask for an unrelated slot.
fn host_record_of(records: &HostSlab, bits: u64) -> Option<&HostRecord> {
    let decoded = decode(bits);
    (decoded.tag == TAG_HOST)
        .then(|| records.get(decoded.host_key()))
        .flatten()
}

/// The `multipart/form-data` encoding of a `FormData` entry list
/// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#multipart-form-data>).
///
/// A string entry's line breaks are written as CRLF. A file entry carries its
/// name as the `filename`, its type — `application/octet-stream` when it has
/// none — and its bytes untouched. An entry whose `File` record is gone is
/// skipped; the entry list is what keeps that record alive, so there is none.
fn multipart_form_data(
    records: &HostSlab,
    list: &[(String, EntryValue)],
    boundary: &str,
) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, value) in list {
        let name = tenta_dom::forms::multipart_name(&normalize_crlf(name));
        match value {
            EntryValue::String(text) => {
                out.extend_from_slice(
                    format!(
                        "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n"
                    )
                    .as_bytes(),
                );
                out.extend_from_slice(normalize_crlf(text).as_bytes());
            }
            EntryValue::File(key) => {
                let Some(HostRecord::File(file)) = records.get(*key) else {
                    continue;
                };
                let filename = tenta_dom::forms::multipart_name(&file.name);
                let content_type = match file.blob.blob_type.as_str() {
                    "" => "application/octet-stream",
                    content_type => content_type,
                };
                out.extend_from_slice(
                    format!(
                        "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; \
                         filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n"
                    )
                    .as_bytes(),
                );
                out.extend_from_slice(&file.blob.bytes);
            }
        }
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    out
}

/// `text` with every line break — a lone CR, a lone LF or a CRLF — written as
/// CRLF, which is how the multipart encoding writes names and string values.
fn normalize_crlf(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push_str("\r\n");
            }
            '\n' => out.push_str("\r\n"),
            c => out.push(c),
        }
    }
    out
}

/// Writes `Content-Type: body_type` into `headers` when [`extract_body`] inferred
/// a type for the body and the list does not already name the header.
///
/// This is the second half of every body step in this file, and it is **one
/// function for the same reason [`extract_body`] is one**: three sites run it —
/// the `Response` constructor's step 6, the `fetch()` initializer's body step and
/// `send()`'s body step — and a copy per site is exactly how the three drift. The
/// two standards spell the write differently: Fetch's `Request` constructor
/// *appends* (`Content-Type`, *type*) to the request's headers
/// (<https://fetch.spec.whatwg.org/#dom-request>), while XHR's `send()` *sets* it
/// in the author request headers (<https://xhr.spec.whatwg.org/#the-send()-method>,
/// step 4). Both are conditioned on the list not already holding the name, so for
/// this engine they are one rule.
///
/// A list that already has the name keeps it, under any casing — every header name
/// this file stores is byte-lowercased. That condition is what makes an author
/// `setRequestHeader('Content-Type', …)` survive the body step.
///
/// A *buffer source* body infers no type at all, so it appends none; `None` is
/// therefore a no-op rather than a mistake.
fn set_inferred_content_type(headers: &mut HeaderList, body_type: Option<&str>) {
    let Some(body_type) = body_type else {
        return;
    };
    if headers
        .iter()
        .any(|(name, _)| name.as_slice() == CONTENT_TYPE)
    {
        return;
    }
    // A list write, not a guarded append: both standards write the pair straight
    // into the header list, and `Content-Type` is not a name either guard would
    // refuse anyway.
    headers.push((CONTENT_TYPE.to_vec(), body_type.as_bytes().to_vec()));
}

/// The Encoding Standard's *UTF-8 decode* as this engine models it: the bytes
/// read as UTF-8 with U+FFFD for every malformed sequence, and a leading U+FEFF
/// dropped.
///
/// Both halves are the standard's. "UTF-8 decode" is a lossy decode — that is
/// what makes it a *decode* rather than a parse — and it begins by reading a BOM
/// if there is one. The BOM rule is the same one [`TextDecoderRecord::serialize`]
/// applies by default ("*ignore BOM* and *BOM seen* are both false, so the first
/// scalar of the output is dropped when it is U+FEFF"), restated only because
/// that rule lives on the decoder record and a `Body` reader has none.
///
/// The two `Body` readers both need it: `text()` resolves with its result, and
/// `json()` parses it, because the standard's "parse JSON from bytes" is a UTF-8
/// decode followed by a parse.
pub fn utf8_decode(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    match text.strip_prefix('\u{FEFF}') {
        Some(text) => text.to_owned(),
        None => text.into_owned(),
    }
}

/// The `Response` record's own fields, before the headers record it owns exists.
///
/// A parts struct rather than a longer argument list so that the insert
/// sequence below is shared by both builders: the `Response` constructor and a
/// completed fetch differ only in these values.
struct NewResponse {
    status: u16,
    status_text: Vec<u8>,
    /// `None` for a constructor-built response, which has no URL.
    url: Option<String>,
    response_type: ResponseType,
    redirected: bool,
    body: Option<Vec<u8>>,
}

/// Inserts the headers record, then the response record that owns it, links the
/// two both ways, and returns the response's key.
///
/// This is the one place the pair is created. A second copy of the sequence is
/// where the two-way link or the `has_wrapper` flag would drift, and the link is
/// what reclamation reads: the response finds its headers to read, and
/// [`reclaim_host_record`] finds the response that keeps a headers record alive.
fn insert_response(ctx: &SharedContext, list: HeaderList, parts: NewResponse) -> HostKey {
    let mut records = ctx.host_records.borrow_mut();
    let headers = records.insert(HostRecord::Headers(HeadersRecord {
        list,
        guard: HeadersGuard::Response,
        response: None,
        has_wrapper: false,
    }));
    let key = records.insert(HostRecord::Response(ResponseRecord {
        status: parts.status,
        status_text: parts.status_text,
        url: parts.url,
        response_type: parts.response_type,
        redirected: parts.redirected,
        headers,
        body: parts.body,
        body_used: false,
    }));
    if let Some(HostRecord::Headers(record)) = records.get_mut(headers) {
        record.response = Some(key);
    }
    key
}

/// Inserts the `Response` record a completed fetch delivers, returning its key.
///
/// It lives here rather than beside the driver's own types because the record
/// is `tenta-js`'s: the driver hands over plain data
/// ([`FetchedResponse`]) and never names a host record.
///
/// # Deliberate limits
///
/// - The response is a *basic* response. A cross-origin request is refused
///   before it is sent (`docs/dev/idl.md` records the deviation and the slice
///   that removes it), so there is no filtered header list to model and no
///   reason to report a type other than the truthful one.
/// - `status_text` is [`http::StatusCode::canonical_reason`]'s text, not the
///   reason phrase the server sent: `tenta-net`'s response carries no
///   reason-phrase field, so the wire's is not available at this boundary. A
///   status with no canonical reason reads back as the empty string.
/// - A header pair is **dropped** rather than faulted when it cannot be
///   delivered: a name Fetch calls a *forbidden response-header name*
///   (`Set-Cookie`, `Set-Cookie2`), a name that is not an HTTP field name, and a
///   value holding a NUL, CR or LF. The constructor path drops the first of
///   those three through the guard and *throws* for the other two, because a
///   page wrote them; a fetched header was written by a server, and failing the
///   whole response over one malformed pair would turn a header into a rejected
///   promise.
pub fn insert_fetched_response(ctx: &SharedContext, data: FetchedResponse) -> HostKey {
    let context = "fetch";
    let FetchedResponse {
        url,
        status,
        redirected,
        headers,
        body,
    } = data;

    let status_text = http::StatusCode::from_u16(status)
        .ok()
        .and_then(|status| status.canonical_reason())
        .map_or_else(Vec::new, |reason| reason.as_bytes().to_vec());

    let list: HeaderList = headers
        .into_iter()
        .filter_map(|(name, value)| {
            let (name, value) = checked_header(context, name.as_bytes(), &value).ok()?;
            (!is_forbidden_response_header_name(&name)).then_some((name, value))
        })
        .collect();

    // A null-body status carries no body. Fetch enforces this when it creates
    // the response, so a driver that hands one over anyway is corrected here
    // rather than trusted — and `body_used` stays false for it, which is what
    // makes `bodyUsed` false and `text()` resolve with the empty string.
    let body = match ResponseRecord::is_null_body_status(status) {
        true => None,
        false => body,
    };

    insert_response(
        ctx,
        list,
        NewResponse {
            status,
            status_text,
            url: Some(url),
            response_type: ResponseType::Basic,
            redirected,
            body,
        },
    )
}

/// Builds the `Response` of a `no-cors` request to another origin: an *opaque
/// filtered response*, which tells script nothing about what was answered
/// (<https://fetch.spec.whatwg.org/#concept-filtered-response-opaque>).
pub fn insert_opaque_response(ctx: &SharedContext) -> HostKey {
    insert_response(
        ctx,
        HeaderList::default(),
        NewResponse {
            status: 0,
            status_text: Vec::new(),
            url: Some(String::new()),
            response_type: ResponseType::Opaque,
            redirected: false,
            body: None,
        },
    )
}

impl ResponseConstructor for HostObject {
    fn construct(
        realm: &Realm,
        body: Option<Option<BodyInit>>,
        init: Option<ResponseInit>,
    ) -> Result<Self, JsError> {
        let context = "Failed to construct 'Response'";
        let ResponseInit {
            status,
            status_text,
            headers,
        } = init.unwrap_or_default();

        // The constructor's steps 1 and 2 make the response and its headers
        // object before `initialize a response` can throw, but neither is
        // observable once the construction has failed: the object under
        // construction is discarded, and its headers object was reachable only
        // through it. Every check therefore runs before the first record is
        // inserted, so a rejected construction leaves the slab exactly as it
        // found it instead of leaking an ownerless record.
        //
        // Step 1 of *initialize a response*.
        if !(200..=599).contains(&status) {
            return Err(JsError::range_error(format!(
                "{context}: the status provided ({status}) is outside the range [200, 599]"
            )));
        }
        // Step 2. The empty string is allowed; anything else has to be a
        // *reason-phrase*.
        if !status_text.is_empty() && !ResponseRecord::is_reason_phrase(&status_text) {
            return Err(JsError::type_error(format!(
                "{context}: the status text is not a reason phrase"
            )));
        }

        // Step 4 of the constructor: *extract a body with type*. The outer
        // `None` is a missing argument and the inner one the IDL's nullable
        // `body`; both are the null body step 3 of the constructor keeps. The
        // rule itself is [`extract_body`], shared with the `fetch()`
        // initializer's body step.
        let body = match body {
            Some(Some(body)) => Some(extract_body(&realm_context(realm)?, body)?),
            Some(None) | None => None,
        };

        // Step 5: "fill response's headers with init["headers"]", which is the
        // same fill `Headers`'s constructor runs — guard rule included, so a
        // `Set-Cookie` here is dropped exactly as `headers.append` drops one.
        let mut list = match headers {
            Some(headers) => fill_header_list(context, headers, HeadersGuard::Response)?,
            None => Vec::new(),
        };

        // Step 6.
        let mut body_bytes = None;
        if let Some((bytes, body_type)) = body {
            if ResponseRecord::is_null_body_status(status) {
                return Err(JsError::type_error(format!(
                    "{context}: a response with a null body status cannot have a body"
                )));
            }
            set_inferred_content_type(&mut list, body_type.as_deref());
            body_bytes = Some(bytes);
        }

        // Steps 1 and 2 of the constructor, once nothing can throw: the headers
        // object is created first, because the response holds its key.
        let ctx = realm_context(realm)?;
        let key = insert_response(
            &ctx,
            list,
            NewResponse {
                status,
                status_text,
                // A constructor-built response has no URL and no URL list, so
                // `url` is the empty string and `redirected` is false. `type` is
                // `"default"`; the other five values name responses only a fetch
                // produces.
                url: None,
                response_type: ResponseType::Default,
                redirected: false,
                body: body_bytes,
            },
        );
        Ok(HostObject::host_record(ctx, key))
    }
}

impl HostObject {
    /// The script host of this world, which is what can make a promise.
    ///
    /// A promise needs an isolate, and a host record has none; the callback
    /// boundary is the only route, exactly as it is for a listener's invocation
    /// or a value the host has to settle a promise with.
    ///
    /// The message names no member: this is the shared half of `fetch`'s
    /// *consume body* and of `Blob.text()`, and every host that reaches it has
    /// already been entered through a wrapper, which is what installs a script
    /// host in the first place.
    fn script_host(&self) -> Result<Rc<dyn ScriptHost>, JsError> {
        self.context()
            .script
            .borrow()
            .as_ref()
            .map(Rc::clone)
            .ok_or_else(|| JsError::type_error("this world has no script host"))
    }

    /// Fetch's *consume body* for a `Body` member, up to but not including
    /// `convertBytesToJSValue`: the body bytes the member should convert, or the
    /// message its promise must be rejected with.
    ///
    /// An `Err` is step 1 of *consume body*: the object is *unusable* — its body
    /// is non-null and already read — and the promise is rejected before any
    /// conversion runs. Marking the body read and reading it happen under one
    /// borrow, so the second call of a member cannot observe a state the first
    /// did not leave: the disturbance *is* synchronously observable through
    /// `bodyUsed` as soon as a read returns, which is what
    /// `Promise.resolve().then(() => r.text())` relies on.
    ///
    /// Steps 5 and 6 are the same statement here. The standard's "fully read
    /// object's body" is asynchronous only because a stream is; the body is
    /// bytes, so a non-null body is already read and a null one is the empty
    /// byte sequence successSteps is run with.
    fn take_body(&self) -> Result<Vec<u8>, &'static str> {
        self.with_body(Ok(Vec::new()), |body, used| {
            if body.is_some() && *used {
                return Err(BODY_UNUSABLE);
            }
            if body.is_some() {
                *used = true;
            }
            Ok(body.clone().unwrap_or_default())
        })
    }

    /// Runs `f` over the body and the *disturbed* flag of the `Response` or the
    /// `Request` this host stands for, the two objects that include `Body`, or
    /// returns `default` for any other host.
    fn with_body<R>(&self, default: R, f: impl FnOnce(&mut Option<Vec<u8>>, &mut bool) -> R) -> R {
        let HostObject::Host { ctx, key } = self else {
            return default;
        };
        match ctx.host_records.borrow_mut().get_mut(*key) {
            Some(HostRecord::Response(record)) => f(&mut record.body, &mut record.body_used),
            Some(HostRecord::Request(record)) => f(&mut record.body, &mut record.body_used),
            _ => default,
        }
    }

    /// The header list of the `Response` or the `Request` this host stands for.
    fn body_headers(&self) -> Option<HostKey> {
        let HostObject::Host { ctx, key } = self else {
            return None;
        };
        match ctx.host_records.borrow().get(*key) {
            Some(HostRecord::Response(record)) => Some(record.headers),
            Some(HostRecord::Request(record)) => Some(record.headers),
            _ => None,
        }
    }

    /// `Body`'s `bodyUsed`: a null body is never disturbed.
    fn body_is_used(&self) -> bool {
        self.with_body(false, |body, used| body.is_some() && *used)
    }

    /// `Body`'s `json()`.
    fn body_json(&self) -> Result<Promise<AnyValue>, JsError> {
        // "consume body with this and parse JSON from bytes". The parse is the
        // *settlement's* work rather than the host's: it needs the isolate, and
        // its result is an arbitrary object graph, so it is the one conversion
        // that cannot be expressed as a `HostValue`.
        self.consume_body(|bytes| Ok(PromiseOutcome::ParseJson(bytes)))
    }

    /// `Body`'s `text()`.
    fn body_text(&self) -> Result<Promise<String>, JsError> {
        // "consume body with this and UTF-8 decode".
        self.consume_body(|bytes| {
            Ok(PromiseOutcome::Resolve(HostValue::String(utf8_decode(
                bytes,
            ))))
        })
    }

    /// `Body`'s `arrayBuffer()`.
    fn body_array_buffer(&self) -> Result<Promise<Vec<u8>>, JsError> {
        // "consume body with this and the following step given a byte sequence
        // bytes: return the result of creating an ArrayBuffer from bytes".
        self.consume_body(|bytes| {
            Ok(PromiseOutcome::Resolve(HostValue::ArrayBuffer(
                bytes.to_vec(),
            )))
        })
    }

    /// `Body`'s `bytes()`: the same bytes as a `Uint8Array`.
    fn body_bytes(&self) -> Result<Promise<Vec<u8>>, JsError> {
        self.consume_body(|bytes| {
            Ok(PromiseOutcome::Resolve(HostValue::Uint8Array(
                bytes.to_vec(),
            )))
        })
    }

    /// `Body`'s `blob()`.
    fn body_blob(&self) -> Result<Promise<Handle<Blob>>, JsError> {
        // "return a Blob whose contents are bytes and whose type attribute is
        // this's MIME type". The type is read before the body is consumed; the
        // record is made inside the conversion, so an unusable body, which
        // never reaches it, leaves no record behind.
        let HostObject::Host { ctx, .. } = self else {
            return Err(JsError::type_error("blob: the receiver has no body"));
        };
        let blob_type = self
            .body_headers()
            .and_then(|headers| match ctx.host_records.borrow().get(headers) {
                Some(HostRecord::Headers(record)) => body_mime_type(&record.list),
                _ => None,
            })
            .unwrap_or_default();
        self.consume_body(|bytes| {
            let key = ctx
                .host_records
                .borrow_mut()
                .insert(HostRecord::Blob(BlobRecord {
                    bytes: bytes.to_vec(),
                    blob_type,
                }));
            Ok(PromiseOutcome::Resolve(HostValue::Blob(encode_host(
                ctx.slot, key,
            ))))
        })
    }

    /// Creates the promise a `Body` member returns and settles it with the
    /// member's own conversion, which is *consume body*'s step 4.
    ///
    /// Every `Body` member is the same algorithm with a different
    /// `convertBytesToJSValue`, and this is that difference: the conversion is
    /// handed the bytes — the body, `take_body`'s empty sequence for a null body,
    /// or nothing at all when the object is unusable. `None` is step 1, the
    /// `TypeError` rejection.
    fn consume_body<T>(
        &self,
        convert: impl FnOnce(&[u8]) -> Result<PromiseOutcome<'_>, ()>,
    ) -> Result<Promise<T>, JsError> {
        let script = self.script_host()?;
        let promise = script
            .new_promise()
            .ok_or_else(|| JsError::type_error("consume body: the promise could not be created"))?;
        // The body is bound before the match so the outcome may borrow it: a
        // conversion that borrows the bytes — `json()`'s does — has to name a
        // local that outlives the outcome, and a match scrutinee temporary does
        // not.
        let body = self.take_body();
        let outcome = match &body {
            Ok(bytes) => convert(bytes).unwrap_or(PromiseOutcome::Reject(HostValue::TypeError {
                message: BODY_UNUSABLE,
            })),
            Err(message) => PromiseOutcome::Reject(HostValue::TypeError { message }),
        };
        script.settle_promise(promise.resolver, outcome);
        Ok(Promise::from_bits(promise.promise))
    }
}

impl ResponseOps for HostObject {
    fn error(realm: &Realm) -> Result<Handle<Response>, JsError> {
        // A *network error*: type `"error"`, status 0, no headers, no body.
        let ctx = realm_context(realm)?;
        let key = insert_response(
            &ctx,
            Vec::new(),
            NewResponse {
                status: 0,
                status_text: Vec::new(),
                url: None,
                response_type: ResponseType::Error,
                redirected: false,
                body: None,
            },
        );
        Ok(Handle::from_bits(encode_host(ctx.slot, key)))
    }

    fn redirect(
        realm: &Realm,
        url: &str,
        status: Option<u16>,
    ) -> Result<Handle<Response>, JsError> {
        let context = "Failed to execute 'redirect' on 'Response'";
        let ctx = realm_context(realm)?;
        // Step 1: parse `url` against the API base URL, the document's.
        let parsed = {
            let doc = ctx.doc.borrow();
            match doc.base_url().or_else(|| doc.url()) {
                Some(base) => base.join(url).ok(),
                None => url::Url::parse(url).ok(),
            }
        }
        .ok_or_else(|| JsError::type_error(format!("{context}: Invalid URL")))?;
        // Step 3: only a redirect status.
        let status = status.unwrap_or(302);
        if !matches!(status, 301 | 302 | 303 | 307 | 308) {
            return Err(JsError::range_error(format!(
                "{context}: Invalid status code {status}"
            )));
        }
        // Steps 4 to 7: the status and a `Location` naming the serialized URL.
        let key = insert_response(
            &ctx,
            vec![(b"location".to_vec(), parsed.as_str().as_bytes().to_vec())],
            NewResponse {
                status,
                status_text: Vec::new(),
                url: None,
                response_type: ResponseType::Default,
                redirected: false,
                body: None,
            },
        );
        Ok(Handle::from_bits(encode_host(ctx.slot, key)))
    }

    fn r#type(&self) -> Result<ResponseType, JsError> {
        Ok(self.with_response(ResponseType::Default, |record| record.response_type))
    }

    fn url(&self) -> Result<String, JsError> {
        // "the empty string if this's response's URL is null": a
        // constructor-built response has none.
        Ok(self.with_response(String::new(), |record| {
            record.url.clone().unwrap_or_default()
        }))
    }

    fn redirected(&self) -> bool {
        self.with_response(false, |record| record.redirected)
    }

    fn status(&self) -> u16 {
        // The brand check makes the default unreachable: only a `Response`
        // wrapper can reach this getter, and its record lives exactly as long as
        // that wrapper does.
        self.with_response(200, |record| record.status)
    }

    fn ok(&self) -> bool {
        // Fetch's *ok status* is 200 to 299 inclusive, which is HTTP's own
        // definition of a successful status.
        self.with_response(false, |record| (200..=299).contains(&record.status))
    }

    fn status_text(&self) -> Result<Vec<u8>, JsError> {
        Ok(self.with_response(Vec::new(), |record| record.status_text.clone()))
    }

    fn headers(&self) -> Result<Handle<Headers>, JsError> {
        // `[SameObject]`: the key was stored by the constructor, so every read
        // answers with the record the first read did — and the wrapper world
        // revives the same wrapper for it while that wrapper lives.
        self.with_response(None, |record| Some(record.headers))
            .map(|headers| Handle::from_bits(encode_host(self.slot(), headers)))
            .ok_or_else(|| JsError::type_error("headers: the receiver is not a Response"))
    }

    fn body_used(&self) -> bool {
        self.body_is_used()
    }

    fn json(&self) -> Result<Promise<AnyValue>, JsError> {
        self.body_json()
    }

    fn text(&self) -> Result<Promise<String>, JsError> {
        self.body_text()
    }

    fn array_buffer(&self) -> Result<Promise<Vec<u8>>, JsError> {
        self.body_array_buffer()
    }

    fn bytes(&self) -> Result<Promise<Vec<u8>>, JsError> {
        self.body_bytes()
    }

    fn blob(&self) -> Result<Promise<Handle<Blob>>, JsError> {
        self.body_blob()
    }

    fn clone(&self) -> Result<Handle<Response>, JsError> {
        // "If this is unusable, then throw a TypeError." The clone is a second
        // response with the same fields, a header list of its own under the
        // same guard, and the same body: the body is a byte sequence held
        // whole, so the standard's tee is a copy of it.
        let HostObject::Host { ctx, .. } = self else {
            return Err(JsError::type_error("clone: the receiver is not a Response"));
        };
        let parts = self.with_response(None, |record| {
            (!record.is_unusable()).then(|| {
                (
                    record.headers,
                    NewResponse {
                        status: record.status,
                        status_text: record.status_text.clone(),
                        url: record.url.clone(),
                        response_type: record.response_type,
                        redirected: record.redirected,
                        body: record.body.clone(),
                    },
                )
            })
        });
        let Some((headers, parts)) = parts else {
            return Err(JsError::type_error(
                "Failed to execute 'clone' on 'Response': the body has already been read",
            ));
        };
        let list = match ctx.host_records.borrow().get(headers) {
            Some(HostRecord::Headers(record)) => record.list.clone(),
            _ => HeaderList::new(),
        };
        let key = insert_response(ctx, list, parts);
        Ok(Handle::from_bits(encode_host(ctx.slot, key)))
    }
}

/// The `type` of the `Blob` a body is read into: the first `Content-Type` value
/// that parses as a MIME type, in the lower-case form a `Blob` stores.
///
/// The standard serializes the MIME type it extracted, which also rewrites the
/// spacing of its parameters; the value is kept as the header spelled it.
fn body_mime_type(headers: &HeaderList) -> Option<String> {
    headers
        .iter()
        .filter(|(name, _)| name.as_slice() == CONTENT_TYPE)
        .flat_map(|(_, value)| split_header_value(value))
        .find(|value| parse_mime_type(value).is_some())
        .and_then(|value| String::from_utf8(value).ok())
        .map(|value| normalize_blob_type(value.trim()))
}

// ---------------------------------------------------------------------------
// The `fetch()` initializer
// ---------------------------------------------------------------------------

/// The prefix every message this module builds for a `fetch()` failure carries.
///
/// The steps are the `Request` constructor's, run by `fetch(input, init)`, so
/// the call that failed is named, exactly as the `fetch` global names it for
/// the throws of its own argument conversion. The constructor names itself
/// when a page calls it directly (`request::REQUEST_CONTEXT`).
pub const FETCH_CONTEXT: &str = "Failed to execute 'fetch'";

/// The `RequestInit` member this engine cannot serve as the page wrote it, or
/// `None` when it can serve every one the page named.
///
/// `RequestInit` declares the Fetch Standard's fifteen members
/// (`idl/fetch/Request.webidl`). A member is refused *by name* rather than
/// ignored whenever ignoring it would send a different request than the page
/// asked for: a `mode` of `no-cors` (there are no opaque responses), a
/// `redirect` of `manual` (no opaque-redirect ones either), a `cache` mode that
/// must be answered from the cache, an `integrity` that is not checked, a
/// `referrer` or `referrerPolicy` other than the default. The refusal names the
/// member so that a page can tell which one it was.
///
/// What is served: `method`, `headers` and `body`; `credentials` in all three
/// modes and `mode` as `cors` or `same-origin` (the driver runs the CORS
/// protocol); `redirect` as `follow` or `error`; `cache` as `default`, or as
/// `no-store`, `reload` and `no-cache`, which all go to the network;
/// `signal`, which each backend handles itself and refuses on its own when it
/// cannot. `keepalive`, `priority` and `duplex` are accepted with any value:
/// the first only matters to a request that outlives its page, the second is
/// a hint, and the third has one value.
///
/// A member written out with its default value — `{integrity: ""}`,
/// `{window: null}`, `{referrer: "about:client"}` — is the request this engine
/// sends anyway and is accepted.
///
/// The order is the dictionary's own declaration order, which is the standard's,
/// so a call naming two unserved members always reports the same one.
pub fn refused_request_init_member(init: &RequestInit) -> Option<&'static str> {
    if init
        .referrer
        .as_deref()
        .is_some_and(|referrer| referrer != "about:client")
    {
        return Some("referrer");
    }
    if init.referrer_policy.is_some_and(|policy| {
        !matches!(
            policy,
            ReferrerPolicy::EmptyString | ReferrerPolicy::StrictOriginWhenCrossOrigin
        )
    }) {
        return Some("referrerPolicy");
    }
    // `navigate` is not a mode a page may ask for; the other three are served.
    if init.mode == Some(RequestMode::Navigate) {
        return Some("mode");
    }
    if init
        .cache
        .is_some_and(|cache| matches!(cache, RequestCache::ForceCache | RequestCache::OnlyIfCached))
    {
        return Some("cache");
    }
    if init.redirect == Some(RequestRedirect::Manual) {
        return Some("redirect");
    }
    if init
        .integrity
        .as_deref()
        .is_some_and(|integrity| !integrity.is_empty())
    {
        return Some("integrity");
    }
    if init
        .window
        .is_some_and(|window| window.0 != AnyValue::NULL.0)
    {
        return Some("window");
    }
    None
}

/// What the `signal` a `fetch()` call was given says about the request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FetchSignal {
    /// Already aborted: the promise rejects with this stored reason and no
    /// request is made.
    Aborted(u64),
    /// Not aborted. Once the driver has the request,
    /// [`watch_fetch_signal`] ties the two together.
    Live(HostKey),
    /// Not a signal of this world; the request proceeds without one.
    None,
}

/// Reads the state of the `AbortSignal` a `fetch()` initializer named.
pub fn fetch_signal(ctx: &SharedContext, handle: Handle<AbortSignal>) -> FetchSignal {
    let decoded = decode(handle.bits());
    if decoded.tag != TAG_HOST {
        return FetchSignal::None;
    }
    let key = decoded.host_key();
    match ctx.host_records.borrow().get(key) {
        Some(HostRecord::Signal(record)) => match record.reason {
            Some(bits) => FetchSignal::Aborted(bits),
            None => FetchSignal::Live(key),
        },
        _ => FetchSignal::None,
    }
}

/// Makes aborting `signal` reject the promise `resolver` names and end its
/// request. The entry is dropped by [`forget_fetch_signal`] when the request
/// is answered first.
pub fn watch_fetch_signal(ctx: &SharedContext, resolver: u64, signal: HostKey) {
    ctx.fetch_signals.borrow_mut().push((resolver, signal));
}

/// Drops the tie [`watch_fetch_signal`] made: the request was answered.
pub fn forget_fetch_signal(ctx: &SharedContext, resolver: u64) {
    let mut fetches = ctx.fetch_signals.borrow_mut();
    // Almost always empty: only a request that was given a signal is here.
    if !fetches.is_empty() {
        fetches.retain(|(watched, _)| *watched != resolver);
    }
}

/// The request `fetch()` sends for a URL and a converted `RequestInit`: the
/// Fetch Standard's *Request* constructor over a string input
/// (`<https://fetch.spec.whatwg.org/#dom-request>`). A `Request` input goes
/// through [`request_parts`], which runs the same steps.
///
/// The steps are the standard's, in the standard's order, for the three members
/// that reach them:
///
/// 1. `init["method"]`: a value that is not a *method* is a `TypeError`, one
///    that is a *forbidden method* is a `TypeError`, and a value that is either
///    is then normalised (`<https://fetch.spec.whatwg.org/#concept-method>`,
///    <https://fetch.spec.whatwg.org/#concept-method-normalize>). An absent
///    `method` is `GET`, which is what the request starts as.
/// 2. `init["headers"]`: *fill* the request's header list with them, under guard
///    `"request"` — so a *forbidden request-header* is dropped silently, and a
///    page cannot name its own `Content-Length`, `Host` or `Cookie`.
/// 3. `init["body"]`: a non-null `body` with a `GET` or a `HEAD` method is a
///    `TypeError`, and otherwise the body is *extracted with type* and the type
///    is appended as `Content-Type` when there is one and the header list does
///    not already name it
///    (<https://fetch.spec.whatwg.org/#concept-bodyinit-extract>).
///
/// The URL and the initializer have already been resolved and refused by the
/// time this runs, so what is left is the request the driver is handed.
///
/// # Why these failures are rejections and not throws
///
/// The standard runs all of these steps *inside* the `Request` constructor,
/// which `fetch()` invokes inside its own steps and converts to a promise
/// rejection: "If this throws an exception, reject p with it and return p"
/// (`<https://fetch.spec.whatwg.org/#dom-global-fetch>`, step 2). So every
/// `Err` here is a rejection, and the *argument conversions* — the dictionary
/// and each member's — are the only throws on this path, which is the
/// distinction the `fetch` global's own comment records.
pub fn request_from_init(
    ctx: &SharedContext,
    init: RequestInit,
    url: Url,
    initiator: Url,
    origin: Origin,
) -> Result<FetchRequest, JsError> {
    // The steps are the `Request` constructor's, over a request that starts as
    // the bare URL (`request::apply_request_init`).
    let parts =
        request::apply_request_init(ctx, RequestParts::new(url), None, init, FETCH_CONTEXT)?;
    Ok(parts.into_fetch_request(initiator, origin))
}

// ---------------------------------------------------------------------------
// TextDecoder / TextEncoder
// ---------------------------------------------------------------------------

impl TextDecoderConstructor for HostObject {
    fn construct(
        realm: &Realm,
        label: Option<&str>,
        options: Option<TextDecoderOptions>,
    ) -> Result<Self, JsError> {
        // Step 1: "Let encoding be the result of getting an encoding from
        // label." `for_label_no_replacement` is that algorithm *and* step 2's
        // "if encoding is failure or replacement": the replacement encoding is
        // not a decoder this interface may hand out, and every label that maps
        // to it (`"replacement"`, `"csiso2022kr"`, `"hz-gb-2312"`,
        // `"iso-2022-cn"`, `"iso-2022-cn-ext"`, `"iso-2022-kr"`) is a
        // `RangeError`. `encoding/api-replacement-encodings.any.js` is that
        // list.
        //
        // The lookup lowercases and strips ASCII whitespace itself, so the
        // spec's label matching is not restated here: `"  UTF-8  "` is the
        // same label as `"utf-8"`.
        let label = label.unwrap_or("utf-8");
        let encoding = encoding_rs::Encoding::for_label_no_replacement(label.as_bytes())
            .ok_or_else(|| {
                JsError::range_error(format!(
                    "Failed to construct 'TextDecoder': '{label}' is not a valid encoding label."
                ))
            })?;
        // Steps 3–5. `error mode` is `"fatal"` or the default `"replacement"`,
        // and the record stores the boolean the adapter needs rather than the
        // mode's name.
        let options = options.unwrap_or_default();
        let ctx = realm_context(realm)?;
        let key = ctx
            .host_records
            .borrow_mut()
            .insert(HostRecord::TextDecoder(TextDecoderRecord {
                encoding,
                fatal: options.fatal,
                ignore_bom: options.ignore_bom,
                bom_seen: false,
                decoder: None,
                do_not_flush: false,
            }));
        Ok(HostObject::host_record(ctx, key))
    }
}

impl TextDecoderOps for HostObject {
    fn decode(
        &self,
        input: Option<Vec<u8>>,
        options: Option<TextDecodeOptions>,
    ) -> Result<String, JsError> {
        // The I/O queue the standard describes holds at most this one argument
        // — a `decode` call pushes nothing else into it — and a synchronous
        // call has no end-of-queue item to wait for, so the queue and the
        // argument are the same slice. `input` is absent when the argument was
        // absent or `undefined`, which pushes nothing.
        let bytes = input.unwrap_or_default();
        let stream = options.unwrap_or_default().stream;
        self.with_decoder_mut(
            // Unreachable: the template's signature brand check admits only a
            // `TextDecoder` wrapper, and the record it was built over lives
            // exactly as long as that wrapper does.
            Err(JsError::type_error(
                "decode: the receiver is not a TextDecoder",
            )),
            |record| record.decode(&bytes, stream),
        )
    }

    fn encoding(&self) -> Result<String, JsError> {
        // The standard's name is lowercased; `encoding_rs` capitalizes nearly
        // all of them (`"Shift_JIS"`, `"UTF-16LE"`, `"ISO-8859-2"`,
        // `"windows-1252"` being the exception that hides the rule).
        Ok(self.with_decoder(String::new(), |record| {
            record.encoding.name().to_ascii_lowercase()
        }))
    }

    fn fatal(&self) -> bool {
        self.with_decoder(false, |record| record.fatal)
    }

    fn ignore_bom(&self) -> bool {
        self.with_decoder(false, |record| record.ignore_bom)
    }
}

impl TextEncoderConstructor for HostObject {
    fn construct(realm: &Realm) -> Result<Self, JsError> {
        // The constructor's steps are "to do nothing": an encoder is stateless
        // and its encoding is the constant UTF-8. The record exists anyway, for
        // the two reasons every `TAG_HOST` wrapper needs one — the record names
        // the interface the wrapper's template comes from, and the wrapper's
        // finalizer frees the slot through `reclaim_host_record`.
        let ctx = realm_context(realm)?;
        let key = ctx
            .host_records
            .borrow_mut()
            .insert(HostRecord::TextEncoder);
        Ok(HostObject::host_record(ctx, key))
    }
}

impl TextEncoderOps for HostObject {
    fn encode(&self, input: Option<&str>) -> Result<Vec<u8>, JsError> {
        // The standard's `encode` is a UTF-8 encode of the scalar-value queue
        // with error mode `"fatal"`, followed by its own
        // `Assert: result is not an error` — UTF-8 encoding cannot fail, which
        // is why this member's `[NewObject]` is not `[Throws]`. Rust's `&str`
        // is already a scalar-value sequence, and its UTF-8 is the encoding.
        //
        // An absent or `undefined` argument is the IDL default `""`, which is
        // the empty byte sequence.
        Ok(input.unwrap_or("").as_bytes().to_vec())
    }

    fn encoding(&self) -> Result<String, JsError> {
        // A constant, and not a read of a record: the encoder has no encoding
        // to vary, which is what makes the record empty.
        Ok(String::from(TEXT_ENCODER_ENCODING))
    }
}

// ---------------------------------------------------------------------------
// Blob / File
// ---------------------------------------------------------------------------

/// The current wall-clock time in milliseconds since the Unix Epoch, which is
/// what `lastModified` defaults to.
///
/// The standard's default is "the current date and time represented as the
/// number of milliseconds since the `Unix Epoch` (which is the equivalent of
/// `Date.now()` [[ECMA-262]])" (<https://w3c.github.io/FileAPI/#file-constructor>,
/// step 3.3, and its note that the member "could be a `Date` object").
///
/// This reads the system clock rather than the event loop's `Clock`, because
/// the loop's clock is a monotonic count from the loop's own origin and may be
/// virtual: `Date.now()` in this engine is V8's, which is the system clock, and
/// a `lastModified` measured from a virtual origin would not be comparable with
/// it. A clock set before the epoch reads as 0, which is what `Clock::real`
/// does with the same duration.
fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// The File API's *type normalization*, which both the `Blob` constructor's step
/// 3 (<https://w3c.github.io/FileAPI/#constructorBlob>) and *slice blob*'s
/// `contentType` substeps (<https://w3c.github.io/FileAPI/#dfn-slice-blob>) run.
///
/// The two clauses are one rule written twice: "If |t| contains any characters
/// outside the range U+0020 to U+007E, then set |t| to the empty string and
/// return from these substeps", and otherwise "Convert every character in |t| to
/// ASCII lowercase". The range is therefore the printable ASCII range: a tab, a
/// newline, a DEL or any non-ASCII character empties the whole type rather than
/// being dropped from it.
///
/// The constructor's step 3 opens with "If the `type` member is not the empty
/// string", so an absent or empty `type` enters no substeps at all, and both
/// constructors then read `t` from outside that guard — `Blob` in its step 4
/// ("its `type` set to the value of |t| from the substeps above") and `File` in
/// its step 3.2 ("Convert every character in |t| to ASCII lowercase") — which is
/// an unbound reference when the member was absent or empty. `BlobPropertyBag`
/// declares `type = ""`, so the empty string is the only value the guarded and
/// the unguarded reading can disagree about, and this function returns the empty
/// string for it: the two readings agree on every input.
fn normalize_blob_type(value: &str) -> String {
    if !value.chars().all(|c| ('\u{20}'..='\u{7E}').contains(&c)) {
        return String::new();
    }
    value.to_ascii_lowercase()
}

/// The bytes a `Blob` or `File` record holds, or `None` when `handle` names
/// neither.
///
/// The generated conversion checks the JavaScript interface brand. Resolving
/// the tag and record variant here also validates handles supplied directly by
/// Rust callers and distinguishes Blob from its File subclass.
///
/// The tag test is not redundant with the record lookup. A window handle carries
/// no key at all — `encode_window` is the tag alone — so reading its
/// `host_key` and looking that up would ask for slab slot 0 and could append an
/// unrelated record's bytes.
fn blob_bytes_of(ctx: &SharedContext, handle: Handle<Blob>) -> Option<Vec<u8>> {
    blob_of(ctx, handle).map(|(bytes, _)| bytes)
}

/// The bytes and the `type` of the `Blob` or `File` `handle` names, with the
/// same tag check as [`blob_bytes_of`].
fn blob_of(ctx: &SharedContext, handle: Handle<Blob>) -> Option<(Vec<u8>, String)> {
    let records = ctx.host_records.borrow();
    let blob = match host_record_of(&records, handle.bits())? {
        HostRecord::Blob(blob) => blob,
        HostRecord::File(file) => &file.blob,
        _ => return None,
    };
    Some((blob.bytes.clone(), blob.blob_type.clone()))
}

/// *process blob parts* (<https://w3c.github.io/FileAPI/#process-blob-parts>).
///
/// Per element: "If |element| is a `USVString` ... Append the result of UTF-8
/// encoding |s| to |bytes|"; "If |element| is a `BufferSource`, get a copy of the
/// bytes held by the buffer source, and append those bytes to |bytes|"; "If
/// |element| is a `Blob`, append the bytes it represents to |bytes|", with the
/// spec's own note that "the `type` of the `Blob` array element is ignored and
/// will not affect `type` of returned `Blob` object".
///
/// The union conversion has already made the three-way split: `Vec<u8>` for the
/// buffer arm is the copy that step takes, `String` for the string arm is
/// already a scalar-value sequence (Rust `String` cannot hold a lone surrogate,
/// so WebIDL's U+FFFD replacement has happened), and its UTF-8 is what Rust's
/// `as_bytes` yields. What is left for this function is the `Blob` arm's brand
/// check ([`blob_bytes_of`]) and the append.
///
/// Substep 1.2's *convert line endings to native* is absent by decision, not by
/// oversight: the host refuses a property bag that names `endings` at all, so no
/// input reaches this function with that branch live. The declaration's own
/// header, `idl/fileapi/Blob.webidl`, records why.
fn append_blob_parts(
    ctx: &SharedContext,
    parts: &[BlobPart],
    constructor: &str,
    argument: &str,
) -> Result<Vec<u8>, JsError> {
    let mut bytes = Vec::new();
    for element in parts {
        match element {
            BlobPart::BufferSource(buffer) => bytes.extend_from_slice(buffer),
            BlobPart::Blob(handle) => {
                let Some(blob) = blob_bytes_of(ctx, *handle) else {
                    return Err(JsError::type_error(format!(
                        "Failed to construct '{constructor}': the {argument} argument contains a \
                         value that is not a Blob"
                    )));
                };
                bytes.extend_from_slice(&blob);
            }
            BlobPart::UsvString(text) => bytes.extend_from_slice(text.as_bytes()),
        }
    }
    Ok(bytes)
}

/// The `BlobPropertyBag`/`FilePropertyBag` member this engine cannot serve, or
/// `None` when it can serve every one the page named.
///
/// The bag declares two members and this engine serves one of them. `endings` is
/// refused *by name* rather than ignored, for the reason slice b-6.5 refuses
/// `RequestInit`'s twelve unserved members
/// ([`refused_request_init_member`]): ignoring it means producing different
/// bytes than the page asked for, silently, and only on a platform whose native
/// line ending is not LF. The refusal names the member so that a page can tell
/// which one it was.
///
/// **The refusal is by presence and not by value, so it over-refuses.**
/// `{endings: "transparent"}` is the standard's own default written out and asks
/// for exactly the bytes this engine would have produced, and is refused here
/// where a browser proceeds. Being value-aware would mean accepting `"native"`
/// on the platforms whose native ending is LF while refusing it on the others,
/// and the repository cannot test the other branch; the two error directions are
/// not equally bad, so this errs at the visible one.
///
/// The member has no default in this repository's IDL so that its presence is
/// *observable* here at all — a dictionary member with a default lowers to a
/// plain field and one without a default to an `Option`. The host applies the
/// standard's `"transparent"` itself by ignoring `None`. That deviation, and
/// this refusal, are recorded in `idl/fileapi/Blob.webidl` and
/// `docs/dev/idl.md`.
fn refused_endings(endings: Option<EndingType>) -> Option<&'static str> {
    endings.map(|_| "endings")
}

/// *slice blob* (<https://w3c.github.io/FileAPI/#dfn-slice-blob>), which
/// `Blob.slice()`'s own steps delegate to
/// (<https://w3c.github.io/FileAPI/#slice-method-algo>).
///
/// `None` is the standard's null: the IDL declares all three arguments
/// `optional` with **no** default, and the generated binding yields `None`
/// exactly when the argument is absent or explicitly `undefined`, which is
/// WebIDL's own rule for a dictionary-free optional argument — so "If `start` is
/// given" and "If `start` is null" select the same branch, and `Some(0)` is not
/// collapsed into `None`. `[Clamp]` rather than `[EnforceRange]` is what lets a
/// negative index arrive here at all: it clamps to the `long long` range and
/// leaves the sign, which is precisely what the negative branch below reads.
///
/// The two normalizations are `max(originalSize + value, 0)` for a negative
/// value and `min(value, originalSize)` otherwise, `end` defaulting to
/// `originalSize`; then `span = max(relativeEnd - relativeStart, 0)`. Both
/// relative bounds therefore lie in `0..=originalSize`, so `from` and `from +
/// span` are in bounds for every input and the byte range cannot panic.
fn blob_slice(
    source: &BlobRecord,
    start: Option<i64>,
    end: Option<i64>,
    content_type: Option<&str>,
) -> BlobRecord {
    let original_size = i64::try_from(source.bytes.len()).unwrap_or(i64::MAX);
    let normalize = |value: Option<i64>, default: i64| match value {
        None => default,
        Some(value) if value < 0 => original_size.saturating_add(value).max(0),
        Some(value) => value.min(original_size),
    };
    let relative_start = normalize(start, 0);
    let relative_end = normalize(end, original_size);
    let span = (relative_end - relative_start).max(0);
    let from = relative_start as usize;
    let to = (relative_start + span) as usize;
    BlobRecord {
        bytes: source.bytes[from..to].to_vec(),
        // "If `contentType` is null, let `relativeContentType` be set to the
        // empty string" — which is why a slice without a third argument loses
        // the source's type rather than inheriting it.
        blob_type: match content_type {
            None => String::new(),
            Some(value) => normalize_blob_type(value),
        },
    }
}

#[cfg(test)]
mod blob_tests {
    use super::{BlobRecord, blob_slice, normalize_blob_type};

    fn blob(bytes: &[u8], blob_type: &str) -> BlobRecord {
        BlobRecord {
            bytes: bytes.to_vec(),
            blob_type: blob_type.to_string(),
        }
    }

    #[test]
    fn a_type_keeps_only_printable_ascii_and_lowercases_it() {
        // The clause is a range test over the *whole* string: one character
        // outside U+0020..=U+007E empties the type rather than being dropped, so
        // the assertions below are the boundary of the range and not a filter.
        assert_eq!(normalize_blob_type("Text/Plain"), "text/plain");
        assert_eq!(normalize_blob_type(""), "");
        assert_eq!(normalize_blob_type(" "), " ");
        assert_eq!(normalize_blob_type("\u{7E}"), "\u{7E}");
        assert_eq!(normalize_blob_type("\u{1F}"), "");
        assert_eq!(normalize_blob_type("\u{7F}"), "");
        assert_eq!(normalize_blob_type("text/plain\n"), "");
        assert_eq!(normalize_blob_type("text/plain\u{00E9}"), "");
        assert_eq!(normalize_blob_type("text/pläin"), "");
        // Lowercasing is ASCII-only, so a non-ASCII character never reaches it.
        assert_eq!(normalize_blob_type("TEXT/PLAIN"), "text/plain");
    }

    #[test]
    fn slice_bounds_are_normalized_against_the_original_size() {
        let source = blob(b"0123456789", "text/plain");

        // Absent `start` is 0 and absent `end` is the original size.
        let all = blob_slice(&source, None, None, None);
        assert_eq!(all.bytes, b"0123456789");
        // `contentType` absent is the empty string, not the source's type.
        assert_eq!(all.blob_type, "");

        // A positive bound clamps to the original size.
        assert_eq!(blob_slice(&source, None, Some(4), None).bytes, b"0123");
        assert_eq!(
            blob_slice(&source, None, Some(99), None).bytes,
            b"0123456789"
        );
        assert_eq!(blob_slice(&source, Some(6), None, None).bytes, b"6789");

        // A negative `start` counts from the end and clamps at 0; each of the
        // three arguments is `Some` here, so the three branches are separate.
        assert_eq!(blob_slice(&source, Some(-3), None, None).bytes, b"789");
        assert_eq!(
            blob_slice(&source, Some(-99), None, None).bytes,
            b"0123456789"
        );

        // An inverted range is empty rather than a panic: `span` is a `max`.
        assert_eq!(blob_slice(&source, Some(7), Some(2), None).bytes, b"");
        assert_eq!(blob_slice(&source, Some(5), Some(5), None).bytes, b"");

        // The third argument is normalized by the same rule as the constructor's.
        let typed = blob_slice(&source, None, None, Some("TEXT/Plain"));
        assert_eq!(typed.blob_type, "text/plain");
        assert_eq!(
            blob_slice(&source, None, None, Some("a\u{00E9}b")).blob_type,
            ""
        );

        // `Some(0)` is not collapsed into `None`: both are 0, but the `[Clamp]`
        // conversion is what the negative branch distinguishes, and an empty
        // source makes the difference visible.
        let empty = blob(b"", "text/plain");
        assert_eq!(blob_slice(&empty, Some(0), Some(0), None).bytes, b"");
        assert_eq!(blob_slice(&empty, Some(-1), None, None).bytes, b"");
    }
}

impl BlobConstructor for HostObject {
    fn construct(
        realm: &Realm,
        blob_parts: Option<Vec<BlobPart>>,
        options: Option<BlobPropertyBag>,
    ) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        // `None` is an `options` argument that was absent or `undefined`; the
        // generated binding supplies `Default::default()` for the absent case,
        // so this is the `undefined` one and the two bags agree.
        let options = options.unwrap_or_default();
        if let Some(member) = refused_endings(options.endings) {
            return Err(JsError::type_error(format!(
                "Failed to construct 'Blob': the BlobPropertyBag member '{member}' is not \
                 implemented"
            )));
        }
        // Step 1 gives the zero-parameter call its own 0-byte answer, and it is
        // the same answer as processing no parts, which is what `None` is here.
        let bytes = match &blob_parts {
            Some(parts) => append_blob_parts(&ctx, parts, "Blob", "blobParts")?,
            None => Vec::new(),
        };
        let key = ctx
            .host_records
            .borrow_mut()
            .insert(HostRecord::Blob(BlobRecord {
                bytes,
                // Step 3, including the empty-string guard the two readings of
                // the spec's step agree on.
                blob_type: normalize_blob_type(&options.r#type),
            }));
        Ok(HostObject::host_record(ctx, key))
    }
}

impl BlobOps for HostObject {
    fn size(&self) -> u64 {
        // "the total number of bytes that can be read by a `FileReader` or
        // `FileReaderSync` object, or 0 if the `Blob` has no bytes to be read" —
        // which is the byte sequence's length, and is never a derived count.
        // (<https://w3c.github.io/FileAPI/#dfn-size>)
        self.with_blob(0, |blob| blob.bytes.len() as u64)
    }

    fn r#type(&self) -> Result<String, JsError> {
        Ok(self.with_blob(String::new(), |blob| blob.blob_type.clone()))
    }

    fn slice(
        &self,
        start: Option<i64>,
        end: Option<i64>,
        content_type: Option<&str>,
    ) -> Result<Handle<Blob>, JsError> {
        // A `Blob` or a `File` receiver is always a `HostObject::Host`, and its
        // record lives exactly as long as the wrapper the template's signature
        // checked, so the error is unreachable rather than expected.
        let HostObject::Host { ctx, key } = self else {
            return Err(JsError::type_error("slice: the receiver is not a Blob"));
        };
        // The new record is built while the slab is borrowed for reading and
        // inserted after that borrow ends: inserting under it would be a second
        // borrow of the same `RefCell`.
        let sliced = match ctx.host_records.borrow().get(*key) {
            Some(HostRecord::Blob(blob)) => blob_slice(blob, start, end, content_type),
            Some(HostRecord::File(file)) => blob_slice(&file.blob, start, end, content_type),
            _ => return Err(JsError::type_error("slice: the receiver is not a Blob")),
        };
        let new_key = ctx
            .host_records
            .borrow_mut()
            .insert(HostRecord::Blob(sliced));
        // The result is a `Blob` even when the receiver was a `File`: `File`
        // declares no `slice()` of its own, and the standard's `Blob` return
        // type is what the inherited member answers with.
        Ok(Handle::from_bits(encode_host(ctx.slot, new_key)))
    }

    fn text(&self) -> Result<Promise<String>, JsError> {
        // The standard's steps read the whole byte sequence through the blob's
        // stream and run UTF-8 decode on it
        // (<https://w3c.github.io/FileAPI/#text-method-algo>). The bytes are
        // held directly here, so the decode is the whole of it — `Response.text`
        // is the same shape.
        let script = self.script_host()?;
        let promise = script
            .new_promise()
            .ok_or_else(|| JsError::type_error("Blob.text: the promise could not be created"))?;
        let text = utf8_decode(&self.with_blob(Vec::new(), |blob| blob.bytes.clone()));
        script.settle_promise(
            promise.resolver,
            PromiseOutcome::Resolve(HostValue::String(text)),
        );
        Ok(Promise::from_bits(promise.promise))
    }

    fn array_buffer(&self) -> Result<Promise<Vec<u8>>, JsError> {
        self.read_blob("Blob.arrayBuffer", HostValue::ArrayBuffer)
    }

    fn bytes(&self) -> Result<Promise<Vec<u8>>, JsError> {
        self.read_blob("Blob.bytes", HostValue::Uint8Array)
    }
}

impl HostObject {
    /// A promise resolved with a copy of this `Blob`'s bytes in the shape
    /// `value` gives them: the read the standard does through the blob's
    /// stream, which here is the byte sequence the record holds.
    fn read_blob<T>(
        &self,
        member: &str,
        value: impl FnOnce(Vec<u8>) -> HostValue<'static>,
    ) -> Result<Promise<T>, JsError> {
        let script = self.script_host()?;
        let promise = script.new_promise().ok_or_else(|| {
            JsError::type_error(format!("{member}: the promise could not be created"))
        })?;
        let bytes = self.with_blob(Vec::new(), |blob| blob.bytes.clone());
        script.settle_promise(promise.resolver, PromiseOutcome::Resolve(value(bytes)));
        Ok(Promise::from_bits(promise.promise))
    }
}

impl FileConstructor for HostObject {
    fn construct(
        realm: &Realm,
        file_bits: Vec<BlobPart>,
        file_name: &str,
        options: Option<FilePropertyBag>,
    ) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        // `FilePropertyBag` inherits `BlobPropertyBag`'s two members ahead of its
        // own `lastModified` — `Dictionary::parent` and `Model::dictionary_members`
        // walk the chain, so the generated `FilePropertyBag` carries all three
        // and the inherited refusal applies unchanged.
        let options = options.unwrap_or_default();
        if let Some(member) = refused_endings(options.endings) {
            return Err(JsError::type_error(format!(
                "Failed to construct 'File': the FilePropertyBag member '{member}' is not \
                 implemented"
            )));
        }
        // Step 1: process `fileBits`, which is a required argument and so is
        // never absent.
        let bytes = append_blob_parts(&ctx, &file_bits, "File", "fileBits")?;
        // Step 3.1 and 3.2: the type, through the same normalization — and
        // through the same editorial defect, which the IDL header records under
        // "One editorial defect in the standard".
        let blob_type = normalize_blob_type(&options.r#type);
        // Step 3.3: `lastModified` when it is provided, and "the current date and
        // time represented as the number of milliseconds since the Unix Epoch"
        // when it is not. The member's own type is `long long`, so a `Date`
        // converts through `ToNumber` like any other `long long` member.
        let last_modified = options.last_modified.unwrap_or_else(now_epoch_ms);
        // Step 4: the new `File` refers to the same byte sequence and size its
        // `Blob` half does.
        let key = insert_file_record(
            &ctx,
            BlobRecord { bytes, blob_type },
            file_name,
            last_modified,
        );
        Ok(HostObject::host_record(ctx, key))
    }
}

impl FileOps for HostObject {
    fn name(&self) -> Result<String, JsError> {
        // "the name of the file" as it was constructed, unnormalized: the
        // standard stores the `USVString` it was handed and only notes that an
        // OS filename uses different conventions.
        Ok(self.with_file(String::new(), |file| file.name.clone()))
    }

    fn last_modified(&self) -> i64 {
        // Unreachable default, like `Response.status`: only a `File` wrapper can
        // reach an accessor carrying `File`'s signature.
        self.with_file(0, |file| file.last_modified)
    }
}

// ---------------------------------------------------------------------------
// FormData
// ---------------------------------------------------------------------------

/// Inserts a `File` record that nothing refers to yet: no wrapper has been
/// adopted for it and no entry list holds it, so nothing keeps it alive but the
/// caller's own use of the key.
///
/// A wrapper marks a record as wrapped when it is created or adopted
/// (`crates/tenta-js-v8/src/world.rs`), and an entry list that names one becomes
/// one of its [`FileRecord::owners`]. Both of those are how a live record
/// survives; this is the state a record is born in.
///
/// The `File` constructor and HTML's *create an entry* are the two producers,
/// and they agree on every field — the constructor's step 4, "the new `File`
/// object refers to the same byte sequence and size as the `Blob`", is `blob`,
/// and the two members left are the name and the modification time each caller
/// computes for itself. This is the one place the record's fields are written,
/// so a field added to [`FileRecord`] cannot be forgotten by one of them.
fn insert_file_record(
    ctx: &SharedContext,
    blob: BlobRecord,
    name: &str,
    last_modified: i64,
) -> HostKey {
    ctx.host_records
        .borrow_mut()
        .insert(HostRecord::File(FileRecord {
            blob,
            name: name.to_string(),
            last_modified,
            has_wrapper: false,
            owners: Vec::new(),
        }))
}

/// The distinct `File` records an entry list names.
///
/// Deduplicated because ownership is per `FormData` and not per entry: a list
/// that names one `File` under two names holds one reference to it, so giving it
/// up once is the whole of what a mutation owes the record. The key's own
/// generation makes the deduplication exact rather than best-effort.
fn file_keys(list: &[(String, EntryValue)]) -> Vec<HostKey> {
    let mut keys: Vec<HostKey> = Vec::new();
    for (_, value) in list {
        if let EntryValue::File(key) = value
            && !keys.contains(key)
        {
            keys.push(*key);
        }
    }
    keys
}

/// The `FormDataEntryValue` a stored entry value converts to.
///
/// The `File` half becomes a handle, which is what makes the identity rule hold
/// with no extra state: the wrapper world keeps one wrapper per handle, so the
/// object `get` answers with is the object `getAll` and the pair iterator answer
/// with, and it is the object the page passed in when the entry kept it.
fn entry_to_union(slot: u16, value: &EntryValue) -> FileOrUsvString {
    match value {
        EntryValue::String(text) => FileOrUsvString::UsvString(text.clone()),
        EntryValue::File(key) => FileOrUsvString::File(Handle::from_bits(encode_host(slot, *key))),
    }
}

/// The `Blob` or `File` record a `Blob`-declared argument names.
enum BlobSource {
    /// The argument named a `Blob` record, which is never a `File`, so the entry
    /// always mints one.
    Blob { bytes: Vec<u8>, blob_type: String },
    /// The argument named a `File` record, whose key is the object the entry
    /// keeps when no `filename` is given, and whose `lastModified` the minted
    /// replacement copies.
    File {
        key: HostKey,
        bytes: Vec<u8>,
        blob_type: String,
        last_modified: i64,
    },
}

/// Resolves the `Blob` or `File` record `handle` names, or refuses it.
///
/// As in [`blob_bytes_of`], record validation protects Rust callers as well as
/// the generated binding path. Distinguishing Blob from File determines the
/// entry's identity and whether its `lastModified` value is preserved.
///
/// The tag test is not redundant with the record lookup. A window handle carries
/// no key at all — `encode_window` is the tag alone — so reading its `host_key`
/// and looking that up would ask for slab slot 0.
///
/// `context` names the operation that failed, so that `append` and `set` report
/// themselves rather than reporting `FormData` at large.
fn blob_source_of(
    ctx: &SharedContext,
    handle: Handle<Blob>,
    context: &str,
) -> Result<BlobSource, JsError> {
    let decoded = decode(handle.bits());
    let source = if decoded.tag == TAG_HOST {
        ctx.host_records
            .borrow()
            .get(decoded.host_key())
            .and_then(|record| match record {
                HostRecord::Blob(blob) => Some(BlobSource::Blob {
                    bytes: blob.bytes.clone(),
                    blob_type: blob.blob_type.clone(),
                }),
                HostRecord::File(file) => Some(BlobSource::File {
                    key: decoded.host_key(),
                    bytes: file.blob.bytes.clone(),
                    blob_type: file.blob.blob_type.clone(),
                    last_modified: file.last_modified,
                }),
                _ => None,
            })
    } else {
        None
    };
    source
        .ok_or_else(|| JsError::type_error(format!("{context}: the value is not a Blob or a File")))
}

/// The `File` record a new entry holds for a `Blob`-declared value.
///
/// HTML's *create an entry* is the algorithm, and its own note states the rule
/// these two branches are: "these operations will create a new File object if
/// either filename is given or the passed Blob is not a File object. In those
/// cases, the identity of the passed Blob object is not kept." When no new
/// object is created the value *is* the argument, which is the identity
/// `wpt/checkout/xhr/formdata/set-blob.any.js`'s `assert_file_identity` pins to
/// the record.
///
/// Two details the algorithm leaves to be read off the corpus, both from that
/// same directory:
///
/// - A minted `File` gets "the current date and time represented as the number
///   of milliseconds since the Unix Epoch" — the `File` constructor's default
///   for an absent `lastModified`, which is what *create an entry* names because
///   it gives no third argument. `set-blob.any.js` pins it within 200 ms of
///   `Date.now()` and `append.any.js`'s "testFormDataAppendEmptyBlob" within two
///   seconds, so [`now_epoch_ms`] — the same clock both of those read — is the
///   only source that satisfies both.
/// - A `File` source **with** a `filename` keeps the source's `lastModified`
///   rather than taking the clock: `set-blob.any.js`'s "file with lastModified
///   and custom name" asserts `lastModified === 123` on a source built with
///   `{ lastModified: 123 }`, while its name becomes the `filename`
///   (`assert_equals(file2.name, "custom name")`) exactly as the algorithm says.
fn entry_file(
    ctx: &SharedContext,
    handle: Handle<Blob>,
    filename: Option<&str>,
    context: &str,
) -> Result<HostKey, JsError> {
    let source = blob_source_of(ctx, handle, context)?;
    // The one case in which the argument itself becomes the entry's value, and
    // therefore the one case in which no record is inserted at all.
    if let BlobSource::File { key, .. } = &source
        && filename.is_none()
    {
        return Ok(*key);
    }
    let (bytes, blob_type, last_modified) = match source {
        BlobSource::Blob { bytes, blob_type } => (bytes, blob_type, now_epoch_ms()),
        BlobSource::File {
            bytes,
            blob_type,
            last_modified,
            ..
        } => (bytes, blob_type, last_modified),
    };
    // The algorithm's second step is the name: `filename` when it is given, and
    // `"blob"` when it is not. Both cases reach here — a `File` source with a
    // `filename` took the second branch of *create an entry*, and every `Blob`
    // source took its first.
    Ok(insert_file_record(
        ctx,
        BlobRecord { bytes, blob_type },
        filename.unwrap_or("blob"),
        last_modified,
    ))
}

/// The entry value a `(Blob or USVString)` argument and an optional `filename`
/// make: HTML's *create an entry* read from the value half alone, with the name
/// half supplied by the caller.
///
/// The string arm is already the standard's "convert value into a scalar value
/// string": the union's conversion ran `string_from_v8`, which is where
/// `append("k", undefined)` becomes `"undefined"` and `append("k", null)`
/// becomes `"null"` (`append.any.js`'s "testFormDataAppendUndefined1" and
/// "testFormDataAppendNull1").
fn entry_value(
    ctx: &SharedContext,
    value: BlobOrUsvString,
    filename: Option<&str>,
    context: &str,
) -> Result<EntryValue, JsError> {
    match value {
        BlobOrUsvString::UsvString(text) => Ok(EntryValue::String(text)),
        BlobOrUsvString::Blob(handle) => Ok(EntryValue::File(entry_file(
            ctx, handle, filename, context,
        )?)),
    }
}

/// Gives back the `File` records a mutation has dropped from an entry list.
///
/// "Dropped" is computed against the list *after* the mutation rather than
/// counted per entry: an entry list may name one `File` under several names, and
/// what a record's owner list holds is the `FormData`, once. A record nothing
/// refers to any more — no owner and no live wrapper — is freed here, which is
/// the one thing a plain drop of the old list could not do.
fn relinquish_files<'a>(
    slab: &mut HostSlab,
    owner: HostKey,
    dropped: impl Iterator<Item = &'a HostKey>,
) {
    // The freed keys are collected but unused: a `File` owns no interned value
    // and is not an event target, so there is nothing for `reclaim_host_record`'s
    // caller to drain on its behalf. `GenSlab::remove` bumps the generation, so
    // a key reached twice here frees at most one record.
    let mut freed = ReclaimedHost::default();
    for file in dropped {
        if let Some(HostRecord::File(record)) = slab.get_mut(*file) {
            record.remove_owner(owner);
        }
        if matches!(slab.get(*file), Some(HostRecord::File(record)) if record.is_free()) {
            free_record(slab, *file, &mut freed);
        }
    }
}

/// Applies a mutation to the entry list of the `FormData` at `key`, keeping
/// every `File` record's ownership in step with the list.
///
/// Every mutating operation goes through this, so that "the list no longer names
/// this key" and "this `FormData` is no longer an owner of that record" cannot
/// drift apart: `append` and `set` register the keys the new list holds, and a
/// key the list has given up is relinquished and freed if nothing else holds it.
fn mutate_form_data(
    slab: &mut HostSlab,
    key: HostKey,
    f: impl FnOnce(&mut Vec<(String, EntryValue)>),
) {
    let (before, after) = {
        let Some(HostRecord::FormData(record)) = slab.get_mut(key) else {
            return;
        };
        let before = file_keys(&record.list);
        f(&mut record.list);
        let after = file_keys(&record.list);
        (before, after)
    };
    for file in &after {
        if let Some(HostRecord::File(record)) = slab.get_mut(*file) {
            record.add_owner(key);
        }
    }
    relinquish_files(
        slab,
        key,
        before.iter().filter(|file| !after.contains(file)),
    );
}

impl FormDataConstructor for HostObject {
    fn construct(
        realm: &Realm,
        form: Option<Handle<HTMLFormElement>>,
        _submitter: Option<Option<Handle<HTMLElement>>>,
    ) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        // The standard's step 3 reads `submitter` only *inside* its `form`
        // non-null branch, so a `submitter` without a `form` is ignored exactly
        // as it is here — the argument still converts (WebIDL converts every
        // argument), and a value that is not an `HTMLElement` still throws.
        //
        // A non-null `form` needs HTML's *constructing the entry list*, which
        // walks a form's submittable elements in tree order. That is the part
        // this engine cannot do, and refusing it by name is the direction slice
        // b-6.5 chose for `RequestInit`'s unserved members: a `FormData` quietly
        // built from no form would be an empty object where the page asked for
        // the form's fields.
        if form.is_some() {
            return Err(JsError::dom_exception(
                "NotSupportedError",
                "this engine cannot construct a FormData from a form",
            ));
        }
        // Nothing is inserted before this point, so a refused construction
        // leaves the slab exactly as it found it.
        let key = ctx
            .host_records
            .borrow_mut()
            .insert(HostRecord::FormData(FormDataRecord { list: Vec::new() }));
        Ok(HostObject::host_record(ctx, key))
    }
}

impl HostObject {
    /// Appends one entry to the `FormData` this host stands for.
    ///
    /// `append` never looks at the names already in the list — the standard's
    /// step is "append an entry ... to this's entry list" and nothing else — so
    /// the order of the entries is the order of the calls, duplicates and all.
    fn append_entry(&self, name: &str, value: EntryValue) {
        let HostObject::Host { ctx, key } = self else {
            return;
        };
        mutate_form_data(&mut ctx.host_records.borrow_mut(), *key, |list| {
            list.push((name.to_owned(), value));
        });
    }

    /// The standard's *set an entry*: the first entry named `name` takes the new
    /// value in place, every *other* entry named `name` is removed, and a name
    /// the list does not hold at all is appended.
    ///
    /// In place is what makes the position observable: `set` on an existing name
    /// leaves the entry where it was, where a remove-then-append pair would move
    /// it to the end. The final append is the step that makes `set` usable on a
    /// fresh `FormData`, which is `set.any.js`'s "testFormDataSet1" — without it
    /// the call would be a silent no-op and `get` would answer `null`.
    fn set_entry(&self, name: &str, value: EntryValue) {
        let HostObject::Host { ctx, key } = self else {
            return;
        };
        mutate_form_data(&mut ctx.host_records.borrow_mut(), *key, |list| {
            let mut replaced = false;
            list.retain_mut(|(entry_name, entry_value)| {
                if entry_name != name {
                    return true;
                }
                if replaced {
                    return false;
                }
                replaced = true;
                *entry_value = value.clone();
                true
            });
            if !replaced {
                list.push((name.to_owned(), value));
            }
        });
    }
}

impl FormDataOps for HostObject {
    fn append_2(&self, name: &str, value: BlobOrUsvString) -> Result<(), JsError> {
        let value = entry_value(
            self.context(),
            value,
            None,
            "Failed to execute 'append' on 'FormData'",
        )?;
        self.append_entry(name, value);
        Ok(())
    }

    fn append_3(
        &self,
        name: &str,
        blob_value: Handle<Blob>,
        filename: Option<&str>,
    ) -> Result<(), JsError> {
        let file = entry_file(
            self.context(),
            blob_value,
            filename,
            "Failed to execute 'append' on 'FormData'",
        )?;
        self.append_entry(name, EntryValue::File(file));
        Ok(())
    }

    fn delete(&self, name: &str) {
        let HostObject::Host { ctx, key } = self else {
            return;
        };
        // Every entry named `name`, and only those: the standard's step is
        // "remove all the entries ... whose name is name".
        mutate_form_data(&mut ctx.host_records.borrow_mut(), *key, |list| {
            list.retain(|(entry_name, _)| entry_name != name);
        });
    }

    fn get(&self, name: &str) -> Result<Option<FileOrUsvString>, JsError> {
        // The *first* entry named `name`, absent as `null`: the generated glue
        // turns `None` into `null` for the nullable return type, which is what
        // `get.any.js`'s "testFormDataGetNull1" asserts.
        Ok(self.with_form_data(None, |record| {
            record
                .list
                .iter()
                .find(|(entry_name, _)| entry_name == name)
                .map(|(_, value)| entry_to_union(self.slot(), value))
        }))
    }

    fn get_all(&self, name: &str) -> Result<Vec<FileOrUsvString>, JsError> {
        // Every entry named `name`, in list order, as an empty sequence when
        // there are none.
        Ok(self.with_form_data(Vec::new(), |record| {
            record
                .list
                .iter()
                .filter(|(entry_name, _)| entry_name == name)
                .map(|(_, value)| entry_to_union(self.slot(), value))
                .collect()
        }))
    }

    fn has(&self, name: &str) -> bool {
        self.with_form_data(false, |record| {
            record.list.iter().any(|(entry_name, _)| entry_name == name)
        })
    }

    fn set_2(&self, name: &str, value: BlobOrUsvString) -> Result<(), JsError> {
        let value = entry_value(
            self.context(),
            value,
            None,
            "Failed to execute 'set' on 'FormData'",
        )?;
        self.set_entry(name, value);
        Ok(())
    }

    fn set_3(
        &self,
        name: &str,
        blob_value: Handle<Blob>,
        filename: Option<&str>,
    ) -> Result<(), JsError> {
        let file = entry_file(
            self.context(),
            blob_value,
            filename,
            "Failed to execute 'set' on 'FormData'",
        )?;
        self.set_entry(name, EntryValue::File(file));
        Ok(())
    }

    fn iterable_entries(&self) -> Result<IterableEntries<String, FileOrUsvString>, JsError> {
        // One snapshot, taken when the iteration begins, which the generated
        // `@@iterator`, `entries`, `keys`, `values` and `forEach` all view. A
        // mutation *during* iteration is therefore not observed, where WebIDL's
        // default iterator for a pair `iterable` is live — the deviation the
        // declaration's header records as deviation 3, and the test
        // `crates/tenta-js-v8/tests/dom_formdata.rs` pins the slice that fixes
        // it has to flip.
        Ok(self.with_form_data(Vec::new(), |record| {
            record
                .list
                .iter()
                .map(|(name, value)| (name.clone(), entry_to_union(self.slot(), value)))
                .collect()
        }))
    }
}

// ---------------------------------------------------------------------------
// XMLHttpRequest
// ---------------------------------------------------------------------------

/// The event type `open()` fires when the state changes to `OPENED`
/// (<https://xhr.spec.whatwg.org/#the-open()-method>, step 9).
///
/// It is also the event type `onreadystatechange`'s listener listens for:
/// [`EventHandlerName::event_type`] derives that one by stripping the `on`
/// prefix, so the two spellings have to name the same event — which the
/// `readystatechange` dispatch test pins by observing the event from both a
/// listener and the handler attribute.
const READY_STATE_CHANGE: &str = "readystatechange";

/// The three *forbidden methods*, byte-case-insensitively
/// (<https://fetch.spec.whatwg.org/#forbidden-method>). `open()` refuses each
/// with a `SecurityError`.
const FORBIDDEN_METHODS: [&[u8]; 3] = [b"CONNECT", b"TRACE", b"TRACK"];

/// The methods `normalize a method` byte-uppercases; every other method keeps
/// the case the page wrote it in (<https://xhr.spec.whatwg.org/#the-open()-method>).
const NORMALIZED_METHODS: [&[u8]; 6] = [b"DELETE", b"GET", b"HEAD", b"OPTIONS", b"POST", b"PUT"];

/// Whether `byte` is one of HTTP's `tchar`s, which is what the `method`
/// production is built from: `method = token` and `token = 1*tchar`, with
/// `tchar = "!" / "#" / "$" / "%" / "&" / "'" / "*" / "+" / "-" / "." / "^" /
/// "_" / "`" / "|" / "~" / DIGIT / ALPHA` (RFC 9110 §5.6.2).
fn is_tchar(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

/// The *method* production of `open()`: a method is `token`, and `token =
/// 1*tchar` — so the empty byte sequence is not a method, and neither is a
/// sequence containing a byte outside the `tchar` set (RFC 9110 §5.6.2).
///
/// Yields the ASCII text the bytes spell, or `None` when they are not a method.
/// The caller must not `String::from_utf8` them itself: a `tchar` is ASCII, so
/// the two agree on every accepted input and the rejected ones never reach a
/// conversion.
fn method_token(bytes: &[u8]) -> Option<String> {
    let mut token = String::with_capacity(bytes.len());
    for &byte in bytes {
        if !is_tchar(byte) {
            return None;
        }
        // Every byte is checked first, so the cast is exact rather than lossy.
        token.push(byte as char);
    }
    (!token.is_empty()).then_some(token)
}

impl XMLHttpRequestEventTargetOps for HostObject {
    fn onloadstart(&self) -> Result<Option<EventHandlerNonNull>, JsError> {
        self.event_handler(EventHandlerName::new("onloadstart"))
    }

    fn set_onloadstart(&self, value: Option<EventHandlerNonNull>) -> Result<(), JsError> {
        self.set_event_handler(EventHandlerName::new("onloadstart"), value)
    }

    fn onprogress(&self) -> Result<Option<EventHandlerNonNull>, JsError> {
        self.event_handler(EventHandlerName::new("onprogress"))
    }

    fn set_onprogress(&self, value: Option<EventHandlerNonNull>) -> Result<(), JsError> {
        self.set_event_handler(EventHandlerName::new("onprogress"), value)
    }

    fn onabort(&self) -> Result<Option<EventHandlerNonNull>, JsError> {
        self.event_handler(EventHandlerName::new("onabort"))
    }

    fn set_onabort(&self, value: Option<EventHandlerNonNull>) -> Result<(), JsError> {
        self.set_event_handler(EventHandlerName::new("onabort"), value)
    }

    fn onerror(&self) -> Result<Option<EventHandlerNonNull>, JsError> {
        self.event_handler(EventHandlerName::new("onerror"))
    }

    fn set_onerror(&self, value: Option<EventHandlerNonNull>) -> Result<(), JsError> {
        self.set_event_handler(EventHandlerName::new("onerror"), value)
    }

    fn onload(&self) -> Result<Option<EventHandlerNonNull>, JsError> {
        self.event_handler(EventHandlerName::new("onload"))
    }

    fn set_onload(&self, value: Option<EventHandlerNonNull>) -> Result<(), JsError> {
        self.set_event_handler(EventHandlerName::new("onload"), value)
    }

    fn ontimeout(&self) -> Result<Option<EventHandlerNonNull>, JsError> {
        self.event_handler(EventHandlerName::new("ontimeout"))
    }

    fn set_ontimeout(&self, value: Option<EventHandlerNonNull>) -> Result<(), JsError> {
        self.set_event_handler(EventHandlerName::new("ontimeout"), value)
    }

    fn onloadend(&self) -> Result<Option<EventHandlerNonNull>, JsError> {
        self.event_handler(EventHandlerName::new("onloadend"))
    }

    fn set_onloadend(&self, value: Option<EventHandlerNonNull>) -> Result<(), JsError> {
        self.set_event_handler(EventHandlerName::new("onloadend"), value)
    }
}

impl HostObject {
    /// The body of [`XMLHttpRequestOps::send`].
    ///
    /// The XHR Standard's `send()` steps are followed in its order
    /// (<https://xhr.spec.whatwg.org/#the-send()-method>).
    ///
    /// The argument arrives already converted: WebIDL converts an operation's
    /// arguments before its steps run, so `body` is whatever the generated glue
    /// made of the declared `XMLHttpRequestBodyInit?` — an owned `Vec<u8>` or
    /// `String`, no stored reference to hand back. That is why this is a plain
    /// method rather than the release-wrapping one it used to be, and why a
    /// conversion that fails is the glue's to raise rather than this function's.
    fn send_request(&self, body: Option<BodyInit>) -> Result<(), JsError> {
        // Step 1: "If this's state is not opened, then throw an
        // InvalidStateError."
        if self.with_xhr(<HostObject as XMLHttpRequestOps>::UNSENT, |record| {
            record.ready_state
        }) != <HostObject as XMLHttpRequestOps>::OPENED
        {
            return Err(JsError::dom_exception(
                "InvalidStateError",
                "the object's state must be opened",
            ));
        }
        // Step 2: "If this's send() flag is set, then throw an
        // InvalidStateError." The state check catches most of these — a request
        // that has been answered is at `DONE` — but not a request still in
        // flight, which is the case the flag exists for.
        if self.with_xhr(false, |record| record.send_flag) {
            return Err(JsError::dom_exception(
                "InvalidStateError",
                "send() has already been invoked",
            ));
        }

        // Steps 3 and 4 are written below rather than here: step 3 compares the
        // request's method and step 4 reads its author request headers, and both
        // are on the record this method reads once the send flag is set — so the
        // two steps run there, in their own order and before the request is
        // handed over.

        // Steps 5 to 9 belong to the upload object: step 5 sets the upload
        // listener flag when the object has a listener, step 6 builds the
        // request, step 7 clears the upload complete flag, step 8 clears the
        // timed-out flag, and step 9 sets the upload complete flag when the
        // request body is null. With a body now expressible step 9's condition is
        // live rather than settled by a step nothing reaches: a request that
        // carries a body leaves the flag clear, and that is the flag `abort()`'s
        // half of the *request error steps* reads. `XMLHttpRequestUpload` is
        // deferred, so there is no object to hold either flag and no listener for
        // step 5 to register — which is the deviation the IDL header records for
        // a page that sends a body: the standard fires `loadstart`, `progress`,
        // `load` and `loadend` at the upload object while such a request is
        // transmitted and this engine fires none of them. The request itself is
        // built below, out of the record `open()` wrote, because it is the host
        // layer that owns the page's half of it.

        // Step 10: "Set this's send() flag." It is set before the request is
        // built and before `loadstart`, so a `loadstart` listener that re-enters
        // `send()` already sees it — which is the whole purpose of the flag.
        let key = self
            .event_target_key()
            .ok_or_else(|| JsError::type_error("the receiver is not an event target"))?;
        let parts = self.with_xhr_mut(None, |record| {
            record.send_flag = true;
            // The request this object is now waiting for. Stamped here rather
            // than read off the record afterwards because a `loadstart` listener
            // below can re-enter `open()`, which bumps the id again — and the
            // handle, and the refusal path's delivery, must name *this* request
            // rather than the one that replaced it.
            record.request_id = record.request_id.wrapping_add(1);
            Some((
                record.request_id,
                record.url.clone()?,
                record.method.clone()?,
                record.author_headers.clone(),
            ))
        });
        // A total refusal rather than an `unwrap`: the state check above passed
        // with the state opened, and only `open()` writes the method and the URL
        // — together with that state — but nothing here is worth a panic on.
        let Some((request_id, url, method, headers)) = parts else {
            return Err(JsError::dom_exception(
                "InvalidStateError",
                "the object's state must be opened",
            ));
        };

        // Step 3: "If this's request method is `GET` or `HEAD`, then set body to
        // null." The comparison is on the method as HTTP reads it, the same one
        // the `fetch()` initializer makes, rather than on the string `open()`
        // stored: `open()` validated that string against HTTP's `token`
        // production, so the conversion here is total and `expect` is not a
        // reachable panic.
        let method = http::Method::from_bytes(method.as_bytes())
            .expect("open() validated the method as HTTP's `token` production");
        let body = match method {
            http::Method::GET | http::Method::HEAD => None,
            _ => body,
        };

        // The tail of step 4 — its own sub-steps 5 and 6. Sub-step 5 is "If
        // originalAuthorContentType is non-null", whose only branch is the
        // `charset` rewrite recorded as a deviation in the IDL header; sub-step 6
        // is its `Otherwise` — "Otherwise, if extractedContentType is not null,
        // set (`Content-Type`, extractedContentType) in this's author request
        // headers" — which is the rule implemented here, and the reason an author
        // `Content-Type` survives: that `Otherwise` is only reached when the
        // author set none. `set_inferred_content_type` is the same one function
        // the `Response` constructor and the `fetch()` initializer run. It writes
        // into the copy step 10 took off the record rather than into the record's
        // own list: nothing reads the author request headers again once the send
        // flag is set — `setRequestHeader` throws on it, and `open()` clears it —
        // so the write reaches the request, which is the only place it is read.
        let mut headers = headers;
        let body = match body {
            Some(body) => {
                let (bytes, body_type) = extract_body(self.context(), body)?;
                set_inferred_content_type(&mut headers, body_type.as_deref());
                Some(bytes)
            }
            None => None,
        };

        // Step 11: "Fire a progress event named loadstart at this with 0 and 0."
        // It is the one event `send()` fires itself, and it comes before the
        // request is handed over, so a listener sees the object as it is about to
        // make its request. The `fetched` marker Fetch's *fire a progress event*
        // reads is deliberately not a parameter: nothing in this tree
        // synthesizes a request from a cached response.
        let ctx = self.context();
        fire_xhr_progress(ctx, DispatchTarget::host(ctx.slot, key), "loadstart", 0, 0);

        // The request crosses the callback boundary here: the page's half is the
        // carrier below, and the engine's — the calling task's provenance — is
        // added by the backend, which is also what holds the driver slot.
        let initiator = ctx
            .doc
            .borrow()
            .url()
            .cloned()
            .unwrap_or_else(|| url.clone());
        let with_credentials = self.with_xhr(false, |record| record.with_credentials);
        let request = XhrRequest {
            url,
            method,
            headers,
            body,
            initiator,
            with_credentials,
        };
        // The borrow of the script slot is taken and released *before* the call:
        // a driver runs from inside `start_xhr`, and a borrow held across it
        // would be a re-entrancy alias on the context the driver's page callback
        // uses — the same rule `release_stored`'s callers state. Binding the
        // clone to a `let` is what ends the borrow here; leaving the `borrow()`
        // in the match's scrutinee would hold it until the end of the statement
        // below, i.e. across the very call it must not span.
        let script = ctx.script.borrow().as_ref().map(Rc::clone);
        let handle = XhrHandle {
            target: encode_host(ctx.slot, key),
            request: request_id,
        };
        let refusal = match script {
            Some(script) => script.start_xhr(request, handle).err(),
            // A world with no script host is a world with no driver either, so
            // it reports the same refusal the backend does for an empty slot.
            None => Some(FetchStartError::new("XMLHttpRequest is not available")),
        };

        // `Err` means no request was started, and what the page observes is the
        // standard's *request error steps* — exactly the failure sequence b-5a2's
        // machine implements — so it is driven through that machine rather than
        // through a second copy of it. The id is the one this `send()` stamped,
        // so the delivery is the live request's and would be discarded if a
        // `loadstart` listener had re-opened the object meanwhile.
        if let Some(error) = refusal {
            let outcome = FetchOutcome::Failed(error.message().to_owned());
            deliver_xhr_completion(
                ctx,
                DispatchTarget::host(ctx.slot, key),
                request_id,
                &outcome,
            );
            return Ok(());
        }

        // The request's `timeout` is armed now, which is where the standard arms
        // it — in this method's "Run these steps in parallel" branch: "Wait until
        // either req's done flag is set or this's timeout is not 0 and this's
        // timeout milliseconds have passed since now. If req's done flag is
        // unset, then set this's timed out to true and terminate this's fetch
        // controller" (<https://xhr.spec.whatwg.org/#the-send()-method>). A
        // timeout of 0 arms nothing, which is the whole of what "the setter's 0
        // disarms it" means, and a refused request has nothing to time.
        //
        // The value is read here rather than carried by the request: it is the
        // record's, and the standard reads the *current* one at the deadline. The
        // deadline itself is fixed here, which is the one place this engine's
        // timer parts company with that sentence — a page that raises `timeout`
        // while a request is in flight moves the bound for its next request and
        // not for this one.
        let timeout = self.with_xhr(0, |record| record.timeout);
        if timeout != 0 {
            arm_xhr_timeout(
                ctx,
                DispatchTarget::host(ctx.slot, key),
                key,
                handle,
                timeout,
            );
        }
        Ok(())
    }
}

impl XMLHttpRequestOps for HostObject {
    fn onreadystatechange(&self) -> Result<Option<EventHandlerNonNull>, JsError> {
        self.event_handler(EventHandlerName::new("onreadystatechange"))
    }

    fn set_onreadystatechange(&self, value: Option<EventHandlerNonNull>) -> Result<(), JsError> {
        self.set_event_handler(EventHandlerName::new("onreadystatechange"), value)
    }

    fn ready_state(&self) -> u16 {
        self.with_xhr(<HostObject as XMLHttpRequestOps>::UNSENT, |record| {
            record.ready_state
        })
    }

    fn open(
        &self,
        method: &[u8],
        url: &str,
        r#async: Option<bool>,
        username: Option<Option<&str>>,
        password: Option<Option<&str>>,
    ) -> Result<(), JsError> {
        // The XHR Standard's `open()` steps, in its order. Step 1 — "if this's
        // relevant global object is not fully active, then throw an
        // InvalidStateError" — is omitted: this product has no browsing context
        // that can be not fully active and no way to ask for one, so there is
        // nothing the step could test.
        //
        // Steps 2 to 4: the method must be a method, must not be forbidden, and
        // is then normalised. The standard declares `method` as a `ByteString`,
        // and its validity test is defined over bytes, so `method_token` reads
        // the argument as the page's own code units rather than as a `USVString`
        // re-encoded to UTF-8.
        let Some(mut normalized) = method_token(method) else {
            return Err(JsError::dom_exception(
                "SyntaxError",
                "the method is not a valid HTTP method token",
            ));
        };
        if FORBIDDEN_METHODS
            .iter()
            .any(|forbidden| method.eq_ignore_ascii_case(forbidden))
        {
            return Err(JsError::dom_exception(
                "SecurityError",
                "the method is forbidden",
            ));
        }
        if NORMALIZED_METHODS
            .iter()
            .any(|known| method.eq_ignore_ascii_case(known))
        {
            normalized.make_ascii_uppercase();
        }

        // Steps 5 to 7: parse the URL against the document's URL, and let an
        // omitted `async` default to true. The base is the *document's* URL, not
        // the caller's: a relative URL in `xhr.open('GET', '/x')` resolves
        // against the page, which is what a page expects and what the fetch path
        // does with its `initiator`.
        let base = self.context().doc.borrow().url().cloned();
        let parsed = match &base {
            Some(base) => base.join(url),
            None => url::Url::parse(url),
        }
        .map_err(|_| JsError::dom_exception("SyntaxError", "the URL could not be parsed"))?;
        let r#async = r#async.unwrap_or(true);

        // Step 8's first part, refused for the same reason `withCredentials` is:
        // the standard writes a non-null username or password onto the parsed
        // URL, and this engine sends no credentialed request at all. Storing
        // them would again promise something the request will not carry — and
        // what the network layer would do with URL userinfo is not something
        // this slice has verified.
        if username.flatten().is_some() || password.flatten().is_some() {
            return Err(JsError::dom_exception(
                "NotSupportedError",
                "this engine cannot send a request with credentials",
            ));
        }

        // Step 9, refused for every `async = false` rather than only for the
        // condition the standard names (a non-zero `timeout` or a non-empty
        // `responseType`): a synchronous request cannot be expressed by the
        // network driver (`tenta-net` is asynchronous all the way down) and
        // would block the page's own turn loop, deadlocking the settle machinery
        // rather than merely answering slowly. Refusing loudly is the only
        // honest answer; silently treating `false` as `true` would hand the page
        // a result it did not ask for.
        if !r#async {
            return Err(JsError::dom_exception(
                "NotSupportedError",
                "this engine cannot send a synchronous request",
            ));
        }

        // Step 10: "Terminate this's fetch controller." `send()` leaves the
        // object at `OPENED` for as long as its request is unanswered, so this
        // step *is* reachable with a live request, and re-opening an object must
        // not leave two of them in flight — one of them answering an object that
        // has moved on. It is terminated through the seam the request left by
        // (`ScriptHost::abort_xhr`), which is the same machinery `abort()` uses
        // and the reason the two landed together.
        //
        // The termination is not the whole of the protection, and both halves
        // are needed. The driver is on the other side of the seam, so an answer
        // can already be in its table when this runs, and an aborted task is
        // still one whose completion was pushed a moment earlier — so the id
        // moves on as well, and `deliver_xhr_completion` discards a completion
        // that does not match it. The id moves first and the driver is asked
        // after, so the one ordering in which a racing completion could still be
        // delivered does not exist: by the time the request can stop costing a
        // socket it is *already* invisible. Both halves are `end_xhr_request`'s.
        //
        // Step 11's *reset associated variables* is otherwise complete here.
        // "Set this's send() flag to unset" and "set this's author request
        // headers to an empty header list" are the two the record now owns, and
        // they matter for the same reason the response reset does: re-opening an
        // object whose request was in flight must leave it able to start a fresh
        // one, and must not carry the previous request's headers into it. "Set
        // this's response to a network error" is the third — with the received
        // bytes and the response object — so that a request whose answer already
        // arrived answers `0` / `""` / `null` again once it is re-opened. The
        // override MIME type is deliberately not reset, and the standard says
        // why: it may be set before `open()` is called.
        //
        // Step 12: "if this's state is not opened, then set this's state to
        // opened and fire an event named readystatechange at this". A second
        // `open()` on an already-opened object therefore rewrites the request
        // and fires nothing.
        let key = self
            .event_target_key()
            .ok_or_else(|| JsError::type_error("the receiver is not an event target"))?;
        let ctx = self.context();
        end_xhr_request(ctx, key);
        let opened = self.with_xhr_mut(false, |record| {
            record.method = Some(normalized);
            record.url = Some(parsed);
            record.send_flag = false;
            record.author_headers.clear();
            record.response = None;
            let opened = record.ready_state != OPENED;
            record.ready_state = OPENED;
            opened
        });
        if !opened {
            return Ok(());
        }

        // The event is UA-created, so `isTrusted` is true; "fire an event named
        // e at target" gives it no bubbles, no cancelable and no default action.
        // It is fired here, on this stack, which is what makes a listener
        // registered before the call run before `open()` returns — the same
        // shape `AbortSignal::abort` gives its `abort` event.
        let mut record = EventRecord::new(
            READY_STATE_CHANGE.to_owned(),
            EventKind::Plain,
            ctx.time_stamp(),
        );
        record.flags.initialized = true;
        record.is_trusted = true;
        let event_key = ctx.events.borrow_mut().insert(record);
        let dispatched = dispatch_at(ctx, DispatchTarget::host(ctx.slot, key), event_key, true);
        // Remove the transient record unless a listener made a wrapper for it
        // (in which case the world's dead-event sweep owns its removal).
        let orphaned = ctx
            .events
            .borrow()
            .get(event_key)
            .is_some_and(|record| !record.has_wrapper);
        if orphaned {
            ctx.events.borrow_mut().remove(event_key);
        }
        dispatched.map(|_| ())
    }

    fn set_request_header(&self, name: &[u8], value: &[u8]) -> Result<(), JsError> {
        // The XHR Standard's `setRequestHeader(name, value)` steps, in its
        // order, which is the part of it that is easy to get wrong
        // (<https://xhr.spec.whatwg.org/#the-setrequestheader()-method>).
        //
        // Steps 1 and 2: the state must be opened and the send() flag unset.
        // They come *before* either argument is looked at, so a bogus name on an
        // unopened object is an `InvalidStateError` rather than the
        // `SyntaxError` step 4 would make of it.
        if self.with_xhr(<HostObject as XMLHttpRequestOps>::UNSENT, |record| {
            record.ready_state
        }) != <HostObject as XMLHttpRequestOps>::OPENED
        {
            return Err(JsError::dom_exception(
                "InvalidStateError",
                "the object's state must be opened",
            ));
        }
        if self.with_xhr(false, |record| record.send_flag) {
            return Err(JsError::dom_exception(
                "InvalidStateError",
                "send() has already been invoked",
            ));
        }

        // Steps 3 and 4: normalize the value, then check the pair against
        // Fetch's *header name* and *header value* definitions. Both checks are
        // the ones `Headers` already makes — `normalize_header_name` is RFC
        // 9110's field-name `token` production and `normalize_header_value` is
        // Fetch's *normalize* followed by its NUL/CR/LF test — so the request
        // side of that rule reuses them rather than restating either. Only the
        // exception differs: the standard says `SyntaxError` here, where the
        // `Headers` mutators throw a `TypeError`.
        //
        // Because step 3 normalizes first, the leading-and-trailing half of the
        // value rule is unreachable from here; the NUL/CR/LF half is not.
        let context = "Failed to execute 'setRequestHeader' on 'XMLHttpRequest'";
        let name = normalize_header_name(context, name)
            .map_err(|_| JsError::dom_exception("SyntaxError", "the header name is not valid"))?;
        let value = normalize_header_value(context, value)
            .map_err(|_| JsError::dom_exception("SyntaxError", "the header value is not valid"))?;

        // Step 5: a *forbidden request-header* is dropped silently — no throw,
        // and nothing stored. The predicate is over the pair, after
        // normalization, because both halves decide it.
        if is_forbidden_request_header(&name, &value) {
            return Ok(());
        }

        // Step 6: "combine (name, value) in this's author request headers."
        // Fetch's *combine* joins the new value onto the one already stored
        // under the name with `", "`, and appends the pair when the name is new.
        // The name comparison is byte-case-insensitive; every stored name has
        // been through `normalize_header_name`, so it is a plain equality.
        self.with_xhr_mut((), |record| {
            match record
                .author_headers
                .iter()
                .position(|(stored, _)| stored.as_slice() == name.as_slice())
            {
                Some(index) => {
                    let stored = &mut record.author_headers[index].1;
                    stored.extend_from_slice(b", ");
                    stored.extend_from_slice(&value);
                }
                None => record.author_headers.push((name, value)),
            }
        });
        Ok(())
    }

    fn timeout(&self) -> u32 {
        self.with_xhr(0, |record| record.timeout)
    }

    fn set_timeout(&self, value: u32) {
        // The standard's setter stores any value here and arms nothing: the
        // timer it describes is armed by `send()`, and 0 is what makes that
        // arming a no-op. The setter's one throw — an `InvalidAccessError` for a
        // synchronous request on a `Window` — is unreachable in this engine,
        // because `open()` refuses `async = false` for every case, so this
        // cannot fail and the member is declared without `[Throws]`
        // (`idl/xhr/XMLHttpRequest.webidl`).
        self.with_xhr_mut((), |record| record.timeout = value);
    }

    fn with_credentials(&self) -> Result<bool, JsError> {
        Ok(self.with_xhr(false, |record| record.with_credentials))
    }

    fn set_with_credentials(&self, value: bool) -> Result<(), JsError> {
        // The value is stored and `send()` hands it to the driver, which sends
        // cookies to another origin only when it is set and shares the response
        // only when that origin allows credentials by name.
        self.with_xhr_mut((), |record| record.with_credentials = value);
        Ok(())
    }

    fn send(&self, body: Option<Option<BodyInit>>) -> Result<(), JsError> {
        // The outer `None` is an omitted argument and the inner one the IDL's
        // nullable `body`; the declared union is converted by the generated glue,
        // which makes all three of an absent argument, `undefined` and `null` the
        // same `None` — so both nulls are the null body step 3 keeps, and there is
        // nothing to release: the conversion produced an owned `Vec<u8>` or
        // `String` rather than a stored reference.
        self.send_request(body.flatten())
    }

    /// The XHR Standard's `abort()` steps, in its order
    /// (<https://xhr.spec.whatwg.org/#the-abort()-method>).
    ///
    ///  1. "Abort this's fetch controller."
    ///  2. "If this's state is opened with this's send() invoked being true,
    ///     headers received, or loading, then run the request error steps for
    ///     this and abort."
    ///  3. "If this's state is done, then set this's state to unsent and this's
    ///     response to a network error." — "No readystatechange event is
    ///     dispatched."
    ///
    /// The three steps answer a page's three cases, and the WPT suite pins each.
    /// An object with a request in flight (`abort-after-send.any.js`) runs step 2
    /// — `readystatechange` at `DONE`, then `abort`, then `loadend`, all
    /// synchronous — and then step 3, so the state a caller reads after the call
    /// is `UNSENT` and not `DONE`. An object whose answer already arrived
    /// (`abort-after-receive.any.js`) runs step 3 alone: the state goes back to
    /// `UNSENT`, the response to a network error, and **nothing fires**. An
    /// object that was opened but never sent (`abort-during-unsent.any.js`)
    /// matches neither condition and does nothing at all; that is the no-op the
    /// standard's own wording implies, rather than a refusal.
    ///
    /// Step 3 running after step 2 is what makes the state `UNSENT` at the end
    /// of the first case, and it is easy to read the two as alternatives: the
    /// condition is evaluated *after* step 2 has set the state to done, so it is
    /// true for an in-flight request as well. The events step 2 fires still see
    /// `DONE` — a `loadend` listener reads 4 — and the reset lands when the call
    /// returns.
    ///
    /// The upload object's half of step 2 is deferred with the object itself
    /// (`idl/xhr/XMLHttpRequest.webidl`). The standard fires the failing event and
    /// its `loadend` at the upload object first, but only when the upload is not
    /// complete, and that flag is set by `send()` step 9 exactly when the request
    /// has no body — so the half is unobservable for a bodyless request and
    /// **observable for one that carries a body**, which `send()` now accepts.
    /// This engine fires neither event at any point, so a page that sends a body
    /// and aborts sees two events where a browser fires four.
    ///
    /// Nothing in these steps raises, which is why the member is declared without
    /// `[Throws]` and returns `()`.
    fn abort(&self) {
        let Some(key) = self.event_target_key() else {
            return;
        };
        let ctx = self.context();
        let target = DispatchTarget::host(ctx.slot, key);
        // Step 2's condition is read *before* step 2 runs: the request error
        // steps set the state to done, and step 3 then reads what they left
        // there.
        let inflight = self.with_xhr(false, |record| {
            let state = record.ready_state;
            (state == OPENED && record.send_flag) || state == HEADERS_RECEIVED || state == LOADING
        });
        // Step 1 ("Abort this's fetch controller") and this engine's half of the
        // guarantee it makes: whatever the driver still holds for this object —
        // or has already pushed into its table — belongs to a request the object
        // no longer recognises, and is ended and discarded together
        // (`end_xhr_request`).
        end_xhr_request(ctx, key);
        if inflight {
            // Step 2. `abort` is the event name the *request error steps* are run
            // with — the same algorithm a network failure runs with `error` and a
            // timeout with `timeout` — and `AbortError` is the exception it is
            // given, which only a synchronous request could have thrown.
            xhr_request_error_steps(ctx, target, key, "abort");
        }
        // Step 3, in the standard's own order: an object that was already done
        // and one step 2 has just put there both reset, and neither fires an
        // event for it.
        if self.with_xhr(false, |record| record.ready_state == DONE) {
            store_xhr_response(ctx, key, None);
            set_xhr_state(ctx, key, UNSENT);
        }
    }

    fn status(&self) -> u16 {
        // One delegating step: "return this's response's status". There is no
        // per-state rule to write, because the state that has no answer is
        // exactly the state whose response *is* Fetch's network error, whose
        // status is 0 — so `None` here is the standard's value and not a stub.
        self.with_xhr(0, |record| {
            record
                .response
                .as_ref()
                .map_or(0, |response| response.status)
        })
    }

    fn status_text(&self) -> Result<Vec<u8>, JsError> {
        // "Return this's response's status message", the network error's being
        // the empty byte sequence. See `status` for why there is no state test.
        Ok(self.with_xhr(Vec::new(), |record| {
            record
                .response
                .as_ref()
                .map_or_else(Vec::new, |response| response.status_text.clone())
        }))
    }

    fn get_response_header(&self, name: &[u8]) -> Result<Option<String>, JsError> {
        // One step: "return the result of getting name from this's response's
        // header list", which is Fetch's *get* — `combined_header_value`, the
        // same function `Headers` reads through — and its answer for a name the
        // list does not hold is null, never the empty string
        // (`xhr/getresponseheader-case-insensitive.htm`). An unsent or opened
        // object answers null for every name, because a network error's header
        // list is empty.
        //
        // Fetch's *get* matches byte-case-insensitively and every stored name is
        // already byte-lowercase, so the argument is lowercased rather than
        // parsed. Parsing is what `Headers.get` does, through
        // `normalize_header_name`, and it *throws* for a name that is not an
        // HTTP token — but this member has no such step: a name that is not a
        // header name is a name the list does not hold, so
        // `getResponseHeader('x-custom-header ')`, `'x¾'` and `'x:'` are each
        // null (`xhr/getresponseheader-special-characters.htm`).
        let name = name.to_ascii_lowercase();
        let value = self.with_xhr(None, |record| {
            record
                .response
                .as_ref()
                .and_then(|response| combined_header_value(&response.headers, &name))
        });
        // A header value is a byte sequence and the IDL declares this member a
        // `DOMString?`, so it reaches script through the Encoding Standard's
        // UTF-8 decode: obs-text is not UTF-8, and a value holding it reads back
        // with U+FFFD in place of the bytes that are not.
        Ok(value.map(|value| String::from_utf8_lossy(&value).into_owned()))
    }

    fn get_all_response_headers(&self) -> Result<Vec<u8>, JsError> {
        // "Let initialHeaders be the result of running sort and combine with
        // this's response's header list", then "let headers be the result of
        // sorting initialHeaders in ascending order, with a being less than b if
        // a's name is legacy-uppercased-byte less than b's name". The second sort
        // is the one place this member is not byte order: the comparison
        // byte-uppercases both names first, "unfortunately needed for
        // compatibility with deployed content", so `x-mix` sorts before
        // `x-zebra` where byte order would put it after
        // (<https://xhr.spec.whatwg.org/#the-getallresponseheaders()-method>).
        //
        // Each pair is `name: value\r\n`; a request with no response has an
        // empty header list, so the output is empty without a branch of its own.
        Ok(self.with_xhr(Vec::new(), |record| {
            let Some(response) = &record.response else {
                return Vec::new();
            };
            let mut headers = sort_and_combine_headers(&response.headers);
            headers.sort_by_key(|header| header.0.to_ascii_uppercase());
            let mut output = Vec::new();
            for (name, value) in headers {
                output.extend_from_slice(&name);
                output.extend_from_slice(b": ");
                output.extend_from_slice(&value);
                output.extend_from_slice(b"\r\n");
            }
            output
        }))
    }

    fn override_mime_type(&self, mime: &str) -> Result<(), JsError> {
        // The standard's three steps. Step 1 throws an `InvalidStateError` when
        // the state is loading or done, which is its own rule rather than one of
        // this engine's narrowings — and it is reachable only now, because only
        // now does a request reach those states.
        //
        // Steps 2 and 3 store the parsed value, or `application/octet-stream`
        // when it does not parse. Only the `charset` parameter is stored, since
        // it is the only part a text response reads; a type that fails to parse
        // has no charset, exactly like the `application/octet-stream` step 3
        // puts in its place, so the two are the same value here.
        let state = self.with_xhr(<HostObject as XMLHttpRequestOps>::UNSENT, |record| {
            record.ready_state
        });
        if state == <HostObject as XMLHttpRequestOps>::LOADING
            || state == <HostObject as XMLHttpRequestOps>::DONE
        {
            return Err(JsError::dom_exception(
                "InvalidStateError",
                "the response is already being received",
            ));
        }
        self.with_xhr_mut((), |record| {
            record.override_charset =
                parse_mime_type(mime.as_bytes()).and_then(|mime_type| mime_type.charset);
        });
        Ok(())
    }

    fn response_type(&self) -> Result<XMLHttpRequestResponseType, JsError> {
        Ok(
            self.with_xhr(XMLHttpRequestResponseType::EmptyString, |record| {
                record.response_type
            }),
        )
    }

    fn set_response_type(&self, value: XMLHttpRequestResponseType) -> Result<(), JsError> {
        // The standard's setter, in its order. Step 1 (a `document` value on a
        // non-`Window` global object is ignored) cannot be reached from a
        // `Window`, and step 3's `InvalidAccessError` needs a synchronous
        // request, which `open()` refuses — but step 2 is reachable now: the
        // state being loading or done is an `InvalidStateError`, thrown before
        // the value is looked at, so `xhr.responseType = ''` on a finished
        // request throws as well.
        //
        // Step 4 is this engine's narrowing rather than the standard's: the
        // enumeration is the standard's and every member of it converts, but
        // only `""` and `"text"` are values a decoded-text response can serve,
        // and the refusal is a `NotSupportedError` and not the `TypeError` an
        // unknown enumeration value produces.
        let state = self.with_xhr(<HostObject as XMLHttpRequestOps>::UNSENT, |record| {
            record.ready_state
        });
        if state == <HostObject as XMLHttpRequestOps>::LOADING
            || state == <HostObject as XMLHttpRequestOps>::DONE
        {
            return Err(JsError::dom_exception(
                "InvalidStateError",
                "the response is already being received",
            ));
        }
        match value {
            XMLHttpRequestResponseType::Document => Err(JsError::dom_exception(
                "NotSupportedError",
                "this engine cannot serve a document response",
            )),
            _ => {
                self.with_xhr_mut((), |record| record.response_type = value);
                Ok(())
            }
        }
    }

    fn response_url(&self) -> Result<String, JsError> {
        // "the empty string if this's response's URL is null; otherwise its
        // serialization with the exclude fragment flag set."
        Ok(self.with_xhr(String::new(), |record| {
            record
                .response
                .as_ref()
                .and_then(|response| Url::parse(&response.url).ok())
                .map(|mut url| {
                    url.set_fragment(None);
                    url.into()
                })
                .unwrap_or_default()
        }))
    }

    fn response(&self) -> Result<AnyValue, JsError> {
        // The standard's getter. For `""` and `"text"` it is the text response
        // (the empty string before the state is loading or done). For the other
        // types it is `null` until the state is done, and then the *response
        // object*, built on the first read and the same value on every later
        // one: an `ArrayBuffer`, a `Blob`, or the parsed JSON, with `null` for a
        // body that does not parse and for a network error.
        let done = <HostObject as XMLHttpRequestOps>::DONE;
        let (built_for, cached) = self.with_xhr(
            (
                (0, 0, XMLHttpRequestResponseType::EmptyString),
                None::<XhrResponseValue>,
            ),
            |record| {
                (
                    (record.request_id, record.ready_state, record.response_type),
                    record.response_value,
                )
            },
        );
        if let Some(value) = cached
            && value.built_for == built_for
        {
            return Ok(AnyValue(value.bits));
        }
        let (_, ready_state, response_type) = built_for;
        let text = matches!(
            response_type,
            XMLHttpRequestResponseType::EmptyString | XMLHttpRequestResponseType::Text
        );
        if !text && ready_state != done {
            return Ok(AnyValue::NULL);
        }
        let script = self.script_host()?;
        let bits = if text {
            script.build_value(HostValue::String(self.text_response()))
        } else {
            let HostObject::Host { ctx, .. } = self else {
                return Ok(AnyValue::NULL);
            };
            // A network error has no response at all, and that is `null`
            // rather than an empty value.
            let Some((body, blob_type)) = self.with_xhr(None, |record| {
                record.response.as_ref().map(|response| {
                    (
                        response.body.clone().unwrap_or_default(),
                        body_mime_type(&response.headers).unwrap_or_default(),
                    )
                })
            }) else {
                return Ok(AnyValue::NULL);
            };
            match response_type {
                XMLHttpRequestResponseType::Arraybuffer => {
                    script.build_value(HostValue::ArrayBuffer(body))
                }
                XMLHttpRequestResponseType::Blob => {
                    let key = ctx
                        .host_records
                        .borrow_mut()
                        .insert(HostRecord::Blob(BlobRecord {
                            bytes: body,
                            blob_type,
                        }));
                    script.build_value(HostValue::Blob(encode_host(ctx.slot, key)))
                }
                XMLHttpRequestResponseType::Json => script.build_value(HostValue::Json(&body)),
                _ => None,
            }
        };
        let Some(bits) = bits else {
            return Ok(AnyValue::NULL);
        };
        let mut replaced = None;
        self.with_xhr_mut((), |record| {
            replaced = record
                .response_value
                .replace(XhrResponseValue { bits, built_for });
        });
        if let Some(old) = replaced {
            script.release_value(old.bits);
        }
        Ok(AnyValue(bits))
    }

    fn response_text(&self) -> Result<String, JsError> {
        // "If this's response type is not the empty string or "text", then
        // throw an "InvalidStateError" DOMException."
        let response_type = self.with_xhr(XMLHttpRequestResponseType::EmptyString, |record| {
            record.response_type
        });
        if !matches!(
            response_type,
            XMLHttpRequestResponseType::EmptyString | XMLHttpRequestResponseType::Text
        ) {
            return Err(JsError::dom_exception(
                "InvalidStateError",
                "responseText is only readable when responseType is '' or 'text'",
            ));
        }
        Ok(self.text_response())
    }
}

impl HostObject {
    /// The standard's *text response*: the empty string for a state that is not
    /// loading or done, and otherwise the received bytes decoded with the
    /// response's final encoding.
    fn text_response(&self) -> String {
        self.with_xhr(String::new(), |record| {
            if record.ready_state != <HostObject as XMLHttpRequestOps>::LOADING
                && record.ready_state != <HostObject as XMLHttpRequestOps>::DONE
            {
                return String::new();
            }
            // A null body is the empty string, which the encoding step below
            // would reach anyway; a network error is the empty string too.
            let Some(response) = &record.response else {
                return String::new();
            };
            let Some(body) = &response.body else {
                return String::new();
            };
            // `Encoding::decode` *is* the standard's *decode*: it reads a BOM and
            // uses its encoding in preference to the fallback, removes the BOM
            // from the output, and replaces every malformed sequence with
            // U+FFFD. The replacement encoding is a label like any other and
            // decodes to U+FFFD, which is what the standard asks for; it is
            // `TextDecoder` that refuses it, not this.
            let encoding = text_response_encoding(record.override_charset.as_deref(), response);
            let (text, _, _) = encoding.decode(body);
            text.into_owned()
        })
    }
}

impl XMLHttpRequestConstructor for HostObject {
    fn construct(realm: &Realm) -> Result<Self, JsError> {
        // The constructor's steps are "to do nothing": the state starts at
        // `UNSENT` and every attribute at its default. The record exists for the
        // two reasons every `TAG_HOST` wrapper needs one — it names the
        // interface the wrapper's template comes from, and the wrapper's
        // finalizer frees the slot through `reclaim_host_record` — and for a
        // third that is XHR's own: the record's key is the object's event-target
        // key, so a listener registered on one object must be keyed by a record
        // that belongs to that object alone.
        let ctx = realm_context(realm)?;
        let key = ctx
            .host_records
            .borrow_mut()
            .insert(HostRecord::Xhr(XhrRecord {
                ready_state: <HostObject as XMLHttpRequestOps>::UNSENT,
                method: None,
                url: None,
                // Nothing is in flight and nothing has been authored yet, which
                // is what `open()`'s step 11 would leave the two as well.
                send_flag: false,
                // No request has been started, so there is none for a
                // completion to belong to. `open()` and `send()` both bump it
                // before any request can leave, so a live request's id is never
                // this value.
                request_id: 0,
                author_headers: Vec::new(),
                timeout: 0,
                with_credentials: false,
                response_type: XMLHttpRequestResponseType::EmptyString,
                // A new object's response is the standard's *network error*, and
                // nothing has been asked for with `overrideMimeType`.
                response: None,
                override_charset: None,
                response_value: None,
            }));
        Ok(HostObject::host_record(ctx, key))
    }
}

/// Delivers one completed request to the `XMLHttpRequest` object `target` names:
/// the standard's response state machine, and the events that go with it.
///
/// Returns whether `target` was an `XMLHttpRequest`. That answer *is* the
/// caller's decision: a completion at one of these is answered here, and every
/// other target keeps whatever the caller did with it before. The handle tag
/// cannot tell the two apart — every JS-owned wrapper carries [`TAG_HOST`] —
/// so only the record does.
///
/// `request` is the id the object's `send()` stamped on this request. A record
/// whose id has moved on has already ended that request — `open()` ran while it
/// was still in flight, `abort()` ended it, or its `timeout` fired, all three of
/// which go through `end_xhr_request` — and then **nothing** happens: no store,
/// no state change, no event. The return is `true` in that case, so the caller's
/// fallback dispatch is suppressed too; a page that re-opened or aborted an
/// object must not receive even a bare `error`/`load` for the request it walked
/// away from.
///
/// The id comparison is the whole protection even though those three callers
/// *also* terminate the request at the driver: a request can complete on the
/// driver's task between the termination and the drain, and only the id can
/// refuse the completion that crossed the seam anyway.
///
/// The events are fired synchronously, on this stack, at the object's own event
/// target key, and the caller holds the native-dispatch guard that makes a
/// listener here runnable with no V8 callback underneath it.
pub fn deliver_xhr_completion(
    ctx: &SharedContext,
    target: DispatchTarget,
    request: u64,
    outcome: &FetchOutcome,
) -> bool {
    let Some(key) = target.host_key() else {
        return false;
    };
    // Scoped to the match: the arms below re-borrow the record table mutably, so
    // the read that decides whether to act cannot be held across them.
    let ours = match ctx.host_records.borrow().get(key) {
        Some(HostRecord::Xhr(record)) => record.request_id == request,
        Some(_) | None => return false,
    };
    if !ours {
        return true;
    }
    match outcome {
        FetchOutcome::Response(data) => xhr_handle_response(ctx, target, key, data),
        // An `XMLHttpRequest` is never `no-cors`, so an opaque answer is one
        // it cannot use: a network error.
        FetchOutcome::Failed(_) | FetchOutcome::Opaque => {
            xhr_request_error_steps(ctx, target, key, "error")
        }
    }
    true
}

/// Reports the image loads that ended: a trusted `load` or `error` at each
/// element, then the `decode()` promises that were waiting on it
/// (<https://html.spec.whatwg.org/multipage/images.html#update-the-image-data>,
/// "queue an element task ... fire an event named load / error").
///
/// The caller holds the native-dispatch guard, as for `deliver_xhr_completion`:
/// a listener runs with no callback on the stack. An element reclaimed since
/// the load began is skipped, and a listener that removes the element does not
/// stop the events after it.
pub fn deliver_image_events(ctx: &SharedContext, events: &[ImageEvent]) {
    for event in events {
        let alive = ctx.doc.borrow().resolve(event.node).is_ok();
        if alive && event.kind != crate::image::ImageEventKind::Release {
            let target = DispatchTarget::node(ctx.slot, event.node);
            fire_xhr_event(ctx, target, event.kind.event_type(), EventKind::Plain);
        }
        // The element may be let go of now that its handlers have run.
        let pin = ctx.image_pins.borrow_mut().remove(&event.node);
        drop(pin);
        let resolvers = ctx.image_decodes.borrow_mut().remove(&event.node);
        let Some(resolvers) = resolvers else {
            continue;
        };
        let Some(script) = ctx.script.borrow().clone() else {
            continue;
        };
        for resolver in resolvers {
            let outcome = match event.kind {
                crate::image::ImageEventKind::Load => PromiseOutcome::Resolve(HostValue::Undefined),
                // A release leaves the element's state as it was, so a
                // `decode()` that was parked on a lazy image keeps waiting.
                crate::image::ImageEventKind::Release => {
                    ctx.image_decodes
                        .borrow_mut()
                        .entry(event.node)
                        .or_default()
                        .push(resolver);
                    continue;
                }
                _ => PromiseOutcome::Reject(HostValue::DomException {
                    name: "EncodingError",
                    message: "The source image cannot be decoded.",
                }),
            };
            script.settle_promise(resolver, outcome);
        }
    }
}

/// The standard's *handle response*, run through to *handle response
/// end-of-body*, for a completion that carries a response.
///
/// A driver hands over a finished response rather than a stream, so this is the
/// one-chunk case of a machine that reads a body a chunk at a time: the response
/// is stored, the first `readystatechange` fires, one chunk arrives
/// (`LOADING`, `readystatechange`, `progress`), and the end of the body follows
/// at once (a second `progress`, `DONE`, `readystatechange`, `load`, `loadend`).
fn xhr_handle_response(
    ctx: &SharedContext,
    target: DispatchTarget,
    key: HostKey,
    data: &FetchedResponse,
) {
    let response = XhrResponse::from_fetched(data);
    // *Extract a length* reads the `Content-Length` header, not the body, and
    // its caller sets a length that is not an integer to 0 — so a body sent
    // without one reports `lengthComputable === false` with `total` 0 while
    // `loaded` is still the body's own length.
    let length = extract_content_length(&response.headers).unwrap_or(0);
    // `received bytes` *is* the body here, and this engine has no chunk smaller
    // than all of it. A body with no bytes has no chunk at all, which is what an
    // empty body does in the standard's own reader too: it reaches the end of
    // the stream without delivering one, so the state goes `HEADERS_RECEIVED` →
    // `DONE` with no `LOADING` and no chunk `progress`. That answers a null-body
    // status, a null body and an empty one by the same rule.
    let transmitted = response.body.as_ref().map_or(0, Vec::len);

    // The response is stored before the first event, so a page that reads
    // `getResponseHeader('Content-Type')` from its `HEADERS_RECEIVED` handler
    // sees the response that just arrived.
    store_xhr_response(ctx, key, Some(response));
    set_xhr_state(ctx, key, HEADERS_RECEIVED);
    fire_xhr_event(ctx, target, READY_STATE_CHANGE, EventKind::Plain);

    if transmitted > 0 {
        set_xhr_state(ctx, key, LOADING);
        fire_xhr_event(ctx, target, READY_STATE_CHANGE, EventKind::Plain);
        fire_xhr_progress(ctx, target, "progress", transmitted, length);
    }

    // The second `progress`, from *handle response end-of-body*: the one event of
    // this sequence Chromium does not fire, since it fires the chunk's
    // `progress` alone and skips this one when the byte count has not changed
    // since (whatwg/xhr#318, still open). Two in one turn is the standard's own
    // sequence — one per chunk, one from the end of the body — and not a
    // duplicated call, and the standard is what this engine follows.
    fire_xhr_progress(ctx, target, "progress", transmitted, length);
    set_xhr_state(ctx, key, DONE);
    // *Handle response end-of-body* step 8: the send flag clears once the body
    // has ended and the state is done (step 7), and before the `readystatechange`
    // of step 9 that reports it. It is the flag's own half of "this request is
    // over" — `send()` reads
    // the state before the flag, so the position is only exactly-the-standard
    // rather than observable, but the flag has two clearers (`abort()` and a
    // fired `timeout` clear it through the *request error steps*) and matching
    // the order is what keeps them from drifting.
    clear_xhr_send_flag(ctx, key);
    fire_xhr_event(ctx, target, READY_STATE_CHANGE, EventKind::Plain);
    fire_xhr_progress(ctx, target, "load", transmitted, length);
    fire_xhr_progress(ctx, target, "loadend", transmitted, length);
}

/// The standard's *request error steps* for an object whose request failed, with
/// `event` naming the event that reports it
/// (<https://xhr.spec.whatwg.org/#request-error-steps>).
///
/// One algorithm and three callers, which is why the name is a parameter rather
/// than the body of a `handle errors`-shaped function: a network failure runs it
/// with `error`, a fired `timeout` runs it with `timeout`, and `abort()` runs it
/// with `abort`. The standard's *handle errors* picks between the last two from
/// the object's own *timed out* and *aborted* flags; this engine holds neither,
/// because the timer carries the timeout and `abort()` is the only caller that
/// runs the steps for an abort — so the choice is made where the cause is known
/// instead of being re-derived here.
///
/// A failure never reaches `HEADERS_RECEIVED` or `LOADING` (only the timeout case
/// can leave from a later state, and it still reports what a failure reports),
/// and none of these three events carries a length — the standard fires the named
/// event and `loadend` with 0 and 0, and fires no `progress` at all. The
/// `exception` argument the standard's signature carries is not passed: it is
/// thrown only for a synchronous request, and `open()` refuses `async = false`
/// for every case.
///
/// The upload half of step 6 is not run: this engine has no
/// `XMLHttpRequestUpload` object yet, so there is no upload object to fire at and
/// no *upload complete* flag to consult. Later `abort()` callers inherit it — the
/// member that lands the upload object owes the failing event and its `loadend`
/// there too (`idl/xhr/XMLHttpRequest.webidl`).
fn xhr_request_error_steps(ctx: &SharedContext, target: DispatchTarget, key: HostKey, event: &str) {
    // Steps 1 to 3, in the standard's order — state, send flag, response — all of
    // them before any event, so every member already reads 0, the empty string
    // and null from inside the `readystatechange` handler onwards, and a `send()`
    // from one of these handlers is refused by the *state* rather than by the
    // flag.
    set_xhr_state(ctx, key, DONE);
    clear_xhr_send_flag(ctx, key);
    store_xhr_response(ctx, key, None);
    // Step 5, then the two progress events of the steps after it.
    fire_xhr_event(ctx, target, READY_STATE_CHANGE, EventKind::Plain);
    fire_xhr_progress(ctx, target, event, 0, 0);
    fire_xhr_progress(ctx, target, "loadend", 0, 0);
}

/// Ends the request `key` currently holds: the driver is told to stop it, and the
/// record's request id moves on so no completion for it is delivered.
///
/// This is the shared half of the three ways a request ends without an answer —
/// `open()` step 10, `abort()` step 1 and a fired `timeout` — and both halves are
/// needed. The driver call stops the work, and the id is what refuses an answer
/// that crossed the seam before the termination landed: `deliver_xhr_completion`
/// compares the id and discards a mismatched completion, so without the bump a
/// racing answer would run the whole state machine again on an object that had
/// already reported the request over.
///
/// The two are deliberately ordered id-then-driver: a driver that answers while
/// `abort` is on the stack pushes a completion the drain will refuse, whereas the
/// reverse order would leave a window in which a completion that arrived first
/// could still be delivered. The id is read and bumped under one borrow, and the
/// driver is called with no record borrow held — `abort` is user code under the
/// seam and must not be able to re-enter the record table.
///
/// The handle is built from the *old* id, because that is the request being
/// ended: the driver knows the request by the identity it was started with.
fn end_xhr_request(ctx: &SharedContext, key: HostKey) {
    let Some(handle) = bump_xhr_request_id(ctx, key) else {
        // Nothing is in flight, or the record is gone: there is no request to
        // name and no driver that could hold one.
        return;
    };
    let script = ctx.script.borrow().as_ref().map(Rc::clone);
    if let Some(script) = script {
        // The answer is deliberately dropped: "nothing was terminated" is a
        // normal outcome (`XhrDriver::abort`) and none of the three callers
        // branches on it — the id is what guarantees no answer arrives either
        // way.
        let _ = script.abort_xhr(handle);
    }
}

/// Bumps `key`'s request id and returns a handle naming the request it left
/// behind, or `None` when there is no request to name.
///
/// This is the id half of [`end_xhr_request`], split out so the borrow is
/// visibly scoped to the one read-modify-write: the caller must not hold the
/// record table across the driver call.
fn bump_xhr_request_id(ctx: &SharedContext, key: HostKey) -> Option<XhrHandle> {
    let mut records = ctx.host_records.borrow_mut();
    let Some(HostRecord::Xhr(record)) = records.get_mut(key) else {
        return None;
    };
    let handle = XhrHandle {
        target: encode_host(ctx.slot, key),
        request: record.request_id,
    };
    record.request_id = record.request_id.wrapping_add(1);
    Some(handle)
}

/// Arms the timer that ends a request that outlives `timeout_ms`.
///
/// The clock is the page's own, through
/// [`ScriptHost::schedule_background_task`]: the timer has to fire on the same
/// loop the page runs on, so a virtual-clock page runs it when the engine
/// advances to its deadline and a real-clock page runs it when the host blocks
/// that long. It is scheduled as **background** rather than as tracked work, so
/// it does not hold the settle open: the request itself already holds it
/// (`RequestRecord::holds_settle`), and a tracked timer for the same request
/// would make the page wait out the whole timeout after the answer had already
/// arrived.
///
/// The `handle` is the one `send()` stamped, and the callback re-checks it
/// against the record before acting: the timer is never cancelled when its
/// request ends early, so a fired timer for a request the object has moved on
/// from has to be recognised and ignored rather than run. The callback holds the
/// context **weakly** for the same reason it is never cancelled — see the
/// comment at the downgrade.
fn arm_xhr_timeout(
    ctx: &SharedContext,
    target: DispatchTarget,
    key: HostKey,
    handle: XhrHandle,
    timeout_ms: u32,
) {
    let script = ctx.script.borrow().as_ref().map(Rc::clone);
    let Some(script) = script else {
        return;
    };
    // A `Weak`, not a clone: the callback outlives the call that armed it, and it
    // is stored in the engine's own timer map — `HostContext` -> script host ->
    // host state -> scheduler -> callback. A strong context there would close
    // that ring and keep a whole page's engine alive after it was dropped. The
    // page's own engine holds the context strongly for as long as the page
    // exists, so the upgrade fails in exactly the two cases there is nothing to
    // do in: the page is gone, or the timer outlived it.
    let ctx = Rc::downgrade(ctx);
    script.schedule_background_task(
        u64::from(timeout_ms),
        Box::new(move || {
            if let Some(ctx) = ctx.upgrade() {
                xhr_timeout_fired(&ctx, target, key, handle);
            }
        }),
    );
}

/// The timer callback: ends the request `handle` names if it is still the one in
/// flight, and reports it as the standard's *handle errors* reports a timeout.
///
/// Liveness is the standard's own test read off this engine's record: "req's done
/// flag is unset" is *the send flag is still set* — the flag is cleared when a
/// request ends whichever way it ended — and the handle's id is the second half,
/// because a request that ended and a *new* one that started on the same object
/// both leave the flag set again. A timer that fires for neither does nothing:
/// the request it belonged to is over, and the events it would fire have already
/// been fired by whatever ended it.
///
/// The steps run only after the termination, so a completion racing the timer is
/// discarded by the id like any other abandoned answer.
fn xhr_timeout_fired(ctx: &SharedContext, target: DispatchTarget, key: HostKey, handle: XhrHandle) {
    let still_in_flight = match ctx.host_records.borrow().get(key) {
        Some(HostRecord::Xhr(record)) => record.send_flag && record.request_id == handle.request,
        Some(_) | None => false,
    };
    if !still_in_flight {
        return;
    }
    end_xhr_request(ctx, key);
    xhr_request_error_steps(ctx, target, key, "timeout");
}

/// Writes one of the five state constants onto the record, if it is still there.
fn set_xhr_state(ctx: &SharedContext, key: HostKey, state: u16) {
    if let Some(HostRecord::Xhr(record)) = ctx.host_records.borrow_mut().get_mut(key) {
        record.ready_state = state;
    }
}

/// Clears the send flag, so the object is no longer considered to have a request
/// in flight.
///
/// A completion whose record has already been reclaimed clears nothing, which is
/// the same no-op every other record write from a completion makes for one.
fn clear_xhr_send_flag(ctx: &SharedContext, key: HostKey) {
    if let Some(HostRecord::Xhr(record)) = ctx.host_records.borrow_mut().get_mut(key) {
        record.send_flag = false;
    }
}

/// Stores the response a completion delivered, or clears it to the network error
/// a failure leaves behind.
fn store_xhr_response(ctx: &SharedContext, key: HostKey, response: Option<XhrResponse>) {
    if let Some(HostRecord::Xhr(record)) = ctx.host_records.borrow_mut().get_mut(key) {
        record.response = response;
    }
}

/// Fires one event the user agent created, at an `XMLHttpRequest` object.
///
/// The same shape `open()` inlines once, written out here because the machine
/// fires it seven times per completion and because the transient record's removal
/// is the part of it that is easy to get subtly wrong: the record is dropped
/// unless a listener made a wrapper for it, in which case the world's dead-event
/// sweep owns its removal.
///
/// The event is trusted — it comes from the user agent and never from script —
/// and "fire an event named e at target" gives it no bubbles, no cancelable and
/// no default action.
fn fire_xhr_event(ctx: &SharedContext, target: DispatchTarget, type_: &str, kind: EventKind) {
    let mut record = EventRecord::new(type_.to_owned(), kind, ctx.time_stamp());
    record.flags.initialized = true;
    record.is_trusted = true;
    let event_key = ctx.events.borrow_mut().insert(record);
    let _ = dispatch_at(ctx, target, event_key, true);
    let orphaned = ctx
        .events
        .borrow()
        .get(event_key)
        .is_some_and(|record| !record.has_wrapper);
    if orphaned {
        ctx.events.borrow_mut().remove(event_key);
    }
}

/// Fires the standard's *progress event* named `type_`, given `transmitted` bytes
/// and a `length`
/// (<https://xhr.spec.whatwg.org/#firing-events-using-the-progressevent-interface>).
///
/// `loaded` is always `transmitted`; `lengthComputable` and `total` are set only
/// when `length` is not zero. That is why a missing `Content-Length` and a
/// `Content-Length: 0` both read `lengthComputable === false`, and why `loaded`
/// is the body's own length in every case.
fn fire_xhr_progress(
    ctx: &SharedContext,
    target: DispatchTarget,
    type_: &str,
    transmitted: usize,
    length: u64,
) {
    let kind = EventKind::Progress {
        length_computable: length != 0,
        loaded: transmitted as f64,
        total: length as f64,
    };
    fire_xhr_event(ctx, target, type_, kind);
}

/// The `Content-Length` header's name, byte-lowercased the way every stored name
/// is.
const CONTENT_LENGTH: &[u8] = b"content-length";

/// The states a delivered response passes through, bound to the constants the
/// generated trait declares so that no state number is written in this file.
const UNSENT: u16 = <HostObject as XMLHttpRequestOps>::UNSENT;
const OPENED: u16 = <HostObject as XMLHttpRequestOps>::OPENED;
const HEADERS_RECEIVED: u16 = <HostObject as XMLHttpRequestOps>::HEADERS_RECEIVED;
const LOADING: u16 = <HostObject as XMLHttpRequestOps>::LOADING;
const DONE: u16 = <HostObject as XMLHttpRequestOps>::DONE;

/// The `charset` parameter's name, ASCII-lowercased by the MIME parser.
const CHARSET: &[u8] = b"charset";

/// The one *essence* Fetch's *extract a MIME type* skips: a `Content-Type` of
/// `*/*` names no type at all, so it is as good as an absent header.
const ANY_MIME_TYPE: &[u8] = b"*/*";

/// Fetch's *extract a length*: the number of bytes `Content-Length` declares, or
/// `None` when the header list holds no length
/// (<https://fetch.spec.whatwg.org/#extract-a-length>).
///
/// One `None` covers all four of the standard's outcomes — no such header, values
/// that disagree with each other, an empty value, and a value that is not a
/// non-negative integer — because the XHR Standard treats them alike: "if length
/// is not an integer, then set it to 0".
fn extract_content_length(headers: &HeaderList) -> Option<u64> {
    let mut values = headers
        .iter()
        .filter(|(name, _)| name.as_slice() == CONTENT_LENGTH)
        .flat_map(|(_, value)| split_header_value(value));
    let candidate = values.next()?;
    if values.any(|value| value != candidate) {
        return None;
    }
    if candidate.is_empty() || !candidate.iter().all(u8::is_ascii_digit) {
        return None;
    }
    String::from_utf8(candidate).ok()?.parse().ok()
}

/// The encoding a text response decodes with: the standard's *get a final
/// encoding*, which reads the `charset` of the value `overrideMimeType` stored
/// and, when that value has none, the `charset` of the response's `Content-Type`
/// (<https://xhr.spec.whatwg.org/#get-a-final-encoding>).
///
/// The override replaces the *label*, not the whole type, which is why
/// `overrideMimeType('text/plain')` and an override that failed to parse both
/// leave the response's own charset standing. A label that names no encoding is
/// UTF-8, which is also what a body with no charset gets.
fn text_response_encoding(
    override_charset: Option<&[u8]>,
    response: &XhrResponse,
) -> &'static encoding_rs::Encoding {
    let response_charset = extract_mime_charset(&response.headers);
    override_charset
        .or(response_charset.as_deref())
        .and_then(encoding_rs::Encoding::for_label)
        .unwrap_or(encoding_rs::UTF_8)
}

/// Fetch's *extract a MIME type*, reduced to the `charset` it carries: the first
/// `Content-Type` value that parses, and the `charset` a later value of the same
/// essence may supply for it
/// (<https://fetch.spec.whatwg.org/#extract-a-mime-type>).
///
/// The loop is the standard's, including the two rules that read like details: a
/// value whose essence is `*/*` is skipped, and a value whose essence matches the
/// one already found inherits its `charset` when it has none of its own.
fn extract_mime_charset(headers: &HeaderList) -> Option<Vec<u8>> {
    let mut essence: Option<Vec<u8>> = None;
    let mut inherited: Option<Vec<u8>> = None;
    let mut charset: Option<Option<Vec<u8>>> = None;
    let values = headers
        .iter()
        .filter(|(name, _)| name.as_slice() == CONTENT_TYPE)
        .flat_map(|(_, value)| split_header_value(value));
    for value in values {
        let Some(parsed) = parse_mime_type(&value) else {
            continue;
        };
        if parsed.essence.as_slice() == ANY_MIME_TYPE {
            continue;
        }
        if essence.as_deref() != Some(parsed.essence.as_slice()) {
            inherited = parsed.charset.clone();
            essence = Some(parsed.essence);
            charset = Some(parsed.charset);
        } else {
            charset = Some(parsed.charset.or_else(|| inherited.clone()));
        }
    }
    charset.flatten()
}

/// A parsed MIME type, reduced to the two parts a text response reads.
struct MimeType {
    /// The *essence*: the type and the subtype, ASCII-lowercased, joined by `/`.
    /// Fetch's *extract a MIME type* compares it from one header value to the
    /// next.
    essence: Vec<u8>,
    /// The `charset` parameter, if the type has one. It is the only parameter this
    /// engine reads, so the parser applies the standard's conditions for storing
    /// a parameter to `charset` alone rather than collecting a map of them.
    charset: Option<Vec<u8>>,
}

/// The MIME Sniffing Standard's *parse a MIME type*
/// (<https://mimesniff.spec.whatwg.org/#parse-a-mime-type>).
///
/// The validity rules are the whole point of the parse: a type or subtype holding
/// a byte that is not an HTTP token, an empty one, or a value with no `/` is
/// *failure* — and a caller can see the difference, because a failed
/// `overrideMimeType` is stored as `application/octet-stream` and so carries no
/// charset.
fn parse_mime_type(input: &[u8]) -> Option<MimeType> {
    let input = strip_http_whitespace(input);
    let mut position = 0;

    let mut essence = collect_until(input, &mut position, |byte| byte == b'/');
    if essence.is_empty() || !essence.iter().all(|byte| is_tchar(*byte)) {
        return None;
    }
    if position == input.len() {
        return None;
    }
    position += 1;

    let subtype = collect_until(input, &mut position, |byte| byte == b';');
    let subtype = strip_trailing_http_whitespace(&subtype);
    if subtype.is_empty() || !subtype.iter().all(|byte| is_tchar(*byte)) {
        return None;
    }
    essence.make_ascii_lowercase();
    essence.push(b'/');
    essence.extend(subtype.iter().map(u8::to_ascii_lowercase));

    let mut charset: Option<Vec<u8>> = None;
    while position < input.len() {
        // The `;` this parameter follows, then the whitespace before its name.
        position += 1;
        while position < input.len() && is_http_whitespace(input[position]) {
            position += 1;
        }
        let mut name = collect_until(input, &mut position, |byte| matches!(byte, b';' | b'='));
        name.make_ascii_lowercase();

        // A parameter with no `=` at all is skipped, and so is a name that ends
        // the input: the standard only reads a value when the `=` is followed by
        // something.
        let mut value = None;
        if position < input.len() {
            if input[position] == b';' {
                continue;
            }
            position += 1;
            if position == input.len() {
                break;
            }
            if input[position] == b'"' {
                let (quoted, next) = collect_http_quoted_string(input, position);
                position = next;
                // "Collect a sequence of code points that are not U+003B (;)" —
                // whatever follows the closing quote is not part of the value.
                let _ = collect_until(input, &mut position, |byte| byte == b';');
                value = Some(quoted);
            } else {
                let raw = collect_until(input, &mut position, |byte| byte == b';');
                let raw = strip_trailing_http_whitespace(&raw);
                if raw.is_empty() {
                    continue;
                }
                value = Some(raw.to_vec());
            }
        }

        // The standard's four conditions for storing a parameter, applied to the
        // one name this engine reads: a name that is not `charset` (which also
        // covers the empty one and every name holding a non-token byte) stores
        // nothing, a `charset` that is already there is not replaced, and a value
        // holding a byte outside HTTP's *quoted-string token* set is dropped.
        let Some(value) = value else {
            continue;
        };
        if name.as_slice() != CHARSET || charset.is_some() || !is_http_quoted_string_token(&value) {
            continue;
        }
        charset = Some(value);
    }

    Some(MimeType { essence, charset })
}

/// Collects the bytes up to the first one `stop` accepts: the standard's *collect
/// a sequence of code points that are not …*, with `position` left on that byte
/// or at the end of the input.
fn collect_until(input: &[u8], position: &mut usize, stop: impl Fn(u8) -> bool) -> Vec<u8> {
    let start = *position;
    while *position < input.len() && !stop(input[*position]) {
        *position += 1;
    }
    input[start..*position].to_vec()
}

/// Fetch's *collect an HTTP quoted string*, with the value extracted: the quotes
/// and the backslash escapes are dropped
/// (<https://fetch.spec.whatwg.org/#collect-an-http-quoted-string>).
///
/// `position` points at the opening quote, and the returned index is the first
/// byte after the closing one — or the end of the input for a string that is
/// never closed, which the standard treats as a string that simply ends there
/// rather than as a failure.
fn collect_http_quoted_string(input: &[u8], position: usize) -> (Vec<u8>, usize) {
    let mut position = position + 1;
    let mut value = Vec::new();
    loop {
        while position < input.len() && !matches!(input[position], b'"' | b'\\') {
            value.push(input[position]);
            position += 1;
        }
        if position == input.len() {
            return (value, position);
        }
        let quote_or_backslash = input[position];
        position += 1;
        if quote_or_backslash == b'\\' {
            // A backslash that ends the input is a literal one, and the string
            // ends with it.
            if position == input.len() {
                value.push(b'\\');
                return (value, position);
            }
            value.push(input[position]);
            position += 1;
        } else {
            return (value, position);
        }
    }
}

/// Whether every byte of a parameter value is an HTTP *quoted-string token code
/// point*: a tab, a printable ASCII byte or obs-text. A parameter whose value
/// holds anything else is not stored, which is what drops `charset=gbk\x7f`
/// (<https://mimesniff.spec.whatwg.org/#http-quoted-string-token-code-point>).
fn is_http_quoted_string_token(value: &[u8]) -> bool {
    value
        .iter()
        .all(|byte| matches!(byte, b'\t' | 0x20..=0x7E | 0x80..=0xFF))
}

/// Removes HTTP whitespace from the end of `value`.
///
/// The MIME parser's own steps are on *HTTP whitespace* while Fetch's *split* is
/// on HTTP *tab or space*, a smaller set — but a stored header value holds no CR
/// or LF (they are rejected on the way into a header list), so the two sets can
/// only differ for input that cannot reach either caller.
fn strip_trailing_http_whitespace(value: &[u8]) -> &[u8] {
    let end = value
        .iter()
        .rposition(|byte| !is_http_whitespace(*byte))
        .map_or(0, |index| index + 1);
    &value[..end]
}

/// Fetch's *get, decode, and split a header value*: the value split on its
/// commas, with HTTP whitespace removed from each part, and with a comma inside a
/// quoted string not splitting anything
/// (<https://fetch.spec.whatwg.org/#collect-an-http-quoted-string>).
///
/// The split is what makes `Content-Type: text/html;x="a,b"` one value rather
/// than two, and what keeps a header whose values disagree — the case Fetch's
/// *extract a length* and *extract a MIME type* both read — visible as separate
/// values.
fn split_header_value(value: &[u8]) -> Vec<Vec<u8>> {
    let mut values = Vec::new();
    let mut temporary = Vec::new();
    let mut position = 0;
    while position < value.len() {
        match value[position] {
            b'"' => {
                let (_, next) = collect_http_quoted_string(value, position);
                // The standard appends the quoted string *as it stands* here:
                // the escapes are the MIME parser's to remove, not the
                // splitter's.
                temporary.extend_from_slice(&value[position..next]);
                position = next;
            }
            b',' => {
                values.push(strip_http_whitespace(&temporary).to_vec());
                temporary.clear();
                position += 1;
            }
            byte => {
                temporary.push(byte);
                position += 1;
            }
        }
    }
    values.push(strip_http_whitespace(&temporary).to_vec());
    values
}

impl HostObject {
    /// Builds and dispatches the synthetic, untrusted `click` `MouseEvent` for
    /// [`HTMLElementOps::click`]. The event bubbles, is cancelable and composed,
    /// carries the default `detail` 0 and `button` 0, with every coordinate and modifier at
    /// its default, and no related target. Its view is the target document's
    /// Window, if any. If no listener created a wrapper for
    /// it, its slab record is removed here so a transient click leaks nothing;
    /// when a wrapper exists, the world's dead-event sweep reclaims the record.
    /// Dispatches a synthetic `click` at the element and reports whether it
    /// was left uncancelled.
    fn dispatch_synthetic_click(&self, doc: &SharedDocument, id: NodeId) -> Result<bool, JsError> {
        let ctx = self.context();
        let target = {
            let doc = doc.borrow();
            // A dead element has no listeners that could legitimately fire, and
            // its arena slot may already belong to an unrelated node, so a
            // dispatch here would be attributed to that node's listeners.
            if !doc.is_live(id) {
                return Ok(false);
            }
            doc.node_ref(id)
        };
        let mut record = EventRecord::new(
            "click".to_owned(),
            EventKind::Mouse {
                view: (ctx.slot == 0).then_some(encode_window()),
                detail: 0,
                which: 0,
                screen_x: 0,
                screen_y: 0,
                client_x: 0,
                client_y: 0,
                modifiers: 0,
                button: 0,
                buttons: 0,
                related_target: None,
                related_value: None,
                wheel: None,
            },
            ctx.time_stamp(),
        );
        record.bubbles = true;
        record.cancelable = true;
        record.composed = true;
        record.flags.initialized = true;
        let key = ctx.events.borrow_mut().insert(record);

        let dispatch_result = dispatch(ctx, target, key, false);

        // Remove the transient record unless a listener made a wrapper for it
        // (in which case the finalizer-driven sweep owns its removal).
        let orphaned = ctx
            .events
            .borrow()
            .get(key)
            .is_some_and(|record| !record.has_wrapper);
        if orphaned {
            ctx.events.borrow_mut().remove(key);
        }
        dispatch_result
    }
}

/// Decodes the handle of a node-typed argument into the node id it names.
///
/// A generated binding hands an interface-typed argument to the host as bare
/// handle bits, and the receiver's template signature says nothing about *which*
/// interface the argument was — so `appendChild(url)` arrives with the bits of a
/// `URL`. Those bits carry their own tag and a payload that is a host-record
/// index, and using that payload as a node id would mutate an unrelated arena
/// slot: silently wrong in release, and only a debug assertion away from an
/// abort before. The tag is therefore checked here and a mismatch becomes the
/// `TypeError` WebIDL's brand check would raise.
///
/// This is a check of the tag, not of the interface: what "a `Node`" means in
/// the tree — `Document`, `Text`, `Element` and the rest — is decided by the
/// arena, which rejects an id it never issued.
///
/// The *slot* check lives with the callers instead, because what a wrong slot
/// means is per operation: [`HostObject::same_document_node`] and
/// [`HostObject::child_argument`] are the two answers, and
/// [`NodeOps::contains`]'s is a third.
fn node_argument(handle: Handle<Node>) -> Result<NodeId, JsError> {
    let decoded = decode(handle.bits());
    if decoded.tag != TAG_NODE {
        return Err(JsError::type_error("the argument is not a Node"));
    }
    Ok(decoded.node_id)
}

/// Maps a `tenta_core::Error` from a mutation to a `JsError`. A DOM error
/// carries its exception name in `code` and the human-readable text in
/// `message`; the name is passed separately so the throw path can build a real
/// `DOMException` rather than folding the name into the message.
fn mutation_error(e: &tenta_core::Error) -> JsError {
    match e {
        tenta_core::Error::Other { code, message } => JsError::dom_exception(code, message),
        _ => JsError::type_error(e.to_string()),
    }
}

/// The HTML Standard's *title element* of a document: the first `title`
/// element in the document in tree order, wherever in the tree it sits, or
/// `None` when there is none
/// (<https://html.spec.whatwg.org/multipage/dom.html#dom-document-title>).
/// The element must be in the HTML namespace, so an SVG `title` is not it
/// (`wpt/checkout/html/dom/documents/dom-tree-accessors/document.title-not-in-html-svg.html`).
fn document_title_element(doc: &Document) -> Option<NodeId> {
    doc.descendants(doc.document_node())
        .find(|id| doc.tag(*id) == Some(atoms::TITLE) && doc.namespace(*id) == Some(atoms::NS_HTML))
}

/// Whether `url`'s scheme makes its document *cookie-averse*
/// (<https://html.spec.whatwg.org/multipage/dom.html#cookie-averse-document-object>).
///
/// The standard defines a cookie-averse `Document` as one with a null browsing
/// context, or whose URL's scheme is neither `http` nor `https`. This tree has
/// no null-browsing-context case, so the whole rule reduces to the scheme test,
/// written out rather than inherited from `canonical_host` — whose
/// permissiveness with `file:` is a property of the network tier, not this
/// member's contract.
fn is_cookie_averse_scheme(url: &url::Url) -> bool {
    !(url.scheme() == "http" || url.scheme() == "https")
}

/// Infra's "strip and collapse ASCII whitespace"
/// (<https://infra.spec.whatwg.org/#strip-and-collapse-ascii-whitespace>):
/// every run of ASCII whitespace becomes one U+0020 SPACE, and any leading or
/// trailing ASCII whitespace is removed. ASCII whitespace is exactly U+0009
/// TAB, U+000A LF, U+000C FF, U+000D CR and U+0020 SPACE — the wider Unicode
/// set is deliberately not included, since `document.title` must preserve it
/// (`wpt/checkout/html/dom/documents/dom-tree-accessors/document.title-05.html`).
fn strip_and_collapse_ascii_whitespace(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut pending_space = false;
    for ch in value.chars() {
        if matches!(ch, '\t' | '\n' | '\u{c}' | '\r' | ' ') {
            // Whitespace before any content is dropped, so a leading run never
            // sets the flag; a trailing run leaves it set and is never flushed.
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(ch);
    }
    out
}

/// The DOM `nodeName` of `id` (https://dom.spec.whatwg.org/#dom-node-nodename).
fn node_name(doc: &Document, id: NodeId) -> String {
    match doc.kind(id) {
        NodeKind::Element => element_tag_name(doc, id),
        NodeKind::Text => "#text".to_owned(),
        NodeKind::CData => "#cdata-section".to_owned(),
        NodeKind::Comment => "#comment".to_owned(),
        NodeKind::Document => "#document".to_owned(),
        NodeKind::Fragment | NodeKind::ShadowRoot => "#document-fragment".to_owned(),
        NodeKind::DocType => doc
            .doctype(id)
            .map(|doctype| doctype.name.clone())
            .unwrap_or_default(),
        NodeKind::ProcessingInstruction => doc
            .processing_instruction(id)
            .map(|(target, _)| target.to_owned())
            .unwrap_or_default(),
    }
}

/// The qualified name of an attribute (`prefix:local`, or just `local` when
/// there is no prefix), as `getAttributeNames` reports it
/// (<https://dom.spec.whatwg.org/#dom-element-getattributenames>).
fn qualified_attr_name(name: &QualName) -> String {
    match &name.prefix {
        Some(prefix) => format!("{}:{}", prefix.as_str(), name.local.as_str()),
        None => name.local.as_str().to_owned(),
    }
}

/// DOM's get-by-name comparison lowercases the requested qualified name only
/// for an HTML-namespace element in an HTML document. XML documents and SVG
/// elements retain case-sensitive qualified names.
fn attribute_qualified_name_for_lookup<'a>(
    doc: &Document,
    id: NodeId,
    name: &'a str,
) -> std::borrow::Cow<'a, str> {
    if name.bytes().any(|byte| byte.is_ascii_uppercase())
        && doc.is_html_document()
        && doc.namespace(id) == Some(atoms::NS_HTML)
    {
        std::borrow::Cow::Owned(name.to_ascii_lowercase())
    } else {
        std::borrow::Cow::Borrowed(name)
    }
}

/// The first attribute with this normalized qualified name, irrespective of
/// namespace. Callers apply the element/document case rule before this lookup.
fn attribute_by_qualified_name<'a>(
    doc: &'a Document,
    id: NodeId,
    name: &str,
) -> Option<(QualName, &'a str)> {
    doc.attributes(id).find(|(stored, _)| match stored.prefix {
        Some(prefix) => {
            name.strip_prefix(prefix.as_str())
                .and_then(|suffix| suffix.strip_prefix(':'))
                == Some(stored.local.as_str())
        }
        None => name == stored.local.as_str(),
    })
}

/// The elements an `Element.getElementsByTagNameNS` /
/// `Document.getElementsByTagNameNS` query selects under `root`, as a snapshot
/// of node references.
///
/// `namespace` and `local_name` are the operation's own arguments: `"*"` matches
/// anything in that position. `None` and the empty string both select the null
/// namespace. Local names have already undergone ordinary DOMString conversion.
fn qualified_snapshot(
    doc: &Document,
    root: NodeId,
    namespace: Option<&str>,
    local_name: &str,
    include_root: bool,
) -> Vec<NodeRef> {
    let namespace = namespace.filter(|ns| !ns.is_empty());
    let matches = |id: NodeId| {
        if doc.kind(id) != NodeKind::Element {
            return false;
        }
        if local_name != "*" && doc.tag(id).is_none_or(|tag| tag.as_str() != local_name) {
            return false;
        }
        match namespace {
            Some("*") => true,
            None => doc.namespace(id).is_none_or(|ns| ns == atoms::EMPTY),
            Some(want) => doc.namespace(id).is_some_and(|ns| ns.as_str() == want),
        }
    };
    let descendants = if include_root {
        doc.descendants_inclusive(root).collect::<Vec<_>>()
    } else {
        doc.descendants(root).collect::<Vec<_>>()
    };
    descendants
        .into_iter()
        .filter(|id| matches(*id))
        .map(|id| doc.node_ref(id))
        .collect()
}

/// Whether the stored attribute `name` is the one selected by `namespace` and
/// `local_name`, both already normalized (`namespace` has had the empty string
/// folded to `None`, `local_name` is a real string).
fn attribute_matches_ns(name: &QualName, namespace: Option<&str>, local_name: &str) -> bool {
    if name.local.as_str() != local_name {
        return false;
    }
    match namespace {
        None => name.ns == atoms::EMPTY,
        Some(want) => name.ns.as_str() == want,
    }
}

/// The stored name of the attribute an element carries in `namespace` with
/// local name `local_name`, or `None` when it has none.
///
/// This is the standard's *get an attribute by namespace and local name*
/// (<https://dom.spec.whatwg.org/#concept-element-attributes-get-by-namespace>):
/// the pair is the key, so a prefixed and an unprefixed attribute of one local
/// name in one namespace are the same attribute.
fn attribute_name_ns(
    doc: &Document,
    id: NodeId,
    namespace: Option<&str>,
    local_name: &str,
) -> Option<AttrName> {
    let namespace = namespace.filter(|ns| !ns.is_empty());
    if local_name.is_empty() {
        return None;
    }
    doc.attributes(id)
        .find(|(name, _)| attribute_matches_ns(name, namespace, local_name))
        .map(|(name, _)| AttrName::from_qual(name))
}

/// The value of the attribute [`attribute_name_ns`] selects.
fn lookup_attribute_ns(
    doc: &Document,
    id: NodeId,
    namespace: Option<&str>,
    local_name: &str,
) -> Option<String> {
    let namespace = namespace.filter(|ns| !ns.is_empty());
    if local_name.is_empty() {
        return None;
    }
    doc.attributes(id)
        .find(|(name, _)| attribute_matches_ns(name, namespace, local_name))
        .map(|(_, value)| value.to_owned())
}

/// The `NotFoundError` `removeAttributeNode` and the `NamedNodeMap` removers
/// throw for an attribute the element does not carry
/// (<https://dom.spec.whatwg.org/#dom-element-removeattributenode>).
fn attr_not_found() -> JsError {
    JsError::dom_exception("NotFoundError", "The element does not carry that attribute")
}

/// Returns the canonical live Attr host for an attached attribute, or creates
/// a detached Attr for Document.createAttribute. The index holds only keys;
/// the wrapper finalizer still owns reclamation of the record and owner pin.
fn mint_attr(ctx: &HostContext, element: Option<NodeRef>, name: QualName) -> Result<u64, JsError> {
    let identity = element.map(|element| attr_identity(element, name));
    if let Some(identity) = identity
        && let Some(key) = ctx.attr_index.borrow().get(&identity).copied()
        && matches!(
            ctx.host_records.borrow().get(key),
            Some(HostRecord::Attr(_))
        )
    {
        return Ok(encode_host(ctx.slot, key));
    }
    let owner_pin = element
        .map(|element| AttrOwnerPin::new(ctx.doc.clone(), element.id))
        .transpose()?;
    let key = ctx
        .host_records
        .borrow_mut()
        .insert(HostRecord::Attr(AttrRecord {
            element,
            name,
            detached_value: String::new(),
            owner_pin,
            child_nodes: None,
        }));
    if let Some(identity) = identity {
        ctx.attr_index.borrow_mut().insert(identity, key);
    }
    Ok(encode_host(ctx.slot, key))
}

/// Mints a *detached* `Attr` carrying `value` — the object a write through a
/// `NamedNodeMap`'s removers returns, which the standard leaves detached with
/// the value the attribute had.
fn mint_detached_attr(ctx: &HostContext, name: QualName, value: String) -> u64 {
    mint_attr_record(
        ctx,
        AttrRecord {
            element: None,
            name,
            detached_value: value,
            owner_pin: None,
            child_nodes: None,
        },
    )
}

/// Inserts `record` into `ctx`'s host-record slab and returns the handle bits of
/// the `TAG_HOST` wrapper that will stand for it.
fn mint_attr_record(ctx: &HostContext, record: AttrRecord) -> u64 {
    let key = ctx
        .host_records
        .borrow_mut()
        .insert(HostRecord::Attr(record));
    encode_host(ctx.slot, key)
}

/// Reads the `AttrRecord` a `TAG_HOST` handle names, or `None` when the handle
/// is not an `Attr` or its record has been reclaimed.
fn attr_record(ctx: &HostContext, key: HostKey) -> Option<AttrRecord> {
    match ctx.host_records.borrow().get(key) {
        Some(HostRecord::Attr(record)) => Some(AttrRecord {
            element: record.element,
            name: record.name.clone(),
            detached_value: record.detached_value.clone(),
            owner_pin: None,
            child_nodes: record.child_nodes,
        }),
        _ => None,
    }
}

/// A Node-typed handle for an Attr in this world, including a minted document.
fn is_attr_handle(ctx: &HostContext, bits: u64) -> bool {
    let decoded = decode(bits);
    decoded.tag == TAG_HOST
        && ctx.documents.get(decoded.slot).is_some_and(|owner| {
            matches!(
                owner.host_records.borrow().get(decoded.host_key()),
                Some(HostRecord::Attr(_))
            )
        })
}

/// Whether `c` may begin a name at the DOM boundary.
///
/// This is XML's `NameStartChar` as `dom/nodes` pins it: an ASCII letter, `_`,
/// `:`, or any non-ASCII scalar value. XML's own table is narrower — it excludes
/// U+0300, U+037E and U+FFFF, all of which
/// `dom/nodes/Document-createElement.html` and
/// `dom/nodes/Document-createElementNS.js` require to be accepted — so the rule
/// implemented here is the one the gate measures, not the XML table.
/// `tenta_dom::Document::validate_xml_name` keeps the strict XML production for
/// XML documents, where the DOM Standard has no say.
fn is_name_start_char(c: char) -> bool {
    !c.is_ascii() || c.is_ascii_alphabetic() || c == '_' || c == ':'
}

/// Whether `c` may continue a name at the DOM boundary: every character except
/// ASCII whitespace and `>`.
///
/// The asymmetry is the gate's, not a guess: the same tables require
/// `createElement("f<oo")` and `createElement("f}oo")` to succeed while
/// `createElement("foo>")` and `createElement("fo o")` must not.
fn is_name_char(c: char) -> bool {
    !c.is_ascii_whitespace() && c != '>'
}

/// The DOM Standard's *validate* step for a name
/// (<https://dom.spec.whatwg.org/#validate>): the name must match XML's `Name`
/// production, throwing an `InvalidCharacterError` when it does not.
///
/// `name` is the whole string whose characters are checked; `start` is the part
/// whose *first* character has to match `NameStartChar`. The two differ for
/// `createElementNS`, which validates the local part's first character but the
/// whole qualified name's remaining ones — `createElementNS(ns, "0:a")` is
/// accepted with the prefix `0`, so the prefix is never checked.
///
/// This is the *element* name rule. An attribute name uses
/// [`validate_attribute_name`], which is a different — and much more
/// permissive — predicate; see it for why.
fn validate_name(name: &str, start: &str) -> Result<(), JsError> {
    let invalid = |message: &str| JsError::dom_exception("InvalidCharacterError", message);
    if name.is_empty() || start.is_empty() {
        return Err(invalid("The name must not be empty"));
    }
    if !start.chars().next().is_some_and(is_name_start_char) {
        return Err(invalid(
            "The name must start with a letter, underscore or colon",
        ));
    }
    if name.chars().any(|c| !is_name_char(c)) {
        return Err(invalid("The name contains an invalid character"));
    }
    Ok(())
}

/// Whether `c` may appear in an *attribute* name at the DOM boundary.
///
/// The four excluded characters are the whole rule the WPT attribute tables pin,
/// and they are a *different* set from the element rule's:
/// `dom/nodes/attributes.html`'s `valid_names` includes `"0"`, `"~"`, `"\\"`,
/// `"'"` and `'"'` and requires every one of them to be accepted by
/// `setAttribute` and `toggleAttribute`, while
/// `dom/nodes/processing-instruction-attributes.html` requires
/// `"$"`, `"_"` and `"a$b"` to be accepted and `"="`, `">"`, `"/"`, tab, LF and
/// CR to be refused wherever they appear. No first-character rule survives
/// both: `"0"` and `"~"` are accepted names.
fn is_attr_name_char(c: char) -> bool {
    !c.is_ascii_whitespace() && !matches!(c, '=' | '>' | '/')
}

/// Validates an *attribute* name at the DOM boundary — `setAttribute`,
/// `toggleAttribute` and `Document.createAttribute` all use it.
///
/// The name must be non-empty and free of the four characters
/// [`is_attr_name_char`] excludes. The first character is deliberately not
/// checked: see that function for the WPT evidence.
fn validate_attribute_name(name: &str) -> Result<(), JsError> {
    let invalid = |message: &str| JsError::dom_exception("InvalidCharacterError", message);
    if name.is_empty() {
        return Err(invalid("The attribute name must not be empty"));
    }
    if name.chars().any(|c| !is_attr_name_char(c)) {
        return Err(invalid("The attribute name contains an invalid character"));
    }
    Ok(())
}

/// Validates a `DOMTokenList` token (`add`/`remove`/`toggle`/`replace`): an
/// empty token is a `SyntaxError`, a token with ASCII whitespace is an
/// `InvalidCharacterError` (<https://dom.spec.whatwg.org/#dom-domtokenlist-add>).
fn validate_token(token: &str) -> Result<(), JsError> {
    if token.is_empty() {
        return Err(JsError::dom_exception(
            "SyntaxError",
            "The token must not be empty",
        ));
    }
    if token.chars().any(|c| c.is_ascii_whitespace()) {
        return Err(JsError::dom_exception(
            "InvalidCharacterError",
            "The token must not contain ASCII whitespace",
        ));
    }
    Ok(())
}

/// The WHATWG "validate and extract" algorithm for `createElementNS`
/// (<https://dom.spec.whatwg.org/#validate-and-extract>): resolve the namespace,
/// split off any prefix, and reject the prefix/namespace combinations the spec
/// forbids. Returns `(namespace, prefix, local name)`.
fn validate_and_extract<'a>(
    namespace: Option<&str>,
    qualified_name: &'a str,
) -> Result<(Atom, Option<Atom>, &'a str), JsError> {
    // An empty namespace string is treated as the null namespace.
    let namespace = namespace.filter(|ns| !ns.is_empty());

    // Split at the *first* colon. An empty part on either side is invalid, but
    // the local part may itself contain colons: `createElementNS(ns, "f:o:o")`
    // must succeed with the local name `o:o`, and `createElementNS(ns,
    // "prefix::local")` with `:local`. Both are `dom/nodes` rows, and both are
    // why the split is `split_once` and not a no-colon-in-the-local check.
    let (prefix, local) = match qualified_name.split_once(':') {
        Some((prefix, local)) => {
            if prefix.is_empty() || local.is_empty() {
                return Err(JsError::dom_exception(
                    "InvalidCharacterError",
                    "Qualified name has an empty prefix or local name",
                ));
            }
            (Some(prefix), local)
        }
        None => (None, qualified_name),
    };
    validate_name(qualified_name, local)?;

    let ns = namespace.map(Atom::new);
    let namespace_error = |msg: &str| JsError::dom_exception("NamespaceError", msg);
    // A prefix requires a namespace; `xml`/`xmlns` are pinned to their namespaces.
    if prefix.is_some() && ns.is_none() {
        return Err(namespace_error("A prefix requires a non-null namespace"));
    }
    if prefix == Some("xml") && ns != Some(atoms::NS_XML) {
        return Err(namespace_error(
            "The `xml` prefix requires the XML namespace",
        ));
    }
    let is_xmlns = qualified_name == "xmlns" || prefix == Some("xmlns");
    if is_xmlns && ns != Some(atoms::NS_XMLNS) {
        return Err(namespace_error(
            "The `xmlns` name requires the XMLNS namespace",
        ));
    }
    if ns == Some(atoms::NS_XMLNS) && !is_xmlns {
        return Err(namespace_error(
            "The XMLNS namespace requires the `xmlns` name",
        ));
    }

    Ok((ns.unwrap_or(atoms::EMPTY), prefix.map(Atom::new), local))
}

/// The DOM `tagName` of an element: the *qualified* name, ASCII-uppercased for
/// elements in the HTML namespace whose node document is an HTML document
/// (<https://dom.spec.whatwg.org/#dom-element-tagname>).
///
/// The qualified name, not the local name: `createElementNS(HTMLNS, "html:span")`
/// reads back as `"HTML:SPAN"` while its `localName` is `"span"`
/// (`dom/nodes/Document-createElementNS.html`). The uppercasing is ASCII-only,
/// so a non-ASCII local name keeps its case while an ASCII one does not.
fn element_tag_name(doc: &Document, id: NodeId) -> String {
    let Some(local) = doc.tag(id) else {
        return String::new();
    };
    let qualified = match doc.element_prefix(id) {
        Some(prefix) if prefix != atoms::EMPTY => {
            format!("{}:{}", prefix.as_str(), local.as_str())
        }
        _ => local.as_str().to_owned(),
    };
    if doc.is_html_document() && doc.namespace(id) == Some(atoms::NS_HTML) {
        qualified.to_ascii_uppercase()
    } else {
        qualified
    }
}

/// The bit layout of a DOM wrapper handle: `tag:4 | doc_slot:12 | node_id:32 |
/// generation:16`, over the `NodeRef` of `docs/spec/04-dom-and-css.md` §1.
///
/// The document slot was eight bits until a world could hold more than one
/// document, and the four bits it gave up are the tag's: ten tags are in use,
/// `TAG_HOST` multiplexes the host-owned classes by a record field rather than
/// by a tag. A node id occupies 32 bits; generations occupy 16 bits. The arena
/// stores a u32 generation but retires each index before a live generation can
/// exceed the packed field, so no node identity is silently truncated.
///
/// The slot is 12 bits because a page's *demand* is wider than eight: the
/// largest `dom/nodes` file, `DOMImplementation-createDocument.html`, mints
/// over four hundred documents in one page (its 214-row table runs a
/// three-document test for each row that expects no exception). See
/// `DocumentSet` for what happens when even 12 bits run out.
const TAG_SHIFT: u32 = 60;
const DOC_SLOT_SHIFT: u32 = 48;
const NODE_ID_SHIFT: u32 = 16;
/// The document slot's width, and so its mask: bits 48..=59.
const DOC_SLOT_MASK: u64 = (1 << (TAG_SHIFT - DOC_SLOT_SHIFT)) - 1;

/// The highest document slot a handle's `doc_slot` field can carry. Slot 0 is
/// the page's own document, so a world can mint at most this many documents
/// before [`DocumentSet::next_slot`] runs out and a `DOMImplementation` factory
/// starts throwing instead of returning a document.
pub const MAX_DOCUMENT_SLOT: u16 = DOC_SLOT_MASK as u16;

/// Handle tag for a single node.
pub const TAG_NODE: u8 = 0;
/// Handle tag for a node's child `NodeList`.
pub const TAG_NODE_LIST: u8 = 1;
/// Handle tag for a node's element-child `HTMLCollection`.
pub const TAG_HTML_COLLECTION: u8 = 2;
/// Handle tag for a static snapshot `NodeList` (querySelectorAll, etc.).
pub const TAG_STATIC_NODE_LIST: u8 = 3;
/// Handle tag for a static snapshot `HTMLCollection` (getElementsByTagName, etc.).
pub const TAG_STATIC_HTML_COLLECTION: u8 = 4;
/// Handle tag for an `Event` (and its subclasses), keyed into the event slab.
pub const TAG_EVENT: u8 = 5;
/// Handle tag for an element's `DOMTokenList` (`classList`); like a live
/// collection, it anchors to the owning element's node.
pub const TAG_TOKEN_LIST: u8 = 6;
/// Handle tag for an element's inline `CSSStyleProperties` (`style`); it
/// anchors to the owning element's node like a live collection.
pub const TAG_STYLE: u8 = 7;
/// Handle tag for the window. The window is not a node — it has no arena slot
/// and no id — so its handle carries no payload at all: the tag alone names it.
/// It exists because the window is the outermost target of every event path,
/// and a listener's `currentTarget` has to be the window object.
pub const TAG_WINDOW: u8 = 8;
/// Handle tag for a JS-owned host object that is not a node: a `URL`, a
/// `URLSearchParams` or a `Headers`, keyed into the host-record slab. The
/// interface is named by the *record*, not by the tag, because all three share
/// this one tag — the tag is 4 bits of a handle and there is no room for one
/// per class.
pub const TAG_HOST: u8 = 9;
/// Handle tag for an element's `NamedNodeMap` (`attributes`); like a live
/// collection, it anchors to the owning element's node.
///
/// It is tag 10 rather than a [`TAG_HOST`] record because `Element.attributes`
/// is `[SameObject]`: the map *is* the element's attribute list, so two reads
/// have to answer with one object, and a handle derived from the owner's
/// [`NodeRef`] gives that for free. An `Attr` — which names an entry *within*
/// that list — cannot be derived from the element alone and is a host record
/// instead.
pub const TAG_NAMED_NODE_MAP: u8 = 10;
/// Handle tag for an element's `DOMStringMap` (`dataset`); it anchors to the
/// owning element's node like the token list, which is what makes
/// `element.dataset === element.dataset` hold.
pub const TAG_DATASET: u8 = 11;
/// Handle tag for a form control's `ValidityState` (`validity`); it anchors
/// to the control's node like the token list, which is what makes
/// `input.validity === input.validity` hold.
pub const TAG_VALIDITY: u8 = 12;

/// The tag field's width. Every tag has to be below this, and the four bits the
/// document slot took from the tag are why `TAG_HOST` multiplexes by record.
pub const TAG_WIDTH: u32 = 64 - TAG_SHIFT;

/// A handle decoded back into its parts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DecodedHandle {
    /// [`TAG_NODE`], [`TAG_NODE_LIST`], [`TAG_HTML_COLLECTION`], or snapshot tags.
    pub tag: u8,
    /// The document slot the payload belongs to: 0 is the page, and every other
    /// value names a document a `DOMImplementation` minted.
    pub slot: u16,
    /// For node handles: the node id. For snapshot handles: the snapshot index.
    pub node_id: NodeId,
    /// For node handles: the node generation. For snapshot handles: the snapshot generation.
    pub generation: u16,
}

impl DecodedHandle {
    /// Returns the node anchor (Some(node_id)) for live handles, None for snapshot tags.
    /// Used in tag-aware scans to avoid corrupting a real node's HAS_WRAPPER bit
    /// when sweeping dead wrappers.
    pub fn node_anchor(&self) -> Option<NodeId> {
        match self.tag {
            TAG_NODE | TAG_NODE_LIST | TAG_HTML_COLLECTION | TAG_TOKEN_LIST | TAG_STYLE
            | TAG_NAMED_NODE_MAP | TAG_DATASET => Some(self.node_id),
            // Snapshot tags 3 and 4, TAG_EVENT, TAG_WINDOW and TAG_HOST: none of
            // them is anchored to a node. A window handle and a host handle are
            // deliberately excluded so a sweep cannot mistake their small
            // payload for a node id.
            _ => None,
        }
    }

    /// Reconstructs a [`HostKey`] from decoded host handle parts. Only valid
    /// when `tag` is [`TAG_HOST`].
    pub fn host_key(&self) -> HostKey {
        HostKey {
            index: self.node_id.raw(),
            generation: self.generation,
        }
    }

    /// Reconstructs a SnapshotKey from decoded snapshot handle parts.
    /// Only valid when tag is a snapshot tag.
    pub fn snapshot_key(&self) -> SnapshotKey {
        SnapshotKey {
            index: self.node_id.raw(),
            generation: self.generation,
        }
    }

    /// Reconstructs an [`EventKey`] from decoded event handle parts. Only valid
    /// when `tag` is [`TAG_EVENT`].
    pub fn event_key(&self) -> EventKey {
        EventKey {
            index: self.node_id.raw(),
            generation: self.generation,
        }
    }
}

/// Packs a `tag`, a document slot and a node reference into the opaque handle
/// bits a wrapper world keys on.
///
/// The slot is masked rather than checked: every caller builds it from a
/// [`DocumentSet`] that never hands out a slot wider than the field, and
/// `DocumentSet::next_slot` is where running out is turned into an error.
fn pack(tag: u8, slot: u16, node: NodeRef) -> u64 {
    let generation = u16::try_from(node.generation)
        .expect("live native generations fit the packed handle identity");
    ((tag as u64) << TAG_SHIFT)
        | (((slot as u64) & DOC_SLOT_MASK) << DOC_SLOT_SHIFT)
        | ((node.id.raw() as u64) << NODE_ID_SHIFT)
        | (generation as u64)
}

/// Packs a node reference into a [`TAG_NODE`] handle, in document `slot`.
///
/// References minted for live document nodes fit the sixteen-bit generation
/// field because the arena retires exhausted indices. Packing does not test
/// current liveness; a previously minted stale reference remains stale when
/// resolved by the wrapper world.
///
/// # Panics
///
/// Panics if `node.generation` exceeds `u16::MAX`, violating the minted-reference
/// precondition. Such a value is never the generation of a live arena node.
pub fn encode_node(slot: u16, node: NodeRef) -> u64 {
    pack(TAG_NODE, slot, node)
}

/// Packs a list owner's reference into a [`TAG_NODE_LIST`] handle.
///
/// # Panics
///
/// Uses the same minted-generation precondition as [`encode_node`].
pub fn encode_collection(slot: u16, owner: NodeRef) -> u64 {
    pack(TAG_NODE_LIST, slot, owner)
}

/// Packs a collection owner's reference into a [`TAG_HTML_COLLECTION`] handle.
///
/// # Panics
///
/// Uses the same minted-generation precondition as [`encode_node`].
pub fn encode_html_collection(slot: u16, owner: NodeRef) -> u64 {
    pack(TAG_HTML_COLLECTION, slot, owner)
}

/// Packs a token-list owner's reference into a [`TAG_TOKEN_LIST`] handle.
///
/// # Panics
///
/// Uses the same minted-generation precondition as [`encode_node`].
pub fn encode_token_list(slot: u16, owner: NodeRef) -> u64 {
    pack(TAG_TOKEN_LIST, slot, owner)
}

/// Packs a `dataset` owner's reference into a [`TAG_DATASET`] handle.
///
/// # Panics
///
/// Uses the same minted-generation precondition as [`encode_node`].
pub fn encode_dataset(slot: u16, owner: NodeRef) -> u64 {
    pack(TAG_DATASET, slot, owner)
}

/// Packs a form control's reference into a [`TAG_VALIDITY`] handle.
///
/// # Panics
///
/// Uses the same minted-generation precondition as [`encode_node`].
pub fn encode_validity(slot: u16, owner: NodeRef) -> u64 {
    pack(TAG_VALIDITY, slot, owner)
}

/// Packs a `NamedNodeMap` owner's reference into a [`TAG_NAMED_NODE_MAP`] handle.
///
/// # Panics
///
/// Uses the same minted-generation precondition as [`encode_node`].
pub fn encode_named_node_map(slot: u16, owner: NodeRef) -> u64 {
    pack(TAG_NAMED_NODE_MAP, slot, owner)
}

/// Packs a style declaration owner's reference into a [`TAG_STYLE`] handle.
///
/// # Panics
///
/// Uses the same minted-generation precondition as [`encode_node`].
pub fn encode_style(slot: u16, owner: NodeRef) -> u64 {
    pack(TAG_STYLE, slot, owner)
}

/// Packs the window into a [`TAG_WINDOW`] handle. Every window handle is
/// identical, which is what lets a listener compare `currentTarget` against the
/// global object.
///
/// The bits carry document slot 0 because they carry no payload at all: the
/// window is the global object of the realm `wrapper` answers from, whichever
/// slot the world was asked for. A document with no browsing context has no
/// window, so nothing ever asks for one in a minted slot.
pub fn encode_window() -> u64 {
    (TAG_WINDOW as u64) << TAG_SHIFT
}

/// Packs a snapshot tag and key into handle bits, in document `slot`.
pub fn encode_snapshot(slot: u16, tag: u8, key: SnapshotKey) -> u64 {
    ((tag as u64) << TAG_SHIFT)
        | (((slot as u64) & DOC_SLOT_MASK) << DOC_SLOT_SHIFT)
        | ((key.index as u64) << NODE_ID_SHIFT)
        | (key.generation as u64)
}

/// Packs an [`EventKey`] into [`TAG_EVENT`] handle bits, in document `slot`.
pub fn encode_event(slot: u16, key: EventKey) -> u64 {
    ((TAG_EVENT as u64) << TAG_SHIFT)
        | (((slot as u64) & DOC_SLOT_MASK) << DOC_SLOT_SHIFT)
        | ((key.index as u64) << NODE_ID_SHIFT)
        | (key.generation as u64)
}

/// Packs a [`HostKey`] into [`TAG_HOST`] handle bits, in document `slot`.
///
/// The bits are the object's identity: the wrapper world keeps one wrapper per
/// handle, so two accesses that produce the same key produce the same JS object.
/// That is what makes `url.searchParams === url.searchParams` hold.
pub fn encode_host(slot: u16, key: HostKey) -> u64 {
    ((TAG_HOST as u64) << TAG_SHIFT)
        | (((slot as u64) & DOC_SLOT_MASK) << DOC_SLOT_SHIFT)
        | ((key.index as u64) << NODE_ID_SHIFT)
        | (key.generation as u64)
}

/// Splits handle bits back into their tag, document slot, node id and
/// generation.
pub fn decode(bits: u64) -> DecodedHandle {
    DecodedHandle {
        tag: (bits >> TAG_SHIFT) as u8,
        slot: ((bits >> DOC_SLOT_SHIFT) & DOC_SLOT_MASK) as u16,
        node_id: NodeId(((bits >> NODE_ID_SHIFT) & 0xFFFF_FFFF) as u32),
        generation: (bits & 0xFFFF) as u16,
    }
}
