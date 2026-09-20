// Toy buffer-source interface used as the fixture for `ArrayBuffer`, the typed
// arrays, `DataView` and the `ArrayBufferView`/`BufferSource` typedefs.
// It is not part of any specification; it exists so that the golden tests
// compile every buffer-source conversion against the pinned V8, in both
// directions. That is the boundary `TextDecoder.decode(optional BufferSource)`
// and `TextEncoder.encode()` need, and no other fixture has it.
//
// One attribute per spelling is what makes the coverage exhaustive: an
// attribute emits a getter and a setter, so each line below compiles the read
// conversion and the write conversion of its type. `decode` is the optional
// argument shape, which lowers to `Option<Vec<u8>>`.

[Exposed=(Window,Worker)]
interface ByteBox {
  attribute ArrayBuffer buffer;
  attribute DataView view;
  attribute Int8Array int8;
  attribute Int16Array int16;
  attribute Int32Array int32;
  attribute Uint8Array uint8;
  attribute Uint16Array uint16;
  attribute Uint32Array uint32;
  attribute Uint8ClampedArray clamped;
  attribute Float32Array float32;
  attribute Float64Array float64;
  attribute ArrayBufferView anyView;
  attribute BufferSource source;

  undefined decode(optional BufferSource data);
};
