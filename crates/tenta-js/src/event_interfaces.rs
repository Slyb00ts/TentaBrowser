//! The event interfaces that are an `Event`, `UIEvent` or `MouseEvent` with a
//! few more attributes read from the init dictionary: `InputEvent`,
//! `PointerEvent`, `PopStateEvent`, `HashChangeEvent`, `PageTransitionEvent`,
//! `TransitionEvent`, `AnimationEvent`, `SubmitEvent`, `CloseEvent`,
//! `StorageEvent`, `ClipboardEvent`, `DragEvent`, `ToggleEvent`,
//! `FormDataEvent`, `MediaQueryListEvent` and `BeforeUnloadEvent`.
//!
//! They are a script because none of them has state the engine reads: each
//! is a class that extends the native interface, so the event is a real one
//! to `dispatchEvent()`, and keeps its own attributes beside it. The host
//! defines the names when the engine is built and runs the script when a page
//! first uses one (`tenta-js-v8`'s `host::event_interfaces`).
//!
//! What is left out: `TouchEvent` and `DataTransfer` (`clipboardData` and
//! `dataTransfer` are whatever the init dictionary gave, `null` otherwise);
//! `getCoalescedEvents()`, `getPredictedEvents()` and `getTargetRanges()`
//! answer an empty list; an init member of the wrong interface is not
//! refused; and the events the engine itself fires for typing, pointer
//! input, form submission, transitions and storage are still plain `Event`,
//! `MouseEvent` or `UIEvent` objects, except `popstate` and `hashchange`.

/// Evaluates to a function that returns the interface objects by name.
pub const EVENT_INTERFACES_PRELUDE_SOURCE: &str = r#"
(function () {
  'use strict';
  var define = Object.defineProperty, apply = Reflect.apply, TypeErr = TypeError;
  var States = WeakMap, mapGet = States.prototype.get, mapSet = States.prototype.set;
  var toText = String, toNumber = Number, tag = Symbol.toStringTag;
  var EventCtor = globalThis.Event, UIEventCtor = globalThis.UIEvent;
  var MouseEventCtor = globalThis.MouseEvent;
  var exports = {};

  function convert(kind, value) {
    switch (kind) {
      case 'string': return toText(value);
      case 'string?': return value === null ? null : toText(value);
      case 'boolean': return !!value;
      case 'double': return toNumber(value);
      case 'long': return value | 0;
      case 'ushort': return value & 65535;
      case 'object?': return value === undefined ? null : value;
      default: return value;
    }
  }

  // `members` is a list of [name, kind, default]; `methods` of [name, value].
  function make(name, Parent, members, methods, required) {
    if (typeof Parent !== 'function') return;
    var states = new States();
    var Interface = ({ [name]: class extends Parent {
      constructor(type) {
        var init = arguments[1];
        if (arguments.length < 1) {
          throw new TypeErr("Failed to construct '" + name +
            "': 1 argument required, but only 0 present.");
        }
        if (required && (init === undefined || init === null ||
            init[required] === undefined)) {
          throw new TypeErr("Failed to construct '" + name + "': required member " +
            required + " is undefined.");
        }
        super(type, init);
        var source = init === undefined || init === null ? {} : init;
        var state = {}, index, member, value;
        for (index = 0; index < members.length; index++) {
          member = members[index];
          value = source[member[0]];
          state[member[0]] = value === undefined ? member[2] : convert(member[1], value);
        }
        apply(mapSet, states, [this, state]);
      }
    } })[name];
    define(Interface, 'length', { value: 1, configurable: true });
    members.forEach(function (member) {
      var key = member[0];
      var get = ({ ['get ' + key]: function () {
        var state = apply(mapGet, states, [this]);
        if (state === undefined) throw new TypeErr('Illegal invocation');
        return state[key];
      } })['get ' + key];
      define(Interface.prototype, key, { get: get, enumerable: true, configurable: true });
    });
    (methods || []).forEach(function (entry) {
      var key = entry[0], result = entry[1];
      var fn = ({ [key]: function () {
        if (apply(mapGet, states, [this]) === undefined) throw new TypeErr('Illegal invocation');
        return result();
      } })[key];
      define(Interface.prototype, key,
        { value: fn, writable: true, enumerable: true, configurable: true });
    });
    define(Interface.prototype, tag, { value: name, configurable: true });
    exports[name] = Interface;
    return states;
  }
  function empty() { return []; }

  make('InputEvent', UIEventCtor, [
    ['data', 'string?', null], ['isComposing', 'boolean', false],
    ['inputType', 'string', ''], ['dataTransfer', 'object?', null]
  ], [['getTargetRanges', empty]]);
  make('PointerEvent', MouseEventCtor, [
    ['pointerId', 'long', 0], ['width', 'double', 1], ['height', 'double', 1],
    ['pressure', 'double', 0], ['tangentialPressure', 'double', 0],
    ['tiltX', 'long', 0], ['tiltY', 'long', 0], ['twist', 'long', 0],
    ['altitudeAngle', 'double', Math.PI / 2], ['azimuthAngle', 'double', 0],
    ['pointerType', 'string', ''], ['isPrimary', 'boolean', false]
  ], [['getCoalescedEvents', empty], ['getPredictedEvents', empty]]);
  make('PopStateEvent', EventCtor, [
    ['state', 'any', null], ['hasUAVisualTransition', 'boolean', false]
  ]);
  make('HashChangeEvent', EventCtor, [['oldURL', 'string', ''], ['newURL', 'string', '']]);
  make('PageTransitionEvent', EventCtor, [['persisted', 'boolean', false]]);
  make('TransitionEvent', EventCtor, [
    ['propertyName', 'string', ''], ['elapsedTime', 'double', 0],
    ['pseudoElement', 'string', '']
  ]);
  make('AnimationEvent', EventCtor, [
    ['animationName', 'string', ''], ['elapsedTime', 'double', 0],
    ['pseudoElement', 'string', '']
  ]);
  make('SubmitEvent', EventCtor, [['submitter', 'object?', null]]);
  make('CloseEvent', EventCtor, [
    ['wasClean', 'boolean', false], ['code', 'ushort', 0], ['reason', 'string', '']
  ]);
  make('StorageEvent', EventCtor, [
    ['key', 'string?', null], ['oldValue', 'string?', null], ['newValue', 'string?', null],
    ['url', 'string', ''], ['storageArea', 'object?', null]
  ]);
  make('ClipboardEvent', EventCtor, [['clipboardData', 'object?', null]]);
  make('DragEvent', MouseEventCtor, [['dataTransfer', 'object?', null]]);
  make('ToggleEvent', EventCtor, [['oldState', 'string', ''], ['newState', 'string', '']]);
  make('FormDataEvent', EventCtor, [['formData', 'any', null]], null, 'formData');
  make('MediaQueryListEvent', EventCtor, [['media', 'string', ''], ['matches', 'boolean', false]]);

  // `BeforeUnloadEvent` has no constructor: only the engine makes one.
  if (typeof EventCtor === 'function') {
    var values = new States();
    var BeforeUnload = function BeforeUnloadEvent() { throw new TypeErr('Illegal constructor'); };
    BeforeUnload.prototype = Object.create(EventCtor.prototype, {
      constructor: { value: BeforeUnload, writable: true, configurable: true }
    });
    Object.setPrototypeOf(BeforeUnload, EventCtor);
    define(BeforeUnload.prototype, 'returnValue', {
      get: function () {
        var value = apply(mapGet, values, [this]);
        return value === undefined ? '' : value;
      },
      set: function (value) { apply(mapSet, values, [this, toText(value)]); },
      enumerable: true, configurable: true
    });
    define(BeforeUnload.prototype, tag, { value: 'BeforeUnloadEvent', configurable: true });
    exports.BeforeUnloadEvent = BeforeUnload;
  }
  return exports;
})
"#;
