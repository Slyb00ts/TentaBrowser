# Fingerprint profiles

A profile is the data behind `tenta_net::ClientProfile`
(`docs/spec/03-network.md` §2): every observable wire property of a browser,
in TOML. Nothing in `tenta-net` knows a browser's name; changing a value here
changes the wire image without touching code.

Files are named `<browser>_<version>_<os>.toml`. The three shipped profiles
are compiled into the library (`tenta_net::profile::BUILTIN_PROFILES`), so
`ClientProfile::builtin("chrome_144_macos")` needs no filesystem access;
`ClientProfile::load(path)` reads any other file.

Chrome ships every four weeks and profiles follow (`docs/spec/21` §3):
`profiles/captures/<name>/` holds the raw capture a profile was derived from.

## Sections

### `[meta]`

| Key | Meaning |
|-----|---------|
| `name` | Must equal the file stem. |
| `browser`, `version`, `os` | Identity of the captured browser. |
| `captured` | ISO date of the capture the profile was derived from. |
| `chrome_build` | Full version string the captured browser reported. |
| `ja4` | JA4 of the ClientHello this profile is expected to produce. Empty means "not measured yet"; the conformance test then only checks internal consistency. |
| `ja4_r` | `JA4_r`: the same fingerprint with the cipher and extension lists unhashed, so a JA4 mismatch names the list that moved. |
| `akamai_h2` | Akamai HTTP/2 fingerprint string. It is derived from `[h2]` (`settings|window_update|priority_frames|pseudo_header_order`) and stored so that a typo in `[h2]` fails at load time. |

There is deliberately no `ja3`. Chrome permutes its extension order per
connection, so JA3 changes every handshake; a `ja3_canonical` key would need a
capture with permutation disabled, which Chrome 144 does not allow (below).

### `[tls]`

| Key | Meaning |
|-----|---------|
| `ciphers` | Cipher suites in the browser's order, as IANA names (`TLS_AES_128_GCM_SHA256`) or identifiers (`0x1301`). GREASE is not listed: BoringSSL inserts it. TLS 1.3 suites are listed for the fingerprint, but BoringSSL selects them itself and always in Chrome's order. |
| `extensions` | The extension identifiers the browser sends. When `permute_extensions` is set this is a *set*, written in the order of one captured hello; only membership is a profile property. |
| `permute_extensions` | Chrome ≥ 110 shuffles the extension order per connection (`SSL_CTX_set_permute_extensions`). |
| `groups` | Supported groups in order, as BoringSSL names (`X25519MLKEM768`, `X25519`, `P-256`, `P-384`). |
| `sigalgs` | Signature algorithms in order, as BoringSSL names. |
| `alpn` | ALPN protocol list in order. |
| `alps`, `alps_new_codepoint` | Protocols offered through ALPS (extension 17513) and whether the new codepoint 17613 is used. Chrome 144 uses the new codepoint. |
| `cert_compression` | `compress_certificate` algorithms (`brotli`, `zlib`, `zstd`). Only decompression is implemented. |
| `grease` | RFC 8701 GREASE values. |
| `ocsp_stapling`, `signed_cert_timestamps` | The `status_request` and `signed_certificate_timestamp` extensions. |
| `session_tickets` | TLS session tickets and resumption. |
| `ech` | The `encrypted_client_hello` extension (65037). Chrome sends a GREASE ECH on every TLS 1.3 connection, including to hosts with no HTTPS resource record. |

### `[h2]`

| Key | Meaning |
|-----|---------|
| `settings` | SETTINGS identifiers and values as `[[id, value], …]`, in the order they are sent. Identifiers `tenta-h2` has no field for (e.g. 9, `NO_RFC7540_PRIORITIES`) are sent verbatim. |
| `window_update` | Connection-level WINDOW_UPDATE increment sent right after the preface. |
| `pseudo_header_order` | `["method", "authority", "scheme", "path"]` for Chrome. |
| `[h2.headers_priority]` | `exclusive`, `dep`, `weight`: the RFC 7540 priority fields Chrome puts in every request HEADERS frame (`weight` is the wire value, one less than the RFC weight). Omit the table for HEADERS without the PRIORITY flag. |
| `never_index` | Header names the HPACK encoder must emit as never-indexed literals. |

### `[headers]`

`order` lists the request headers of a **navigation** in the order the browser
sends them, spelled with the casing it uses on HTTP/1.1 (`sec-ch-ua` is
lower-case, `Sec-Fetch-Site` is not); HTTP/2 lower-cases them.
`subresource_order` is the same list for everything a document loads, which
Chrome orders differently. Headers a request does not carry are skipped,
headers not in the list follow in the order the caller supplied them. The
remaining keys are the values the browser always sends.

| Key | Meaning |
|-----|---------|
| `order` | Navigation header order (`resource_type` = document). |
| `subresource_order` | Order for every other resource type. |
| `accept_encoding` | Sent to a potentially trustworthy origin. |
| `insecure_accept_encoding` | Sent to an origin that is not (below). |
| `sec_fetch_storage_access` | Value of `Sec-Fetch-Storage-Access` on cross-site sub-resources; omit the key to never send it. |
| `[headers.priority]` | RFC 9218 `Priority` per resource type, HTTP/2 only (below). |

#### Secure-origin-only headers

Chrome withholds a group of headers from an origin that is not *potentially
trustworthy* (W3C Secure Contexts §3.1): the `sec-ch-ua*` client hints, every
`Sec-Fetch-*` header, and `br`/`zstd` in `Accept-Encoding`. `headers.rs` is the
one place that knows the rule; the profile only supplies the values.

Potentially trustworthy means `https:`/`wss:`, or a loopback host: `127.0.0.0/8`,
`::1`, and the name `localhost` (or a `.localhost` subdomain). A plain-`http:`
request to a loopback address therefore *does* carry the hints, which is why
`profiles/captures/chrome_144_macos/http1_request.txt` had to be captured
against a non-loopback name (`fp-capture.test`) to show the withheld set.

#### `[headers.priority]`

Chrome 144 sends RFC 9218 `Priority` as an ordinary header on HTTP/2 and never
on HTTP/1.1. The value depends only on the resource type, and every value in
the table was read off the capture (`h2_requests.json`); the two types the
capture could not produce (`media`, `prefetch`) follow Chromium's urgency
mapping for their priority class and are marked in the file.

#### A `User-Agent` override and the client hints

A caller that overrides `User-Agent` must not keep the profile's `sec-ch-ua`,
or the two disagree and the disagreement is the fingerprint. `headers.rs`
implements one rule: if the override parses as a Chromium user agent
(`Chrome/<major>`), the three hints are rebuilt from it — the brand list keeps
the profile's shape with the new major version, `sec-ch-ua-platform` follows
the platform token in the override — and otherwise all three are dropped, the
way a non-Chromium browser sends none. The profile is not consulted for the
hints in either case.

## Chrome 144 (2026-09-06)

`profiles/captures/chrome_144_macos/` was recorded from Google Chrome
144.0.7559.60 on macOS 26.5 (Apple silicon) with

```bash
cargo run -p tenta-testutil --features capture --bin fp-capture -- \
    --runs 3 --headful --lang en-US --canonical
```

`docs/dev/fingerprint.md` explains each flag and how to re-run the capture on
Chrome's four-week cadence. Everything in the three profiles comes from those
bytes; `cargo nextest run -p tenta-testutil` re-derives the fingerprints from
them on every commit (`crates/tenta-testutil/tests/chrome_144_capture.rs`).

Measured values:

| | |
|---|---|
| JA4 | `t13d1516h2_8daaf6152771_d8a2da3f94cd`, identical across six runs |
| Akamai HTTP/2 | `1:65536;2:0;4:6291456;6:262144\|15663105\|0\|m,a,s,p` |
| JA3 | different on every run; not stored |

### Extension permutation

Chrome 144 cannot be told to stop permuting. The string `PermuteTLSExtensions`
no longer occurs anywhere in the shipped framework binary, and six runs — three
with `--disable-features=PermuteTLSExtensions` and three without — produced six
different extension orders. So:

- `[tls] extensions` is the extension **set**, listed in the order of one
  capture. Do not treat its order as meaningful.
- GREASE is pinned first and last in every capture; `permute_extensions = true`
  asks BoringSSL to reproduce that shape.
- The conformance test compares extension sets, and JA4 (which sorts) rather
  than JA3 (which does not).

### Linux and Windows

Chrome's TLS and HTTP/2 stacks are the same code on every desktop platform, so
`chrome_144_linux.toml` and `chrome_144_windows.toml` carry the macOS `[tls]`,
`[h2]`, `ja4` and `akamai_h2` verbatim and differ only in `user_agent` and
`sec_ch_ua_platform`. That is an **assumption, not a measurement**: ECH and the
post-quantum key share are field-trial controlled and could roll out at
different rates per platform. Re-capture on a Linux runner when CI can run a
browser there, and record the result here.

### What each header value came from

All of them were measured; nothing in `[headers]` is inferred.

- The navigation header order and casing come from an HTTPS connection where the
  server chose ALPN `http/1.1` (`https_http1_request.txt`). A plain-`http://`
  capture is kept too (`http1_request.txt`): on an insecure origin Chrome sends
  no client hints, no `Sec-Fetch-*` and only `gzip, deflate`, which is why it is
  not the source of the profile.
- `Referer` and `Cookie` come from the sub-resource request on the same
  connection (`https_http1_subresource.txt`); `Cookie` is the last header Chrome
  sends.
- `accept_language` is `en-US,en;q=0.9` because the capture passes
  `--lang en-US --accept-lang en-US`; without it Chrome reports the machine's
  locale. A context that sets a locale overrides the profile value.
- `user_agent` and `sec_ch_ua` need `--headful`: the headless build reports
  `HeadlessChrome/144.0.0.0` in `User-Agent`.
- HTTP/2 sends the same names lower-cased, drops `Host`/`Connection` and appends
  `priority`, whose value comes from `[headers.priority]`.
- The sub-resource order and `Sec-Fetch-Storage-Access` come from the HTTP/2
  capture (`h2_requests.json`): `/sub.css`, `/sub.js`, `/sub.png`, `/sub.woff2`
  and the cross-site image, which is the request that carries
  `sec-fetch-storage-access: none`.
- `[headers.priority]` was read off the same file, one request per resource
  type; the values were identical across runs.

### What is reproduced in code, not in the profile

- **HTTP/1.1 casing and order.** hyper writes request headers in `HeaderMap`
  order with a single casing rule, and its per-header casing extension is
  private, so `tenta-net`'s `h1_case` records the head hyper is about to write
  and substitutes the profile's own bytes when they match exactly. The
  conformance tests in `crates/tenta-net/tests/fingerprint.rs` compare the
  wire bytes with `https_http1_request.txt`, `https_http1_subresource.txt` and
  `http1_request.txt` field for field.
- **HPACK literals.** `[h2] huffman = "when_shorter"` selects Chromium's rule:
  a literal is left uncompressed unless the Huffman form is strictly shorter.
  Stock `h2` (and `huffman = "always"`) always compresses, which changes the
  bytes of every short value.
- **Session resumption.** A resumed ClientHello carries `pre_shared_key` last
  (RFC 8446 §4.2.11) and `psk_key_exchange_modes`, so it has one extension more
  than the full handshake: `t13d1517h2_…` against `t13d1516h2_…`. The cipher
  list, the GREASE positions and JA4_b are unchanged, and real Chrome behaves
  the same way; the JA4 of a resumed connection is not the profile's stored
  value and is not supposed to be.
