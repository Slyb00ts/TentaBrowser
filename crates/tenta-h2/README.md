# tenta-h2

A fork of [`h2`](https://github.com/hyperium/h2) (v0.4.19, MIT) carrying
TentaBrowser's HTTP/2 client fingerprint patch. The engine drives HTTP/2
through this crate directly instead of through hyper, because Akamai-style
HTTP/2 fingerprints key on the exact SETTINGS identifiers, their order, the
initial WINDOW_UPDATE, the pseudo-header order and the priority fields of the
first HEADERS frame.

Everything the patch adds is opt-in through
`tenta_h2::client::Builder::fingerprint(H2Fingerprint)`. Without that call the
crate behaves exactly like upstream `h2`.

See `TENTA-PATCH.md` for the full list of changes against upstream.
