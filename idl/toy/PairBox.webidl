// Toy pair-iterable interface used as the fixture for `iterable<K, V>`.
// It is not part of any specification; it exists so that the golden tests
// cover a pair iterator on an interface with no array-like surface at all:
// no `length` attribute and no indexed getter. That is the shape `Headers`
// and `URLSearchParams` have, and it is the only shape that exercises the
// host hook rather than the `%Array.prototype%` intrinsics.

[Exposed=(Window,Worker)]
interface PairBox {
  iterable<DOMString, DOMString>;
};
