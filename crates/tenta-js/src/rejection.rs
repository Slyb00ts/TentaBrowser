//! `PromiseRejectionEvent` and the dispatch of `unhandledrejection`, as a
//! script the engine runs before any page script.
//!
//! The interface is a script class over `Event` rather than a binding because
//! an event record owns one stored value and this event carries two, the
//! promise and the reason. The class keeps both itself.
//!
//! The script's completion value is the function the engine calls for a
//! promise that was rejected with nobody handling it: it makes the event,
//! dispatches it at the window with the `dispatchEvent` taken before page
//! script could replace it, and answers whether the event was cancelled.
//!
//! Because script makes the event, its `isTrusted` is `false`, where a
//! browser's is `true`. `rejectionhandled` is not fired.

/// Defines `PromiseRejectionEvent` and evaluates to
/// `function (promise, reason) -> boolean`, the cancelled flag of the
/// `unhandledrejection` event it dispatched. Evaluates to `undefined` in a
/// realm without `Event`.
pub const REJECTION_PRELUDE_SOURCE: &str = r#"(function () {
  'use strict';
  var EventCtor = globalThis.Event, Win = globalThis, Target = globalThis.EventTarget;
  if (typeof EventCtor !== 'function' || typeof Target !== 'function') return undefined;
  var define = Object.defineProperty, TypeErr = TypeError, apply = Reflect.apply;
  var dispatch = Target.prototype.dispatchEvent, states = new WeakMap();
  function state(event) {
    var found = states.get(event);
    if (found === undefined) throw new TypeErr('Illegal invocation');
    return found;
  }
  function isObject(value) {
    return value !== null && (typeof value === 'object' || typeof value === 'function');
  }
  class PromiseRejectionEvent extends EventCtor {
    constructor(type, init) {
      if (arguments.length < 2) {
        throw new TypeErr("Failed to construct 'PromiseRejectionEvent': 2 arguments required, but only " +
          arguments.length + ' present.');
      }
      if (!isObject(init)) {
        throw new TypeErr("Failed to construct 'PromiseRejectionEvent': The provided value is not of type 'PromiseRejectionEventInit'.");
      }
      var promise = init.promise, reason = init.reason;
      if (!isObject(promise)) {
        throw new TypeErr("Failed to construct 'PromiseRejectionEvent': Failed to read the 'promise' property from 'PromiseRejectionEventInit': Required member is not an object.");
      }
      super(type, init);
      states.set(this, { promise: promise, reason: reason });
    }
    get promise() { return state(this).promise; }
    get reason() { return state(this).reason; }
  }
  ['promise', 'reason'].forEach(function (name) {
    var descriptor = Object.getOwnPropertyDescriptor(PromiseRejectionEvent.prototype, name);
    descriptor.enumerable = true;
    define(PromiseRejectionEvent.prototype, name, descriptor);
  });
  define(PromiseRejectionEvent.prototype, Symbol.toStringTag,
    { value: 'PromiseRejectionEvent', writable: false, enumerable: false, configurable: true });
  define(Win, 'PromiseRejectionEvent',
    { value: PromiseRejectionEvent, writable: true, enumerable: false, configurable: true });
  return function (promise, reason) {
    var event = new PromiseRejectionEvent('unhandledrejection',
      { promise: promise, reason: reason, cancelable: true });
    return !apply(dispatch, Win, [event]);
  };
})()"#;
