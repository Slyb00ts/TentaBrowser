// DOM events WebIDL for P2 binding Phase 3a (`docs/spec/05-javascript.md`).
//
// This is the first surface that calls back into script: an `EventListener` is
// a JS callback that `dispatchEvent` (and later `element.click()`) invokes.
// Phase 3c adds the `Event`/`CustomEvent` constructors (JS-constructed events
// via generator gap G8: the `new`-allocated object is adopted into the wrapper
// world so it gets a slab-backed host, a finalizer and stable identity).

callback interface EventListener {
  undefined handleEvent(Event event);
};

dictionary EventListenerOptions {
  boolean capture = false;
};

dictionary AddEventListenerOptions : EventListenerOptions {
  boolean passive;
  boolean once = false;
};

dictionary EventInit {
  boolean bubbles = false;
  boolean cancelable = false;
  boolean composed = false;
};

// `detail` has no default: `any x = null` does not lower cleanly, so an
// absent member is `None` and the host treats it as the JS `null`.
dictionary CustomEventInit : EventInit {
  any detail;
};

typedef double DOMHighResTimeStamp;

[Exposed=Window]
interface EventTarget {
  undefined addEventListener(DOMString type, EventListener? callback, optional (AddEventListenerOptions or boolean) options = {});
  undefined removeEventListener(DOMString type, EventListener? callback, optional (EventListenerOptions or boolean) options = {});
  boolean dispatchEvent(Event event);
};

[Exposed=Window]
interface Event {
  constructor(DOMString type, optional EventInit eventInitDict = {});

  readonly attribute DOMString type;
  readonly attribute EventTarget? target;
  readonly attribute EventTarget? currentTarget;
  readonly attribute EventTarget? srcElement;
  sequence<EventTarget> composedPath();

  const unsigned short NONE = 0;
  const unsigned short CAPTURING_PHASE = 1;
  const unsigned short AT_TARGET = 2;
  const unsigned short BUBBLING_PHASE = 3;
  readonly attribute unsigned short eventPhase;

  undefined stopPropagation();
  attribute boolean cancelBubble;
  undefined stopImmediatePropagation();

  readonly attribute boolean bubbles;
  readonly attribute boolean cancelable;
  attribute boolean returnValue;
  undefined preventDefault();
  readonly attribute boolean defaultPrevented;
  readonly attribute boolean composed;

  [LegacyUnforgeable] readonly attribute boolean isTrusted;
  readonly attribute DOMHighResTimeStamp timeStamp;

  undefined initEvent(DOMString type, optional boolean bubbles = false, optional boolean cancelable = false);
};

[Exposed=Window]
interface CustomEvent : Event {
  constructor(DOMString type, optional CustomEventInit eventInitDict = {});
  readonly attribute any detail;
  undefined initCustomEvent(DOMString type, optional boolean bubbles = false, optional boolean cancelable = false, optional any detail = null);
};

// Phase 3d: UIEvent / MouseEvent. The record already carries the Ui/Mouse
// payload (dom/events EventKind); these interfaces expose it and add the
// constructors. `relatedTarget` has no default so an absent member is `None`.
dictionary UIEventInit : EventInit {
  long detail = 0;
};

[Exposed=Window]
interface UIEvent : Event {
  constructor(DOMString type, optional UIEventInit eventInitDict = {});
  readonly attribute long detail;
};

dictionary MouseEventInit : UIEventInit {
  long screenX = 0;
  long screenY = 0;
  long clientX = 0;
  long clientY = 0;
  boolean ctrlKey = false;
  boolean shiftKey = false;
  boolean altKey = false;
  boolean metaKey = false;
  short button = 0;
  unsigned short buttons = 0;
  EventTarget? relatedTarget;
};

[Exposed=Window]
interface MouseEvent : UIEvent {
  constructor(DOMString type, optional MouseEventInit eventInitDict = {});
  readonly attribute long screenX;
  readonly attribute long screenY;
  readonly attribute long clientX;
  readonly attribute long clientY;
  readonly attribute boolean ctrlKey;
  readonly attribute boolean shiftKey;
  readonly attribute boolean altKey;
  readonly attribute boolean metaKey;
  readonly attribute short button;
  readonly attribute unsigned short buttons;
  readonly attribute EventTarget? relatedTarget;
};

// XHR Standard. The record already carries the Progress payload (dom/events
// EventKind); this interface exposes it and adds the constructor. The three
// members are independent: the getter steps "return the value they were
// initialized to", so `lengthComputable` is not derived from `total`.
dictionary ProgressEventInit : EventInit {
  boolean lengthComputable = false;
  double loaded = 0;
  double total = 0;
};

[Exposed=Window]
interface ProgressEvent : Event {
  constructor(DOMString type, optional ProgressEventInit eventInitDict = {});
  readonly attribute boolean lengthComputable;
  readonly attribute double loaded;
  readonly attribute double total;
};
