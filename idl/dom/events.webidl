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
  constructor();
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

// ErrorEvent (HTML, <https://html.spec.whatwg.org/multipage/webappapis.html#errorevent>).
// The engine fires one named `error` at the window for an exception nothing
// caught. `error` has no default for the reason `CustomEventInit.detail` has
// none; an absent member reads back as `undefined`, which is the standard's
// default for it.
dictionary ErrorEventInit : EventInit {
  DOMString message = "";
  USVString filename = "";
  unsigned long lineno = 0;
  unsigned long colno = 0;
  any error;
};

[Exposed=Window]
interface ErrorEvent : Event {
  constructor(DOMString type, optional ErrorEventInit eventInitDict = {});
  readonly attribute DOMString message;
  readonly attribute USVString filename;
  readonly attribute unsigned long lineno;
  readonly attribute unsigned long colno;
  readonly attribute any error;
};

// UIEvent / MouseEvent. `Window` is an opaque interface marker here: the page
// global is its only supported instance until the Window bindings are modeled.
dictionary UIEventInit : EventInit {
  Window? view = null;
  long detail = 0;
  // UI Events legacy supplemental dictionary member (§7.1).
  unsigned long which = 0;
};

[Exposed=Window]
interface UIEvent : Event {
  constructor(DOMString type, optional UIEventInit eventInitDict = {});
  readonly attribute Window? view;
  readonly attribute long detail;
  readonly attribute unsigned long which;
  undefined initUIEvent(DOMString typeArg, optional boolean bubblesArg = false, optional boolean cancelableArg = false, optional Window? viewArg = null, optional long detailArg = 0);
};

dictionary EventModifierInit : UIEventInit {
  boolean ctrlKey = false;
  boolean shiftKey = false;
  boolean altKey = false;
  boolean metaKey = false;
  boolean modifierAltGraph = false;
  boolean modifierCapsLock = false;
  boolean modifierFn = false;
  boolean modifierFnLock = false;
  boolean modifierHyper = false;
  boolean modifierNumLock = false;
  boolean modifierScrollLock = false;
  boolean modifierSuper = false;
  boolean modifierSymbol = false;
  boolean modifierSymbolLock = false;
};

dictionary FocusEventInit : UIEventInit {
  EventTarget? relatedTarget = null;
};

[Exposed=Window]
interface FocusEvent : UIEvent {
  constructor(DOMString type, optional FocusEventInit eventInitDict = {});
  readonly attribute EventTarget? relatedTarget;
};

dictionary KeyboardEventInit : EventModifierInit {
  DOMString key = "";
  DOMString code = "";
  unsigned long location = 0;
  boolean repeat = false;
  boolean isComposing = false;
  // UI Events legacy supplemental dictionary members (§7.2.2), retained for
  // Event-subclasses-constructors.html compatibility.
  unsigned long charCode = 0;
  unsigned long keyCode = 0;
};

[Exposed=Window]
interface KeyboardEvent : UIEvent {
  constructor(DOMString type, optional KeyboardEventInit eventInitDict = {});
  const unsigned long DOM_KEY_LOCATION_STANDARD = 0;
  const unsigned long DOM_KEY_LOCATION_LEFT = 1;
  const unsigned long DOM_KEY_LOCATION_RIGHT = 2;
  const unsigned long DOM_KEY_LOCATION_NUMPAD = 3;
  readonly attribute DOMString key;
  readonly attribute DOMString code;
  readonly attribute unsigned long location;
  readonly attribute boolean ctrlKey;
  readonly attribute boolean shiftKey;
  readonly attribute boolean altKey;
  readonly attribute boolean metaKey;
  readonly attribute boolean repeat;
  readonly attribute boolean isComposing;
  boolean getModifierState(DOMString keyArg);
  readonly attribute unsigned long charCode;
  readonly attribute unsigned long keyCode;
  undefined initKeyboardEvent(DOMString typeArg, optional boolean bubblesArg = false, optional boolean cancelableArg = false, optional Window? viewArg = null, optional DOMString keyArg = "", optional unsigned long locationArg = 0, optional boolean ctrlKey = false, optional boolean altKey = false, optional boolean shiftKey = false, optional boolean metaKey = false);
};

dictionary MouseEventInit : EventModifierInit {
  long screenX = 0;
  long screenY = 0;
  long clientX = 0;
  long clientY = 0;
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
  boolean getModifierState(DOMString keyArg);
  undefined initMouseEvent(DOMString typeArg, optional boolean bubblesArg = false, optional boolean cancelableArg = false, optional Window? viewArg = null, optional long detailArg = 0, optional long screenXArg = 0, optional long screenYArg = 0, optional long clientXArg = 0, optional long clientYArg = 0, optional boolean ctrlKeyArg = false, optional boolean altKeyArg = false, optional boolean shiftKeyArg = false, optional boolean metaKeyArg = false, optional short buttonArg = 0, optional EventTarget? relatedTargetArg = null);
};

dictionary WheelEventInit : MouseEventInit {
  double deltaX = 0.0;
  double deltaY = 0.0;
  double deltaZ = 0.0;
  unsigned long deltaMode = 0;
};

[Exposed=Window]
interface WheelEvent : MouseEvent {
  constructor(DOMString type, optional WheelEventInit eventInitDict = {});
  const unsigned long DOM_DELTA_PIXEL = 0;
  const unsigned long DOM_DELTA_LINE = 1;
  const unsigned long DOM_DELTA_PAGE = 2;
  readonly attribute double deltaX;
  readonly attribute double deltaY;
  readonly attribute double deltaZ;
  readonly attribute unsigned long deltaMode;
};

dictionary CompositionEventInit : UIEventInit {
  DOMString data = "";
};

[Exposed=Window]
interface CompositionEvent : UIEvent {
  constructor(DOMString type, optional CompositionEventInit eventInitDict = {});
  readonly attribute USVString data;
  undefined initCompositionEvent(DOMString typeArg, optional boolean bubblesArg = false, optional boolean cancelableArg = false, optional WindowProxy? viewArg = null, optional DOMString dataArg = "");
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
