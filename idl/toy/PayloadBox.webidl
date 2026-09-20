// Toy union interface used as the fixture for a union arm that is a buffer
// source. It is not part of any specification; it exists so that the golden
// tests pin the discriminating test of that arm — the shape every body-bearing
// web API uses (`typedef (BufferSource or USVString) BodyInit;` and its
// relatives), where a wrong or missing test routes a typed array to the string
// arm and stringifies it instead of taking its bytes. The `sequence<octet>`
// return is the direction a body is read back in; no other fixture has a union
// at all.

typedef (BufferSource or USVString) PayloadSource;

[Exposed=(Window,Worker)]
interface PayloadBox {
  undefined send(PayloadSource data);
  sequence<octet> bytes();
};
