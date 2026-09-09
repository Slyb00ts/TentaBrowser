// DOM events WebIDL for P2 binding Phase 3a (`docs/spec/05-javascript.md`).
//
// This is the first slice that calls back into script: an `EventListener` is a
// JS callback that `dispatchEvent` (and later `element.click()`) invokes. It
// carries the smallest surface that exercises the callback boundary end to end
// via `document.createEvent` + `initEvent` — no constructors yet (JS-constructed
// events are Phase 3c / generator G8), and no `srcElement`/`composedPath`/
// `cancelBubble`/`returnValue` (Phase 3b).

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

typedef double DOMHighResTimeStamp;

[Exposed=Window]
interface EventTarget {
  undefined addEventListener(DOMString type, EventListener? callback, optional (AddEventListenerOptions or boolean) options = {});
  undefined removeEventListener(DOMString type, EventListener? callback, optional (EventListenerOptions or boolean) options = {});
  boolean dispatchEvent(Event event);
};

[Exposed=Window]
interface Event {
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
