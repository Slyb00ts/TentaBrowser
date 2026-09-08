# Fingerprint conformance

The P1 exit gate of `docs/spec/03-network.md` §10: the JA4 of the ClientHello
**our** transport sends, and the Akamai fingerprint of the HTTP/2 preface it
sends, equal the values stored in the active `profiles/*.toml`.

This directory holds the conformance test that drives `tenta-net`. It does not
re-implement the fingerprint algorithms; those live in
`tenta_testutil::fingerprint` (`crates/tenta-testutil/src/fingerprint/`) and are
shared with the capture tool, so the browser and our client are measured by
exactly the same code:

| | |
|---|---|
| `parse_client_hello` | TLS record and handshake parsing, every extension a profile records. |
| `ja4`, `ja3` | FoxIO JA4 (with `JA4_r`) and Salesforce JA3. |
| `parse_h2_preface`, `akamai_h2` | HTTP/2 client preface, SETTINGS, WINDOW_UPDATE, PRIORITY and the first HEADERS frame, including a decode-only HPACK implementation. |

## What is already checked

Nothing here depends on `tenta-net` yet, so two suites in `tenta-testutil`
carry the load until the transport lands:

- `crates/tenta-testutil/tests/ja4_vectors.rs` — the algorithms against FoxIO's
  published expected outputs.
- `crates/tenta-testutil/tests/chrome_144_capture.rs` — the shipped profiles
  against the real Chrome 144 bytes in `profiles/captures/chrome_144_macos/`.

Both run with `cargo nextest run -p tenta-testutil`.

## Refreshing the reference capture

Chrome ships every four weeks and the profile follows
(`docs/spec/21-project-operations.md` §3). `fp-capture`
(`crates/tenta-testutil/src/bin/fp-capture.rs`) drives a real browser into a
loopback TLS listener and stores its ClientHello, HTTP/2 preface and HTTP/1.1
requests. `docs/dev/fingerprint.md` is the operator's guide, including why the
capture is taken headful, against a hostname, and with an explicit locale.

## Notes for a JA3 assertion

Chrome permutes its extension order per connection, so a JA3 assertion is only
meaningful in a deterministic test mode with permutation disabled
(`permute_extensions = false` on our side). Chrome 144 itself no longer offers
such a mode, so no browser-measured JA3 is stored to compare against; a JA3 test
here can only pin *our* output, not prove it matches Chrome.
