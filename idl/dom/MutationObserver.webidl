// MutationObserver and MutationRecord (DOM Standard §4.3,
// <https://dom.spec.whatwg.org/#mutation-observers>).
//
// The native half lives in `tenta-dom` (`document/observer_*.rs`): the
// registrations, the transient registered observers, the record queue and the
// pending-observer set. These interfaces are the script-visible half. A
// `MutationObserver` is a host record that owns its callback and one
// `NativeObserver`; a `MutationRecord` is a host record that owns one published
// native record (see `HostRecord::MutationObserver` / `MutationRecord` in
// `crates/tenta-js/src/bindings/host.rs`).
//
// Deviations, all recorded in `docs/dev/implementation-status.md`:
//  - `addedNodes` / `removedNodes` are static `NodeList`s built on first read.
//    Their `[SameObject]` identity holds while a wrapper is alive.
//  - slot-change records are not produced: shadow DOM does not exist yet.
//  - `MutationObserverInit.attributeFilter` is stored as atoms, so a filter
//    entry matches the attribute's local name only, as the standard's "name"
//    comparison does for a null namespace.

callback MutationCallback = undefined (sequence<MutationRecord> mutations, MutationObserver observer);

dictionary MutationObserverInit {
  boolean childList = false;
  boolean attributes;
  boolean characterData;
  boolean subtree = false;
  boolean attributeOldValue;
  boolean characterDataOldValue;
  sequence<DOMString> attributeFilter;
};

[Exposed=Window]
interface MutationObserver {
  constructor(MutationCallback callback);

  [Throws] undefined observe(Node target, optional MutationObserverInit options = {});
  [Throws] undefined disconnect();
  [Throws] sequence<MutationRecord> takeRecords();
};

[Exposed=Window]
interface MutationRecord {
  readonly attribute DOMString type;
  [SameObject] readonly attribute Node target;
  [SameObject] readonly attribute NodeList addedNodes;
  [SameObject] readonly attribute NodeList removedNodes;
  readonly attribute Node? previousSibling;
  readonly attribute Node? nextSibling;
  readonly attribute DOMString? attributeName;
  readonly attribute DOMString? attributeNamespace;
  readonly attribute DOMString? oldValue;
};
