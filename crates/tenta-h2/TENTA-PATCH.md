# tenta-h2: the fingerprint patch

Upstream: [`h2`](https://github.com/hyperium/h2) **v0.4.19** (MIT), vendored
from the crates.io release plus the repository's `tests/` tree (the published
crate does not ship them).

The fork exists for one reason (`docs/spec/03-network.md` §1): an
Akamai-style HTTP/2 fingerprint is computed from the client's SETTINGS
identifiers, their order and values, the connection-level `WINDOW_UPDATE`
that follows the preface, the presence of priority information and the
pseudo-header order. `h2` hard-codes all four. Everything below is opt-in
through `client::Builder::fingerprint(H2Fingerprint)`: without that call the
crate is byte-for-byte upstream on the wire, which is why the vendored test
suite still passes unchanged.

## Source diff against upstream `src/`

14 files, +261 / -19 lines.

| File | Change |
|------|--------|
| `src/fingerprint.rs` | **new**. `H2Fingerprint` (settings list, window update, pseudo order, HEADERS priority, never-index set), `PseudoId`, `HeadersPriority`, the `upstream()` and `chrome()` constructors, and the crate-internal `EncodeFingerprint` the HPACK encoder holds. |
| `src/lib.rs` | `pub mod fingerprint;`, re-exports, and one `allow` block for clippy lints newer than upstream's pinned clippy (the vendored code is not rewritten). |
| `src/client.rs` | `Builder::fingerprint()`; `handshake2` installs the fingerprint on the codec before the first frame, writes the SETTINGS built from the fingerprint's ordered list, then the connection-level `WINDOW_UPDATE`. |
| `src/frame/settings.rs` | `Settings::wire_order: Option<Vec<(u16, u32)>>` plus `Settings::from_wire_order`. When set, `encode` writes the identifiers verbatim in that order, including identifiers this implementation has no field for (e.g. `SETTINGS_NO_RFC7540_PRIORITIES` = 9). The typed fields are still filled for every known identifier, so local limits and flow control are unchanged. |
| `src/frame/headers.rs` | `Iter` carries the pseudo-header order and follows it (`:protocol`/`:status` keep their upstream position, so responses are untouched); `Headers::encode` writes the profile's priority fields into request HEADERS frames and sets the PRIORITY flag; `HeadersFlag::set_priority`. |
| `src/frame/priority.rs` | `StreamDependency::encode` (the five priority octets) and `#[derive(Copy, Clone)]` on it. |
| `src/hpack/encoder.rs` | The encoder holds the connection's `EncodeFingerprint` (`set_fingerprint`, `fingerprint`) and marks header names from the never-index set as never-indexed literals. |
| `src/codec/mod.rs`, `src/codec/framed_write.rs` | `set_fingerprint` plumbing from the codec to the HPACK encoder. |
| `src/proto/streams/recv.rs`, `src/proto/streams/streams.rs`, `src/proto/connection.rs` | `grant_connection_window(incr)`: moves both halves of the connection flow controller by the increment the client already wrote, so the local accounting matches what the peer was told and no second, automatic `WINDOW_UPDATE` is queued. Needed because `h2`'s automatic update is suppressed below a threshold, which would silently drop a small profile increment. |
| `src/proto/streams/recv.rs` (test) | `clear_recv_buffer_caps_capacity_before_overflow` now removes its stream from the `Store`. The fork's unit tests build with `unstable` (the vendored support crate needs it) and that feature arms `Store`'s drop assertion; upstream runs this test without the feature. Pre-existing upstream failure, reproduced on unmodified `h2` with `--features unstable`. |
| `src/hpack/test/` | `fixture.rs` dropped: it walks the 58 MB HPACK story corpus (`fixtures/`), which is not vendored. `fuzz.rs` (the encoder/decoder round-trip property test) is kept. |

## Wire image with `H2Fingerprint::chrome()`

```
PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n
SETTINGS        1=65536, 2=0, 4=6291456, 6=262144   (in that order)
WINDOW_UPDATE   stream 0, +15663105
HEADERS         flags END_STREAM|END_HEADERS|PRIORITY
                exclusive=1, dep=0, weight=255
                :method :authority :scheme :path
```

`SETTINGS_NO_RFC7540_PRIORITIES` (id 9) is *not* in the `chrome()` default:
profiles carry the settings list, and `profiles/chrome_144_*.toml` is where
that decision is recorded and can be changed without touching this crate.

## Test suite

`crates/tenta-h2/tests/` is upstream's `tests/h2-tests/tests/` with `h2::`
renamed to `tenta_h2::`; `tests/h2-support/` is upstream's support crate with
the same rename, with `atty` and `tracing-tree` replaced by
`tracing-subscriber`'s `fmt` layer (the `atty` advisory would fail
`cargo deny`), and with `#![allow]` headers for post-upstream clippy lints.
`tests/fingerprint_patch.rs` is new: it reads the raw bytes a client writes
and asserts the frame sequence, the SETTINGS payload, the `WINDOW_UPDATE`,
the priority octets, the pseudo-header order and the never-indexed literals.

## Upstream tracking

Rebase recipe: `cargo download h2==<new>` (or the GitHub tag tarball for the
tests), copy `src/` over, re-apply the table above (every hunk is marked with
a `TentaBrowser patch:` comment), then run `cargo nextest run -p tenta-h2`.
