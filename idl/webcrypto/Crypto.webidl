[Exposed=Window]
interface Crypto {
  [Throws, TentaBufferReferences] ArrayBufferView getRandomValues(ArrayBufferView array);
  // A version 4 UUID from the same source as `getRandomValues`. The standard
  // limits it to secure contexts; this engine does not model them.
  [Throws] DOMString randomUUID();
};
