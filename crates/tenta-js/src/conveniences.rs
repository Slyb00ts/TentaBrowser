//! Static members that are a few lines over interfaces the bindings already
//! have, as a script the engine runs before any page script:
//! `AbortSignal.timeout()`, `AbortSignal.any()`, `Response.json()` and
//! `navigator.sendBeacon()`.
//!
//! They are a script because each is what a page would write itself.
//! `timeout` is a controller aborted by a timer, and the timer is the page's
//! own, so it is ordered and accounted with the page's other timers. `any`
//! is a controller aborted by the first of its sources, through an `abort`
//! listener on each. `json` serializes with `JSON.stringify`, which may run
//! the page's `toJSON`, and hands the text to the `Response` constructor.
//! `sendBeacon` is a `fetch()` whose answer nobody reads.
//! The natives they use are taken before page script could replace them.
//!
//! What is left out: a signal `any()` returned stays reachable from its
//! sources for as long as they live (the standard holds it weakly), and the
//! timer of `timeout()` keeps its signal until it fires.

/// Defines `AbortSignal.timeout`, `AbortSignal.any`, `Response.json` and
/// `navigator.sendBeacon`.
/// Run once when a page's engine is built; a member whose interface the
/// realm lacks is not defined.
pub const CONVENIENCES_PRELUDE_SOURCE: &str = r#"(function () {
  'use strict';
  var define = Object.defineProperty, apply = Reflect.apply, TypeErr = TypeError;
  var describe = Object.getOwnPropertyDescriptor, truncate = Math.trunc, toNumber = Number;
  function getter(prototype, name) {
    var descriptor = describe(prototype, name);
    return descriptor && descriptor.get;
  }
  function method(target, name, length, fn) {
    define(fn, 'name', { value: name, configurable: true });
    define(fn, 'length', { value: length, configurable: true });
    define(target, name, { value: fn, writable: true, enumerable: true, configurable: true });
  }
  function required(count, interfaceName, name) {
    if (count < 1) {
      throw new TypeErr("Failed to execute '" + name + "' on '" + interfaceName +
        "': 1 argument required, but only 0 present.");
    }
  }

  var Signal = globalThis.AbortSignal, Controller = globalThis.AbortController;
  var Target = globalThis.EventTarget, Exception = globalThis.DOMException;
  var setTimer = globalThis.setTimeout;
  if (typeof Signal === 'function' && typeof Controller === 'function' &&
      typeof Target === 'function') {
    var abort = Controller.prototype.abort, listen = Target.prototype.addEventListener;
    var signalOf = getter(Controller.prototype, 'signal');
    var abortedOf = getter(Signal.prototype, 'aborted');
    var reasonOf = getter(Signal.prototype, 'reason');
    if (signalOf && abortedOf && reasonOf && typeof setTimer === 'function') {
      method(Signal, 'timeout', 1, function (milliseconds) {
        required(arguments.length, 'AbortSignal', 'timeout');
        // `[EnforceRange] unsigned long long`.
        var delay = truncate(toNumber(milliseconds));
        if (!(delay >= 0) || delay > 9007199254740991) {
          throw new TypeErr("Failed to execute 'timeout' on 'AbortSignal': " +
            "Value is outside the 'unsigned long long' value range.");
        }
        var controller = new Controller();
        setTimer(function () {
          apply(abort, controller, [new Exception('signal timed out', 'TimeoutError')]);
        }, delay);
        return apply(signalOf, controller, []);
      });
    }
    if (signalOf && abortedOf && reasonOf) {
      method(Signal, 'any', 1, function (signals) {
        required(arguments.length, 'AbortSignal', 'any');
        // The sequence is converted first, and every member has to be a
        // signal: the accessor is the brand check.
        var sources = [], count = 0;
        for (var source of signals) {
          apply(abortedOf, source, []);
          define(sources, count++, { value: source, writable: true, enumerable: true, configurable: true });
        }
        var controller = new Controller(), result = apply(signalOf, controller, []);
        var index;
        for (index = 0; index < count; index++) {
          if (apply(abortedOf, sources[index], [])) {
            apply(abort, controller, [apply(reasonOf, sources[index], [])]);
            return result;
          }
        }
        function follow(source) {
          // The first source to abort decides; `abort()` ignores the rest.
          apply(listen, source, ['abort', function () {
            apply(abort, controller, [apply(reasonOf, source, [])]);
          }]);
        }
        for (index = 0; index < count; index++) follow(sources[index]);
        return result;
      });
    }
  }

  var navigatorObject = globalThis.navigator, fetchFn = globalThis.fetch;
  var UrlCtor = globalThis.URL, BlobCtor = globalThis.Blob, NodeCtor = globalThis.Node;
  var PromiseCtor = Promise;
  if (navigatorObject !== null && typeof navigatorObject === 'object' &&
      typeof fetchFn === 'function' && typeof UrlCtor === 'function' &&
      typeof NodeCtor === 'function') {
    var then = PromiseCtor.prototype.then, hrefOf = getter(UrlCtor.prototype, 'href');
    var protocolOf = getter(UrlCtor.prototype, 'protocol');
    var baseOf = getter(NodeCtor.prototype, 'baseURI');
    var blobType = typeof BlobCtor === 'function' ? getter(BlobCtor.prototype, 'type') : undefined;
    var lower = String.prototype.toLowerCase, cut = String.prototype.split;
    var trim = String.prototype.trim, documentObject = globalThis.document;
    var ignore = function () {};
    // A body type a form could have sent goes out without asking; any other
    // makes the request a CORS one, as the Beacon standard has it.
    var formType = function (type) {
      var essence = apply(lower, apply(trim, apply(cut, type, [';'])[0], []), []);
      return essence === 'application/x-www-form-urlencoded' ||
        essence === 'multipart/form-data' || essence === 'text/plain';
    };
    if (hrefOf && protocolOf && baseOf) {
      method(navigatorObject, 'sendBeacon', 1, function (url) {
        required(arguments.length, 'Navigator', 'sendBeacon');
        var data = arguments[1];
        var target = new UrlCtor(url, apply(baseOf, documentObject, []));
        var protocol = apply(protocolOf, target, []);
        if (protocol !== 'http:' && protocol !== 'https:') {
          throw new TypeErr("Failed to execute 'sendBeacon' on 'Navigator': " +
            "Beacons are only supported over HTTP(S).");
        }
        var init = { method: 'POST', keepalive: true, credentials: 'include', mode: 'no-cors' };
        if (data !== undefined && data !== null) {
          init.body = data;
          if (blobType) {
            var type;
            try { type = apply(blobType, data, []); } catch (error) { type = ''; }
            if (type !== '' && !formType(type)) init.mode = 'cors';
          }
        }
        // The request is queued and the answer is nobody's: true says only
        // that it was accepted.
        try {
          apply(then, fetchFn(apply(hrefOf, target, []), init), [ignore, ignore]);
        } catch (error) {
          return false;
        }
        return true;
      });
    }
  }

  var ResponseCtor = globalThis.Response, HeadersCtor = globalThis.Headers;
  if (typeof ResponseCtor === 'function' && typeof HeadersCtor === 'function') {
    var stringify = JSON.stringify;
    var has = HeadersCtor.prototype.has, set = HeadersCtor.prototype.set;
    method(ResponseCtor, 'json', 1, function (data) {
      required(arguments.length, 'Response', 'json');
      var init = arguments[1];
      var text = stringify(data);
      if (text === undefined) {
        throw new TypeErr("Failed to execute 'json' on 'Response': " +
          "The data is not JSON serializable");
      }
      var options = init === undefined || init === null ? {} : init;
      var given = options.headers;
      var headers = given === undefined ? new HeadersCtor() : new HeadersCtor(given);
      if (!apply(has, headers, ['content-type'])) {
        apply(set, headers, ['content-type', 'application/json']);
      }
      return new ResponseCtor(text, {
        status: options.status, statusText: options.statusText, headers: headers
      });
    });
  }
})();
"#;
