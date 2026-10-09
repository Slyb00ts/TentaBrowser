[Exposed=Window]
interface Crypto {
  [Throws, TentaBufferReferences] ArrayBufferView getRandomValues(ArrayBufferView array);
};
