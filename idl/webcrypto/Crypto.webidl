[Exposed=Window]
interface Crypto {
  [Throws, TentaBufferReferences] ArrayBufferView getRandomValues(ArrayBufferView array);
  // A version 4 UUID from the same source as `getRandomValues`. The standard
  // limits it to secure contexts; this engine does not model them.
  [Throws] DOMString randomUUID();
  // `readonly attribute SubtleCrypto subtle` is not declared here: on V8 the
  // host defines it, with `SubtleCrypto`, as a script built at first use
  // (`crates/tenta-js/src/subtle.rs`), because every operation of
  // `SubtleCrypto` returns a promise and has to reject where generated glue
  // throws.
};
