//! `ReadableStream` and what reads one (Streams Standard,
//! <https://streams.spec.whatwg.org/>), as a script the engine runs before any
//! page script, with `Response.prototype.body` and `Blob.prototype.stream()`
//! on top of it.
//!
//! # Why a script
//!
//! A stream is a queue between two pieces of page script — the underlying
//! source's `start`, `pull` and `cancel` on one side, a reader's promises on
//! the other — and the standard's algorithms are written in terms of promises
//! and callbacks at every step. Nothing in them touches the document or the
//! network, so there is no engine state to keep in Rust, and each step done
//! across the host seam would be a conversion in each direction. The
//! algorithms below are the standard's, in its order and under its names
//! (`ReadableStreamDefaultControllerCallPullIfNeeded` is
//! `callPullIfNeeded`, and so on).
//!
//! # What is here
//!
//!  - `ReadableStream` with the default controller: the constructor with an
//!    underlying source and a queuing strategy, `locked`, `cancel()`,
//!    `getReader()`, `tee()`, `values()` and `[Symbol.asyncIterator]`, and the
//!    static `from()`.
//!  - `ReadableStreamDefaultReader`, `ReadableStreamDefaultController`,
//!    `CountQueuingStrategy` and `ByteLengthQueuingStrategy`.
//!  - `Response.prototype.body` and `Blob.prototype.stream()`.
//!
//! # What is not, and what differs
//!
//!  - **No byte streams.** `new ReadableStream({ type: 'bytes' })` and
//!    `getReader({ mode: 'byob' })` throw a `TypeError` that says so;
//!    `ReadableByteStreamController` and `ReadableStreamBYOBReader` are not
//!    defined. `Response.body` and `Blob.stream()` are byte streams in the
//!    standard and default streams of `Uint8Array` chunks here.
//!  - **No `WritableStream` or `TransformStream`**, so no `pipeTo()` or
//!    `pipeThrough()`: the two methods are absent rather than present and
//!    failing.
//!  - **A body is one chunk.** The engine holds a body as a whole byte
//!    sequence, so the stream of a response or a blob delivers it in one
//!    `Uint8Array` and then closes; an empty body closes without a chunk.
//!    Reading the first chunk is what uses the body up (`bodyUsed`), as the
//!    standard's *disturbed* is; a stream that is only obtained, or locked,
//!    does not stop `text()` from reading the body, where the standard rejects
//!    for a locked stream.
//!  - **A stream is not a body.** `new Response(stream)`, a `Request` body and
//!    `fetch()` with a stream are not supported: the body union has no stream
//!    arm, so a stream given there is converted to a string like any other
//!    object.
//!  - **`tee()` hands both branches the same chunk object**, as the standard
//!    does for a default stream.
//!  - Streams are not transferable and `structuredClone` does not take one.

/// Evaluates to a function taking `responseHasBody`, a native function that
/// tells whether a `Response` has a body at all. The function defines nothing
/// on the global: it returns an object with the five interface objects under
/// their names, `responseBody(response)` — the getter of
/// `Response.prototype.body` — and `blobStream(blob)` — `Blob.prototype
/// .stream()`. The engine defines the names and runs this when a page first
/// uses one (`crates/tenta-js-v8/src/host/streams.rs`), so the script must not
/// read any of the five names from the global itself.
pub const STREAMS_PRELUDE_SOURCE: &str = r#"(function (responseHasBody) {
  'use strict';
  var exports = {};
  var define = Object.defineProperty, TypeErr = TypeError, RangeErr = RangeError;
  var PromiseCtor = Promise, Slots = WeakMap, Num = Number;
  var streams = new Slots(), readers = new Slots(), controllers = new Slots();
  var internal = {};

  function method(target, name, fn) {
    define(target, name, { value: fn, writable: true, enumerable: true, configurable: true });
  }
  function getter(target, name, get) {
    define(target, name, { get: get, set: undefined, enumerable: true, configurable: true });
  }
  function interfaceObject(name, Constructor) {
    define(Constructor.prototype, Symbol.toStringTag,
      { value: name, writable: false, enumerable: false, configurable: true });
    exports[name] = Constructor;
  }
  function slot(map, object) {
    var found = map.get(object);
    if (found === undefined) throw new TypeErr('Illegal invocation');
    return found;
  }
  function resolved(value) { return PromiseCtor.resolve(value); }
  function rejected(reason) { return PromiseCtor.reject(reason); }
  // A rejection the standard marks as handled: nobody has to observe it.
  function handled(promise) { promise.then(undefined, function () {}); return promise; }
  function deferred() {
    var d = { promise: null, resolve: null, reject: null };
    d.promise = new PromiseCtor(function (resolve, reject) { d.resolve = resolve; d.reject = reject; });
    return d;
  }
  // A promise-returning operation rejects where another would throw.
  function promising(fn) {
    return function () {
      try { return resolved(fn.apply(this, arguments)); } catch (error) { return rejected(error); }
    };
  }
  function dictionary(value, what) {
    if (value === undefined || value === null) return {};
    if (typeof value !== 'object' && typeof value !== 'function') {
      throw new TypeErr(what + ' is not an object.');
    }
    return value;
  }
  function callback(value, what) {
    if (value === undefined) return undefined;
    if (typeof value !== 'function') throw new TypeErr(what + ' is not a function.');
    return value;
  }

  // ---- Queuing strategies ---------------------------------------------------

  function highWaterMarkOf(strategy, fallback) {
    if (strategy.highWaterMark === undefined) return fallback;
    var mark = Num(strategy.highWaterMark);
    if (mark !== mark || mark < 0) throw new RangeErr('Invalid highWaterMark.');
    return mark;
  }
  function sizeAlgorithmOf(strategy) {
    var size = callback(strategy.size, "The strategy's size");
    if (size === undefined) return function () { return 1; };
    return function (chunk) { return size(chunk); };
  }
  function strategyClass(name, sizeFunction) {
    var marks = new Slots();
    function Strategy(init) {
      if (new.target === undefined) {
        throw new TypeErr("Failed to construct '" + name + "': Please use the 'new' operator");
      }
      if (arguments.length < 1) {
        throw new TypeErr("Failed to construct '" + name + "': 1 argument required, but only 0 present.");
      }
      if (init === null || (typeof init !== 'object' && typeof init !== 'function')) {
        throw new TypeErr("Failed to construct '" + name + "': The provided value is not of type 'QueuingStrategyInit'.");
      }
      if (init.highWaterMark === undefined) {
        throw new TypeErr("Failed to construct '" + name + "': Failed to read the 'highWaterMark' property from 'QueuingStrategyInit': Required member is undefined.");
      }
      marks.set(this, Num(init.highWaterMark));
    }
    getter(Strategy.prototype, 'highWaterMark', function () { return slot(marks, this); });
    getter(Strategy.prototype, 'size', function () { slot(marks, this); return sizeFunction; });
    interfaceObject(name, Strategy);
  }
  strategyClass('CountQueuingStrategy', function size() { return 1; });
  strategyClass('ByteLengthQueuingStrategy', function size(chunk) { return chunk.byteLength; });

  // ---- The queue of a controller ---------------------------------------------

  function resetQueue(controller) { controller.queue = []; controller.queueTotalSize = 0; }
  function enqueueWithSize(controller, value, size) {
    if (typeof size !== 'number' || size !== size || size < 0 || size === Infinity) {
      throw new RangeErr("The return value of a queuing strategy's size function must be a finite, non-NaN, non-negative number");
    }
    controller.queue.push({ value: value, size: size });
    controller.queueTotalSize += size;
  }
  function dequeue(controller) {
    var pair = controller.queue.shift();
    controller.queueTotalSize -= pair.size;
    if (controller.queueTotalSize < 0) controller.queueTotalSize = 0;
    return pair.value;
  }

  // ---- The stream ------------------------------------------------------------
  //
  // A stream's record: { state, reader, storedError, controller, disturbed }.
  // A reader's: { stream, closed (a deferred), requests }. A read request is
  // { chunk, close, error }, the standard's three steps.

  function streamClose(stream) {
    stream.state = 'closed';
    var reader = stream.reader;
    if (reader === undefined) return;
    reader.closed.resolve(undefined);
    var requests = reader.requests;
    reader.requests = [];
    for (var i = 0; i < requests.length; i++) requests[i].close();
  }
  function streamError(stream, error) {
    stream.state = 'errored';
    stream.storedError = error;
    var reader = stream.reader;
    if (reader === undefined) return;
    reader.closed.reject(error);
    handled(reader.closed.promise);
    errorRequests(reader, error);
  }
  function errorRequests(reader, error) {
    var requests = reader.requests;
    reader.requests = [];
    for (var i = 0; i < requests.length; i++) requests[i].error(error);
  }
  function streamCancel(stream, reason) {
    stream.disturbed = true;
    if (stream.state === 'closed') return resolved(undefined);
    if (stream.state === 'errored') return rejected(stream.storedError);
    streamClose(stream);
    return cancelSteps(stream.controller, reason).then(function () { return undefined; });
  }
  function pendingRequests(stream) {
    return stream.reader === undefined ? 0 : stream.reader.requests.length;
  }

  // ---- The default controller --------------------------------------------------

  function canCloseOrEnqueue(controller) {
    return !controller.closeRequested && controller.stream.state === 'readable';
  }
  function desiredSize(controller) {
    var state = controller.stream.state;
    if (state === 'errored') return null;
    if (state === 'closed') return 0;
    return controller.highWaterMark - controller.queueTotalSize;
  }
  function shouldCallPull(controller) {
    if (!canCloseOrEnqueue(controller) || !controller.started) return false;
    if (pendingRequests(controller.stream) > 0) return true;
    return desiredSize(controller) > 0;
  }
  function callPullIfNeeded(controller) {
    if (!shouldCallPull(controller)) return;
    if (controller.pulling) { controller.pullAgain = true; return; }
    controller.pulling = true;
    controller.pull().then(function () {
      controller.pulling = false;
      if (controller.pullAgain) {
        controller.pullAgain = false;
        callPullIfNeeded(controller);
      }
    }, function (error) { controllerError(controller, error); });
  }
  function clearAlgorithms(controller) {
    controller.pull = undefined;
    controller.cancel = undefined;
    controller.size = undefined;
  }
  function controllerClose(controller) {
    if (!canCloseOrEnqueue(controller)) return;
    controller.closeRequested = true;
    if (controller.queue.length === 0) {
      clearAlgorithms(controller);
      streamClose(controller.stream);
    }
  }
  function controllerEnqueue(controller, chunk) {
    if (!canCloseOrEnqueue(controller)) return;
    var stream = controller.stream;
    if (pendingRequests(stream) > 0) {
      stream.reader.requests.shift().chunk(chunk);
    } else {
      try {
        enqueueWithSize(controller, chunk, controller.size(chunk));
      } catch (error) {
        controllerError(controller, error);
        throw error;
      }
    }
    callPullIfNeeded(controller);
  }
  function controllerError(controller, error) {
    var stream = controller.stream;
    if (stream.state !== 'readable') return;
    resetQueue(controller);
    clearAlgorithms(controller);
    streamError(stream, error);
  }
  function cancelSteps(controller, reason) {
    resetQueue(controller);
    var result = controller.cancel(reason);
    clearAlgorithms(controller);
    return result;
  }
  function pullSteps(controller, request) {
    var stream = controller.stream;
    if (controller.queue.length > 0) {
      var chunk = dequeue(controller);
      if (controller.closeRequested && controller.queue.length === 0) {
        clearAlgorithms(controller);
        streamClose(stream);
      } else {
        callPullIfNeeded(controller);
      }
      request.chunk(chunk);
    } else {
      stream.reader.requests.push(request);
      callPullIfNeeded(controller);
    }
  }

  function ReadableStreamDefaultController(token) {
    if (token !== internal) throw new TypeErr('Illegal constructor');
  }
  getter(ReadableStreamDefaultController.prototype, 'desiredSize', function () {
    return desiredSize(slot(controllers, this));
  });
  method(ReadableStreamDefaultController.prototype, 'close', function close() {
    var controller = slot(controllers, this);
    if (!canCloseOrEnqueue(controller)) {
      throw new TypeErr("Failed to execute 'close' on 'ReadableStreamDefaultController': Cannot close a " +
        (controller.closeRequested ? 'stream that is already closing' : controller.stream.state + ' readable stream'));
    }
    controllerClose(controller);
  });
  method(ReadableStreamDefaultController.prototype, 'enqueue', function enqueue(chunk) {
    var controller = slot(controllers, this);
    if (!canCloseOrEnqueue(controller)) {
      throw new TypeErr("Failed to execute 'enqueue' on 'ReadableStreamDefaultController': Cannot enqueue a chunk into " +
        (controller.closeRequested ? 'a stream that is closing' : 'a ' + controller.stream.state + ' readable stream'));
    }
    controllerEnqueue(controller, chunk);
  });
  method(ReadableStreamDefaultController.prototype, 'error', function error(reason) {
    controllerError(slot(controllers, this), reason);
  });
  interfaceObject('ReadableStreamDefaultController', ReadableStreamDefaultController);

  // Makes a stream over the three algorithms. `start` and `pull` are given
  // the controller's record, whose `object` is what a page sees; `start` may
  // throw and returns anything, `pull` and `cancel` return promises.
  function createStream(object, start, pull, cancel, highWaterMark, size) {
    var stream = { state: 'readable', reader: undefined, storedError: undefined,
      controller: null, disturbed: false, object: object };
    var controller = { stream: stream, queue: [], queueTotalSize: 0, started: false,
      closeRequested: false, pullAgain: false, pulling: false, object: null,
      highWaterMark: highWaterMark, size: size, cancel: cancel,
      pull: function () { return pull(controller); } };
    stream.controller = controller;
    controller.object = new ReadableStreamDefaultController(internal);
    controllers.set(controller.object, controller);
    streams.set(object, stream);
    var started = start(controller);
    resolved(started).then(function () {
      controller.started = true;
      callPullIfNeeded(controller);
    }, function (error) { controllerError(controller, error); });
    return stream;
  }
  // A stream the engine makes itself: `pull` and `cancel` may return anything
  // and may throw.
  function internalStream(pull, cancel, highWaterMark) {
    var object = Object.create(ReadableStream.prototype);
    return createStream(object, function () {}, function (controller) {
      try { return resolved(pull(controller)); } catch (error) { return rejected(error); }
    }, function (reason) {
      try { return resolved(cancel(reason)); } catch (error) { return rejected(error); }
    }, highWaterMark, function () { return 1; });
  }

  function ReadableStream() {
    if (new.target === undefined) {
      throw new TypeErr("Failed to construct 'ReadableStream': Please use the 'new' operator");
    }
    var source = arguments[0] === null ? undefined : arguments[0];
    var given = dictionary(source, "Failed to construct 'ReadableStream': The underlying source");
    var strategy = dictionary(arguments[1], "Failed to construct 'ReadableStream': The strategy");
    var type = given.type;
    if (type !== undefined) {
      if (String(type) === 'bytes') {
        throw new TypeErr("Failed to construct 'ReadableStream': byte streams (type: 'bytes') are not supported.");
      }
      throw new TypeErr("Failed to construct 'ReadableStream': The provided value '" + String(type) +
        "' is not a valid enum value of type ReadableStreamType.");
    }
    var prefix = "Failed to construct 'ReadableStream': The underlying source's ";
    var start = callback(given.start, prefix + 'start');
    var pull = callback(given.pull, prefix + 'pull');
    var cancel = callback(given.cancel, prefix + 'cancel');
    var size = sizeAlgorithmOf(strategy);
    var highWaterMark = highWaterMarkOf(strategy, 1);
    createStream(this, function (controller) {
      return start === undefined ? undefined : start.call(source, controller.object);
    }, function (controller) {
      if (pull === undefined) return resolved(undefined);
      try { return resolved(pull.call(source, controller.object)); } catch (error) { return rejected(error); }
    }, function (reason) {
      if (cancel === undefined) return resolved(undefined);
      try { return resolved(cancel.call(source, reason)); } catch (error) { return rejected(error); }
    }, highWaterMark, size);
  }

  // ---- The default reader ------------------------------------------------------

  function acquireReader(stream, object) {
    if (stream.reader !== undefined) {
      throw new TypeErr('ReadableStreamDefaultReader constructor can only accept readable streams that are not yet locked to a reader');
    }
    var reader = { stream: stream, closed: deferred(), requests: [] };
    stream.reader = reader;
    if (stream.state === 'closed') {
      reader.closed.resolve(undefined);
    } else if (stream.state === 'errored') {
      reader.closed.reject(stream.storedError);
      handled(reader.closed.promise);
    }
    if (object !== undefined) readers.set(object, reader);
    return reader;
  }
  function readerRead(reader, request) {
    var stream = reader.stream;
    stream.disturbed = true;
    if (stream.state === 'closed') request.close();
    else if (stream.state === 'errored') request.error(stream.storedError);
    else pullSteps(stream.controller, request);
  }
  function readerRelease(reader) {
    var stream = reader.stream;
    if (stream === undefined) return;
    var released = new TypeErr('This readable stream reader has been released and cannot be used to monitor the stream\'s state');
    if (stream.state === 'readable') {
      reader.closed.reject(released);
    } else {
      reader.closed = deferred();
      reader.closed.reject(released);
    }
    handled(reader.closed.promise);
    stream.reader = undefined;
    reader.stream = undefined;
    errorRequests(reader, new TypeErr('This readable stream reader has been released and cannot be used to read from its previous owner stream'));
  }
  function readResult(reader) {
    var result = deferred();
    readerRead(reader, {
      chunk: function (value) { result.resolve({ value: value, done: false }); },
      close: function () { result.resolve({ value: undefined, done: true }); },
      error: function (error) { result.reject(error); }
    });
    return result.promise;
  }

  function ReadableStreamDefaultReader(stream) {
    if (new.target === undefined) {
      throw new TypeErr("Failed to construct 'ReadableStreamDefaultReader': Please use the 'new' operator");
    }
    if (arguments.length < 1) {
      throw new TypeErr("Failed to construct 'ReadableStreamDefaultReader': 1 argument required, but only 0 present.");
    }
    var record = streams.get(stream);
    if (record === undefined) {
      throw new TypeErr("Failed to construct 'ReadableStreamDefaultReader': parameter 1 is not of type 'ReadableStream'.");
    }
    acquireReader(record, this);
  }
  getter(ReadableStreamDefaultReader.prototype, 'closed', function () {
    var reader = readers.get(this);
    if (reader === undefined) return rejected(new TypeErr('Illegal invocation'));
    return reader.closed.promise;
  });
  method(ReadableStreamDefaultReader.prototype, 'cancel', promising(function cancel(reason) {
    var reader = slot(readers, this);
    if (reader.stream === undefined) {
      throw new TypeErr('This readable stream reader has been released and cannot be used to cancel its previous owner stream');
    }
    return streamCancel(reader.stream, reason);
  }));
  method(ReadableStreamDefaultReader.prototype, 'read', promising(function read() {
    var reader = slot(readers, this);
    if (reader.stream === undefined) {
      throw new TypeErr('This readable stream reader has been released and cannot be used to read from its previous owner stream');
    }
    return readResult(reader);
  }));
  method(ReadableStreamDefaultReader.prototype, 'releaseLock', function releaseLock() {
    readerRelease(slot(readers, this));
  });
  interfaceObject('ReadableStreamDefaultReader', ReadableStreamDefaultReader);

  // ---- tee() ---------------------------------------------------------------------

  function tee(stream) {
    var reader = acquireReader(stream);
    var reading = false, readAgain = false, canceled1 = false, canceled2 = false;
    var reason1, reason2, branch1, branch2;
    var cancelled = deferred();
    function pull() {
      if (reading) { readAgain = true; return resolved(undefined); }
      reading = true;
      readerRead(reader, {
        chunk: function (chunk) {
          // A microtask, so that an error thrown by one branch's enqueue
          // cannot stop the other's.
          resolved(undefined).then(function () {
            readAgain = false;
            if (!canceled1) controllerEnqueueSafely(branch1.controller, chunk);
            if (!canceled2) controllerEnqueueSafely(branch2.controller, chunk);
            reading = false;
            if (readAgain) pull();
          });
        },
        close: function () {
          reading = false;
          if (!canceled1) controllerClose(branch1.controller);
          if (!canceled2) controllerClose(branch2.controller);
          if (!canceled1 || !canceled2) cancelled.resolve(undefined);
        },
        error: function () { reading = false; }
      });
      return resolved(undefined);
    }
    function cancel1(reason) {
      canceled1 = true; reason1 = reason;
      if (canceled2) cancelled.resolve(streamCancel(stream, [reason1, reason2]));
      return cancelled.promise;
    }
    function cancel2(reason) {
      canceled2 = true; reason2 = reason;
      if (canceled1) cancelled.resolve(streamCancel(stream, [reason1, reason2]));
      return cancelled.promise;
    }
    branch1 = internalStream(pull, cancel1, 1);
    branch2 = internalStream(pull, cancel2, 1);
    reader.closed.promise.then(undefined, function (error) {
      controllerError(branch1.controller, error);
      controllerError(branch2.controller, error);
      if (!canceled1 || !canceled2) cancelled.resolve(undefined);
    });
    return [branch1.object, branch2.object];
  }
  function controllerEnqueueSafely(controller, chunk) {
    try { controllerEnqueue(controller, chunk); } catch (error) { /* the branch is errored */ }
  }

  // ---- Async iteration ---------------------------------------------------------------

  var AsyncIteratorPrototype =
    Object.getPrototypeOf(Object.getPrototypeOf(async function* () {}).prototype);
  var iterations = new Slots();
  var StreamIteratorPrototype = Object.create(AsyncIteratorPrototype);
  method(StreamIteratorPrototype, 'next', function next() {
    var iteration = iterations.get(this);
    if (iteration === undefined) return rejected(new TypeErr('Illegal invocation'));
    function step() {
      if (iteration.finished) return resolved({ value: undefined, done: true });
      return readResult(iteration.reader).then(function (result) {
        if (result.done) { iteration.finished = true; readerRelease(iteration.reader); }
        return result;
      }, function (error) {
        iteration.finished = true;
        readerRelease(iteration.reader);
        throw error;
      });
    }
    iteration.ongoing = iteration.ongoing === undefined ? step() : iteration.ongoing.then(step, step);
    return iteration.ongoing;
  });
  method(StreamIteratorPrototype, 'return', function (value) {
    var iteration = iterations.get(this);
    if (iteration === undefined) return rejected(new TypeErr('Illegal invocation'));
    function step() {
      if (iteration.finished) return resolved({ value: value, done: true });
      iteration.finished = true;
      var reader = iteration.reader;
      if (iteration.preventCancel) {
        readerRelease(reader);
        return resolved({ value: value, done: true });
      }
      var result = streamCancel(reader.stream, value);
      readerRelease(reader);
      return result.then(function () { return { value: value, done: true }; });
    }
    iteration.ongoing = iteration.ongoing === undefined ? step() : iteration.ongoing.then(step, step);
    return iteration.ongoing;
  });
  define(StreamIteratorPrototype, Symbol.toStringTag,
    { value: 'ReadableStream AsyncIterator', writable: false, enumerable: false, configurable: true });

  function values(options) {
    var stream = slot(streams, this);
    var given = dictionary(options, "Failed to execute 'values' on 'ReadableStream': The options");
    var iterator = Object.create(StreamIteratorPrototype);
    iterations.set(iterator, { reader: acquireReader(stream), finished: false,
      ongoing: undefined, preventCancel: Boolean(given.preventCancel) });
    return iterator;
  }

  // ---- ReadableStream's members ---------------------------------------------------------

  getter(ReadableStream.prototype, 'locked', function () {
    return slot(streams, this).reader !== undefined;
  });
  method(ReadableStream.prototype, 'cancel', promising(function cancel(reason) {
    var stream = slot(streams, this);
    if (stream.reader !== undefined) {
      throw new TypeErr("Failed to execute 'cancel' on 'ReadableStream': Cannot cancel a locked stream");
    }
    return streamCancel(stream, reason);
  }));
  method(ReadableStream.prototype, 'getReader', function getReader(options) {
    var stream = slot(streams, this);
    var given = dictionary(options, "Failed to execute 'getReader' on 'ReadableStream': The options");
    if (given.mode !== undefined) {
      if (String(given.mode) === 'byob') {
        throw new TypeErr("Failed to execute 'getReader' on 'ReadableStream': BYOB readers are not supported.");
      }
      throw new TypeErr("Failed to execute 'getReader' on 'ReadableStream': The provided value '" +
        String(given.mode) + "' is not a valid enum value of type ReadableStreamReaderMode.");
    }
    if (stream.reader !== undefined) {
      throw new TypeErr("Failed to execute 'getReader' on 'ReadableStream': ReadableStreamDefaultReader constructor can only accept readable streams that are not yet locked to a reader");
    }
    var object = Object.create(ReadableStreamDefaultReader.prototype);
    acquireReader(stream, object);
    return object;
  });
  method(ReadableStream.prototype, 'tee', function () {
    var stream = slot(streams, this);
    if (stream.reader !== undefined) {
      throw new TypeErr("Failed to execute 'tee' on 'ReadableStream': Cannot tee a stream that already has a reader");
    }
    return tee(stream);
  });
  method(ReadableStream.prototype, 'values', values);
  define(ReadableStream.prototype, Symbol.asyncIterator,
    { value: values, writable: true, enumerable: false, configurable: true });
  method(ReadableStream, 'from', function from(iterable) {
    if (iterable === null || iterable === undefined ||
        (typeof iterable !== 'object' && typeof iterable !== 'function' && typeof iterable !== 'string')) {
      throw new TypeErr("Failed to execute 'from' on 'ReadableStream': The provided value is not iterable.");
    }
    var asyncMethod = iterable[Symbol.asyncIterator];
    var sync = asyncMethod === undefined || asyncMethod === null;
    var make = sync ? iterable[Symbol.iterator] : asyncMethod;
    if (typeof make !== 'function') {
      throw new TypeErr("Failed to execute 'from' on 'ReadableStream': The provided value is not iterable.");
    }
    var iterator = make.call(iterable);
    if (iterator === null || (typeof iterator !== 'object' && typeof iterator !== 'function')) {
      throw new TypeErr("Failed to execute 'from' on 'ReadableStream': The iterator is not an object.");
    }
    var nextMethod = iterator.next;
    var stream = internalStream(function (controller) {
      return resolved(nextMethod.call(iterator)).then(function (result) {
        if (result === null || (typeof result !== 'object' && typeof result !== 'function')) {
          throw new TypeErr('The promise returned by the iterator.next() method must fulfill with an object');
        }
        if (result.done) { controllerClose(controller); return undefined; }
        // A synchronous iterator's values are awaited, as `for await` does.
        if (!sync) { controllerEnqueue(controller, result.value); return undefined; }
        return resolved(result.value).then(function (value) { controllerEnqueue(controller, value); });
      });
    }, function (reason) {
      var returnMethod = iterator.return;
      if (returnMethod === undefined || returnMethod === null) return undefined;
      return resolved(returnMethod.call(iterator, reason)).then(function (result) {
        if (result === null || (typeof result !== 'object' && typeof result !== 'function')) {
          throw new TypeErr('The promise returned by the iterator.return() method must fulfill with an object');
        }
      });
    }, 0);
    return stream.object;
  });
  interfaceObject('ReadableStream', ReadableStream);

  // ---- Bodies ------------------------------------------------------------------------------

  // The whole body in one chunk, read when the first chunk is asked for:
  // `read` returns a promise for a Uint8Array and uses the body up.
  function bodyStream(read) {
    return internalStream(function (controller) {
      return read().then(function (bytes) {
        if (bytes.byteLength > 0) controllerEnqueue(controller, bytes);
        controllerClose(controller);
      });
    }, function () { return undefined; }, 0).object;
  }

  // The receiver's own `bytes()` is looked up when the stream is made, so a
  // page that replaced the method before it first used a stream is asked.
  var ResponseCtor = globalThis.Response, BlobCtor = globalThis.Blob;
  var bodies = new Slots();
  exports.responseBody = function (response) {
    // Throws for a receiver that is not a Response.
    if (!responseHasBody(response)) return null;
    var stream = bodies.get(response);
    if (stream === undefined) {
      var bytes = ResponseCtor.prototype.bytes;
      stream = bodyStream(function () { return bytes.call(response); });
      bodies.set(response, stream);
    }
    return stream;
  };
  var blobSize = Object.getOwnPropertyDescriptor(BlobCtor.prototype, 'size').get;
  exports.blobStream = function (blob) {
    blobSize.call(blob);  // Throws for a receiver that is not a Blob.
    var bytes = BlobCtor.prototype.bytes;
    return bodyStream(function () { return bytes.call(blob); });
  };
  return exports;
})
"#;
