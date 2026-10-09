// Internal toy only: verifies callback-scoped native buffer references without
// changing ByteBox's ordinary copied buffer conversions.
[Exposed=Window]
interface BufferReferenceProbe {
  [TentaBufferReferences] ArrayBufferView echoView(ArrayBufferView value);
  [TentaBufferReferences] BufferSource echoSource(BufferSource value);
  [TentaBufferReferences] undefined acceptSource(BufferSource value);
  [TentaBufferReferences] undefined acceptThenString(BufferSource value, DOMString label);
};
