//! The performance timeline: `PerformanceEntry` and its kinds,
//! `PerformanceObserver`, and the members of `performance` that keep and
//! read entries (`mark`, `measure`, `getEntries*`, `clear*`).
//!
//! It is a script because the timeline is bookkeeping over a clock: the
//! entries are objects the page reads, an observer is a list and a callback,
//! and nothing in the engine looks at either. The host hands the script the
//! clock and the times of the document's lifecycle events, defines the names
//! when the engine is built, and runs the script when a page first uses one
//! (`tenta-js-v8`'s `host::performance_timeline`).
//!
//! What a page can observe: `mark`, `measure` and `navigation`. The
//! navigation entry has the document's URL and the times `DOMContentLoaded`
//! and `load` were fired; its network milestones are 0, because the script
//! world is not told them. There are no `resource`, `paint`,
//! `largest-contentful-paint`, `layout-shift`, `longtask` or `event`
//! entries, and `PerformanceObserver.supportedEntryTypes` says so, which is
//! what a measuring library reads before it asks.

/// Evaluates to a function of `(now, lifecycle)` that returns the exports:
/// the interface objects by name and, under `methods`, the functions of the
/// `performance` object. `now()` is the clock in milliseconds; `lifecycle()`
/// answers `[dclStart, dclEnd, loadStart, loadEnd]`, 0 for what has not
/// happened.
pub const PERFORMANCE_TIMELINE_PRELUDE_SOURCE: &str = r#"
(function (now, lifecycle) {
  'use strict';
  var define = Object.defineProperty, apply = Reflect.apply, TypeErr = TypeError;
  var create = Object.create, setProto = Object.setPrototypeOf, freeze = Object.freeze;
  var States = WeakMap, mapGet = States.prototype.get, mapSet = States.prototype.set;
  var tag = Symbol.toStringTag, toText = String, toNumber = Number, isArray = Array.isArray;
  var Exception = globalThis.DOMException, setTimer = globalThis.setTimeout;
  var clone = globalThis.structuredClone, report = globalThis.reportError;
  var documentObject = globalThis.document, NodeCtor = globalThis.Node;
  var urlOf = documentObject && Object.getOwnPropertyDescriptor(
    globalThis.Document ? globalThis.Document.prototype : {}, 'URL');
  var exports = {}, methods = {}, states = new States(), attributesOf = new States();
  var building = false;
  // The attributes of the legacy `performance.timing`, which a measure may
  // name instead of a mark and a mark may not be named after.
  var MILESTONES = ['navigationStart', 'unloadEventStart', 'unloadEventEnd', 'redirectStart',
    'redirectEnd', 'fetchStart', 'domainLookupStart', 'domainLookupEnd', 'connectStart',
    'connectEnd', 'secureConnectionStart', 'requestStart', 'responseStart', 'responseEnd',
    'domLoading', 'domInteractive', 'domContentLoadedEventStart', 'domContentLoadedEventEnd',
    'domComplete', 'loadEventStart', 'loadEventEnd'];
  var SUPPORTED = freeze(['mark', 'measure', 'navigation']);

  function stateOf(object) {
    var state = apply(mapGet, states, [object]);
    if (state === undefined) throw new TypeErr('Illegal invocation');
    if (state.refresh) state.refresh(state);
    return state;
  }
  function method(target, name, length, fn) {
    define(fn, 'name', { value: name, configurable: true });
    define(fn, 'length', { value: length, configurable: true });
    define(target, name, { value: fn, writable: true, enumerable: true, configurable: true });
    return fn;
  }
  function declare(name, Parent, attributes, Constructor) {
    var Interface = Constructor || ({ [name]: function () {
      if (!building) throw new TypeErr('Illegal constructor');
    } })[name];
    if (!Constructor && Parent) {
      Interface.prototype = create(Parent.prototype, {
        constructor: { value: Interface, writable: true, configurable: true }
      });
      setProto(Interface, Parent);
    }
    attributes.forEach(function (key) {
      var get = ({ ['get ' + key]: function () { return stateOf(this)[key]; } })['get ' + key];
      define(Interface.prototype, key, { get: get, enumerable: true, configurable: true });
    });
    define(Interface.prototype, tag, { value: name, configurable: true });
    var inherited = Parent ? apply(mapGet, attributesOf, [Parent]) : [];
    apply(mapSet, attributesOf, [Interface, inherited.concat(attributes)]);
    exports[name] = Interface;
    return Interface;
  }
  function make(Interface, state) {
    var entry = create(Interface.prototype);
    apply(mapSet, states, [entry, state]);
    return entry;
  }

  var Entry = declare('PerformanceEntry', null, ['name', 'entryType', 'startTime', 'duration']);
  method(Entry.prototype, 'toJSON', 0, function () {
    var state = stateOf(this), result = {};
    var names = apply(mapGet, attributesOf, [state.kind]);
    for (var index = 0; index < names.length; index++) result[names[index]] = state[names[index]];
    return result;
  });

  function detailOf(value) {
    if (value === undefined || value === null) return null;
    return typeof clone === 'function' ? clone(value) : value;
  }
  function syntaxError(message) {
    return typeof Exception === 'function' ? new Exception(message, 'SyntaxError')
      : new SyntaxError(message);
  }
  function markState(name, options) {
    var given = options === undefined || options === null ? {} : options;
    if (MILESTONES.indexOf(name) !== -1) {
      throw syntaxError("'" + name + "' is part of the PerformanceTiming interface, " +
        'and cannot be used as a mark name.');
    }
    var start = given.startTime === undefined ? now() : toNumber(given.startTime);
    if (!(start >= 0)) throw new TypeErr("'" + name + "' cannot have a negative start time.");
    return { kind: Mark, name: name, entryType: 'mark', startTime: start, duration: 0,
      detail: detailOf(given.detail) };
  }
  var Mark = declare('PerformanceMark', Entry, ['detail'],
    class PerformanceMark extends Entry {
      constructor(markName) {
        if (arguments.length < 1) {
          throw new TypeErr("Failed to construct 'PerformanceMark': " +
            '1 argument required, but only 0 present.');
        }
        var state = markState(toText(markName), arguments[1]);
        building = true;
        try { super(); } finally { building = false; }
        apply(mapSet, states, [this, state]);
      }
    });
  var Measure = declare('PerformanceMeasure', Entry, ['detail']);
  var Resource = declare('PerformanceResourceTiming', Entry, ['initiatorType', 'nextHopProtocol',
    'workerStart', 'redirectStart', 'redirectEnd', 'fetchStart', 'domainLookupStart',
    'domainLookupEnd', 'connectStart', 'connectEnd', 'secureConnectionStart', 'requestStart',
    'responseStart', 'responseEnd', 'transferSize', 'encodedBodySize', 'decodedBodySize',
    'responseStatus', 'renderBlockingStatus', 'deliveryType', 'serverTiming']);
  var Navigation = declare('PerformanceNavigationTiming', Resource, ['unloadEventStart',
    'unloadEventEnd', 'domInteractive', 'domContentLoadedEventStart', 'domContentLoadedEventEnd',
    'domComplete', 'loadEventStart', 'loadEventEnd', 'type', 'redirectCount', 'activationStart']);

  var marks = [], measures = [], observers = [], pending = false;
  var navigation = null;
  function navigationEntry() {
    if (navigation !== null) return navigation;
    var state = { kind: Navigation, entryType: 'navigation', startTime: 0, duration: 0,
      name: urlOf && urlOf.get ? apply(urlOf.get, documentObject, []) : '',
      initiatorType: 'navigation', nextHopProtocol: '', workerStart: 0, redirectStart: 0,
      redirectEnd: 0, fetchStart: 0, domainLookupStart: 0, domainLookupEnd: 0, connectStart: 0,
      connectEnd: 0, secureConnectionStart: 0, requestStart: 0, responseStart: 0,
      responseEnd: 0, transferSize: 0, encodedBodySize: 0, decodedBodySize: 0,
      responseStatus: 200, renderBlockingStatus: 'non-blocking', deliveryType: '',
      serverTiming: freeze([]), unloadEventStart: 0, unloadEventEnd: 0, type: 'navigate',
      redirectCount: 0, activationStart: 0,
      // The milestones move while the document loads, so they are read when
      // the entry is.
      refresh: function (state) {
        var times = lifecycle();
        state.domInteractive = times[0];
        state.domContentLoadedEventStart = times[0];
        state.domContentLoadedEventEnd = times[1];
        state.domComplete = times[2];
        state.loadEventStart = times[2];
        state.loadEventEnd = times[3];
        state.duration = times[3];
      } };
    navigation = make(Navigation, state);
    return navigation;
  }

  function startOf(entry) { return apply(mapGet, states, [entry]).startTime; }
  function select(type, name) {
    var found = [], lists = [];
    if (type === undefined || type === 'navigation') lists.push([navigationEntry()]);
    if (type === undefined || type === 'mark') lists.push(marks);
    if (type === undefined || type === 'measure') lists.push(measures);
    lists.forEach(function (list) {
      list.forEach(function (entry) {
        if (name === undefined || apply(mapGet, states, [entry]).name === name) found.push(entry);
      });
    });
    // Sorting is stable, so entries that began together keep their order.
    return found.sort(function (a, b) { return startOf(a) - startOf(b); });
  }

  function deliver() {
    pending = false;
    observers.slice().forEach(function (record) {
      if (record.queue.length === 0 || observers.indexOf(record) === -1) return;
      var entries = record.queue;
      record.queue = [];
      var list = make(EntryList, { entries: entries });
      var extra = record.called ? [list, record.observer]
        : [list, record.observer, { droppedEntriesCount: 0 }];
      record.called = true;
      try {
        apply(record.callback, record.observer, extra);
      } catch (error) {
        if (typeof report === 'function') report(error);
      }
    });
  }
  function queue(entry, type) {
    observers.forEach(function (record) {
      if (record.types.indexOf(type) !== -1) record.queue.push(entry);
    });
    if (!pending && observers.length !== 0 && typeof setTimer === 'function') {
      pending = true;
      setTimer(deliver, 0);
    }
  }

  // ---- performance.mark() and performance.measure() -----------------------
  function timeOf(mark, what) {
    if (typeof mark === 'number') {
      if (!(mark >= 0)) throw new TypeErr("'" + what + "' cannot be negative.");
      return mark;
    }
    var name = toText(mark);
    for (var index = marks.length - 1; index >= 0; index--) {
      var state = apply(mapGet, states, [marks[index]]);
      if (state.name === name) return state.startTime;
    }
    // A milestone of `performance.timing`: every one of them is the origin.
    if (MILESTONES.indexOf(name) !== -1) return 0;
    throw syntaxError("The mark '" + name + "' does not exist.");
  }
  methods.mark = function (markName) {
    if (arguments.length < 1) {
      throw new TypeErr("Failed to execute 'mark' on 'Performance': " +
        '1 argument required, but only 0 present.');
    }
    var entry = make(Mark, markState(toText(markName), arguments[1]));
    marks.push(entry);
    queue(entry, 'mark');
    return entry;
  };
  methods.measure = function (measureName) {
    if (arguments.length < 1) {
      throw new TypeErr("Failed to execute 'measure' on 'Performance': " +
        '1 argument required, but only 0 present.');
    }
    var name = toText(measureName), first = arguments[1], endMark = arguments[2];
    var options = first !== null && typeof first === 'object' ? first : null;
    var start, end, detail = null;
    if (options !== null) {
      if (endMark !== undefined) {
        throw new TypeErr("Failed to execute 'measure' on 'Performance': " +
          'an end mark cannot be given with an options object.');
      }
      if (options.start === undefined && options.end === undefined) {
        throw new TypeErr("Failed to execute 'measure' on 'Performance': " +
          'the options need a start or an end.');
      }
      if (options.start !== undefined && options.end !== undefined &&
          options.duration !== undefined) {
        throw new TypeErr("Failed to execute 'measure' on 'Performance': " +
          'start, end and duration cannot all be given.');
      }
      detail = detailOf(options.detail);
    }
    if (endMark !== undefined) end = timeOf(endMark, 'endMark');
    else if (options !== null && options.end !== undefined) end = timeOf(options.end, 'end');
    else if (options !== null && options.start !== undefined && options.duration !== undefined) {
      end = timeOf(options.start, 'start') + toNumber(options.duration);
    } else end = now();
    if (options !== null && options.start !== undefined) start = timeOf(options.start, 'start');
    else if (options !== null && options.duration !== undefined) {
      start = end - toNumber(options.duration);
    } else if (options === null && first !== undefined && first !== null) {
      start = timeOf(first, 'startMark');
    } else start = 0;
    var entry = make(Measure, { kind: Measure, name: name, entryType: 'measure',
      startTime: start, duration: end - start, detail: detail });
    measures.push(entry);
    queue(entry, 'measure');
    return entry;
  };
  methods.getEntries = function () { return select(undefined, undefined); };
  methods.getEntriesByType = function (type) {
    if (arguments.length < 1) {
      throw new TypeErr("Failed to execute 'getEntriesByType' on 'Performance': " +
        '1 argument required, but only 0 present.');
    }
    type = toText(type);
    return SUPPORTED.indexOf(type) === -1 ? [] : select(type, undefined);
  };
  methods.getEntriesByName = function (name) {
    if (arguments.length < 1) {
      throw new TypeErr("Failed to execute 'getEntriesByName' on 'Performance': " +
        '1 argument required, but only 0 present.');
    }
    var type = arguments[1];
    if (type !== undefined) {
      type = toText(type);
      if (SUPPORTED.indexOf(type) === -1) return [];
    }
    return select(type, toText(name));
  };
  function clear(list, name) {
    if (name === undefined) { list.length = 0; return; }
    name = toText(name);
    for (var index = list.length - 1; index >= 0; index--) {
      if (apply(mapGet, states, [list[index]]).name === name) list.splice(index, 1);
    }
  }
  methods.clearMarks = function () { clear(marks, arguments[0]); };
  methods.clearMeasures = function () { clear(measures, arguments[0]); };
  methods.clearResourceTimings = function () {};
  methods.setResourceTimingBufferSize = function () {};
  [['mark', 1], ['measure', 1], ['getEntries', 0], ['getEntriesByType', 1],
   ['getEntriesByName', 1], ['clearMarks', 0], ['clearMeasures', 0],
   ['clearResourceTimings', 0], ['setResourceTimingBufferSize', 1]].forEach(function (entry) {
    define(methods[entry[0]], 'name', { value: entry[0], configurable: true });
    define(methods[entry[0]], 'length', { value: entry[1], configurable: true });
  });

  // ---- PerformanceObserverEntryList and PerformanceObserver ---------------
  var EntryList = declare('PerformanceObserverEntryList', null, []);
  function filtered(list, type, name) {
    return stateOf(list).entries.filter(function (entry) {
      var state = apply(mapGet, states, [entry]);
      return (type === undefined || state.entryType === type) &&
        (name === undefined || state.name === name);
    });
  }
  method(EntryList.prototype, 'getEntries', 0, function () {
    return filtered(this, undefined, undefined);
  });
  method(EntryList.prototype, 'getEntriesByType', 1, function (type) {
    return filtered(this, toText(type), undefined);
  });
  method(EntryList.prototype, 'getEntriesByName', 1, function (name) {
    var type = arguments[1];
    return filtered(this, type === undefined ? undefined : toText(type), toText(name));
  });

  var records = new States();
  function recordOf(observer) {
    var record = apply(mapGet, records, [observer]);
    if (record === undefined) throw new TypeErr('Illegal invocation');
    return record;
  }
  var Observer = class PerformanceObserver {
    constructor(callback) {
      if (typeof callback !== 'function') {
        throw new TypeErr("Failed to construct 'PerformanceObserver': " +
          'parameter 1 is not of type \'Function\'.');
      }
      apply(mapSet, records, [this, { observer: this, callback: callback, types: [],
        queue: [], mode: '', called: false }]);
    }
  };
  method(Observer.prototype, 'observe', 0, function () {
    var record = recordOf(this), options = arguments[0];
    var given = options === undefined || options === null ? {} : options;
    var entryTypes = given.entryTypes, type = given.type;
    if (entryTypes === undefined && type === undefined) {
      throw new TypeErr("Failed to execute 'observe' on 'PerformanceObserver': " +
        'An observe() call must include either entryTypes or type arguments.');
    }
    if (entryTypes !== undefined && (type !== undefined || given.buffered !== undefined)) {
      throw new TypeErr("Failed to execute 'observe' on 'PerformanceObserver': " +
        'An observe() call must not include both entryTypes and type arguments.');
    }
    var mode = entryTypes !== undefined ? 'multiple' : 'single';
    if (record.mode !== '' && record.mode !== mode) {
      var message = 'This PerformanceObserver has performed observe({' +
        (record.mode === 'multiple' ? 'entryTypes' : 'type') + ': ...}), so it cannot also use ' +
        (mode === 'multiple' ? 'entryTypes' : 'type') + '.';
      throw typeof Exception === 'function'
        ? new Exception(message, 'InvalidModificationError') : new TypeErr(message);
    }
    var wanted = [];
    if (mode === 'multiple') {
      if (!isArray(entryTypes)) entryTypes = Array.from(entryTypes);
      entryTypes.forEach(function (name) {
        name = toText(name);
        if (SUPPORTED.indexOf(name) !== -1 && wanted.indexOf(name) === -1) wanted.push(name);
      });
    } else {
      type = toText(type);
      if (SUPPORTED.indexOf(type) !== -1) wanted.push(type);
    }
    // Types this engine does not record are ignored, and with none left the
    // call registers nothing.
    if (wanted.length === 0) return;
    record.mode = mode;
    if (mode === 'multiple') record.types = wanted;
    else if (record.types.indexOf(wanted[0]) === -1) record.types.push(wanted[0]);
    if (observers.indexOf(record) === -1) observers.push(record);
    if (mode === 'single' && given.buffered) {
      select(wanted[0], undefined).forEach(function (entry) { record.queue.push(entry); });
      if (record.queue.length !== 0 && !pending && typeof setTimer === 'function') {
        pending = true;
        setTimer(deliver, 0);
      }
    }
  });
  method(Observer.prototype, 'disconnect', 0, function () {
    var record = recordOf(this), index = observers.indexOf(record);
    if (index !== -1) observers.splice(index, 1);
    record.queue = [];
    record.types = [];
    record.mode = '';
  });
  method(Observer.prototype, 'takeRecords', 0, function () {
    var record = recordOf(this), taken = record.queue;
    record.queue = [];
    return taken;
  });
  define(Observer, 'supportedEntryTypes', {
    get: function () { return SUPPORTED; }, enumerable: true, configurable: true
  });
  define(Observer.prototype, tag, { value: 'PerformanceObserver', configurable: true });
  exports.PerformanceObserver = Observer;
  exports.methods = methods;
  return exports;
})
"#;
