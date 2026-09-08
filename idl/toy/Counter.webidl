// Toy interface used as the small end-to-end fixture for `tenta-idl`.
// It is not part of any specification; it exists so that a golden test can
// cover a whole model (enum, dictionary, mixin, inheritance, constants,
// attributes, operations) in a file that fits on one screen.

enum CounterState {
  "idle",
  "running",
  "paused"
};

dictionary CounterOptions {
  required long initialValue;
  DOMString label;
  boolean autoStart = false;
};

interface mixin Stoppable {
  undefined stop();
  readonly attribute boolean stopped;
};

[Exposed=(Window,Worker)]
interface Counter {
  constructor(CounterOptions options);
  const unsigned long MAX_COUNT = 100000;
  const unsigned long MIN_COUNT = 0;
  attribute unsigned long count;
  readonly attribute boolean isActive;
  readonly attribute CounterState state;
  undefined reset();
  undefined advance(optional unsigned long by = 1);
};

Counter includes Stoppable;

[Exposed=Window]
interface TimedCounter : Counter {
  attribute unsigned long interval;
  undefined startTimer();
  DOMString describe();
};
