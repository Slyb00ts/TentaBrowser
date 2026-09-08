# Fingerprint capture and conformance

How to measure a browser's wire image, turn it into a `profiles/*.toml`, and
check that our transport reproduces it. The what and why are in
`docs/spec/03-network.md` §2; the profile file format is `profiles/README.md`.

## The pieces

| | |
|---|---|
| `tenta_testutil::fingerprint` | JA3, JA4 and Akamai HTTP/2 over captured bytes. Pure functions, no IO. |
| `fp-capture` | Binary that drives a real browser into a loopback listener and stores what it sent. |
| `profiles/captures/<name>/` | The raw bytes a profile was derived from, checked in. |
| `profiles/<name>.toml` | The profile. |
| `crates/tenta-testutil/tests/chrome_144_capture.rs` | Re-derives every fingerprint in the profiles from the stored bytes. |
| `tests/fingerprint/` | Runs the same algorithms over bytes from **our** transport. |

## Re-capturing (Chrome ships every four weeks)

`docs/spec/21-project-operations.md` §3 puts a profile update on Chrome's
release cadence. On a machine with the new Chrome installed:

```bash
cargo run -p tenta-testutil --features capture --bin fp-capture -- \
    --runs 3 --headful --lang en-US --canonical
```

That writes `profiles/captures/chrome_<version>_macos/` and prints the JA4,
JA4_r, JA3 and Akamai strings of each run. Every flag earns its place:

| Flag | Why |
|------|-----|
| `--runs 3` | JA4 and Akamai must be identical across runs; JA3 must not be, because Chrome permutes extensions. A JA4 that moves between runs means the algorithm or the parser is wrong, not Chrome. |
| `--headful` | The headless build reports `HeadlessChrome/<version>` in `User-Agent`. A profile named `chrome_<v>_<os>` must not. |
| `--lang en-US` | Without it Chrome sends the machine's locale in `Accept-Language`, which is not a property of the browser. |
| `--canonical` | Repeats the runs with `--disable-features=PermuteTLSExtensions` and reports whether that still produces a stable order. In Chrome 144 it does not; see below. |
| (implicit) `--host-resolver-rules=MAP fp-capture.test 127.0.0.1` | Points the browser at a *name*, so the ClientHello carries SNI. Against an IP literal JA4's third character is `i` and the extension count is one lower. Use `--ip` to measure that variant deliberately. |

What the tool does: it binds three loopback listeners (TLS preferring ALPN
`h2`, TLS forced to `http/1.1`, and plain HTTP), launches the browser at each,
reads the ClientHello with `MSG_PEEK` before BoringSSL touches the socket,
completes the handshake with a throw-away `rcgen` certificate, and records the
HTTP/2 preface up to the first HEADERS frame. The `http/1.1` listener sets a
cookie and serves a page with one sub-resource, so the follow-up request shows
where Chrome places `Referer` and `Cookie`.

Inspect an existing capture without touching the browser:

```bash
cargo run -p tenta-testutil --features capture --bin fp-capture -- \
    --dump profiles/captures/chrome_144_macos
```

Every line of that output maps to a key in the profile.

## Writing the profile

1. Copy the previous profile and replace `[meta]`, `[tls]`, `[h2]` and
   `[headers]` from the `--dump` output.
2. `ja4`, `ja4_r` and `akamai_h2` are copied verbatim from the run. Never type
   a fingerprint by hand: `chrome_144_capture.rs` recomputes them and will
   disagree.
3. Derive the Linux and Windows profiles by changing `user_agent` and
   `sec_ch_ua_platform` only, and say so in `profiles/README.md`.
4. `cargo nextest run -p tenta-testutil` must pass before the capture is
   committed. The `.bin` files go into the tree with the profile; they are
   about two kilobytes.

## Extension permutation

Chrome ≥ 110 shuffles its ClientHello extension order per connection, which is
why profiles store JA4 (which sorts the lists) and not JA3 (which does not).

In Chrome 144 the permutation cannot be turned off: the feature name
`PermuteTLSExtensions` does not appear anywhere in the shipped framework
binary, and `--disable-features=PermuteTLSExtensions` changes nothing. So the
profile has no `ja3_canonical`, `[tls] extensions` is a *set* rather than an
order, and `client_hello_canonical.bin` is absent. `--canonical` re-checks this
at each refresh: if a future Chrome regains the flag, `fp-capture` prints
`the flag works` and stores the canonical hello, and the profile can then gain
a JA3.

`permute_extensions = true` in the profile is what makes our own stack shuffle
too, via BoringSSL's `SSL_CTX_set_permute_extensions`. A client that sends a
*fixed* order is as distinctive as one that sends the wrong extensions.

## Conformance in CI

`crates/tenta-testutil/tests/` covers the algorithms and the stored capture:

- `ja4_vectors.rs` — JA4 against FoxIO's published expected values.
- `chrome_144_capture.rs` — `profiles/*.toml` against
  `profiles/captures/chrome_144_macos/`.

`tests/fingerprint/` closes the loop: it points `tenta-net` at a loopback
server, captures our own ClientHello and HTTP/2 preface, and asserts they
produce the profile's `ja4` and `akamai_h2`. That equality is the P1 exit gate
on macOS, Linux and Windows and a P8 gate on Android and iOS
(`docs/spec/03-network.md` §10).

Both run under `cargo nextest run`; neither needs a browser. Only re-capturing
does, which is why `fp-capture` is behind the `capture` feature — it is the only
thing in `tenta-testutil` that pulls in BoringSSL and `rcgen`. Lint it with:

```bash
cargo clippy -p tenta-testutil --all-targets --features capture -- -D warnings
```

## Reading a mismatch

A failing conformance test prints the two JA4 strings. Compare their `JA4_r`
forms, which are the same fingerprint unhashed:

```
t13d1516h2_<sorted ciphers>_<sorted extensions>_<signature algorithms>
```

- The `a` component moved: a cipher or extension count changed, SNI stopped
  being sent, or ALPN changed. `1516` is 15 ciphers and 16 extensions.
- `b` moved: the cipher list differs.
- `c` moved: the extension set or the signature algorithms differ. `44cd`
  (17613) versus `4469` (17513) is the ALPS code point; `fe0d` is ECH.
