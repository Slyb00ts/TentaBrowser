# Implementation status and completion ledger

Updated: 2026-11-16. This is an execution ledger, not a replacement for
`docs/spec/16-roadmap.md` or its exit gates.

The owner has requested continuous implementation through the whole P0–P8
roadmap. Work advances through coherent packages and independent reviews;
completion of a package does not imply completion of a phase. Later phases
are authorized, with prerequisite gates still tracked explicitly.

## Phase evidence

| Phase | Current evidence | Remaining completion evidence |
| --- | --- | --- |
| P0 | Foundation and recorded local exit in the roadmap | Revalidate affected foundation gates as dependencies and platforms change |
| P1 | Network, HTML, arena DOM, extraction, fast search and CLI implemented | Full current platform, fingerprint, fuzz and performance exit-gate revalidation |
| P2 | V8 and QuickJS page execution, generated DOM bindings, event loop, watchdogs, fetch/XHR/forms/Blob/File/XML subsets, live Range work, Stylo inline styles and native CSSOM model, atomic CLI evaluation | QuickJS parity, full CSSOM, workers, storage, streams/WebSocket, Web Crypto, IntersectionObserver and the other observers, navigation/history/frames/CSP, XML, persistent evaluation/remote objects, Full search, all roadmap conformance and budget gates |
| P3 | Crate skeletons and reference render corpus | Stylo, layout, geometry, paint, image/font/canvas/screenshot/PDF implementations and rendering gates |
| P4 | Host crate skeletons and protocol inventory/checklists | CDP/MCP/FFI, workers/storage extensions, editing, downloads/dialogs and client/device conformance |
| P5 | Audio crate skeleton | Audio graph, decoders/sinks/worklets/capture and five-platform gates |
| P6 | Media crate skeleton | Software/hardware video, MSE, scheduling/capture and ladder/device gates |
| P7 | WebRTC crate skeleton | str0m integration, TURN, codecs/audio processing/data channels and interoperability gates |
| P8 | Platform policy and hardening specifications | Mobile releases, isolation, size/security/conformance/golden-site gates and 1.0 API freeze |

## Current packages

| Package | State | Ownership and acceptance evidence |
| --- | --- | --- |
| Generated V8 panic containment | Implemented and reviewed | Sticky isolate failure, payload cleanup, 20 generated callback subprocess cases, real engine/page/async-stream failure tests |
| Atomic `fetch --eval` | Implemented and reviewed | Optional CLI `v8` feature, shared navigation deadline, JSON/string output, explicit Promise refusal; the engine is now retained by the page, but the public `Page::evaluate` remains outstanding |
| Persistent page engine (PE1) | Implemented; workspace gates recorded below | A page keeps its V8 or QuickJS engine, document, timers and pending completions after `goto` settles; `Page::with_engine` runs a closure in it on the page thread, `Page::wait_for_settle` starts a new settle call over the work begun after `load`, and a new navigation, `close`, `take_document` or a crash drops it. Evaluation, handles, Promise awaiting and console/exception events are PE2 (next row). Idle retained engine (release, 16 and 64 pages on one page thread, `examples/idle_page_rss.rs`): V8 +1.57 MiB per page, QuickJS +0.31 MiB per page, DOM-only +0.04 MiB per page; `Page::take_document` is now `async` (it ends a kept engine first; callers add `.await`) and `Page::document()` is empty while an engine is kept, so JS pages read their tree through `with_engine` or `Page::title()`; script CPU is a per-30 s-window rate (D-033); against the `14-memory.md` §5 floors of ~4 MB (V8) and ~300 KB (QuickJS) |
| Remote evaluation (PE2) | Implemented on both backends; focused and workspace gates recorded | `Page::evaluate`/`evaluate_with`/`call_function_on`, remote objects with previews and object groups, awaited Promises, `PageEvent::{Console, Exception, BindingCalled}`, `add_binding`. One realm script (`tenta-js/src/inspect/inspector.js`) serves both engines; V8 and QuickJS add only the install, the dispatcher call and uncaught-exception/unhandled-rejection reporting. Isolated worlds are refused with `Error::Unsupported` (one world per realm). 14 page-level tests per backend (`tenta-browser/tests/evaluate.rs`) plus in-crate handle-limit and virtual-clock tests; the realm script adds about 0.3 ms to V8 engine creation (debug build, 1.7 ms without) |
| Real `S-dom-calls` driver | Implemented and reviewed | `bench/dom_calls.py`, preserved before/after binaries, 25 interleaved runs with hashes and workload provenance |
| Real `S-storage-calls` driver | Implemented, Sol/root reviewed and measured | Shared Rust/JavaScript workload and thin backend adapters verify changing writes and final values; 25 alternating V8/QuickJS pairs recorded with p50/p95, CPU and RSS; this is an initial backend comparison, not a before/after optimization claim |
| Handwritten V8 native boundaries | Implemented and reviewed; integration checks pending | Function callbacks, promise settlement, GC/heap hooks and weak finalizers guarded; backend suite: 746 passed, 2 skipped; independent Sol review found no blocking issue |
| QuickJS runtime and DOM backend | Implemented subset and reviewed; aggregate checks pending | 29 current runtime/native-finalizer tests and browser QuickJS navigation/panic tests passed; independent Sol review accepted panic containment fixes; full workspace remains pending |
| P3 inline completion (L3b) | Implemented; Chrome 154 recorded cases pass | Atomic inlines, `vertical-align`, bidirectional lines (levels recovered from the shaper's visual order), `text-overflow: ellipsis`; about 240 Chrome-recorded cases in `crates/tenta-layout/tests/inline_complete`. Gaps: `unicode-bidi` isolate/embed/override and `dir`/`<bdi>`/`<bdo>` on inline elements, bidi context across atomic inlines, tabs and forced breaks (each chunk starts at sos), explicit embeddings above level 2, start-side ellipsis, inside shape list markers sizing |
| Remaining package inventory | Inspected and reviewed | Luna mapped missing P2 APIs and P3–P8 dependencies; see the implementation queue below |
| Stylo CSSOM model and inline bridge | Implemented subset and reviewed | 14 native model/live-rule/mutation tests, native CSS Clippy and 14 earlier V8 style tests passed; stable native rule handles survive index changes/deletion; declaration/selector mutations update the actual Stylo rule, with namespace-aware parsing; default JS build excludes Stylo; handwritten inline parser replaced with Stylo; aggregate checks pending |
| Scheduled cross-compilation | Implemented and reviewed | CI accepts the existing Nightly workflow's reusable-workflow call and explicitly tests both-engine browser/CLI/IDL assembly; YAML and source review passed; hosted execution and device tests remain separate |
| Native V8 CI provisioning | Incomplete; fail-closed prerequisites identified | All desktop workspace checks require V8. macOS bundle provisioning, Linux/Windows reviewed artifacts and macOS 14 runtime validation are missing. CI Cargo validation/mobile builds now use `--locked` and successful desktop runs check Cargo.lock remains unchanged; independent Sol/root source review passed, hosted execution remains unperformed |
| In-process `S-js-xhr` probe | Implemented and reviewed | Release example: 25 verified samples, navigation p50/p95 2.96/3.10 ms; complete gap and memory-delta gates remain open |
| V8 paste maintenance advisory | Scoped exception reviewed; deny passed | Only RUSTSEC-2024-0436 accepted with review notes and stale-exception denial; advisory, ban, license and source checks passed; see dependency-audits.md |
| DOMParser and XMLSerializer bindings | Implemented subset and reviewed | 5 V8 integration tests and 3 XML round-trip tests passed; Attr-as-Node/serialization now has separate focused coverage; XHR responseXML remains open |
| Dual-engine WPT transport | Implemented, reviewed and focused validation passed | Explicit backend in plans/results; separate QuickJS expectations; misfiled reports cannot generate or promote expectations; 11 Python tests, 28 combined-backend browser/WPT/QuickJS panic tests, and 26 QuickJS-only WPT tests passed; no conformance percentage claimed |
| Windows FsPolicy hard-link protection | Implemented, validated and Sol-reviewed | 17 focused tests and core Clippy passed on macOS, including validation-before-truncation and hard-link refusal for append; GNU/MSVC Windows target checks passed again after the opened-handle and append hardening; opened-handle hard-link refusal preserved; Windows runtime execution remains CI evidence |
| Detached listener ownership cycles | Bounded collector implemented and Sol/root source-reviewed; focused checks passed, aggregate pending | Private ephemeron graphs on both engines collect plain-node self/cross cycles after native pins are released; 96 focused backend tests passed, including 14 listener controls per backend, post-GC recovery failure and handler metadata cleanup; JS-mediated Attr/Range/static-snapshot pin cycles remain separate |
| Static snapshot node lifetime | Implemented and source-reviewed; four backend regressions passed | SnapshotSlab owns counted node pins with rollback and release on reclamation; shared NodeList/HTMLCollection retention-and-release regressions passed on both engines; native rollback test passed; no executed pre-fix failure claimed |
| Attr-as-Node identity and lifetime | Implemented subset; aggregate checks pending | Corrected native mutation: 16 passed; 20 earlier dual-engine identity/prefix/case/GC tests and deferred-reclamation guard test passed; queued-finalizer reacquisition RAII fix independently reviewed; current indexed/named collection checks have separate evidence below; live Range Attr endpoints and cross-document adoption remain open |
| Runtime initialization failure propagation | Implemented and reviewed | Navigation reports configured backend initialization errors; static pages avoid engine creation; both regressions passed in the 128-test page/CSS run |
| HTTP/2 retry and fixture driving | Implemented, Sol/root reviewed and package-validated | All 350 network tests passed after the final pooled-ready correction (8 skipped), and net Clippy passed; remote REFUSED_STREAM can replay a buffered POST once, ambiguous transport closure cannot replay a processed POST; typed ready/GOAWAY metadata and actual pool-eviction decision have deterministic regression coverage |
| Native QuickJS property hooks | Fork and focused integration/sanitizer checks passed | Latest core run: 161 tests; latest C/Rust ASAN run: 11 native exotic probes plus 9 actual LegacyHost tests and one finalizer subprocess, all passed; compiled C object instrumentation was verified; leak detection was disabled, so no leak claim; restoring the earlier borrowed-value bug reproduced SIGSEGV in isolation; general GC trace/finalizer panic containment remains open |
| Web Storage | Context/page stores and bindings implemented; integration under validation | Local/session stores enforce origin and lifetime boundaries, stage state imports and check quotas; current QuickJS storage probes passed, but V8 named-property integration, StorageEvent delivery and the complete storage budget/conformance gates remain open |
| Namespace local-name conversion | Corrected, Sol/root reviewed and tested on both engines | Eight Element/Document/NamedNodeMap signatures now use ordinary DOMString local-name conversion while preserving nullable namespaces; three shared regressions passed on each engine for null/undefined, canonical Attr identity/removal and namespaced tag queries |
| Qualified attribute names and case | Corrected, Sol/root reviewed, tested and measured | Element and NamedNodeMap share first-qualified-name lookup; HTML-only case folding and namespace/prefix identity are preserved; 38 shared Attr/named collection checks and both-backend Clippy passed after borrowed-name optimization; isolated 25-pair DOM measurements are recorded below |
| Element qualified-name case | Corrected, Sol/root reviewed and tested on both engines | tagName and nodeName now uppercase HTML-namespace names only in HTML documents; identical shared XML/XHTML regressions failed on both engines before the one-condition correction and passed afterward, with HTML, SVG, null-namespace and non-ASCII controls |
| V8 abrupt argument conversions | Implemented, Sol/root reviewed, focused checks passed and release checkpoint measured | Tests-only baseline passed 22/34; the final implementation passed 34/34, preserving exact page-thrown values and preventing host mutations; fetch argument errors reject a returned Promise without driver requests; redundant string helpers removed, goldens 20/20 and dual-engine IDL 67/67 passed; cumulative release comparison recorded below |
| Incoming interface argument lifetime | Scoped roots implemented, Sol/root reviewed, focused checks passed and measured | All eight pre-host probes pass across both engines; QuickJS focused tests passed 99/99, IDL passed 67/67 and targeted all-targets Clippy passed; the complete ownership probe run improved from 9/34 to 14/34, leaving ten general wrapper-graph failures per engine; isolated 25-pair DOM measurements are recorded below |
| QuickJS node-wrapper ownership graph | Implemented and Sol/root source-reviewed; focused/broader checks, release measurements and scoped ASAN passed | All 17 wrapper-state assertions and six pressure controls passed in the 79/79 focused run. The broader QJS run passed 127/127 while explicitly excluding the separate CSS host-wrapper probes. Browser native-failure isolation and real QuickJS page fixtures passed 7/7; IDL goldens passed 20/20, dual-engine IDL 67/67, and scoped QJS/IDL/DOM all-targets Clippy passed. C+Rust ASAN passed 59/59 after external-document-borrow parity, with leak detection disabled. Paired DOM/listener measurements and 25 pressure runs per shape are recorded below. V8 validation and the subsequent CSS/Event extensions are tracked separately below |
| Materialized Event target ownership and propagation | Implemented subset, Sol/root reviewed and tested | Directed target edges preserve node wrapper state across GC and foreign-document dispatch; node/Document paths use actual parents. Combined Event/owner/path checks passed 289/289 and QJS C+Rust ASAN 11/11. Pre-dispatch stop flags subsequently reproduced four failures and passed the 281-test corrected dispatch run; linked host targets and detail cycles remain open. Standalone targets have the separate checkpoint below |
| Standalone EventTarget and passive defaults | Implemented; focused checks pass, broader validation pending | Both engines construct standalone targets and preserve/reclaim listener and Event.target cycles. Default passive values are chosen at registration; Document.body now shares the correct namespace-aware identity. Focused native/shared checks pass 174/174. Sanitizer, release WPT and performance results for this checkpoint are pending |
| Constructed CSSOM wrapper ownership | Directed graph implemented, Sol/root reviewed, tested and measured | Current sheets, detached subtrees and empty lists preserve getter-reachable state without retaining unrelated removed rules. Native CSS 25/25, shared CSSOM 60/60, broader graph/Event/Attr 129/129 and QJS C+Rust ASAN 78/78 passed. Cumulative Event+CSS comparisons and explicit mutation/trim costs are recorded below |
| Fresh event WPT diagnostics | QuickJS bootstrap fixed; directory gates still fail | Original V8 directory run: 202 files, 234/486 recorded subtests passed. Fresh QuickJS directory run: 202 files, 227/449 recorded subtests passed, 80 OK harnesses, no unstarted files. After receiver/dispatch corrections, seven pinned files pass 39/39 subtests on V8 and 37/39 on QuickJS, with all harnesses OK. The remaining two focused QuickJS failures are Event calls without new. None of these results is a P2 conformance pass |
| Active parser node ownership | Scoped native pins implemented and Sol/root source-reviewed; native checks and paired measurement complete | Tests-only baseline passed 6/10 with all four new ownership regressions failing; the pin implementation passed 31/31 SharedSink, lifetime and parse integration tests. Published parser handles survive unrelated detached-node sweeps, normal finish/drop releases pins, and unwind preserves the original failure. The measured 12.3–18.2% parse overhead is recorded below. Paused-parser pressure controls passed on both engines; scoped QJS/IDL/DOM and V8/IDL all-targets Clippy passed; aggregate checks remain separate |
| Dependency alias enforcement | Implemented, Sol/root reviewed and focused checks passed | Five behavioral tests, xtask Clippy and actual 26-crate lint passed; explicit/inherited aliases and target tables are checked; native fork consumers and optional shared glue are restricted; deny advisories/bans/licenses/sources and IDL manifest checks passed on the renamed dependency graph |
| Workstation setup and Linux x86_64 V8 bundle | Implemented; executed in containers on 2026-11-16, macOS route not executed | `setup.sh` installs system packages, the pinned toolchain, cargo-nextest/cargo-deny and the V8 bundle. From bare images it finished with exit 0 on Ubuntu 24.04 and Arch including the V8 source build (30/30 native tests each), on Fedora 43 with `--all --no-v8`, and on Debian 12 before the V8 stage existed (its Python 3.11 is below the recipe's 3.12, so V8 is refused there); the default CLI built and fetched a page on Ubuntu, Debian and Arch. The Fedora V8 build ran in a container whose packages were installed by hand first; a later run of `./setup.sh` from a bare Fedora 43 image against that build tree passed 30/30 and wrote the local pin. `bootstrap_source.py` verifies the 46 supplemental inputs by Git tree id because Gitiles archives are not byte-stable (8 unit tests); `install_bundle.py` installs the bundle and writes a local pin in the cache (9 unit tests); `artifact_pins.rs` keeps reviewed bundles only, and the guard reads a local pin when `TENTA_V8_LOCAL_PIN` names it (2 guard tests; a build without the variable, and pins with another archive hash, another source recipe, an extra field or no file, were each refused). With the Fedora local pin the `tenta-v8` binding-package route passed 60/60, its Clippy passed, and `tentabrowser-cli --features full` built and ran `fetch --eval`; with the earlier tracked workstation pin of the same bundle, `tentabrowser-cli --features full` built, `fetch --eval` and `screenshot` ran, and `cargo nextest run --locked --workspace` ran 4654 tests: 4625 passed, 27 failed, 2 timed out, 70 skipped, while two other V8 builds shared the machine. 26 failures are `tenta-cdp::interaction`, which has no `required-features` and needs `v8,render` (`-p tenta-cdp --features v8,render`: 174/174); the two timeouts passed alone in 108 s and 223 s; `tenta-net::http a_stale_while_revalidate_entry_answers_at_once_and_refreshes_behind_it` fails alone as well (`swr-1` where `swr-2` is expected) and is open. The bundle is a workstation build without reviewed toolchain provenance; archives are not reproducible across machines, so each machine pins its own; workspace Clippy, deny and benchmarks were not run on Linux |
| Linux x86_64 host gates | Executed on Fedora 43 on 2026-11-16 with the local V8 bundle; all pass | First run: 4629 of 4656 tests passed. 26 failures were `tenta-cdp::interaction` built without its features, now gated by `required-features = ["v8", "render"]` like `page` and `network`; one was `tenta-net::http a_stale_while_revalidate_entry_answers_at_once_and_refreshes_behind_it`, whose wait (`cache.len() == 1`) was already true before the refresh stored its body, so the third request raced the store; it now waits for the stored body (30/30 under `--stress-count 30`). `cargo deny check` failed on six advisories (tokio RUSTSEC-2026-0386, webpki-root-certs RUSTSEC-2026-0383, zstd/zstd-safe/zstd-sys RUSTSEC-2026-0391/0398/0403/0410); the lockfile moves those five packages only (tokio 1.53.3, webpki-root-certs 1.0.12, zstd 0.14.4, zstd-safe 8.3.0, zstd-sys 2.1.3) and the Temporal lock validator still verifies 7 packages. After the changes: `cargo nextest run --locked --workspace` 4654/4654 (70 skipped), the feature-gated run of `tenta-browser`, `tentabrowser-cli`, `tenta-idl` and `tenta-cdp` with V8, QuickJS and render 550/550, workspace and feature Clippy, fmt, `lint-deps`, `idl-manifest` and `deny` pass. `tests/linktest/Cargo.lock` still has tokio 1.53.1; benchmarks and WPT directories were not run on Linux |
| `window.location` read side, `atob`, `btoa` | Implemented on both engines; focused, page and workspace checks passed; not independently reviewed | See the section below. Navigation through `location`, `document.location` and `History` remain open |
| MCP reading, history, log and search tools (MC3a) | Implemented; focused checks passed; not independently reviewed; built without `docs/spec/10-mcp.md` at hand | `browser_extract`, `browser_back`/`forward`/`reload`, `browser_console_messages`, `browser_network_requests`, `browser_web_search`; see the section below |
| `window.history` (HS1) | Implemented on V8 only; focused checks passed; not independently reviewed | Same-document session history; see the section below. QuickJS has no `history` |
| Event handler attributes (EH1) | IDL attributes on both engines, content attributes and window handlers on V8; focused checks passed; not independently reviewed | `element.onclick = fn`, `<button onclick="...">`, `window.on*`, `<body onload>`; see the section below |
| Navigation started by the page (NAV1) | Implemented on V8; focused, page and host checks passed; not independently reviewed | `location` write side, POST navigations, form submission, link activation, fragment navigation; see the section below |
| Form control interfaces (FM1) | Implemented on both engines (generated); focused checks passed; not independently reviewed | `<select>`/`<option>`, text selection, `form.submit()`/`requestSubmit()`/`reset()`, reflected attributes, `innerText`, `hidden`; see the section below |
| Interface objects installed after context creation (RC1) | Implemented on V8; measured; not independently reviewed | Removes the realm-creation cost the packages above added; see the section below |
| Window and tree members pages expect (WIN1) | `matchMedia`, window members and `history` on V8; the tree and form members on both engines (generated); focused checks passed; not independently reviewed | `matchMedia`, `dataset`, `currentScript`, element siblings, node comparison, document collections, dialogs, `screen`, `Window`, user timing; see the section below |
| Cross-origin requests, request modes and provenance of continuations (CO1) | CORS in the page driver for `fetch()` and XHR on both engines; `signal` and the provenance fix on V8; focused, page and workspace checks passed; measured; not independently reviewed; changes a documented refusal, see the section below | CORS check, preflight, credentials modes, `mode`/`redirect`/`cache`/`signal`, `withCredentials`; a request made after an `await` now holds the settle |
| Module scripts and `import()` (MOD1) | Implemented on V8; focused, page and workspace checks passed; measured; not independently reviewed | `<script type="module">`, static imports, import maps, `import()`, `import.meta.url`, `nomodule`; see the section below. QuickJS runs the `nomodule` fallback |
| `ResizeObserver`, `IntersectionObserver` (OB1) | Implemented on V8, built lazily; focused checks passed; not independently reviewed; records a model decision | Each target is reported once; everything counts as in view because nothing scrolls; see the section below |
| Media, canvas and SVG interfaces (IF1) | Implemented on both engines (generated); focused checks passed; not independently reviewed | `HTMLMediaElement`/`Video`/`Audio`, `HTMLCanvasElement`, `SVGGraphicsElement`/`SVGSVGElement`/`SVGAElement`; no playback and no canvas context; see the section below |
| `customElements` (CE1) | Implemented on V8, built lazily; focused checks passed; not independently reviewed | Autonomous custom elements: `define`/`get`/`getName`/`whenDefined`/`upgrade`, the HTML element constructor, upgrade and the connected, disconnected and attribute reactions; no customized built-ins, no shadow DOM yet; see the section below |
| Shadow trees (SD1) | Script, events and extraction implemented on both engines; focused checks passed; not independently reviewed | `attachShadow`, `ShadowRoot`, `shadowRoot`, slots, composed event paths with retargeting, flat-tree text and snapshot; see the section below |
| Shadow trees are styled, laid out and painted (SD2) | Implemented; focused checks passed; not independently reviewed | Sheets inside a shadow root style that tree, `:host` and `::slotted()`; boxes follow the flat tree; a screenshot shows a component; see the section below |
| `document.domain`, `crypto.randomUUID()`, `<meta>`/`<source>`/`<dialog>` interfaces (WIN2) | Implemented on both engines (generated); focused checks passed; not independently reviewed | What the sweep after CE1 and SD1 named most; a dialog has no top layer, `returnValue` or `close` event; see the section below |
| Declarative shadow roots (DSD1) | Implemented in the parser and both sinks; focused checks passed; not independently reviewed | `<template shadowrootmode>` in a navigated document attaches a shadow root; `attachShadow()` takes it over; see the section below |
| `MessageChannel`, `MessagePort`, `MessageEvent`, `window.postMessage()`; the window as receiver of `EventTarget` operations (MSG1) | Implemented on V8; focused checks passed; not independently reviewed | youtube.com, which stopped at both, renders; nothing is transferred between ports; QuickJS has no messaging; see the section below |
| `TreeWalker`, `NodeIterator`, `NodeFilter`, `CSS.escape()` (TW1) | Traversal implemented as a prelude script on V8, `CSS.escape` generated on both engines; focused checks passed; not independently reviewed | Pages built with Lit threw at their first render; see the section below |
| A host navigation starts a new script CPU window (SW1) | Implemented; focused checks passed; changes D-033 with the owner's agreement of 2026-11-17, the decision entry is still to be written | The commit of a navigation the host issued (`Page::goto`, `goto_with_eval`, `begin_goto`) empties the page's script CPU window; a navigation the document asks for (`location`, a form submission, an activated link) goes on spending the window it started in, so a document that reloads itself still has one budget. |
| Nested flex and grid containers share their answers (LP1) | Implemented; measured; fragment output compared on a real page; not independently reviewed | One layout of a component-built page solved its containers 12 times less often; see the section below. Layout still starts over after every mutation |
| Wrapper-graph collection at task ends is amortised (GC1) | Implemented on V8; measured; not independently reviewed; **changes when detached wrappers are reclaimed, which the specification may record as a decision** | A task end collects once 1024 wrappers are owed or the last collection is 250 ms old, not every time; see the section below |
| QuickJS classic-script mode | Corrected; QuickJS suite passed | The binding's default evaluation options force strict mode, so every page script, string timer handler and inspector evaluation ran strict on QuickJS and an undeclared assignment threw. They now evaluate as classic (sloppy) scripts; a regression test covers an implicit global, `this` of a plain call, a page script and an explicit `"use strict"` function. 314/314 `tenta-js-qjs` tests pass |

QuickJS runtime implementation alone does not complete its timezone hook,
Intl, engine-independent Wasm, SAB/workers or cross-platform gates. Other
QuickJS gaps include modules, bytecode caching, lazy prototypes, full
indexed/named collection conformance, WebIDL conversion and live iterable semantics, Date
constructor virtualization, precise Promise provenance, automatic GC epoch
notification and OOM budget classification.
Generated V8 callback tests do not establish containment of handwritten
callbacks or other native libraries.

The earlier property-hook QuickJS integration run passed 64 tests across legacy
properties, named collections, CSSOM, Web Storage, runtime, finalization and
Attr behavior. It includes regressions for two previously aborting teardown
cases: retained property-key atoms must expose their context edges to the
collector. Window-owned Storage wrappers now retain prototype and symbol
state between accessor reads and across collection. Hostile prototype cycles,
PutForwards error propagation, Storage receiver brands and numeric key ordering
also pass. This is focused integration evidence, not a full workspace or WPT
result. The subsequent node-wrapper checkpoint below addresses a separate lifetime gap.
Its node-anchored graph covers nodes and their live collections,
attribute maps, inline style and token lists. Sheet/rule/declaration host-record
graphs need separate ownership work; native CSS rule identity alone does not
preserve JavaScript expando or prototype state through collection.
QuickJS fork containment currently covers property-operation callbacks. Its
upstream GC trace/finalizer dispatch still needs a deliberate fail-closed
contract: catching a tracing panic and continuing with unmarked edges would
make collection unsafe. That native-boundary gate remains open; see D-025.

The earlier property-hook V8 integration checkpoint ran all 41 focused tests: 39 passed and
two failed. All 16 Attr, ten CSSOM and three named collection tests passed,
along with Storage receiver brands, descriptors and Window wrapper ownership.
The Window keeps exactly two Storage roots; the new native-origin-transition
test proves release of old wrappers while preserving explicit script roots.
Remaining failures are readonly unsupported/derived indexed assignment and
Storage supported-name/symbol ordering in `Reflect.ownKeys`. Both are being
investigated without weakening their expected behavior. Capturing pristine
`Reflect.set` preserves its Boolean result, but V8's internal creation path
still bypasses the indexed definer. Its key collector also puts ordinary own
symbols before named-interceptor keys. Combined workspace
checks follow; this checkpoint is not a full V8 conformance claim.

A subsequent shared diagnostic run passed 24 of 26 tests: all 13 QuickJS
tests passed, while V8 failed numeric Storage property operations and the
existing own-key-order case. V8 reports the ordinary Symbol before supported
names (`10`, `2`, `alpha`); bracket access and definition of numeric Storage
names bypass its named interceptor. HTMLCollection numeric-ID precedence
passes on both engines. The reviewed named-only numeric adapter subsequently
fixed the Storage case: the post-change V8 run passed 12 of 14 tests,
including canonical numeric Storage operations and the HTMLCollection indexed
precedence control. The two remaining failures are still readonly indexed
assignment and key ordering; their expected behavior is unchanged.

## Validation checkpoints

The callback-root prerequisite passed 99 focused QuickJS tests, including
throwing conversion getters, iterable reentry, a native lookup without WeakRef,
and native destructor failure controls. Both engines pass all eight pre-host
probes; V8 now preserves the original dictionary getter exception. At that pre-graph checkpoint, the full
wrapper-state probe run was 14 passed and 20 failed, with exactly ten general
node/owner-wrapper graph failures per engine. No assertion was weakened.

The subsequent QuickJS graph checkpoint passed 79/79 focused tests, including
all 17 shared wrapper-state assertions and six uninterrupted pressure probes.
It keeps native-pinned parser nodes and connected trees rooted, preserves
detached subtree wrapper state, and collects unreachable cycles. Provisional
roots are classified at safe points before host effects; nested conversion
frames defer collection and have a checked allocation limit. Deterministic
watchdog-during-GC and restoration/finalizer failure controls passed. The
broader QJS run passed 127/127 with the explicit filter
`not binary(cssom_wrapper_state)`; that separate CSS graph is still open.
The real-browser isolation/QuickJS-page subset passed 7/7. These runs do not
claim the V8 port, full workspace or P2 WPT conformance. Release comparisons
and pressure measurements for this checkpoint are recorded below. The subsequent
external-borrow parity snapshot passed the scoped C+Rust ASAN run described below. Final ownership review also
identified raw Event.target references outside the node-anchored graph; shared
held-target, redispatch, reset and cycle probes were then run separately.
The tests-only Event baseline passed 4/10 and failed 6/10: both engines
lose held/redispatched target state, including under uninterrupted pressure,
while reset and unreachable-cycle controls pass. The reviewed one-way
Event-to-node-group edge passed 147/147 initial graph regressions. An
additional foreign-document target probe then exposed a shared dispatcher
bug before collection: it looked up the event key in the target's slab. A
same-key collision control also reproduced mutation of the wrong event on
both engines (0/2). The reviewed correction keeps path/listeners in the
target context but reads/writes/encodes the original event in its owning
context. The resulting shared-library, both-engine and focused event run
passed 283/283, including all 14 target/dispatch probes. One earlier nextest
process-output LEAK classification did not recur in isolated reruns; this
was not an ASAN memory-leak report. Three further shared path controls failed
on both engines (0/6), exposing unconditional Window inclusion for detached
nodes, auxiliary documents and load events. The reviewed condition matches
the existing defaultView/browsing-context distinction and the DOM Document
parent rule; positive connected-page and direct-Window dispatch controls
remain intact. The final combined Event/owner/path run passed 289/289 with
no skips or nextest process-output leak classifications. Focused QJS
C+Rust ASAN passed all 11 Event/path/finalizer tests; all 608 captured
source/config hashes remained unchanged, with leak detection disabled.
Workspace formatting and diff checks passed. Non-node target ownership, relatedTarget and
CustomEvent detail cycles remain open.
IDL verification passed 67 tests, 20 generated goldens were refreshed, and
targeted QJS/IDL all-targets Clippy with both binding backends passed.
These focused results do not replace the earlier workspace aggregate below.

The V8 node-wrapper graph subsequently passed 59/59 focused tests across
wrapper state, uninterrupted pressure, listener cycles, snapshots, Attr and
native-panic isolation; generated goldens passed 20/20 and both-feature IDL
passed 67/67. Initial failures exposed a task-end collection borrow conflict
and allocation admission on an internal no-op interface brand probe. Both
were corrected; the multi-wrapper publication control now creates its native
ancestor tree before attempting one large composedPath conversion. Independent
Sol review additionally required mutable-borrow preflight, covering both
shared and exclusive embedding borrows. The final library run passed 39/39, including that borrow regression and
actual weak-finalizer failure after graph publication. Scoped V8/IDL
all-targets Clippy, workspace formatting and diff checks passed. Release
measurements are recorded below; these focused results do not claim a fresh
aggregate workspace pass.

Final cross-engine review also found that QJS housekeeping did not defer
legitimate embedding borrows of SharedDocument. Mutable and immutable borrow
regressions failed before the narrow preflight; the corrected admission probe
holds a separate document so wrapper publication in the primary document
remains legal. It reproduced the pressure-sweep conflict, then the package
passed 43/43 focused tests after the fix. A first green attempt had one test
accounting error: fragment parsing leaves temporary arena records until a
sweep, so the test now requires traversal to preserve the measured node count
and independently checks its 5000 children. The preflight preserves pressure
limits and collection epochs when it defers. C+Rust AddressSanitizer passed
59/59 tests with no skips across library, wrapper-state/pressure, listeners,
snapshots and native finalizers on aarch64 macOS. QuickJS C object ASAN symbols
were reverified, and all 544 captured source/config hashes remained unchanged.
Leak detection was disabled; this is no claim of complete leak freedom or
cross-platform sanitizer coverage.
The older browser trim fault fixture consequently stopped manufacturing a
collector panic from a legitimate document borrow (V8 baseline 0/1). Its
reviewed replacement returns genuine generated-getter poison from the trim
wrapper and retains page-discard, no-arena-sweep and same-thread neighbor
assertions. All six browser native-failure cases and the three final borrow
controls passed together (9/9) with both engine features. Engine-private
tests continue to exercise actual failures within collection transactions.

The CSS host-wrapper tests-only baseline was rerun after correcting the
WeakRef controls to perform the caller-owned microtask checkpoint before
collection. It reports 4/16 passing and 12/16 failing: both engines pass the
unreachable-cycle and removed-rule anti-retention controls, while six held
sheet/rule/style/list state cases fail per engine. Those state losses are
observable between ordinary script tasks. The initial two additional V8
WeakRef failures were fixture scheduling errors, not established product
failures. The directed CSS graph remains off-tree until real-engine release
baseline binaries are preserved; the native CSS lifecycle workload does not
measure JavaScript wrapper-graph cost.

The final V8 conversion package passed 34/34 fetch/conversion regressions after
removing unused conversion helpers and regenerating bindings. Goldens passed
20/20, both-feature IDL passed 67/67, and combined all-targets Clippy passed
for IDL, shared JavaScript, both backends and CSS. Workspace formatting and
diff checks passed. Ordinary conversion errors preserve their original
JavaScript value; native poison and execution termination remain distinct.
The API still lacks the Request arm of RequestInfo and other full-fetch
conformance work; this package does not claim that broader gate.

Nested style-rule CSSOM integration passed 23 native tests and 28 shared
real-engine tests (14 per backend). Unsupported nested rule kinds still fail
atomically; valid nested style rules now expose stable child/parent identities.
The same dual-engine run recorded the XML element-name defect as two failures;
after the reviewed document-type guard, the combined run passed 30/30.
Scoped all-targets Clippy passed for CSS and both JavaScript backends. Nested
CSSOM release measurements detected a handle-lifecycle regression. Reviewed
ownership consolidation reduced it; the remaining cost and detached-marker
scaling are recorded below rather than treated as a passed broader budget.

The latest integration passed workspace formatting and full workspace,
all-targets Clippy with V8 and QuickJS enabled together. Dependency layering
(26 crates), the generated IDL manifest, and all four cargo-deny check groups
also passed. The full nextest aggregate ran 2695 tests: 2692 passed, three
failed, and 55 were skipped. The failures were the two V8 conformance gaps
above and a stale native CSSOM test expectation for namespace-rule insertion
after a style rule. All 91 QuickJS backend tests, 161 native QuickJS core
tests, 67 IDL tests and 350 network tests passed in this aggregate. The new Storage
concurrency test compiles after joining owned thread handles before counting
successful writes. Both engines' Storage holder cleanup and the narrow
type-alias cleanup passed independent Sol review.
The Sol/root-reviewed CSS test correction retains separate InvalidState and
unsupported-rule cases; all four focused constructed-sheet tests passed.
No green full aggregate is claimed while the two V8 gaps remain open.
The Python WPT runner's 11 tests and benchmark-script/TOML syntax checks also
passed; these are tooling checks, not browser conformance evidence.

Earlier, a combined V8/QuickJS workspace run completed 2414 tests:
2406 passed, 8 failed, 55 skipped. Six failures reproduce listener cycles
(self capture, cross-node capture and collection after native-pin release,
on both engines). One older Attr test expected different objects for the
same attribute; its corrected canonical-identity expectation is reviewed
and passed the focused rerun. The session-cache flush test exceeded its 5 s
debug limit at 5.893 s under concurrent load; its focused single-thread
rerun passed in a 0.533 s test. The threshold is unchanged. This supports
load sensitivity, not a claim that every performance gate is satisfied. All 20 current Attr integration tests and
the V8 deferred-reclamation failure-path guard test passed.

A separate lifetime audit found that static querySelectorAll snapshots held
NodeRef values without native node pins. SnapshotSlab now has a reviewed
ownership design and a source implementation with four passing dual-engine
lifetime regressions. No pre-fix red run has been claimed.
Static collection backing does not establish required live HTMLCollection
semantics, which remain separate conformance work.

After that aggregate run, the bounded listener collector passed 96 focused
V8/QuickJS tests, including the six originally failing lifetime cases,
retention through live collections, timers and native event details,
hostile page intrinsics, and failure during post-GC restoration. The
collector deliberately leaves JS-mediated native-pin cycles rooted. The
native snapshot rollback test, workspace formatting and independent Sol/root
source reviews also passed. Six browser-level native-failure checks passed for both backends, including
trim failure before arena sweep, runtime discard and same-thread neighbor
isolation. Twenty-one additional QuickJS runtime/finalizer tests passed.
The combined-feature Clippy run exposed five repeated QuickJS generator lint
patterns (437 emitted diagnostics). Template and narrow backend/test cleanups
were independently reviewed without blanket suppression; regenerated golden
tests passed. The full workspace/all-targets Clippy check with both engines
and workspace formatting now pass. The first post-collector aggregate ran 2441 tests: 2439 passed, 2 failed,
55 skipped. Only two older drain fixtures failed because the collector now
releases unrooted callbacks before native sweep. Their scenarios now retain
noncapturing callbacks globally so the original deferred drain and reused
slot checks remain exercised, with unchanged expected counts. Independent
Sol review and all three focused drain tests passed. The next aggregate ran
2441 tests: 2440 passed, one HTTP/2 retry fixture timed out waiting for response
headers, and 55 were skipped. Its server awaited the request body without
continuously driving the connection. The subsequent fixture and retry safety
corrections passed all 350 network tests (8 skipped), including HTTP/1,
HTTP/2, cookie import, processed-POST refusal to replay, and typed GOAWAY
eviction decisions; net Clippy passed. There is no green post-collector
workspace aggregate yet; CSS/Storage/native-fork integration is in progress.

The earlier combined V8/QuickJS workspace attempt ran 702 tests: 701 passed
and one failed because the generated prelude golden still had a trailing
blank line removed from its source. Goldens were regenerated and all 65
IDL tests then passed without update mode; the both-engine IDL configuration
also passed 63 tests. Both-engine IDL Clippy passed after two behavior-neutral
generator warning cleanups, independently reviewed by Sol.
That golden-file failure is resolved; the later full-run failures have
focused fixes or follow-up evidence but need a fresh aggregate checkpoint.

Earlier, before the packages now in progress:

- `cargo nextest run --workspace --test-threads 4`: 2259 passed, 55 skipped.
- Browser, CLI and IDL combined V8 configuration: 224 passed, 2 skipped.
- Workspace and V8-feature Clippy, formatting, dependency layering and IDL
  manifest checks passed.
- `cargo deny check` at that checkpoint: advisories failed for transitive `paste 1.0.15`
  (`RUSTSEC-2024-0436`, through V8); bans, licenses and sources passed.
  After adding the reviewed maintenance exception, `cargo deny check`
  passed all four check groups, including the new QuickJS/Stylo dependencies.
  It passed again after adding the Windows-only `winapi-util` route for core
  and the direct Stylo-aligned `selectors` route for CSS; dependency layering
  and the IDL manifest also passed at that later checkpoint.
- The full P2 WPT gate has not passed. Earlier focused Range and React/Vue
  smoke probes are not full DOM or hydration conformance.

Benchmark artifacts (local, gitignored):

- `bench/results/20260923-S-dom-calls-native-boundaries.json`
- `bench/results/20260923T060251Z-S-static-text-cli-p2-eval-native-audit.json`
- `bench/results/20260923-S-js-xhr-inprocess-first.json` (subsequent first
  in-process baseline: p50/p95 2.96/3.10 ms, whole-process RSS 23.7/23.8 MiB;
  one build only, not an interleaved regression comparison)

Other-project compilation was active during these measurements. DOM call
medians changed by less than the 10% claim threshold. Static CLI p95 was
25.3 ms versus 21.0 ms before the changes; that observation is provisional
and is not a claim that a regression exists or has been ruled out.

## Pre-collector performance checkpoint

Release examples were built with both browser backend features and preserved
before the listener collector or static-snapshot pin fix. Exact binary hashes
are in `bench/results/20260923-pre-collector-binaries.json`; the source hash
manifest and build command are in
`bench/results/20260923-pre-collector-release-source.json`. Other applications
were active; before/after host-load captures accompany the artifacts.

- `20260923-S-dom-calls-pre-collector-v8.json`: 25 interleaved pairs against
  the preserved generated-boundary build. Primitive call p50/p95 changed
  from 27.14/36.66 ns to 27.54/32.53 ns; string call from 73.30/90.85 ns to
  75.50/92.04 ns. Median changes (+1.5%, +3.0%) are below the 10% noise
  threshold. Whole-process peak RSS p50 was 27.4/27.3 MiB.
- `20260923-S-dom-calls-backend-comparison.json`: 25 interleaved V8/QuickJS
  pairs, a backend comparison rather than an optimization claim. QuickJS
  primitive p50/p95 133.71/186.06 ns, string 183.16/205.81 ns, process RSS
  p50 9.4 MiB. These focused calls meet the 500 ns QuickJS budget on this
  host; this does not establish the complete P2 budget gate.
- `20260923-S-js-xhr-pre-collector.json`: 25 interleaved old/current pairs,
  navigation p50/p95 4.93/9.01 ms versus 4.88/10.48 ms. The 16.3% p95
  increase prompted one separate 25-pair run, retained as
  `20260923-S-js-xhr-pre-collector-tail-followup.json`: 3.30/4.16 ms versus
  3.42/4.33 ms. The second p50/p95 differences are below 5%; both reports
  remain evidence of load-sensitive tails, not proof that every regression
  is ruled out. Current process RSS is about 24.1 MiB; page RSS delta and
  last-mutation gap are still unmeasured.
- `20260923-S-listener-gc-v8-before.json` and
  `20260923-S-listener-gc-qjs-before.json`: 25 baseline samples per engine,
  five real-engine phases. Both engines retain all 256 unreachable
  self/cross-cycle nodes; noncapturing controls reclaim 256/256, and live
  listener dispatch remains correct. Three complete trim/sweep/trim rounds
  cost roughly 2.3–2.8 ms p50 on V8 and 0.13–0.20 ms on QuickJS per phase.
  These are before-fix baselines, not successful cycle collection.

## Post-collector performance checkpoint

The same release command produced five preserved post-change binaries. All
594 source fingerprints were unchanged between snapshot and build completion;
exact hashes are in `20260923-post-collector-binaries.json` and
`20260923-post-collector-release-source.json` under `bench/results/`.
Each comparison below used 25 interleaved pre/post pairs. Other-project
compilation remained active; load captures are retained. These comparisons
cover the intervening native-boundary, snapshot and collector changes together,
not an isolated attribution to the collector.

- `20260923-S-listener-gc-v8-after.json`: self/cross captures now reclaim
  256/256 nodes; live controls remain intact. Three GC/sweep rounds for
  reachable detached nodes increased from 6.39/12.47 to 11.00/20.32 ms
  p50/p95 (+72% p50). Whole-process RSS p50 rose from 19.8 to 20.4 MiB.
- `20260923-S-listener-gc-qjs-after.json`: the same ownership checks pass.
  Reachable detached rounds increased from 0.41/0.84 to 5.06/8.28 ms
  p50/p95; self-capture rounds from 0.41/0.97 to 1.98/3.46 ms. Whole-process
  RSS p50 rose from 11.1 to 11.5 MiB. These material collection costs remain
  an investigation item, not an optimization claim.
- `20260923-S-dom-calls-v8-after.json`: primitive calls 52.43/75.87 to
  45.32/92.56 ns p50/p95; string calls 127.96/208.56 to 126.45/237.60 ns.
  `20260923-S-dom-calls-qjs-after.json`: primitive calls 270.12/377.22 to
  261.65/385.26 ns; string calls 351.24/492.87 to 362.33/552.53 ns.
  QuickJS median differences are below the 10% claim threshold. Both engines'
  median per-call values meet the focused limits, but V8 primitive p95 exceeds
  60 ns and QuickJS string p95 exceeds 500 ns under this load. Variable tails
  preclude an unqualified budget-gate pass or a broad speedup claim.
- `20260923-S-js-xhr-post-collector.json`: navigation p50/p95
  14.50/25.93 to 12.55/18.94 ms; whole-process RSS p50 24.1 to 24.0 MiB.
  This focused navigation stays within 40 ms; it does not measure the page
  RSS delta or last-mutation-to-settle gap and does not close the P2 gates.

## Legacy-property integration performance checkpoint

Seven release binaries are preserved in `20260923-legacy-binaries.json`;
all 891 source hashes in `20260923-legacy-release-source.json` were checked
again after the successful build. Each report below contains 25 interleaved
pairs, CPU and peak process RSS. Root ran no compiler during measurement;
other-project compilation was not stopped. Comparisons against the preserved
post-collector binaries cover the intervening changes together.

- `20260923-S-dom-calls-v8-legacy.json`: hasAttribute p50/p95
  31.88/36.41 → 36.35/38.76 ns; getAttribute 87.68/91.52 → 91.75/97.77 ns.
  The 14.0% primitive median increase exceeds the noise floor and requires
  isolation; the string median difference is below it. Process RSS p50/p95
  27.4/27.6 → 27.7/27.8 MiB.
- `20260923-S-dom-calls-qjs-legacy.json`: primitive 165.13/171.94 →
  179.16/188.53 ns; string 226.24/241.95 → 245.28/255.41 ns. Both median
  differences are below 10%; RSS 9.5/9.5 → 9.8/9.8 MiB.
- `20260923-S-listener-gc-{v8,qjs}-legacy.json`: all self/cross cycles
  reclaim 256 nodes and live controls remain intact. Every phase's median
  difference is within 10%. Reachable-detached p50/p95 is 3.44/5.32 ms for
  V8 and 1.67/2.99 ms for QuickJS; RSS is 20.3/20.8 and 11.7/11.9 MiB.
  This does not resolve the earlier collector-versus-leaking-baseline cost.
- `20260923-S-js-xhr-legacy.json`: navigation 4.75/6.84 → 4.68/5.86 ms;
  RSS 24.0/24.1 → 24.4/24.5 MiB. The median difference is below 10%.
- `20260923-S-storage-calls-initial.json`: initial V8/QuickJS comparison,
  with 128 keys and 100,000 verified calls per operation. V8 getItem,
  named read, setItem and named write p50/p95 are respectively
  259.27/445.38, 408.31/1078.27, 479.88/1241.16 and 331.66/557.85 ns.
  QuickJS values are 901.76/1434.41, 1209.51/2367.15, 1028.31/2376.64 and
  447.97/977.56 ns. Process RSS is 26.8/26.8 MiB and 9.8/9.9 MiB;
  these are whole-process measurements, not Storage heap sizes.

These are focused workloads. They do not establish the complete P2 budget,
reference-browser, WPT or mobile gates.

## Isolated qualified-name allocation correction

The measured primitive regression prompted a narrow change: unchanged
qualified names are borrowed, and HTML/namespace case checks run only when
the input contains an ASCII uppercase byte. First-qualified-name ordering
and XML/SVG case behavior remain unchanged. Sol/root source review, 38
dual-engine Attr/named collection regressions and both-backend all-targets
Clippy passed. The seven rebuilt binaries and 894 checked source hashes are
recorded in `20260923-qualified-borrow-binaries.json` and its source manifest;
this helper was the only production-source change from the legacy checkpoint.

Both comparisons used 25 interleaved pairs against preserved legacy binaries:

- `20260923-S-dom-calls-v8-qualified-borrow.json`: hasAttribute p50/p95
  34.98/36.14 → 21.68/23.58 ns (-38.0% median); getAttribute
  88.02/92.91 → 73.81/76.96 ns (-16.1%). Process CPU p50/p95
  134.59/139.43 → 107.24/109.36 ms; RSS 27.7/27.8 → 27.8/27.9 MiB.
- `20260923-S-dom-calls-qjs-qualified-borrow.json`: primitive
  175.04/186.41 → 156.74/166.87 ns (-10.5%, close to the noise floor);
  string 240.04/249.20 → 219.25/230.43 ns (-8.7%, below the claim threshold).
  CPU 419.67/437.03 → 383.34/401.61 ms; RSS 9.8/9.8 MiB for both.

This resolves the observed V8 primitive regression for the focused workload;
it does not change the outstanding full-engine conformance and budget gates.

## Incoming argument-root cost

The preserved QuickJS release checkpoint records 896 source fingerprints,
rechecked after the build, in `20260923-transient-roots-release-source.json`;
the immutable executable is identified by `20260923-transient-roots-binaries.json`.
The build matches the previous combined-backend feature configuration. V8-only
and CSS benchmark registration changes are included in the source snapshot;
the QuickJS JavaScript workload is unchanged.

`20260923-S-dom-calls-qjs-transient-roots.json` contains 25 interleaved pairs
against the qualified-name borrowing checkpoint. hasAttribute p50/p95 is
140.54/169.32 → 154.37/170.56 ns (+9.8% median), and getAttribute is
195.71/204.76 → 211.11/226.54 ns (+7.9%). Both median differences are below
the 10% claim threshold. Process wall p50/p95 is 343.90/380.42 →
377.52/402.64 ms; CPU is 337.22/368.79 → 370.01/386.24 ms, and RSS is
9.8/9.8 MiB for both. The focused QuickJS DOM-call
budget is met in this run; broader phase budgets remain open.

## V8 conversion release checkpoint

`20260923-v8-conversions-binaries.json` identifies the preserved V8 DOM-call
executable and its 902 source fingerprints, verified unchanged after the
release build. The comparison against qualified-borrow includes the conversion
fix, numeric Storage bridge, pending-exception helper, nested CSSOM and XML
element-name changes; it is not an isolated conversion-only experiment.

`20260923-S-dom-calls-v8-conversions.json` contains 25 interleaved pairs.
hasAttribute p50/p95 is 20.12/26.13 → 19.17/21.45 ns, and getAttribute is
67.01/73.25 → 66.05/112.19 ns. Median differences (-4.7% and -1.4%) are
below the claim threshold; the string-path tail increased in this run.
Process wall p50/p95 is 98.64/117.16 → 96.77/145.34 ms, CPU is
97.75/107.16 → 94.89/131.18 ms, and RSS is 27.7/27.8 → 27.6/27.7 MiB.
Both focused V8 budgets are met in this run: primitive calls are below 60 ns
and string calls below 250 ns (`05-javascript.md` §13). This preserves a
checkpoint for subsequent wrapper-graph work; it does not close the broader
reference-browser, platform or P2 budget gates.

## Nested CSSOM lifecycle measurement

`20260923-S-cssom-lifecycle-nested-first.json` preserves 25 alternating AB/BA
pairs at each flat-sheet size, plus candidate-only detached-rule churn.
Adjacent identity manifests record the corrected pre-nested baseline and
the first nested candidate. The original short baseline was superseded:
the corrected workload forces the complete handle through `black_box` and
uses 1,048,576 handle operations and 16,384 insertion/deletion pairs.

At 64/256/1024 rules, handle p50/p95 changed respectively from
9.25/12.23 → 12.66/18.25, 7.16/12.05 → 14.34/18.89 and
9.13/12.63 → 14.53/19.47 ns. Median regressions are 36.9%, 100.2% and
59.2%, above the noise floor. Whole-process CPU p50/p95 changed from
26.02/32.70 → 26.46/37.56, 22.52/30.93 → 29.26/39.32 and
24.41/32.67 → 28.97/40.57 ms. RSS p50/p95 changed from
3.00/3.03 → 3.03/3.06, 3.09/3.12 → 3.09/3.14 and
3.55/3.59 → 3.59/3.62 MiB.

Detached last-handle-drop p50/p95 at 16/64/256 live markers was
142.11/232.89, 229.48/366.80 and 479.86/666.40 ns per handle;
there is no pre-nested equivalent. These results motivate removing duplicate
link ownership and reducing repeated marker scans while preserving actual
last-handle reclamation.

The final ownership change keeps the Stylo sheet in the shared link state and
one link owner in each rule handle. It removes redundant reference counts
without changing parent links or reclamation. Native tests passed 23/23;
the combined CSSOM and element-name regressions passed 30/30 across both
engines. Formatting and scoped all-targets Clippy passed. Independent Sol
and root reviews accepted the ownership change.

`20260923-S-cssom-lifecycle-nested-one-handle-arc.json` and its identity
companion preserve the final 25-pair comparison against the same corrected
pre-nested binary. At 64/256/1024 rules, handle p50/p95 changed from
7.23/8.96 → 8.15/14.87, 6.61/8.24 → 7.96/11.84 and
6.90/9.61 → 8.26/11.07 ns. Residual median costs are 12.7%, 20.3% and
19.7%; there is no specified flat-CSSOM nanosecond gate. The ±10% rule is
a noise threshold for claims, not a newly imposed regression budget.
Mutation medians stayed within ±4.3%. Whole-process CPU p50/p95 changed
from 19.05/22.97 → 20.50/28.50, 18.89/23.49 → 19.92/25.08 and
19.67/24.21 → 21.55/24.05 ms. RSS p50/p95 was
2.98/3.02 → 2.98/3.02, 3.08/3.12 → 3.06/3.08 and
3.55/3.59 → 3.55/3.58 MiB. Detached last-handle-drop p50/p95 remained
147.99/216.32, 246.62/369.93 and 478.90/584.52 ns at 16/64/256 markers.
The repeated marker scan remains a separate measured scaling concern.

## QuickJS wrapper graph measurements

The preserved release checkpoint is identified by
`20260923-wrapper-graph-qjs-binaries.json` and its 912-file source manifest,
rechecked after the build. Three scenarios completed; independent compiler
activity in other TentaFlow workspaces was present and left untouched. These
are interleaved same-machine comparisons, not idle-system measurements.

`20260923-S-dom-calls-qjs-wrapper-graph.json` compares 25 pairs against the
transient-root checkpoint. Primitive p50/p95 was
257.56/301.53 → 248.83/291.59 ns and string p50/p95 was
320.82/405.70 → 328.82/434.25 ns. Median differences of -3.4% and +2.5%
are below the claim noise floor; both focused QuickJS operations remain below
500 ns. Whole-process CPU p50/p95 was
486.17/567.54 → 500.75/565.43 ms; RSS was approximately 9.8 → 9.7 MiB.

`20260923-S-listener-gc-qjs-wrapper-graph.json` compares 25 alternating pairs
against the older qualified-name-borrow checkpoint, so this is a cumulative
comparison. Whole-process CPU p50/p95 increased
24.047/29.577 → 29.659/34.811 ms (+23.3% median); wall time increased
30.511/43.341 → 36.019/50.525 ms (+18.1%). RSS p50/p95 was
11.656/11.844 → 12.016/12.125 MiB. Connected explicit trim/sweep p50/p95
increased 0.330/0.723 → 0.609/1.109 ms. Self/cross-capture explicit trim
medians fell 83.3%/78.2%, but task-end GC moves collection into untimed
setup; those decreases are not net GC savings. Whole-process totals include
that work. Every self/cross/noncapturing phase freed all 256 nodes, and both
live listener controls dispatched 256 callbacks. Empty microtask checkpoints
currently also trigger task-end GC; avoiding that redundant work is a
separate measured optimization candidate.

`20260923-S-wrapper-pressure-qjs.json` records 25 fresh processes per shape,
with rotated shape order and no warmup or pre-graph comparison. Detached
churn / owner wrappers / connected nodes / deep tree task p50/p95 was
60.980/99.241, 150.069/197.881, 13.298/21.832 and 8.943/18.029 ms.
Whole-process CPU p50/p95 was 63.478/97.855, 134.797/171.114,
45.823/71.231 and 24.814/34.260 ms; RSS p50/p95 was
13.406/13.703, 12.641/12.906, 16.312/16.594 and 13.656/13.781 MiB.
The 12,000-node detached task left 740 native nodes; the 6,000-owner task
left 115. Connected and deep tasks preserved exactly 5,002 and 1,202 nodes.
All samples passed state checksums and released back to the single document
node. Collection epoch deltas were 12/24/5/3, proving collection inside the
uninterrupted tasks. This is boundedness and focused cost evidence, not a
full context-memory or P2 gate.

## V8 wrapper graph measurements

The V8 graph checkpoint uses three binaries from
`20260923-cssom-wrapper-graph-baseline-binaries.json`, with its 920-file
source manifest rechecked after the build. That same build preserved both
CSSOM workload baselines before the Event.target and CSS host-graph changes.
All measured binaries were hash-checked before and after execution. Other
TentaFlow compiler activity remained present; no TentaBrowser compilation
ran during these measurements. These are not idle-system results.

`20260923-S-dom-calls-v8-wrapper-graph.json` compares 25 alternating pairs
against the prior conversion checkpoint. Primitive p50/p95 changed
26.08/36.07 → 27.65/40.89 ns (+6.0% median, below the claim noise floor);
string p50/p95 changed 93.66/128.51 → 110.30/142.18 ns (+17.8%). Both
operations meet the focused 60/250 ns budgets. Whole-process CPU p50/p95
increased 120.17/142.17 → 151.35/175.25 ms; wall time increased
131.53/174.02 → 155.99/196.29 ms. RSS p50/p95 was approximately
27.61/27.81 → 27.39/27.44 MiB. Meeting those focused budgets does not
remove the measured string-operation and total CPU costs.

`20260923-S-listener-gc-v8-wrapper-graph.json` compares 25 alternating pairs
against the older qualified-name-borrow checkpoint and is therefore
cumulative. Whole-process CPU p50/p95 increased
64.105/74.535 → 102.692/120.083 ms (+60.2% median); wall time increased
46.762/67.482 → 68.327/110.579 ms (+46.1%). RSS p50/p95 was
20.766/20.875 → 20.859/21.047 MiB. Explicit trim/sweep medians were
3.608 → 3.455, 4.014 → 3.453, 3.892 → 3.448, 6.238 → 6.382 and
3.873 → 3.632 ms for connected, self-capture, cross-capture,
reachable-detached and noncapturing phases. Task-end collection also runs
outside those explicit timers, so the apparent self/cross reductions are not
net collection savings. Every unreachable phase released 256 nodes and both
reachable controls dispatched 256 callbacks. Empty or duplicate task
checkpoints are a separate optimization investigation, not an accepted fix.

`20260923-S-wrapper-pressure-v8.json` records 25 fresh processes per shape,
without a pre-graph comparison. Detached / owner wrappers / connected / deep
task p50/p95 was 42.202/62.631, 58.672/92.634, 11.477/18.560 and
5.203/7.598 ms. Whole-process CPU p50/p95 was 87.279/106.241,
100.291/129.721, 46.346/52.999 and 35.435/41.602 ms. RSS p50/p95 was
30.281/30.406, 24.281/24.516, 24.109/24.234 and 21.109/21.234 MiB.
Every sample preserved state checksums; native nodes after task were
741/115/5002/1202, then returned to the single document node after release
and sweep. Epoch deltas were 24/48/10/6; these are engine epoch increments,
not a cross-engine comparison of collection counts. This establishes the
focused uninterrupted-allocation controls, not the full context-memory gate.

## Cumulative Event and CSS wrapper graph checkpoint

The constructed-CSSOM graph now shares the existing node/Event collection
transaction in both engines. Current sheet components, detached rule subtrees
and empty-list sinks retain only getter-reachable wrappers; no new native
records or wrappers are created during graph preparation. Shared pressure
admission covers both node and CSS publication. The corrected tests-only
baseline passed 4/16 and failed 12/16. Current validation passed native CSS
25/25, shared CSSOM 60/60, both engine libraries 61/61, broader node/Event/Attr
integration 129/129 and V8 native-panic isolation 3/3. The private pressure
controls sample collection before task end, including nested-conversion
refusal without per-refusal native-record leaks. Scoped all-targets Clippy,
workspace formatting and dependency lint passed. Independent Sol reviews and
root review covered native directionality, both bridges and test oracles.
Focused QuickJS C+Rust ASAN passed 78/78 CSS, pressure, listener, Event and
finalizer tests. The 932-file candidate source manifest was rechecked
unchanged after the run, and both compiled QuickJS C objects expose ASAN
report symbols. Leak detection was disabled, so this is not a leak claim.
The run log is `/tmp/tenta-qjs-css-graph-asan.log`; source and C-object
fingerprints are in `/tmp/tenta-qjs-css-graph-asan-evidence.json`.

`20260923-cssom-wrapper-graph-event-css-candidate-binaries.json` preserves
nine release binaries and references a 932-file source manifest rechecked
after building. All eight workload smokes and the dual-engine WPT driver
invocation contract passed. The reports below measure cumulative Event/dispatch
and CSS graph changes, not an isolated Event or CSS-only delta. The shared
workload source hashes match their preserved baselines. CSS and listener
comparisons use 25 alternating AB/BA pairs and two process warmups; DOM uses
25 fixed-order AB pairs and two process warmups. Pressure uses 25 fresh
processes per shape. No TentaBrowser compilation overlapped measurements.
Other-workspace compilation and system background load were present, so this
is not an idle-machine claim. Differences within ±10% do not support speed
claims; that noise floor is not a generic allowed regression budget.

CSS reports are `20260923-S-cssom-wrapper-graph-{v8,qjs}-event-css.json`.
They compare against the pre-Event/CSS baseline manifest. Each subject checks
64 current rules, 8192 reads, 256 insert/read/delete pairs and three explicit
trims. Native values and checksums pass on both subjects; separate behavior
tests establish wrapper-state correctness. Timed read/mutation tasks include
task-end collection. Large detached nested components and native marker-scan
scaling are outside this workload.

| Engine / metric | Baseline p50/p95 | Candidate p50/p95 | p50 change |
|---|---:|---:|---:|
| V8 read ms | 4.693/4.971 | 4.636/4.883 | -1.2% |
| V8 mutation ms | 1.268/1.355 | 1.700/1.940 | +34.1% |
| V8 trim ms | 0.939/1.088 | 1.117/1.192 | +19.0% |
| V8 whole CPU ms | 23.921/25.319 | 24.700/27.331 | +3.3% |
| V8 RSS MiB | 27.594/27.688 | 28.172/28.219 | +2.1% |
| QuickJS read ms | 23.955/25.208 | 11.638/11.987 | -51.4% |
| QuickJS mutation ms | 1.641/1.728 | 1.963/2.090 | +19.7% |
| QuickJS trim ms | 0.046/0.047 | 0.246/0.267 | +438.7% |
| QuickJS whole CPU ms | 31.484/33.354 | 19.463/20.134 | -38.2% |
| QuickJS RSS MiB | 11.859/11.891 | 12.766/12.812 | +7.6% |

Both backends have higher mutation and explicit-trim costs. QuickJS total
CPU improves in this workload, but the reports alone do not isolate its
cause. V8 whole-process CPU is within the noise floor.

DOM reports `20260923-S-dom-calls-{v8,qjs}-event-css.json` compare V8 against
the same CSS baseline and QuickJS against its earlier node-graph checkpoint
(which also predates external-borrow parity). V8 has/get p50/p95 are
20.638/21.541 and 74.703/76.463 ns; QuickJS 149.482/161.677 and
202.140/229.873 ns. Median differences (+3.0/+3.4% and −3.8/−2.8%) are
below the claim floor; focused 60/250 ns V8 and 500 ns QuickJS budgets hold.

Listener reports `20260923-S-listener-gc-{v8,qjs}-event-css.json` use the
same respective baselines. Whole-process CPU p50/p95 is
60.155/65.250 → 61.197/67.203 ms for V8 and
14.711/15.341 → 14.380/14.903 ms for QuickJS. RSS p50/p95 is
20.234/20.359 → 20.281/20.563 MiB and
11.875/11.953 → 11.813/11.984 MiB. All phase median timing differences
are within ±10%; unreachable controls reclaim all 256 nodes and live
controls preserve state and dispatch. No net collection speedup is claimed.

Candidate-only `20260923-S-wrapper-pressure-{v8,qjs}-event-css.json`
records detached/owner/connected/deep task p50/p95:
V8 23.146/23.933, 33.711/34.633, 7.547/7.731, 3.368/3.494 ms;
QuickJS 38.642/40.251, 78.399/80.255, 6.839/7.316, 5.399/5.933 ms.
The reports include whole-process CPU and RSS. Post-task node counts are
741/115/5002/1202 for V8 and 740/115/5002/1202 for QuickJS; all return to
one document after release and sweep. Epoch increments are respectively
24/48/10/6 and 12/24/5/3, not comparable collection counts. These focused
controls do not close the context-memory or P2 WPT gates.

## V8 callback admission lock removal

The ordinary slow-callback path now polls the watchdog gate once, retaining
a second poll only when a pressure checkpoint ran. Both polls still protect
the collection recovery path. Independent Sol and root source review found
no changed required watchdog guarantee. Focused V8 checks passed 110/110;
scoped all-targets Clippy passed. The first run exposed one stale trim test
that expected a connected node's wrapper to disappear. It was replaced by
connected identity/expando retention and detached-unreachable collection
controls; both explicitly finish the WeakRef creation job before trimming.

The first release candidate had a narrower feature union and is excluded
from isolated optimization comparisons. The replacement used the exact
three-package, dual-engine, nine-target Event+CSS build command. Its
932-file source manifest was rechecked after building; only the admission
file changes production behavior. Other differences are test additions and
one obsolete test-comment cleanup. Preserved identities are in
`20260923-v8-one-lock-feature-matched-{source,binary}.json`.

`20260923-S-dom-calls-v8-one-lock-matched.json` records 25 fixed-order AB
pairs, two process warmups, p50/p95, whole-process CPU and RSS against the
preserved Event+CSS V8 binary. HasAttribute p50/p95 is
22.084/25.703 → 22.147/27.863 ns; getAttribute is
82.399/91.646 → 77.672/94.496 ns. Median changes (+0.3%/−5.7%) are below
the ±10% claim floor; there is no measured speedup claim. The focused
60/250 ns budgets hold. Whole-process CPU changes −1.9% and RSS +0.4%,
also below the floor. No TentaBrowser build, WPT run or cache cleanup
ran during measurement; other-workspace and system background load remained.

## Fresh event WPT measurement

The preserved Event+CSS dual-engine driver completed all 202 `dom/events`
files on V8 in 301.1 seconds without truncation: 81 harness-OK files and
234/486 recorded passing subtests (48.15%). Exit 1 reports unexpected
results; this is not a P2 conformance gate pass. The pinned WPT revision is
`b89af32bc8f42d678f444eb0703bca015ddcf240`, and the report preserves driver
SHA `90fff04b93bbed3d919083094cd49e9a32200e7f95d04c0c3da8e42aa559e879`.
The immutable copy is
`wpt/results/checkpoints/20260923-event-css-v8-dom-events.json`.
Harness timeouts without subtests do not contribute to the recorded-subtest
denominator; the percentage must be read together with file outcomes.
The matching QuickJS run exited 3 as not comparable: 169 harness timeouts,
33 deadline-NOTRUN files, zero registered subtests, and truncation after
501.3 seconds. Its preserved report is
`wpt/results/checkpoints/20260923-event-css-qjs-dom-events.json`. Repeated
`Illegal invocation` during testharness bootstrap, followed by missing setup,
requires a reduced reproduction; zero registered subtests is not a valid
0% conformance measurement.

A reduced diagnostic subsequently isolated the QuickJS bootstrap failure:
qualified `window.addEventListener` and document collection prerequisites pass,
but unqualified `addEventListener('error', ...)` throws `Illegal invocation`.
The original harness stack points to pinned `testharness.js:5074`, before
`on_tests_ready()` initializes its output handler; the later setup-null error
is secondary. The diagnostic uses the actual pinned harness only off-tree.
The generated QuickJS receiver lookup now substitutes its realm global for
null or undefined, before the existing interface brand checks, as required
by [Web IDL operations and attributes](https://webidl.spec.whatwg.org/#es-operations).
Interface argument conversion stays separate. Two self-contained regressions
cover unqualified calls, explicit nullish receivers, invalid object/primitive
receivers, getter brands and a null Node argument: they reproduced the failure
before the correction and pass afterward. An off-tree probe using the exact
pinned testharness plus `setup({output:false})` also passes; its temporary
checkout-dependent test was removed. Sol and root source review accepted the
scope. IDL validation passed 67/67 (including 20 code-generation tests), QuickJS
passed 161/161, and workspace formatting plus scoped all-target Clippy for IDL,
shared JS and QuickJS passed with both shared engine features and warnings
denied. Two generated prelude goldens were refreshed. The earlier QuickJS
directory report above still describes the original bootstrap failure.

Pre-dispatch stop flags were independently corrected after the V8 result.
Four new tests failed on both engines before the fix. Dispatch now preserves
pre-set stop flags until exit and skips the first listener invocation when
stopped, while reusing existing loop checks thereafter. The same event can
be redispatched after transient flags reset; its canceled flag remains sticky.
The 281-test shared-library/both-engine/Event/path/basic/click run passed with
no skips; workspace formatting and diff checks passed. A first attempted
command named a nonexistent test target and ran no tests; the corrected run
is `/tmp/tenta-event-stop-flags-green.log`. The subsequent receiver-package
Clippy run also covers this shared dispatch change.

The nine-target release rebuild used the exact earlier feature/build command;
all 933 source hashes matched before and after binary preservation. Evidence:
`bench/results/20260923-event-stop-qjs-receiver-release-{source,binaries}.json`.
The preserved dual-engine driver SHA-256 is
`e92a9a4014862e17119542062d97749bd4982d6d2b265ea8b9de4a8310a1395c`.
Seven pinned event files now have OK harness status on both engines. V8 passes
39/39 subtests and QuickJS passes 37/39. The pre-dispatch cancelBubble test
passes on both; both QuickJS failures are calls to `Event()` without `new`,
which incorrectly succeed. Focused reports are
`wpt/results/checkpoints/20260923-event-receiver-focused-{v8,quickjs}.json`.
These are diagnostic subsets, not directory or P2 conformance gates.

The fresh full QuickJS dom/events run completed in 310.4 seconds with the same
preserved driver and pinned WPT revision. All 202 files were attempted:
80 harnesses were OK, 121 timed out and one errored. Of 449 recorded subtests,
227 passed, 197 failed, 16 timed out and nine were not run within their files.
The report is not truncated, and no file was left unstarted. The command exited
1 because expectations still fail; this is not a conformance pass. Evidence:
`wpt/results/checkpoints/20260923-event-receiver-qjs-dom-events.json`
(SHA-256 `df8898c895cf3d7c8292419046bb08d3793b3db617c3bd8b6a41a5b57fa61563`).
The previous QuickJS bootstrap failure is resolved. Among the remaining
recorded failures, 32 report an illegal constructor and 32 report an unexpected
defaultPrevented value; these messages are triage evidence, not established
root causes. The pre-dispatch cancelBubble regression passes in the full run.

Four matched release comparisons are recorded in
`bench/results/20260923-S-{dom-calls,listener-gc}-{v8,qjs}-event-receiver.json`.
Each has 25 measured pairs and two process warmups; DOM uses fixed AB pairs,
while listener GC alternates AB/BA. The cumulative production delta is the
receiver correction, dispatch stop flags and the reviewed V8 lock removal.
TentaBrowser/native-V8 compilation was idle during measurement, but unrelated
workspace compilation competed for CPU and memory. Sol independently checked
binary hashes, workload arithmetic and every listener phase's native counts.

DOM hasAttribute/getAttribute p50/p95 (ns) are:

| Engine/operation | Earlier checkpoint | Receiver/dispatch checkpoint |
| --- | ---: | ---: |
| V8 hasAttribute | 23.824 / 37.095 | 23.490 / 27.864 |
| V8 getAttribute | 91.009 / 128.658 | 84.139 / 109.685 |
| QuickJS hasAttribute | 219.992 / 318.585 | 230.991 / 459.153 |
| QuickJS getAttribute | 342.463 / 820.596 | 323.350 / 669.107 |

All median deltas are below the noise floor. QuickJS getAttribute p95 remains
above 500 ns, so this loaded-machine run is not a full DOM-budget pass.
DOM peak RSS p50/p95 is 27.578/27.719 → 27.578/27.641 MiB on V8 and
9.781/9.828 → 9.766/9.828 MiB on QuickJS. Listener whole-process CPU p50/p95
is 80.957/103.150 → 77.836/96.409 ms on V8 and
20.792/26.290 → 20.072/28.530 ms on QuickJS; peak RSS is
20.578/20.984 → 20.375/20.906 MiB and 12.016/12.156 → 12.047/12.172 MiB,
respectively. Broad wall-time/phase tails prevent attributing individual
phase decreases to the code change. No speedup is claimed. All listener
reachability, reclamation and dispatch controls remain correct.

Initial actionable failures include the missing standalone EventTarget
constructor (once/passive option tests report `Illegal constructor`),
pre-dispatch propagation flags being reset, and cross-document node adoption
or doctype cloning in auxiliary-document bubbling tests. Other failures cover
unimplemented Workers, location, shadow DOM, activation, scrolling and test
interaction infrastructure. Expectations were not promoted or changed.

## Generic native-wrapper materialization

Both engines now account for every newly materialized non-Window native
wrapper, including Event and ordinary host records, under the existing
1024/4096 checkpoint/admission bounds. Cached wrapper lookup adds no debt;
reminting does. QuickJS reserves before publication, so successful reservation
followed by a publication failure conservatively consumes debt until a safe
checkpoint. A refused reservation itself adds no debt. Provisional graph roots
remain limited to their existing node/CSS ownership cases in this package.

Generated constructors track each converted stored-value reference in their
own callback frame, check interruption/poison/debt after page-controlled
conversion, and transfer ownership only after successful host construction.
Failed publication rolls back only the newly constructed Event/Host record
using its original context and exact generational key. Existing-record remints
and Node/Document ownership are excluded from that rollback. Sticky native
failure retains remaining actual roots until teardown.

V8 validation passed 50/50 library tests, 13/13 shared pressure tests across
both engines, 67/67 IDL tests, scoped all-target Clippy, formatting and diff
checks. The focused V8 integration run passed 90/91; its sole failure remains
the previously identified Storage ownKeys ordering defect awaiting the native
V8 hook. Logs: `/tmp/tenta-v8-materialization-lib.log`,
`/tmp/tenta-generic-materialization-pressure-both.log`,
`/tmp/tenta-v8-materialization-integrations-final.log` and
`/tmp/tenta-v8-materialization-clippy.log`. One pressure test reported a
nextest LEAK status despite passing its assertions; no leak-free claim follows
from that run.

QuickJS red probes each retained 12,000 Event/TextEncoder records before task
completion (`/tmp/tenta-qjs-generic-materialization-red.log`). The corrected
library passed 30/30, and the complete backend package passed 171/171 with no
skips (`/tmp/tenta-qjs-generic-lib-green.log`,
`/tmp/tenta-qjs-generic-backend-green.log`). The ten added tests cover admission,
detail-reference multiplicity, exact failed-adoption cleanup, remint and
Document preservation, and sticky cleanup failure through teardown. IDL passed
67/67 and scoped all-target Clippy passed. Both packages received independent
Sol source review and root ownership/integration review. Matched release
performance and the QuickJS C+Rust sanitizer run remain pending; the earlier
receiver checkpoint does not contain these changes.

### V8 wrapper publication failure

Final source review found that ordinary wrapper creation marked Event and
linked Host records as wrapped before template/instance creation succeeded.
A failed first wrap then had no published weak finalizer to clear that flag.
Publication now sets Event, linked Host and node flags after V8 allocations
and the native-failure check, immediately before installing the host/weak
entries. It preserves admission accounting and failed-remint bookkeeping.
A deterministic missing-template probe failed on the Event flag before the
fix; afterward all 55 V8 library tests pass, including Event cleanup, linked
Signal owner cleanup and detached-node sweep controls. Both Sol reviewers
and root accepted the source. Logs:
`/tmp/tenta-v8-wrapper-publication-red.log` and
`/tmp/tenta-v8-wrapper-publication-green.log`.

## Standalone EventTarget and default passive listeners

Standalone targets use their exact generational HostRecord identity and the
existing listener table. Their graph group retains callbacks while the target
is reachable, and an Event retains its target through a directed edge; targets
do not retain previously dispatched events. Both collectors now include these
groups in the same transaction as node and CSS ownership. Linked targets such
as AbortSignal and XMLHttpRequest require their separate ownership work.

The four DOM event types with special passive defaults now use the target's
identity at registration. Absent and undefined options differ from explicit
false, and moving a body afterward does not change a registered listener.
The shared Document::body helper selects the first HTML body/frameset child
under an HTML html root, including XHTML documents; SVG lookalikes do not mask
the actual body. The getter and passive classification reuse that helper.

Shared standalone behavior tests reproduced 14/14 failures before the
constructor landed, and native body tests reproduced two failures before the
identity correction. After integration, 174/174 focused DOM and both-engine
library/ownership/passive tests pass with no skips. The first two compile
attempts exposed three exhaustive HostRecord matches requiring the new
EventTarget variant; these now correctly classify it as having no URL pairs
or linked-record wrapper flag. Evidence:
`/tmp/tenta-standalone-eventtarget-red.log`,
`/tmp/tenta-default-passive-body-red.log`, and
`/tmp/tenta-eventtarget-passive-focused-green3.log`.
Both Sol reviewers and root checked the graph, namespace and registration
semantics. The full DOM/V8/QuickJS run subsequently executed 1,297 tests:
1,292 passed and five failed; 18 explicitly ignored release-budget or
React/Vue gate probes were skipped. Two failures are the known V8 indexed-set
and Storage ownKeys defects; the other three concern detached listener cleanup,
XML element-name expectations and a throwing Response status getter.
Independent Sol/root review identified stale test expectations: task-end
collection already drains the old listener generation, XML tagName retains
case, and throwing dictionary getters propagate their original exception.
The corrected three integration binaries subsequently passed 51/51
(`/tmp/tenta-backend-fixture-triage-green.log`), preserving generation reuse,
listener dispatch and error-type assertions. No production behavior was
changed for those expectation updates. This aggregate is not green. The long randomized DOM membership
test passed in 99.1 seconds. Log:
`/tmp/tenta-eventtarget-passive-backends-dom.log`. The separate codegen-only
golden refresh passed 20/20, followed by 69/69 read-only default-feature IDL tests
(`/tmp/tenta-generic-event-target-idl-{golden-refresh,green}.log`). Fresh
release/sanitizer measurements remain pending.

## QuickJS constructor-call discrimination

The native QuickJS class callback now preserves `JS_CALL_FLAG_CONSTRUCTOR` in
its `Params`. Generated WebIDL interfaces use the opt-in
`Constructor::new_prototype_construct_only` API, which rejects ordinary calls
before argument conversion or reading the supplied receiver's prototype. The
existing callable `new_prototype` API keeps its prior behavior. Both Sol
reviewers and root accepted the source and ordering.

The backend regression failed before the change and passes afterward;
163/163 QuickJS Core library tests and 172/172 complete backend tests pass.
Coverage includes direct construction, derived `Reflect.construct`, a hostile
ordinary-call receiver, and the default permissive API. IDL passes 67/67 after
golden refresh. An initial command incorrectly refreshed goldens concurrently
with the CLI's golden-reader test and produced 66/67; the subsequent read-only
run passed all 67. Logs are `/tmp/tenta-qjs-construct-only-{red,core,idl,idl-green,backend-green}.log`.
Formatting, diff checks and scoped all-target Clippy pass; fresh release WPT
is still pending. The preserved receiver WPT driver predates this correction.

This does not establish full constructor conformance. Source review identified
the inherited non-object `new.target.prototype` fallback as a separate path
requiring behavior probes and correct realm/prototype selection. A subsequent
root review also found that dictionary conversions currently use declaration
order rather than lexicographic own-member order per inheritance level; a
separate conversion-order package is in progress. Outgoing dictionaries also
need own-data-property creation and omission of absent optional members;
changing conversion order alone does not establish output conformance.

The subsequent binding/UI integration below includes the single-realm
prototype fallback, dictionary conversion order and outgoing own-property/
optional-presence corrections. Its output converter preserves present `null`
and omits absent optional members. It also moves QuickJS constructor argument
conversion before `new.target.prototype` access, retaining the callback frame
and stored-value lease through both operations. Cross-realm intrinsic selection
is still unsupported; current page engines have one realm each.

## Generic materialization/EventTarget validation checkpoint

The integrated checkpoint passes `cargo fmt --all --check`, `git diff --check`,
and all-target Clippy with warnings denied for QuickJS Core, DOM, IDL, shared
JS and both engine crates. Clippy completed in 25m23s; macOS emitted malloc
stack-logging diagnostics, but Cargo exited zero and reported no Rust lint
failure. `cargo xtask lint-deps` validates 26 crates, and the IDL manifest is
up to date. `cargo deny check` passes its advisory, ban, license and source
policies; its output retains four unused-license-allowance warnings and ten
duplicate-version-family warnings. Logs use the prefix
`/tmp/tenta-generic-event-target-` with `clippy.log`, `lint-deps.log`,
`idl-manifest.log` and `deny.log`.

`bench/results/20260923-generic-event-target-release-source.json` freezes
945 input files for the same nine release targets as the receiver checkpoint,
including hashes of measurement drivers and validation scripts. The matched
release build completed successfully in 20m26s. All nine binaries are preserved
in the adjacent `20260923-generic-event-target-release-binaries.json` manifest;
all 945 source hashes were rechecked after the build and copies. The preserved
WPT driver has SHA-256
`1e95dc887ef9f808f99ccf0a59b150778e4993735bf9db1d5ee035849d7bb7e1`.
This is not yet evidence of WPT, sanitizer or budget success.
The standalone IDL run above used default features. The subsequent native
generated-binding run with both engine features passes 67/67 tests, including
the generated V8 compilation and panic-containment controls. QuickJS passes
109/109 focused tests with Rust and QuickJS C AddressSanitizer instrumentation;
both available QuickJS C objects expose ASAN symbols. Leak detection was
disabled, so this is not leak-detector evidence. The validation receipt is
`bench/results/20260923-generic-event-target-validation.json`; all 945 frozen
source hashes remained unchanged through these checks.

Six `20260923-S-*-generic-event-target.json` reports compare this checkpoint
against the preserved receiver binaries with 25 pairs and two process warmups.
DOM uses fixed AB order; listener and CSSOM use alternating AB/BA. Own Cargo
and native builds were idle, while an external TentaFlow compilation remained
active. V8 has/get p50/p95 changed from 21.29/31.54 and 74.19/80.64 to
21.92/26.38 and 73.93/95.55 ns; QuickJS changed from 160.42/188.84 and
219.02/274.73 to 173.01/201.44 and 225.82/255.50 ns. All DOM median changes
are below the 10% claim floor. These focused samples meet their method budgets;
they do not erase the earlier loaded QuickJS p95 miss or establish all P2
performance gates.

V8 listener GC p50 increased 25.2% for connected nodes and 18.2% for self-capture,
while cross-capture decreased 13.2%; CSSOM trim increased 30.1%
(1.327/2.999 to 1.725/4.108 ms p50/p95). These phase costs exceed the noise
floor and remain reported regressions. Whole-process listener/CSSOM CPU medians
changed 1.2%/2.2%, with RSS nearly flat. QuickJS listener/CSSOM phase, CPU and
RSS changes remain below 10%. Both engines retain/reclaim every expected
listener target and preserve CSSOM checksums in every measured sample. Sol
independently checked sample counts, binary identities and interpretation;
these are cumulative checkpoint costs, not isolated feature speed claims.

The fresh focused 18-file WPT diagnostic has identical results on both engines:
175 PASS, 12 FAIL and one NOTRUN; harnesses report 16 OK, one TIMEOUT and one
ERROR. All 100 default-passive assertions pass, but missing distinct HTML root
and body interface names produce duplicate test names and a harness error.
The failures require additional `createEvent` interfaces; the platform-object
listener fixture requires custom-element upgrade and callbacks and times out.
The V8 full-directory diagnostic reaches its ten-minute limit (603.5s), exits
3 and remains truncated: 202 files, 79 OK, 119 TIMEOUT, two ERROR and two NOTRUN;
453 recorded subtests contain 280 PASS, 144 FAIL, 16 TIMEOUT and 13 NOTRUN.
The 61.81% figure describes recorded subtests only. The second harness error
is scrollend-test cleanup timing out. QuickJS likewise exits 3 at 601.2s,
truncated: 202 files, 78 OK, 119 TIMEOUT, two ERROR and three NOTRUN;
485 recorded subtests contain 288 PASS, 150 FAIL, 34 TIMEOUT and 13 NOTRUN
(59.38% of recorded subtests). Both preserved reports and execution receipts
are in `wpt/results/checkpoints/20260923-generic-event-target-dom-events-*`.
Their differing recorded coverage prevents treating these percentages as a
controlled engine comparison. No expectations were promoted or P2 gate marked
complete.

## Binding conversion and UI event integration

Root integrated the Sol-reviewed stored callback-value lease, dictionary input
ordering/output-property semantics and QuickJS constructor prototype/order
packages, followed by UIEvent view/legacy initialization, CompositionEvent and
KeyboardEvent. Before production integration, 17 real-engine regression tests
ran: seven passed and ten failed on the intended callback retention, dictionary
getter order and prototype/DOMException behaviors. The red log is
`/tmp/tenta-binding-layered-regressions-red.log`.

The constructor API keeps generic QuickJS constructors permissive, while the
WebIDL opt-in rejects ordinary calls before conversion. DOMException defaults
missing or undefined arguments and converts message, name, then prototype;
`null` remains a string conversion. Root review removed obsolete fallback
plumbing and corrected native Event-count tests. The UI packages retain owned
keyboard/composition state across inherited initializers, ignore legacy
initialization while dispatching, and give synthetic HTMLElement clicks the
page view with detail zero. The click interface is still MouseEvent; PointerEvent
coverage and relatedTarget ownership remain separate gaps.

An isolated golden refresh passes 21/21 tests, and the read-only IDL suite with
both engine features passes 70/70, including actual outgoing dictionary
conversions. Formatting and diff checks passed before the two integration
corrections. Expanded engine/core/event/GC validation ran 650 tests: 645 passed,
five failed and none were skipped (`/tmp/tenta-bindings-ui-focused-retry2.log`).
Four failures expose constructor length zero instead of one for CompositionEvent
and KeyboardEvent on both engines; the fifth exposes QuickJS lone-surrogate
string conversion. Both fixes are now integrated after Sol/root review: shared
minimum constructor arity and a native QuickJS UTF-16 RAII view with a lossy Rust
text adapter. The exact UTF-16 view preserves unpaired code units; the generated
Rust String host representation still does not provide full DOMString fidelity. Three fallible string-getter
signatures and a missing test-only atom import were corrected first and received
Sol review. This section does not yet claim backend, sanitizer or performance
acceptance for the new integration. A read-only generator rerun passes 22/22
with the refreshed goldens. Its first run had one stale test counting every
`.length(0)` in a file; the corrected assertion checks each named iterator
initializer and received Sol review. The broader DOM/IDL/core/both-engine suite
completed on its third build attempt: 1,808 tests ran, 1,805 passed, three
failed, and 18 were skipped (`/tmp/tenta-bindings-ui-related-full-attempt3.log`).
Two failures are the already-open native V8 inherited-indexed-setter and
Storage key-order defects. The third is a stale Blob/File test expecting the
old incorrect `File.length` of zero; it now expects the correct two required
arguments, after Sol review. The separate constructor-function inheritance
gap remains explicit. The first build stopped at native `size_t`
conversion types in the UTF-16 helper; the second stopped at two new V8 test
calls missing the `PageEngine` trait qualification. Both corrections received
Sol review; neither failed build executed the runtime suite. Their separate
logs remain at `/tmp/tenta-bindings-ui-related-full-attempt{1,2}.log`.
The preceding preserved WPT driver and measurement reports
predate these changes.

Root's native-string review also found that the existing safe QuickJS
`CString` constructor accepted lone-surrogate WTF-8 while exposing unchecked
Rust `str` views. Its regression reproduced the unsafe admission without
constructing an invalid `str`: one test failed and the valid-Unicode control
passed (`/tmp/tenta-qjs-cstring-safety-red.log`). The Sol-reviewed fix validates
UTF-8 before exposing the RAII object and frees rejected buffers on error.
Focused core/V8/IDL validation passes 254/254, including the corrected Blob
test (`/tmp/tenta-cstring-blob-green-attempt2.log`). Its first invocation
selected features from an unselected package and stopped before compilation;
the second selected the owning IDL package explicitly. QuickJS C/Rust ASAN
passes 328/328 with no sanitizer diagnostic and leak detection disabled
(`/tmp/tenta-qjs-bindings-ui-related-asan.log`). Nextest marked one timeout
regression as a process-output leak; the focused repeat passes without that
marker (`/tmp/tenta-qjs-bindings-ui-related-asan-leak-control.log`). This is
not a claim about memory leaks. Both existing native QuickJS objects retain
their recorded hashes and 37 ASAN undefined symbols each. Formatting passes.
Workspace all-target Clippy initially rejected the long standard-defined
legacy initializer signatures; the Sol-reviewed generator fix emits a
method-local arity lint allowance only when the signature exceeds seven
parameters. The all-workspace, all-target Clippy rerun passes with warnings
denied in 17m42s (`/tmp/tenta-bindings-ui-related-clippy-attempt2.log`).

Final source review found two additional defects for the same pending
release checkpoint: outgoing collection values must define own data properties,
and stale cached 16-bit native keys must never alias a different record after
generation exhaustion. The unchanged engines reproduced seven failures among
eight focused tests, with the V8 pair-iterator control passing
(`/tmp/tenta-collection-generation-red-attempt1.log`). The Sol-reviewed
production corrections are now integrated: collection output defines own data
properties, and exhausted identity slots retire while reusable DOM payloads
remain recycled. The unused identity-resetting `SnapshotSlab::clear` API was
removed. The first generator refresh passes 22/22. The green run passes 7/8 focused
tests, and the C/Rust ASAN run passes 494/495 without a sanitizer diagnostic
(`/tmp/tenta-collection-generation-green-attempt1.log`,
`/tmp/tenta-qjs-collection-generation-asan-attempt1.log`). Both expose the same
additional QuickJS overload-selection bug: `FormData.append(name, string)`
selects the Blob overload with optional filename before reaching the iterator.
A shared arity dispatcher correction includes ignoring
unconsumed extra arguments per [WebIDL overload resolution](https://webidl.spec.whatwg.org/#es-overloads).
The Sol-reviewed correction is integrated, its golden refresh passes 22/22,
and the second C/Rust ASAN run passes 496/496 with zero skips and no sanitizer
diagnostic (`/tmp/tenta-qjs-collection-generation-asan-attempt2.log`).
The expanded DOM/IDL/core/both-engine run completes with 1,821 tests:
1,819 pass, two fail and 18 are skipped
(`/tmp/tenta-collection-generation-overload-full-attempt1.log`). The only
failures are the two open V8 native legacy-property controls. Compilation took
18m21s; runtime took 102.4s, including one slow DOM membership stress test.
A final Sol-reviewed QuickJS emission correction uses sequential pattern guards
to avoid an unreachable fallback for a future zero-required-argument overload
set. Its final focused run passes 32/32 with zero skips
(`/tmp/tenta-collection-overload-final-focused.log`), and the two affected
QuickJS iterator/overload controls pass again under C/Rust ASAN
(`/tmp/tenta-qjs-overload-final-focused-asan.log`). These two are repeated
controls, not additional unique tests beyond the earlier 496-test run.
Formatting and whitespace checks pass. Workspace Clippy with all targets and
warnings denied passes in 22m31s
(`/tmp/tenta-collection-overload-final-clippy-attempt1.log`).
The Storage benchmark also now measures
own-key enumeration and ordinary Symbol creation/deletion, with untimed
membership/descriptor checks, for the forthcoming native before/after comparison.
This remains limited to the normalized IDL signatures; full type-based overload
resolution is still outstanding. No new release/performance acceptance is claimed.

The combined release checkpoint is now built and preserved: 978 source files,
the same nine primary targets as the generic EventTarget baseline, and two
additional Storage examples with the same feature configuration. Both build
commands pass (16m01s plus 2m43s); source and validation-script hashes match
before/after preservation. The source manifest is
`bench/results/20260924-bindings-ui-related-release-source.json`, SHA-256
`2e55f14b9b28973a122ddbd09dd4876b5af4419e6f5f59815832e712f026bc27`.
Both Storage examples pass their untimed correctness checks. Their single-run
timings are smoke evidence, not an interleaved performance comparison.

Six 25-pair comparisons against the preserved generic EventTarget binaries
complete with compilers idle. V8 DOM, listener GC and CSS graph median changes
remain within 5%; no speedup is claimed below the 10% noise floor. QuickJS
`hasAttribute` rises from 149.14/152.67 to 169.45/182.14 ns (p50/p95, +13.6%
median), and `getAttribute` from 198.78/210.63 to 218.45/238.47 ns (+9.9%).
A further 25-pair run reverses subject order: normalized old/new medians are
152.34/173.73 ns for `hasAttribute` (+14.0%) and 205.90/223.52 ns for
`getAttribute` (+8.6%). RSS remains about 9.8 MiB. The reverse run has noisy
tails; the repeated median regression is retained for investigation, not
attributed to a particular correctness change without profiling. Both methods
remain within the separately specified 500 ns QuickJS budget. Results are
`bench/results/20260924-S-*-bindings-ui-related.json` plus the explicitly
reversed-label and normalized QuickJS reports.

The pinned 22-file event/UI WPT diagnostic subset produces the same result on
both engines: 231 PASS, 30 FAIL, one NOTRUN; 20 OK files, one TIMEOUT and one
ERROR. The preserved release driver ran each backend in about 10.1 seconds.
These reports under `wpt/results/checkpoints/20260924-bindings-ui-related-*`
do not establish a directory conformance gate; expectations are unchanged.

The separate patched V8 native build finished on 2026-09-23 at 19:59:21 UTC:
19/19 native legacy-property, key-order, GC and snapshot controls passed with
zero skips (`/tmp/tenta-v8-native-build/host-default-nextest-attempt12.log`).
The archive, matching generated bindings, exact source patch and toolchain
provenance are preserved under
`/tmp/tenta-v8-native-build/artifacts-attempt12/manifest.json`. This is the
macOS arm64 default configuration only. A separately resolved binding-only package now passes 46/46 tests through
actual Cargo linking: the same 19 native controls plus 27 build/guard tests
(`/tmp/tenta-v8-artifact-route-attempt1.json`). Its 71 source-file hashes were
verified before and after the run. Root review caught a Temporal dependency
mismatch before linking: the native archive used C API 0.2.3 while the workspace
selected 0.2.6. The tested local package pins 0.2.3 and verifies seven related
versions/checksums against the accepted native build lockfile. The separate
30-case guard probe covers corrupted bytes, bypass overrides, incompatible
configurations and independent final-link output directories. Live workspace
routing and the two previously failing browser-level controls remain
unvalidated; other platforms, pointer compression and native sanitizer
configurations remain open.

A separate incremental ordinary-object classifier build completed on
2026-09-24 at 06:00:16 UTC. The native command exited zero and all 23 tests
passed with zero skips: the previous 19 controls plus four classifier tests
covering ordinary prototypes, specialized/host objects, prototype tampering,
Proxy callbacks and cross-realm GC
(`/tmp/tenta-v8-ordinary-brand-stage/run-attempt1/nextest.log`). Ninja rebuilt
only `binding.o` and the archive. The orchestration script then rejected its
own evidence parsing because it did not handle nextest's `(n/23)` prefix;
separate reviewed postprocessing preserved the successful run and artifacts
without repeating compilation or tests
(`/tmp/tenta-v8-ordinary-brand-stage/run-attempt1/preserved-native23/receipt.json`).
This does not establish a V8 `structuredClone` browser implementation.

The combined native23 binding package subsequently passed actual artifact
linking: 51/51 tests, zero skips, in the same isolated Cargo target as the
earlier route (`/tmp/tenta-v8-native23-artifact-route-attempt2.json`). The first
attempt exposed a repeat-build defect: `fs::copy` could not overwrite a prior
read-only private artifact. The reviewed correction verifies a fresh sibling
copy before replacing the private output; a new repeated same/changed-bundle
control passes. The route verifies all 72 package source files and the three
native artifact hashes before and after execution. The workspace alias now
selects this local package, and its lockfile passes the separate seven-package
Temporal ABI validator. The first workspace integration run passed 149/149
tests, zero skips, including both previously failing V8 controls: inherited
setter behavior for readonly indexed properties and Storage own-key order before
Symbols (`/tmp/tenta-native23-workspace-focused-attempt1.log`; 19m55s build,
0.871s test run). The selected suite also covers QuickJS legacy/Storage behavior
and the 51 native/build-guard tests. The broader matching seven-package run
then passed 1885/1885 tests, with 18 existing skips (16m31s build, 72.324s run;
`/tmp/tenta-native23-workspace-regression-attempt1.log`). The randomized DOM
class-membership comparison took 72.088s and passed. Formatting, whitespace,
`lint-deps`, IDL-manifest and `cargo deny` checks pass. Full integrated workspace
Clippy and performance acceptance remain open.
Cargo reports the known manifest warning that build.rs also hosts its
build-script test target; the test command exited zero. The native attribution is retained
in both the manual notice template and regenerated workspace dependency inventory
(`cargo about generate --workspace --locked --fail about.hbs` passed; its manual
section is unchanged by regeneration). Other-platform native provisioning remains
open.

The matching release checkpoint built nine primary executables in 20m15s and
two Storage executables in 4m30s, with one Cargo job. All 1179 captured source
files were rechecked before preserving the binaries. The source manifest is
`bench/results/20260924-native23-workspace-release-source.json`
(`13cfa851477812cf580d7f1ad89fbcef33abf3c640a2c22a24143e00d835e5a0`);
the matching `release-binaries.json` and `storage-binaries.json` manifests
record the exact executables. The same pinned 22-file WPT diagnostic returned
231 PASS, 30 FAIL and 1 NOTRUN on each backend, unchanged from the preceding
checkpoint. Each backend has 20 OK files, one existing TIMEOUT and one existing
ERROR; durations were 10.2s V8 and 10.1s QuickJS. Raw results are
`wpt/results/checkpoints/20260924-native23-workspace-focused-{v8,quickjs}.json`.
This is a diagnostic comparison, not a P2 directory conformance gate.

All eight native23 release comparisons completed with 25 interleaved pairs per
scenario/backend, followed by an unchanged source/helper hash check. DOM calls,
listener collection, CSSOM ownership and five of the six Storage operations
remain inside the ±10% noise floor. V8 Storage Symbol creation/deletion regressed:
p50 rose from 50.37 to 100.99 ns, with the candidate slower in 24/25 pairs and a
paired median ratio of 1.994. Its p95 fell from 123.00 to 112.75 ns; that tail
change does not negate the p50 regression. QuickJS Storage p50 changes are within
±2%. Raw results are `bench/results/20260924-S-*-native23-workspace.json`;
`20260924-native23-workspace-host-context.json` records the 12m49s measurement
window and unrelated host load. Native performance acceptance remains open.
The expanded native baseline controls subsequently passed 28/28 tests with zero
skips. The same-artifact Symbol diagnostic uses separate marked, unmarked and
plain-object call sites; its three warmup configurations are diagnostic only,
not an acceptance benchmark. The focused StoreIC candidate subsequently passed
30/30 native controls against the helper-only native30 baseline. Its first build
was refused before compilation because the anticipated two-action plan omitted
the `mksnapshot` dependency. The reviewed retry validated the six-action dependency
plan, verified 3,731 invariant inputs, and preserved unchanged generated snapshot
sources and objects. The experimental archive is
`70311a835827c234c3866c400217d7634d054e5a2d46fe7bf61eb9c454d0bb40`;
`/tmp/tenta-v8-symbol-storeic-run/attempt2/receipt.json` records the result.
The first 25-pair interleaved Storage comparison reduced Symbol creation/deletion
p50/p95 from 99.764/154.553 to 54.748/78.931 ns; the candidate was faster in
24/25 pairs (paired median ratio 0.5523). RSS p50/p95 was
30,425,088/30,539,776 versus 30,359,552/30,441,472 bytes. The other five operation
p50 changes were within ±6%, but their p95 increased by 12–51% under unrelated
host compilation. These tail results require a repeat before acceptance.
`bench/results/20260924-S-storage-calls-v8-storeic.json` and the adjacent
`20260924-S-storage-storeic-host-context.json` preserve the samples, binary/build
identities, one-file source delta and observed host load. Both frozen builds
predate the registered-slow-callback panic correction below. The 25-pair QuickJS
control completed with all operation p50 changes between -3.81% and +2.89%; its
get/set tails also increased under the same changing host load. The QuickJS raw
report is `20260924-S-storage-calls-quickjs-storeic.json`. The second 25-pair V8
run (`20260924-S-storage-calls-v8-storeic-repeat.json`) reduced Symbol p50/p95
from 101.761/195.376 to 56.751/78.969 ns, with 25/25 paired wins and a paired
median ratio of 0.5544. Its other operation p50 changes stayed within ±6%, while
p95 increases of 36–89% remained unresolved under external compiler load. RSS
p50/p95 was 30,457,856/30,507,008 versus 30,392,320/30,457,856 bytes. Source
reconstruction subsequently matched all 35 candidate native inputs, and the
six source-preparation tests passed, including preservation of existing files,
directories and dangling symlinks before input work in both modes. The reviewed
optional source recipe is now installed; the native artifact remains unchanged.
This supports the bounded
Symbol median result, not a clean-tail claim or a closed performance gate.
Current-workspace candidate validation and artifact promotion remain pending;
live pins remain on the helper-only archive.

The separately staged QuickJS-first structured-clone package passed source
review after correcting reservation lifetime on error paths and the observable
transfer-sequence error behavior. Its isolated native C API suite builds with
Release/WERROR and exits zero, with unchanged copied source hashes
(`/tmp/tenta-qjs-clone-native-controls-attempt1/result.json`, staged package
manifest `32fdd10ff2d95b1db519d9518ba1245f604ad37cbe4bc969880c9570eed5a267`).
The direct shared-wire probe passes 14/14 using a documented standalone mutex
shim. Neither result establishes workspace Rust FFI or browser behavior: the
package was subsequently integrated with its V8 adapter, shared page ledger,
event surface, constructor inheritance and benchmark harnesses in a reviewed
65-file batch. The integration preserved before/after hashes and backups under
`/tmp/tenta-p2-integrated-batch-20260924/`. Updated IDL goldens pass 24/24 tests.
The first completed backend/browser run passed 131/132 selected tests (1579
outside the filter). The V8 structured-clone budget test failed because a DOM
write after the refused clone executed before queued termination was serviced;
the equivalent QuickJS control and both browser terminal-error controls passed.
An additive slow-callback native helper now sets V8's termination exception
synchronously, giving a sticky memory failure priority over an ordinary pending
page exception. Its native controls passed 30/30; the subsequent shared/V8/QJS
structured-clone library run passed 34/34, including direct checks that no
post-failure JavaScript, catch or finally block executed. Evidence is preserved
in `/tmp/tenta-v8-sync-termination-run/attempt1/receipt.json` and
`/tmp/tenta-p2-sync-clone-focused-attempt1.receipt.json`. The subsequent selected
native/backend/browser integration run passed 203/203 tests (1579 outside the
filter), recorded in `/tmp/tenta-p2-sync-broad-focused-attempt1.receipt.json`.
This is not a full workspace or WPT gate. The unrelated Storage Symbol
performance regression remains open. The Unicode benchmark is
integrated without its proposed QuickJS DOMString optimization, so a baseline
can be measured before that separate change.

Two additional slow-callback panic controls then reproduced same-task JavaScript
continuation after both a generated `document.body` getter panic and a handwritten
`fetch` panic. The selected run passed all 34 structured-clone controls, including
the new reentrant getter budget case, and failed those two panic controls.
`/tmp/tenta-p2-sync-failure-controls-attempt1.receipt.json` records this red result.
The correction adds an explicitly unsafe registered-slow-callback wrapper around
the existing safe containment core. Only after admission, unwind handling and
frame cleanup does it synchronously terminate a poisoned isolate. Generic direct
embedder calls, Fast API callbacks and GC/heap hooks retain their existing path.
Callback bodies are defined outside the narrow unsafe invocation. The resulting
focused run passed 81/81 tests (364 outside the filter), including both preceding
failures, structured clone, lease/panic cleanup and watchdog controls;
`/tmp/tenta-p2-slow-panic-green-attempt1.receipt.json` records the executed results.
Regenerated IDL goldens passed 24/24; only V8 outputs and the shared preludes
changed against the frozen baseline. The subsequent dual-engine IDL run passed
75/75 with no skips. Broader lint and performance checks remain separate gates.

The separately staged MutationObserver native harness passes 71/71 tests with
zero skips, using actual repository Document/core dependencies and the locked
dependency graph (`/tmp/tenta-observer-native-check-20260924-attempt4/nextest.log`).
This includes stale-node pin rollback and a real live-Range capacity refusal
after observer pins, with unchanged tree, range, queue and handle state. It is
native-stage evidence only: actual Document mutation hooks, charged semantic
capture, JS delivery, wrapper-graph publication and scheduler integration remain
under development. The later capture overlay was not part of this test snapshot.
Its separate real-workspace Document-hook snapshot initially stopped on three
denied unused-code warnings. After removing the unused import/method and naming
the lifetime-only reservation field explicitly, the second snapshot passed
156/156 core/DOM tests with zero skips. All 80 expected observer/core controls
were matched against executed test identifiers, including the charged capture
and mutation-hook controls. Evidence is preserved in
`/tmp/tenta-observer-dom-check-20260924-attempt2-receipt.json`. This does not
establish JS delivery or multi-world attachment/teardown.
The subsequent immutable registry snapshot passed 164/164 tests with zero skips,
with all 156 prior identifiers and eight new registry identifiers checked against
actual PASS records. The new controls cover retirement without losing later
nodes, retained pin charges, concurrent attachment and iterative destruction of
long chains. Evidence is in
`/tmp/tenta-observer-world-check-20260924-registry-nextest.receipt.json`.
The separate multi-world mutation fan-out overlay is not part of that result.
Its first snapshot stopped on an unused test-only helper under `-D warnings`.
Removing that unreferenced method allowed the second immutable snapshot to pass
171/171 tests with zero skips, preserving the exact 164 prior identifiers and
adding seven fan-out/root-binding/teardown controls. The snapshot covers native
all-world preparation, rollback and poisoning after effect/cleanup panics;
it does not install observers into JavaScript worlds. Evidence is in
`/tmp/tenta-observer-world-check-20260924-fanout-attempt2-nextest.receipt.json`.

The artifact-only V8 package no longer registers three absent native source
files as Cargo rebuild inputs. The same `cargo check --locked -p tenta-v8 -v`
command passed twice; the second run reported `Fresh tenta-v8`, finished in
0.66 seconds and invoked neither a build script nor rustc. This verifies Cargo
freshness, not runtime performance. The native artifact and its pins were
unchanged by this correction.

## MouseEvent relatedTarget ownership integration

The shared regression suite first ran against the existing engines: 16 tests,
eight passed and eight failed, with no skips. Both engines lost detached related
wrapper state or failed to preserve distinct dispatch-target/related-target
edges (`/tmp/tenta-related-target-red.log`). Root then integrated the independently
Sol-reviewed ownership patch and corrected a misplaced helper doc comment.

An Event now owns one stored reference to its exact converted related wrapper.
The existing collector combines dispatch target and owned-value edges into one
directed Event payload; this also accounts for CustomEvent detail references
without weakening primitive values. Legacy replacement releases the prior edge
outside the Event slab borrow. Exact failed-adoption reference counts and
QuickJS reentrant-GC conversion have private regression probes. The expanded
both-engine runtime suite passes 411/411 tests with no skips, including the
previously failing shared cases, exact failed-adoption refcounts, collector
cycles and wrapper pressure (`/tmp/tenta-related-target-green-attempt1.log`).
Dependency-direction lint (26 crates), the generated IDL manifest check and
cargo-deny all pass on this integration. The later combined QuickJS C/Rust
ASAN run above covers this package; release performance acceptance is pending. Native-fired related targets are not covered by this package.

## Active parser ownership measurement

`20260923-S-shared-parse-pins.json` records 25 alternating AB/BA pairs for
flat sibling elements, templates and deep input. Both release binaries were
built with `cargo build --release -p tenta-dom --example shared_parse`;
adjacent `20260923-parser-pin-{baseline,candidate}-{source,binary}.json`
manifests identify the binaries and 110 local source/dependency files,
rechecked after each build. Each process warms twice, then times eight
fresh parses including complete parser/document teardown. Exact node counts,
shared-document identity and release of parser ownership are checked.
The deep input has 1024 nested div tokens under the existing 512-level DOM
depth cap. No JavaScript or pressure collection runs in this workload.

Flat/template/deep parse p50/p95 changed from
4.109/4.851 → 4.722/6.167, 0.806/1.268 → 0.953/1.257 and
0.262/0.368 → 0.294/0.564 ms. Median costs are 14.9%, 18.2% and 12.3%,
above the claim noise floor. Whole-process CPU p50/p95 changed from
41.428/47.445 → 46.245/52.716, 10.409/12.117 → 11.943/14.393 and
5.175/6.867 → 5.884/7.433 ms. RSS p50/p95 changed from
8.984/10.000 → 9.734/11.125, 4.219/4.234 → 4.703/5.031 and
3.359/3.438 → 3.578/3.703 MiB. These are measured costs of retaining
parser-owned raw node handles safely; they do not establish the separate
1 MB HTML parse budget or a full-page budget gate.

## Observer foundation and Crypto integration checkpoints

The staged Observer foundation now passes 185 native library tests, 306
shared/V8/QuickJS library tests and 232 selected DOM/lifetime integration tests,
with no skips. The two original shared-document lifecycle failures pass on both
engines without changing their assertions. Generation-checked wrapper claims
preserve the surviving world's nodes; root/Sol review removed the superseded,
unused subtree helper. These results do not establish JavaScript observer
delivery, which remains a separate package.

The first 25-pair performance comparison initially blocked promotion. With no compiler
processes recorded before or after any scenario, attached-but-empty observer
worlds add about 20.8 ms per 40,960 mutations on both engines. Native parser
overhead also exceeds the noise floor in the flat and chunked-text cases.

| Scenario | Baseline p50 / p95 (ms) | Candidate p50 / p95 (ms) |
| --- | --- | --- |
| V8 detached mutations | 29.926 / 32.288 | 50.712 / 55.934 |
| V8 connected mutations | 30.245 / 31.056 | 50.843 / 54.415 |
| QuickJS detached mutations | 46.930 / 49.603 | 67.796 / 72.136 |
| QuickJS connected mutations | 44.614 / 45.487 | 65.568 / 68.008 |
| Flat HTML parse | 2.958 / 3.056 | 3.275 / 3.341 |
| Chunked text parse | 0.127 / 0.132 | 0.187 / 0.190 |

The three `bench/results/20260924-*-observer-foundation.json` reports preserve
all samples, p50/p95, CPU, RSS and binary/source identities; the adjacent
`20260924-observer-foundation-host-context.json` records build identities and
host load. V8 RSS p50/p95 is 28,901,376/28,966,912 versus
29,032,448/29,081,600 bytes; QuickJS is 11,272,192/11,321,344 versus
11,501,568/11,567,104 bytes. Empty-work routing is under correction, without
relaxing lifetime, sticky-failure or all-world admission checks.

The separate callback-scoped buffer-reference prerequisite passed 36 generated
dual-engine binding tests, 298 production library tests and scoped all-targets
Clippy. The combined Observer/Crypto source composition passed independent Sol
review; its refreshed IDL goldens pass 27/27, and the subsequent dual-backend
IDL suite passes 81/81. The first combined native/backend/Crypto run passed
511 of 521 tests; all ten failures were V8 API fixtures using a script-only
engine without a Window. Changing those fixtures to the page constructor,
without weakening assertions, produced a separate 11/11 V8 Crypto pass.
QuickJS API, GC and post-write watchdog controls already passed in the earlier
combined run. This is not a fresh aggregate 521-test pass. Later combined
execution and the subsequent lint/performance checks are recorded below.

The empty-observer routing correction passes 206 native tests, 306 backend
library tests and 232 selected DOM/lifetime integration tests. One native
nextest process-output LEAK classification did not recur in the isolated
rerun; this is not an allocator or sanitizer leak measurement. Its second
25-pair release comparison also held promotion during investigation. No compiler processes were
recorded before or after the scenarios. Flat/template/deep parser median changes
are +0.7%/+1.2%/+1.4%, inside the noise floor; chunked text remains +29.0%.
V8 detached/connected mutation medians remain +32.8%/+33.5% and QuickJS
+31.9%/+35.7% relative to the preserved pre-observer baseline.

| Empty-world correction scenario | Baseline p50 / p95 (ms) | Candidate p50 / p95 (ms) |
| --- | --- | --- |
| V8 detached mutations | 31.480 / 32.493 | 41.807 / 46.003 |
| V8 connected mutations | 31.117 / 32.398 | 41.530 / 43.147 |
| QuickJS detached mutations | 48.144 / 52.758 | 63.496 / 69.878 |
| QuickJS connected mutations | 45.296 / 49.233 | 61.487 / 63.495 |
| Chunked text parse | 0.131 / 0.137 | 0.169 / 0.172 |

The `bench/results/20260924-*-observer-empty-fastpath.json` reports retain
all samples and workload/binary identities; the adjacent host-context report
retains source/build identities and load readings. V8 RSS p50/p95 is
28,901,376/28,966,912 versus 29,032,448/29,114,368 bytes; QuickJS is
11,272,192/11,321,344 versus 11,436,032/11,485,184 bytes. Further correction
followed, preserving lifetime and native-failure behavior.

A profile-guided refinement removes duplicate attribute lookups and restores
borrowed parser text coalescing before any observer world attaches. It passes
209 native tests and 232 selected dual-backend DOM/lifetime tests without skips.
The third 25-pair comparison removes the measured parser regression: all four
parser median changes are within 0.8%, inside the noise floor. Mutation overhead
remains measurable: V8 detached/connected medians are +15.9%/+14.6%, and
QuickJS +10.7%/+10.7%. No compiler processes were recorded before or after the
scenarios; source and binary identities were checked around execution.

| Profile-guided refinement scenario | Baseline p50 / p95 (ms) | Candidate p50 / p95 (ms) |
| --- | --- | --- |
| V8 detached mutations | 30.749 / 31.548 | 35.631 / 38.951 |
| V8 connected mutations | 31.293 / 33.751 | 35.852 / 37.385 |
| QuickJS detached mutations | 47.872 / 50.536 | 52.985 / 55.139 |
| QuickJS connected mutations | 45.996 / 46.779 | 50.911 / 51.865 |
| Chunked text parse | 0.131 / 0.135 | 0.131 / 0.136 |

The `bench/results/20260924-*-observer-profiled-refinement.json` reports and
adjacent host-context report retain the raw samples and provenance. V8 RSS
p50/p95 is 28,901,376/28,966,912 versus 29,048,832/29,114,368 bytes; QuickJS
is 11,288,576/11,321,344 versus 11,468,800/11,550,720 bytes. This candidate
contains the observer foundation and its performance corrections; it is not
a measurement of the later combined Crypto/facade/replace-all composition.
The earlier promotion holds were engineering checkpoints during investigation,
not an additional spec budget: `13-performance.md` §8 defines ±10% as the
claim noise floor and fails regressions above a declared budget. The diagnostic
mutation-hooks scenario has no declared numeric budget; P2's actual
`S-dom-calls` budget is validated on the combined source below.

The native host facade, grouped replace-all operation and callback conversion
packages now compose with the observer performance corrections. The fresh
combined native/backend/Crypto run passes 560/560 tests with no skips after
the reviewed redundant-check removal and host-health prerequisite. The prior
558-test composition also passes workspace formatting. This includes the previously failing 11 native
observer-agent teardown tests, whose assertions remain unchanged. Agent
attachment now claims the exact Document ID; wrong-Document teardown still
refuses, while correct repeated teardown releases pins and charges. Native
graph controls and both engines' Crypto API/hard-stop budget tests pass in this
same run. The combined DOM/lifetime integration suite also passes 232/232
with no skips. Combined release measurements follow below. Formatting,
dependency direction, IDL manifest and cargo-deny checks pass; Clippy found
20 source-style issues in the observer package. Their reviewed mechanical
corrections are included in source `02755dc9…`; formatting passes again and
the full all-target Clippy rerun reached one additional test-only capacity
assertion lint. The equivalent `capacity() > initial_capacity` correction is
reviewed and present in both the working tree and source `4aae8c8c…`;
formatting and full workspace/all-target Clippy pass with warnings denied
(18m58s). The Clippy receipt is
`docs/dev/evidence/20260924-observer-crypto-clippy-pass.json`.

The combined release source (`54972ae6…`) passes the actual `S-dom-calls`
mean-nanoseconds-per-call budgets. Each engine uses 25 interleaved measured
pairs after two process warmups, with one million calls per operation after
10,000 JavaScript warmups. No compiler ran during measurement. The comparison
baseline is the preserved native23 workspace binary identified in the prior
report; this run verifies that binary identity without asserting its full
source inventory.

| Combined S-dom-calls operation | Mean ns/call | p50 / p95 ns/call | Budget ns/call |
| --- | --- | --- | --- |
| V8 hasAttribute | 18.767 | 18.786 / 19.262 | 60 |
| V8 getAttribute | 72.002 | 71.987 / 74.002 | 250 |
| QuickJS hasAttribute | 175.236 | 174.996 / 181.687 | 500 |
| QuickJS getAttribute | 227.622 | 226.305 / 236.823 | 500 |

The `bench/results/20260924-S-dom-calls-*-observer-crypto-combined.json`
reports retain every sample; the adjacent budget receipt records all four
mean comparisons. V8 RSS p50/p95 changes from 28,655,616/28,770,304 to
29,147,136/29,196,288 bytes; QuickJS from 10,256,384/10,289,152 to
10,534,912/10,567,680 bytes. All median operation deltas versus these prior
binaries lie within ±10%, so no speedup/regression claim follows from them.

The separate combined mutation-hooks comparison retains the observable
empty-world cost against the original pre-observer binaries:

| Combined mutation diagnostic | Baseline p50 / p95 (ms) | Candidate p50 / p95 (ms) |
| --- | --- | --- |
| V8 detached | 31.028 / 34.937 | 35.275 / 37.786 |
| V8 connected | 31.317 / 33.597 | 35.786 / 37.622 |
| QuickJS detached | 48.328 / 51.415 | 52.971 / 53.341 |
| QuickJS connected | 46.174 / 46.795 | 51.250 / 52.265 |

V8 mutation medians are +13.7%/+14.3%, QuickJS +9.6%/+11.0%; the detached
QuickJS delta falls inside the noise floor. All four parser median changes
are within 1.9%. Mutation-process RSS p50/p95 is 28,884,992/28,966,912 versus
29,147,136/29,229,056 bytes on V8 and 11,288,576/11,321,344 versus
11,468,800/11,534,336 bytes on QuickJS. The corresponding mutation, parser
and host-context `*-observer-crypto-combined.json` reports preserve the
25-pair measurements. The source inventory and verified five-example release
build receipt are retained under `docs/dev/evidence/`. These results cover
the native foundation/Crypto/facade composition; they do not measure the
later shared observer wrapper transactions, JS delivery or full P2 gates.

The reviewed 107-path composition is now integrated into the working tree.
The promotion receipt and complete promoted source inventory are retained as
`20260924-observer-crypto-live-promotion.json` and
`20260924-observer-crypto-promoted-source-manifest.json` under
`docs/dev/evidence/`. Promotion verified every prior destination hash, kept
backups and preserved unrelated changes. The broad workspace test run after
the mechanical Clippy corrections is still pending. This integration does not
expose JavaScript MutationObserver constructors or delivery.

QuickJS callback-function and callback-interface conversion now have separate
callability/object gates, with the annotated nullable setter exception.
Callback-interface union selection follows callable, iterable and native-brand
arms on both backends. Their combined WebIDL run passes 87/87 after the root
removed an unused fixture import and changed two invalid anonymous function
statement inputs to function expressions; all conversion assertions remain.
These are focused binding results, not full WebIDL or P2 conformance.

## JS1: native MutationObserver and focus (P3 package)

`MutationObserver`/`MutationRecord` and the focus model are native on the V8
arm; the CDP page shim lost its polling `MutationObserver` and its focus
bookkeeping. Design and file list: `docs/dev/p3-plan.md` ("As built (JS1)") and
`docs/dev/idl.md`.

- **MutationObserver.** Host records over the existing `tenta-dom` observer
  foundation: options validation with TypeErrors, `observe`/`disconnect`/
  `takeRecords`, records with `type`, `target`, static `addedNodes`/
  `removedNodes`, siblings, `attributeName`/`attributeNamespace`, `oldValue`,
  transient registrations for removed subtrees, and delivery in one V8
  microtask queued right after the mutating binding call returns (so it runs
  before promise reactions queued later). Native mutations (parser, CDP input)
  are queued by the engine's next checkpoint. Pending observers are notified in
  creation order and each takes its records just before its own callback, so a
  mutation made by an earlier callback lands in a later observer's single batch;
  every observer gets one complete batch per notification (10,000 records in one
  call, `takeRecords()` returns all of them) because `MutationRecord` wrappers
  are exempt from the world's per-checkpoint wrapper bound (their records are
  already charged to the page budget). The notification microtask is queued only
  once some observer has a pending record, as browsers do (the native layer still
  requests it for every mutation). An observer keeps itself alive while it
  observes, so an observer whose targets became unreachable lives, with its
  pinned targets (about 2.6 KB each, measured), until it is disconnected or the
  world ends: the collector does not walk `ObserverAgent::visit_gc_edges` yet.
  A page may keep at most 10,000 observing observers; `observe()` past that
  throws `QuotaExceededError`, which is the warning the page and the CDP
  console see. Backlog: put each observer and its callback into its targets'
  ephemeron group (`cycles.rs`) and subtract the observer pin with
  `conditional_handle_subtraction`, so the cap and `self_root` can go.   ends. Not done: slot-change records, `normalize()`/`splitText()` (the
  bindings do not exist; they are the prerequisite of the Range mutators),
  `Range.deleteContents/extractContents/insertNode/surroundContents`, the
  QuickJS arm (generated and installed, never compiled here because `rquickjs`
  is unbuildable; delivery is refused by `ScriptHost`'s default), and
  `ParentNode.replaceChildren` of an existing child, which queues two records
  where the standard queues one.
- **Focus.** `HTMLElement.focus()/blur()/tabIndex`, `Document.hasFocus()`,
  `document.activeElement`, trusted `focus`/`blur`/`focusin`/`focusout` with
  `relatedTarget`, and `:focus`/`:focus-within`/`:focus-visible` in both selector
  engines (screenshots show focused styles). `autofocus` focuses the first
  focusable candidate once, when `DOMContentLoaded` has fired. Not done: the
  standard's per-insertion candidate queue, iframes, shadow trees, sequential focus navigation (Tab order stays in the CDP shim),
  the window's own focus events.
  `inert` (on the element or an ancestor) and `hidden` refuse focus. Elements
  with `display: none` (self or ancestor) or `visibility: hidden` are refused
  through `ScriptHost::computed_style_value`, which asks the geometry host
  (a page at the render tier has the production host since CSS1 below; `<area>`
  is exempt from the refusal; below it a page relies on `hidden`/`inert`). Removing the focused element fires `blur` and
  `focusout` with a null `relatedTarget` once the removing binding call returns.

- **Also changed.** `outerHTML` now queues one replace record instead of an insert
  and a remove. The V8 callback boundary gained `V8CallbackAdmission::
  after_callback` (default no-op); the three prelude goldens were re-blessed and
  differ only by that hook. `ObserverPins` lost an O(entries) document scan per
  queued record (see the measurement below).

Evidence (2026-10-10, V8, this working tree; the baseline binary was built from
HEAD d459038 with no JS1 changes):

| Check | Before | After |
| --- | --- | --- |
| WPT `dom/nodes` (tentabrowser, 4 jobs) | 3,649/6,090 subtests (59.92%), 253/334 files OK | 3,765/6,098 (61.74%), 254/334 files OK; no file lost a passing subtest |
| WPT `dom/nodes/MutationObserver-*.html` (11 files) | 11/135 subtests | 118/138 subtests (the rest: Range mutators, `normalize`, parser mutation timing, a cross-realm timeout) |
| WPT `html/interaction/focus` | 9/598 subtests (1.51%), 179/197 files OK | 62/599 (10.35%), 180/197 files OK (most of the rest is `focusgroup`, iframes and `window.open`) |
| Unit and integration tests | -- | `tenta-js-v8`, `tenta-js`, `tenta-idl`, `tenta-css`, `tenta-dom` 1,910 passed; `tenta-page`, `tenta-browser`, `tenta-cdp` (`v8,render`) 539 passed (one earlier run timed out `image_events::a_thousand_image_events_are_all_delivered` while another build shared the machine; it passed alone and in the rerun) |
| Puppeteer smoke (`tests/cdp-clients/smoke`, `tentabrowser cdp`) | -- | all 14 steps OK, `waitForSelector` served by the native observer |

Release mutation loop (`tenta-js-v8/tests/perf_mutation_observer.rs`, 15
interleaved rounds, 10,000 nodes built and 20,000 attribute writes, p50/p95):

| Variant | mutate | deliver |
| --- | --- | --- |
| no observer | 59.6 / 85.8 ms | 0.8 ms |
| idle observer (detached target) | 87.2 / 138.6 ms | 0.6 ms |
| observer on the subtree, callback counts records (30,000 records) | 90.3 / 125.8 ms | 200 / 275 ms |

The first measurement of the last row was 609 ms to mutate: the pin ledger
compared every pinned node's document with the mutated one for each queued
record, which made queueing N records O(N^2); a stored owner document made it
constant time (6.8x faster). The remaining recording cost is about 1 us per
recorded mutation over the no-observer loop. The idle row is a 46% increase
(0.9 us per mutation) from the registered-world mutation path that already
existed; it is above the 10% noise floor and was not investigated further.
Delivery was measured before records were exempted from the wrapper bound,
when wrapper-pressure collections dominated it; it has not been re-measured.
`bench/` has no
in-process scenario for this; the figures come from the test above.

## CSS1: stylesheets in the page runtime, `getComputedStyle` and script geometry (P3 package)

Details, files and numbers are in `docs/dev/p3-plan.md` ("As built (CSS1)").

- **Stylesheets load with the document** (render-tier ceiling, feature `render`): `<link
  rel=stylesheet>` and `@import` chains are fetched when they enter the document, in parallel, through
  `tenta-net` with the page's request site, so the D4b hub reports `Stylesheet` requests during the
  load. A parser-inserted sheet holds back the scripts after it; any non-alternate sheet delays
  `load` and `networkIdle`; `<link>` gets `load`/`error`; the fetched sheets are the cache the
  screenshot and the geometry host use (a capture of a loaded page sends no request).
- **`window.getComputedStyle(el, pseudo)`** (V8): a live read-only `CSSStyleProperties` of Stylo's
  computed values with Chromium's used-value fixups; **element geometry** (`getBoundingClientRect`,
  `getClientRects`, `offset*`, `client*`, `scroll*`, `elementFromPoint`, `DOMRect*`) from the page's
  layout, lazily and invalidated by the document generation. 44 recorded Chrome pages (943 values)
  agree except two listed divergences. **`document.styleSheets`, `link.sheet`, `style.sheet`** reflect
  what loaded (read side only).
- **Measured** (release, `bench/css_load.py`, 20 ms RTT): 10 sheets, load + screenshot 294 -> 93 ms
  p50; `load` itself 36 -> 88 ms. WPT `css/cssom` 63.5% -> 73.9% (0 regressions), `css/cssom-view`
  28.7% (first run).
- **Gaps:** edits to a document sheet (`insertRule`, `deleteRule`) are not applied to layout (S1);
  QuickJS has neither `getComputedStyle` nor an executed arm; no scrolling model; `IntersectionObserver`
  is still the CDP shim's one-shot stub; no early fetch from the preload scanner.

## LOC1: `window.location` read side, `atob` and `btoa` (P2 package)

Chosen from a run of the release CLI against eight live sites on 2026-11-16
(`screenshot` and `fetch --eval`): screenshots were written, but page scripts
failed first on `location` (`location is not defined`, reads of `search`,
`pathname`, `href`, `host` on undefined) and on `btoa`. After this package the
same evaluation completes on the seven sites that answered 200; the remaining
script failures name `window.matchMedia`, `history`, and on one site
`document.currentScript`. That run is a diagnostic, not a conformance or
golden-site result.

A `typeof` probe of a V8 page after this package still reports `undefined`
for, among others: `document.links`/`images`/`forms`/`scripts`,
`document.currentScript`, `document.visibilityState`/`hidden`,
`element.innerText`, `element.dataset`, `element.hidden`; and on the window
`history`, `matchMedia`, `screen`, `requestIdleCallback`, `scrollTo`,
`postMessage`, `MessageChannel`, `customElements`, `getSelection`,
`IntersectionObserver`, `ResizeObserver`, `ReadableStream`, `FileReader`,
`WebSocket`, `Worker`, `indexedDB`, `isSecureContext`, `origin`, and the
`Window`, `Navigator`, `History`, `Screen` and `Performance` interface objects.

- **Location.** `tenta_js::location::LocationPart` holds the URL Standard's
  nine component getters; the `URL` interface reads through the same function.
  `HostObject::location_part` answers from the document's current URL, or
  `about:blank` for a document without one, on every read. V8 installs a
  `Location` interface object (illegal constructor, `Symbol.toStringTag`) and
  one instance whose attributes and `toString` are own, non-configurable
  properties with V8 signature checks; `window.location` is a non-configurable
  accessor on the global. QuickJS installs one plain object with the same own
  properties as a non-writable, non-configurable data property, without the
  interface object or receiver checks.
- **Not built, and absent instead of inert:** the attribute setters, `assign`,
  `replace`, `reload`, `ancestorOrigins`, `document.location`, and everything
  of `History`. A sloppy assignment to `location` or `location.href` is
  ignored and a strict one throws `TypeError`; neither navigates. Their
  semantics belong to the session-history design the manifest points at
  (`idl/MANIFEST.toml`, `History`).
- **`atob`/`btoa`.** `tenta_js::base64`: forgiving-base64 decode and Latin-1
  encode; a refusal is `InvalidCharacterError` on both engines.
- **Checks:** 7 unit tests (`tenta-js`), 11 V8 engine tests
  (`tenta-js-v8::dom_location`), 5 QuickJS engine tests
  (`tenta-js-qjs::location`), and a page test per engine through the fixture
  server in which an inline script reads `location` during the parse
  (`tenta-browser::location_js`).
- **Measured** (release, `bench/js_xhr.py`, `S-js-xhr`, 25 interleaved runs,
  Fedora 43 x86_64, both executables on the same filesystem; the baseline is
  HEAD abe95a2 with the updated lockfile and the local-pin guard): navigation
  p50/p95 16.09/16.84 ms before and 16.58/17.60 ms after, whole-process peak
  RSS p50 29.1 and 29.4 MiB. Both differences are inside the 10% noise floor.
  A first comparison with the candidate on disk and the baseline on tmpfs
  showed 2.2 MiB more RSS; one executable with the new members switched off by
  an environment variable showed none, so that figure measured the filesystem.

## MC3a: MCP reading, history, log and search tools (P4 package)

Driving `tentabrowser mcp` over stdio against live sites on 2026-11-16 showed
the MC2 tool set working (navigate, snapshot, click, fill, key, screenshot on
Hacker News, Wikipedia, DuckDuckGo, GitHub, MDN, BBC) and an agent missing the
means to read a page as text, to go back, to see why a page is empty, and to
search. `docs/spec/10-mcp.md` is not on this workstation, so the names and
arguments below follow the limitations list of `docs/dev/mcp.md` and have to be
reconciled with spec 10 §2; `SCHEMA_VERSION` is 1.2.0.

- **`browser_extract`** (`tools/extract.rs`): `text`, `markdown`, `links`,
  `html`, `metadata`, `forms` from the extractors of `tenta-dom`; `main_content`,
  `ref`/`selector`, `max_chars` with a continuation token over the stored text;
  fenced as untrusted page text.
- **`browser_back`, `browser_forward`, `browser_reload`** (`tools/history.rs`):
  a host-side list of the URLs the session's page has shown (`session::History`),
  fed by `browser_navigate` and by the navigations actions cause. Entries are
  loaded again from their URL; the list moves only when the load succeeds.
- **`browser_console_messages`, `browser_network_requests`** (`tools/log.rs`):
  a task per page moves `PageEvent`s into a bounded `session::PageLog` (500
  lines, 1000 requests) that starts over at each main-frame navigation.
- **`browser_web_search`** (`tools/search.rs`): `tenta-mcp` defines `WebSearch`;
  `tentabrowser mcp` implements it over `EngineSearch` on a worker thread (a
  search future is not `Send`). `mcp --search-endpoint PLAN=URL` (hidden) replays
  a recorded SERP.
- **Checks:** `tenta-mcp` 91 tests with `v8,render` (new: `tests/reading.rs`,
  a console test in `tests/tools.rs`, unit tests of chunking and filtering);
  `tentabrowser-cli` 90 tests with `full` (new: search over stdio against the
  recorded DuckDuckGo SERP). The tool list golden was regenerated.
- **Found on the way, not fixed:** only the document request reaches
  `PageEvent::Request` on the sites tried (subresource requests are missing
  from the list); the engines compile page scripts without a resource name, so
  an exception has no `url:line`; `extract::main_content` picks the reference
  list instead of the article on a Wikipedia article.

## HS1: `window.history` (P2 package, V8)

A React application (TodoMVC) rendered nothing: its router read
`history.state` of an undefined `history`. `docs/spec/` is not on this
workstation, so the package follows the HTML Standard for what one document can
do alone and leaves cross-document traversal out.

- **Built:** the `History` interface object and `window.history` with
  `length`, `state`, `scrollRestoration`, `pushState`, `replaceState`, `go`,
  `back`, `forward` (`crates/tenta-js-v8/src/host/history.rs`). `pushState` and
  `replaceState` clone the data with the realm's `structuredClone`, resolve the
  URL against the base URL, refuse one the document cannot be rewritten to with
  `SecurityError` (`tenta_js::history`), and rewrite the document's URL, which
  `location` and `document.URL` report. `go`, `back` and `forward` queue a task
  that moves among the document's own entries, restores the URL and fires
  `popstate`.
- **Deviations:** `popstate` is a plain `Event` with an own `state` property
  (no `PopStateEvent`); `hashchange` is not fired; an entry keeps the clone
  made when it was pushed; a step that would leave the document and `go(0)` do
  nothing; a new document starts a new history.
- **Checks:** 3 unit tests (`tenta-js::history`), 11 engine tests
  (`tenta-js-v8::dom_history`), stable under `--stress-count 20`.
- **Open:** QuickJS; navigation through `history` and `location`; the host's
  page URL (`Navigation::final_url`, which the MCP tools print) does not follow
  `pushState`.

## EH1: event handler attributes (P2 package)

No element had an event handler IDL attribute (`element.onclick` was an
expando that never ran) and no handler content attribute was compiled, so
`<button onclick="...">` did nothing and React's event-support probe
(`setAttribute('oninput', 'return;')`, then `typeof element.oninput`) sent it
down a legacy path that threw.

- **IDL attributes (both engines, generated):** a `GlobalEventHandlers` mixin
  in `idl/dom/dom.webidl` (the HTML Standard's list plus the CSS animation,
  transition and pointer handlers; no touch handlers), included by
  `HTMLElement`, `SVGElement` and `Document`. The implementations are two lines
  each over the existing `HostObject::event_handler`/`set_event_handler`
  (`event_handler_attributes!`). `onload`/`onerror` of images, links and style
  elements are now these members.
- **Content attributes (V8):** `dom::events::sync_attribute_handler` compiles
  an element's `on<type>` attribute when a dispatch is about to walk a path
  that holds the element, and when script reads the IDL attribute; a changed
  attribute replaces the handler, a removed one takes its handler away, and an
  assignment from script stands until the attribute changes. The function is
  compiled by `ScriptHost::compile_event_handler` (V8:
  `ScriptCompiler::CompileFunction` with the document, the form owner and the
  element as context extensions; QuickJS: not implemented, the handler stays
  null). A body that does not parse is reported and leaves the handler null.
- **Window (V8):** every global handler and the `WindowEventHandlers` set are
  accessors on the global (`window.onpopstate`, `onresize`, ...), and the
  attributes of the Window-reflecting set on `<body>` (`onload`, `onresize`,
  `onhashchange`, ...) set the window's handler.
- **Deviations:** writing an attribute's current value again does not displace
  a handler assigned after the first write; `onerror` has the plain
  `EventHandler` signature everywhere; `body.onload` (the IDL attribute on the
  element) does not forward to the window; `<frameset>` is not handled.
- **Checks:** 23 engine tests (`tenta-js-v8::dom_event_handlers`); the three
  JS crates pass 1654 tests.
- **Measured:** see "RC1" below, which measures this package together with
  HS1, NAV1 and FM1. The dispatch path gained a table lookup per dispatch and
  an attribute lookup per element on the path for event types that have a
  handler; no event-dispatch scenario exists to time that path alone.

## NAV1: navigation started by the page (P2 package)

A page could not navigate itself: `location` had no write side, a POST form
did nothing (`interaction.js` could only report a GET URL to its host), and
`form.submit()` and `a.click()` did not exist as navigations. An agent could
not log in anywhere.

- **The request.** Script cannot replace its own document, so it leaves a
  `tenta_dom::NavigationRequest` (URL, optional `PostData`, `replace`) on the
  document, the one object the script world and the page both hold
  (`Document::request_navigation`); only the page's own document (slot 0) may
  ask. `PageRuntime` takes it after each engine entry
  (`forward_navigation_request`) and hands it to the page through
  `ScriptBudget::request_navigation`; `PageInner` (which now holds a weak
  reference to itself) starts an ordinary navigation nobody awaits
  (`start_requested_navigation`), counted as in flight from that moment so a
  host that asks right after the script entry sees it. Only `http`/`https`
  targets are followed. The navigation supersedes one still loading, so a
  `goto` to a page that redirects in script ends with an aborted error and
  the page shows the redirect's target.
- **POST.** `Goto::post(content_type, body)` and `Goto::initiator(url)`;
  `build_request` sends the body and lets `tenta-net` derive `Origin` and
  `Referer` from the initiator. `PageEvent::Request` reports the method.
- **`location` (V8).** Setters for every component but `origin`, `assign`,
  `replace`, `reload`, and `window.location = url`. `LocationPart::write`
  applies the URL Standard's setters (`url::quirks`).
- **Fragment navigation.** A target that differs from the document's URL in
  its fragment at most stays in the document: `HostObject::navigate_to`
  rewrites the URL and `ScriptHost::fragment_navigated` (V8) adds the history
  entry and queues `hashchange` (a plain `Event` with `oldURL`/`newURL`).
- **Forms.** `tenta_dom::forms::submission` builds the request: the entry list
  (successful controls in tree order, live values, selectedness, the
  submitter, `form=` association, disabled fieldsets), encoded as
  `application/x-www-form-urlencoded`, `multipart/form-data` or `text/plain`;
  GET replaces the action's query. `form.submit()`, `form.requestSubmit()`
  (fires `submit`, cancellable) and the activation behaviour of
  `element.click()` (submit and reset buttons, links) use it.
- **Hosts.** `interaction.js` submits POST forms through `requestSubmit`,
  moves to a fragment through `location.assign` and keeps the caret in the
  engine's text selection. The MCP tools follow a navigation the page started
  (`tools/dom.rs::page_navigation`); see `docs/dev/mcp.md`.
- **Checks:** `tenta-dom::forms` 6 tests; `tenta-js-v8::dom_location` 16,
  `dom_forms` 10, `dom_history` 11; `tenta-browser::navigation_js` 5 (through
  the page loop and the fixture server, POST echo included); `tenta-mcp` 93;
  `tenta-cdp::interaction` (POST form test rewritten). A live run submitted
  `https://httpbin.org/forms/post` and read back the fields with `Origin`,
  `Referer` and `Sec-Fetch-Site: same-origin`.
- **Deviations and gaps:** no `beforeunload`/`unload`/`pagehide`; no
  `SubmitEvent` (`submit` is a plain `Event`); no constraint validation; no
  file upload (a file control submits the empty file); `target=_blank` and
  `download` links are not followed; activation behaviour runs for
  `element.click()` and the host's interaction script, not for a `click` event
  script dispatches itself; `<meta http-equiv=refresh>` is not followed;
  QuickJS has neither `location` writes nor `history`.

## FM1: form control interfaces, `innerText`, `hidden` (P2 package)

`HTMLSelectElement` and `HTMLOptionElement` had no members (`select.value`
was undefined), inputs had no `type`, `form` or text selection (React's
controlled inputs threw in `getSelection`), and `element.hidden`/`innerText`
were expandos.

- **State (`tenta_dom::forms`).** Option selectedness (`dirty_selectedness`
  plus the `selected` attributes; a single select has at most one selected
  option, a list box none by default), text selection per control (UTF-16
  offsets, clamped; the caret is at the end after `value` is set), control
  values and checkedness, form owner, disabledness. `extract::forms` reads
  the same state (a password field reports its attribute only, so a typed
  secret is never extracted).
- **IDL (`idl/dom/dom.webidl`, both engines).** `HTMLFormElement`: `method`,
  `enctype`, `encoding`, `target`, `noValidate`, `elements`, `length`,
  `submit`, `requestSubmit`, `reset`. `HTMLInputElement`: `type`,
  `defaultValue`, `defaultChecked`, `required`, `readOnly`, `multiple`,
  `autofocus`, `min`, `max`, `step`, `pattern`, `autocomplete`, `accept`, `alt`,
  `maxLength`, `minLength`, `form`, `selectionStart`, `selectionEnd`,
  `selectionDirection`, `setSelectionRange`, `select`. `HTMLTextAreaElement`:
  the same selection members, `type`, `defaultValue`, `required`, `readOnly`,
  `rows`, `cols`, `maxLength`, `textLength`, `form`. `HTMLSelectElement`:
  `type`, `multiple`, `required`, `size`, `form`, `options`, `selectedOptions`,
  `length`, `selectedIndex`, `value`. `HTMLOptionElement`: `selected`,
  `defaultSelected`, `value`, `text`, `label`, `index`, `form`.
  `HTMLButtonElement`: `type`, `value`, `form`. `HTMLElement`: `title`,
  `lang`, `dir`, `hidden`, `inert`, `accessKey`, `innerText`.
- **Deviations:** `elements`, `options` and `selectedOptions` are snapshots,
  not live collections, and are plain `HTMLCollection`s; no `select` event;
  `innerText` reads the block text extractor of `tenta-dom` (hiddenness from
  attributes and inline style, not from the layout).
- **Checks:** see NAV1; the three JS crates and `tenta-dom`, `tenta-page` pass
  together.

## RC1: interface objects are defined after the context exists (performance)

The packages of 2026-11-16 (LOC1, HS1, EH1, NAV1, FM1) added about 720
attributes and a few interfaces. The first `S-js-xhr` comparison against HEAD
abe95a2 (with the updated lockfile and the local-pin guard) read navigation
p50/p95 16.98/17.59 ms before and 18.45/19.29 ms after: inside the 10% noise
floor, but in the same direction in both percentiles, so it was taken apart.

- **Where the time went.** An in-process probe of `Engine::with_document`
  (60 engines per process, median) read 2.25 ms before and 3.5 ms after.
  Timing its stages put all of it in one call: `v8::Context::new` with the
  global object template, 0.92 ms before and 2.40 ms after. Callgrind
  (`valgrind` 3.27 in a container) showed the instructions under
  `Genesis::ConfigureGlobalObject`: V8 instantiates every function template
  on the global template while it bootstraps the context, and there a
  prototype stays in fast mode, so each member added copies the prototype's
  descriptor array (`Map::CopyAddDescriptor`, `TransitionToDataProperty`).
  Three single-factor builds ruled out the suspects first assumed: without
  the `GlobalEventHandlers` mixin, without the window's handler accessors, and
  without `history` and the `location` write side, the engine was built no
  faster.
- **The change.** `Engine::install_dom` hands the generated installers a
  template that is never instantiated, and
  `SkeletonRegistry::install_constructors` defines each interface object on
  the global (non-enumerable data property, registration order) once the
  context exists. The `CSS` namespace object stays on the global template.
- **Result** (release, Fedora 43 x86_64, both executables on tmpfs):
  `Engine::with_document` 2.0 to 2.4 ms over three runs of the probe against
  1.9 to 2.3 ms for HEAD, with `v8::Context::new` at 0.49 ms.
  `bench/js_xhr.py` (`S-js-xhr`, three sets of 25 interleaved pairs):
  navigation p50 16.68/16.66/16.75 ms before and 17.08/16.81/17.20 ms after
  (+2.4%, +0.9%, +2.7%); p95 17.39/17.42/17.60 and 19.74/17.78/18.61 ms;
  whole-process peak RSS p50 29.0 to 29.1 MiB before and 29.8 to 30.0 MiB
  after. `bench/dom_calls.py` (`S-dom-calls`, 25 pairs): `has` 28.01 and
  27.98 ns per call, `get` 82.95 and 83.68 ns, wall 123.42 and 124.65 ms, RSS
  33.2 and 34.1 MiB. All of it is inside the noise floor; the RSS growth of
  about 0.8 MiB is consistent across runs. Stripped, the `dom_calls` example
  grew from 58.8 to 61.2 MB and is 61.2 MB with this change.
- **Checks:** `tenta-js-v8`, `tenta-js`, `tenta-browser`, `tenta-mcp` and
  `tenta-cdp` with their V8 and render features: 1834 passed.
- **Open:** the interface objects are still all instantiated when the engine
  is built; a lazy data property per interface would take that off the
  navigation path. QuickJS was not measured.

## WIN1: window and tree members pages expect (P2 package)

A sweep of 35 live sites through the MCP server on 2026-11-16 (navigate,
console errors, snapshot) named what page scripts failed on most:
`matchMedia` on seven sites, `document.currentScript` on about nine (webpack's
"chunk path", `currentScript.parentElement`), `isEqualNode` (Next.js: its
client rendered nothing without it), then `dataset`, `performance.mark` and
`timing`, `scroll`, `Window`. After this package the same sweep reads, for
example, bbc.com 12 errors to 0, tailwindcss.com 12 to 1, nextjs.org 12 to 1,
wp.pl 12 to 2, and duckduckgo.com and npmjs.com render (0 to 98 and 0 to 186
snapshot elements). The sweep is a diagnostic, not a conformance result.

- **`matchMedia` (V8).** `tenta_css::cascade::match_media` parses and
  evaluates the query with Stylo against the page's viewport (a light screen
  with a fine pointer, as the render tier's device is); `MediaQueryList` is a
  real `EventTarget` with `media`, `matches` (evaluated on every read),
  `onchange`, `addListener`, `removeListener`. No `change` event is fired.
- **`dataset` (both engines).** `DOMStringMap` is generated from IDL with
  anonymous named getter, setter and deleter; `tenta-idl` now names anonymous
  special operations (`named_getter`, ...), which it emitted with an empty
  Rust name before. A new handle tag anchors the map to its element.
- **`document.currentScript` (both engines).** The page threads the script
  element to wherever its classic script runs (inline, parser-blocking,
  `defer`, `async`) and names it on the document for the length of the run.
- **Tree (both engines).** `Node`: `isEqualNode`, `isSameNode`,
  `compareDocumentPosition` and its constants, `getRootNode`, `normalize`,
  `baseURI` (`tenta_dom::compare`). `Element` and `CharacterData`:
  `previousElementSibling`, `nextElementSibling`. `Element`:
  `insertAdjacentElement`, `insertAdjacentText`, `webkitMatchesSelector`, and
  `scrollIntoView`/`scrollTo`/`scroll`/`scrollBy`, which do nothing because
  nothing scrolls. `Document`: `visibilityState`, `hidden`, `forms`,
  `images`, `links`, `scripts`, `embeds`, `anchors` (snapshots),
  `getElementsByName`, `importNode`, `adoptNode` (same document only).
- **Window (V8).** `Window` interface object with `Window.prototype` in the
  global's chain; `alert`/`confirm`/`prompt` dismissed at once and logged to
  the console; `print`, `stop`, `focus`, `blur` (nothing to do); `screen`
  (the viewport, 24-bit); `origin`, `isSecureContext`, `name`, `closed`,
  `length`, `opener`, `frameElement`, `crossOriginIsolated`; `scrollTo`,
  `scroll`, `scrollBy`; `requestIdleCallback`/`cancelIdleCallback` (a timer
  task); `document.location`; `performance.mark`, `measure`, `getEntries*`,
  `clearMarks`, `clearMeasures` over plain entry objects, and the legacy
  `performance.timing` and `navigation` objects with every milestone at the
  time origin.
- **Checks:** `tenta-css` media 3; `tenta-js` dataset names 1;
  `tenta-js-v8`: `dom_dataset` 4, `dom_window_extras` 5, `dom_tree_extras` 9,
  `dom_window_misc` 6; `tenta-browser::current_script_js` 1 (through the page
  loader, all four script kinds).
- **Added after the first draft of this section:** `querySelector`,
  `querySelectorAll`, `getElementById` and the element-children members on
  `DocumentFragment`; `innerHTML` on a `<template>` now fills its contents
  fragment (it replaced the element's own children before);
  `navigator.javaEnabled()`; `performance.measure` accepts the navigation
  timing milestone names.
- **Still missing on the sweep** (module scripts and `import()` are MOD1
  below): `customElements`; `ReadableStream`; `HTMLVideoElement` and the
  other per-tag interfaces; `getSelection`; `IntersectionObserver`,
  `ResizeObserver`, `MessageChannel`. QuickJS has none of the window members.

## CO1: cross-origin requests, request modes, provenance of continuations (P2 package)

Until 2026-11-16 a page could only fetch its own origin: a cross-origin
`fetch()` or XHR was refused before it was sent, and `RequestInit` served
three members and refused the other twelve by presence. Both stopped real
pages (amazon.com and imdb.com on the sweep; any client that passes
`credentials`, `mode` or an `AbortSignal`).

- **CORS (both engines; the driver is engine-independent).** The page's
  `tenta-net` driver runs the protocol for `fetch()` and `XMLHttpRequest`:
  the CORS check on the final response over the whole redirect chain, a
  preflight for a method or header that is not CORS-safelisted, and
  `Access-Control-Expose-Headers` filtering. The decisions are pure functions
  in `crates/tenta-page/src/cors.rs`. `docs/dev/idl.md` describes the steps
  and what is not built (no preflight cache; `Response.type` is `opaque` for
  `no-cors` and otherwise still `basic`).
- **Credentials.** `tenta_net::CredentialsMode` (`Include`, `SameOrigin`,
  `Omit`) decides per hop whether a request sends a `Cookie` header and
  stores `Set-Cookie`. Navigations and document subresources keep `Include`.
  `Request::same_origin_redirects` fails a redirect that leaves the origin,
  for a request that would need a preflight there. `Origin` is now written
  on a request that owes one even without a profile placing it.
- **`RequestInit`.** `credentials`, `mode` (`cors`, `same-origin`, and
  `no-cors`, whose cross-origin answer is an opaque `Response`: the sweep
  showed pages firing uncaught `no-cors` beacons on 7 of 35 sites),
  `redirect` (`follow`, `error`), `cache` (`default`; `no-store`, `reload`,
  `no-cache` bypass the cache) are served; `keepalive`, `priority`, `duplex`
  and written-out defaults are accepted. **This changes a documented
  choice**: the refusal was by presence on purpose
  (`refused_request_init_member`), and is now by value. It needs a decision
  entry in `docs/spec/18-decisions.md`, which was not at hand.
- **`signal` (V8).** `fetch(url, {signal})`: an aborted signal rejects with
  its reason and makes no request; aborting later rejects with the same
  reason value, in the order the calls were made, and ends the transport
  (`FetchDriver::abort`). QuickJS refuses a non-null `signal`.
  `AbortSignal.timeout` and `AbortSignal.any` are not built.
- **`XMLHttpRequest.withCredentials = true`** is stored and honoured (it
  threw `NotSupportedError`).
- **Provenance of continuations (V8), a correction.** `tenta_core::provenance`
  says the completion of a tracked request is tracked. It was not: fetch and
  XHR completions were delivered under the default `Background`, and the
  microtask checkpoint a script owed ran outside the scope the script ran
  in. A `fetch()` made after any `await` — after a promise, a short timer,
  or the answer to an earlier request — therefore did not hold the settle,
  and a navigation could return with the page's data still in flight.
  Completions now carry the provenance of their request
  (`FetchCompletion::origin`, `XhrCompletion::origin`) and the owed
  checkpoint runs under the provenance of the entries that owed it
  (`HostState::microtask_origin`). Chains are still bounded by
  `SettleConfig::max_reentries`. QuickJS is unchanged.
- **Measured** (release, Fedora 43 x86_64, against the tree before this
  package): `bench/js_xhr.py` (`S-js-xhr`, three sets of 25 interleaved
  pairs) navigation p50 16.68/16.98/17.07 ms before and 17.15/17.13/17.38 ms
  after (+2.8%, +0.9%, +1.8%), p95 17.84/17.97/18.28 and 17.84/18.06/18.45
  ms, RSS p50 29.9 to 30.0 MiB before and 29.9 to 30.2 MiB after.
  `bench/dom_calls.py` (`S-dom-calls`, 25 pairs): `has` 27.63 and 28.02 ns
  per call, `get` 84.10 and 83.31 ns, wall 123.57 and 124.06 ms, RSS 33.9
  and 34.3 MiB. All inside the noise floor; no claim either way.
- **Checks:** `tenta-page::cors` 7 and the driver's unit tests;
  `tenta-net::http` 3 (credentials modes, redirect guard, `Origin`);
  `tenta-js` `request_init` and `tenta-js-v8::dom_fetch` (served members,
  refused values, signal); `tenta-browser::cors_js` 9 over two loopback
  origins (shared and refused responses, preflight, credentials, XHR, modes,
  `no-cors`, abort, a request after an `await`); the fixture server gained `/api/cors`.
  Workspace 4774 passed twice in a row.
- **An intermittent failure, explained and fixed in the tests.** Two
  `tenta-js-v8` native-budget tests
  (`nested_event_constructors_refuse_before_publishing_native_records`,
  `converted_custom_event_detail_is_released_when_nested_constructors_spend_budget`)
  failed three times on 2026-11-16, always under a full parallel run (2 of 7
  workspace runs, 1 of 15 runs of five crates at `-j 64`), with every event
  record (4094 or 4095) still alive after `low_memory_gc()`. It is not a
  regression of these packages and not an engine leak. Both tests keep their
  events in a global array filled from a getter that loops 10000 times; V8
  starts concurrent Maglev and TurboFan OSR jobs for that getter
  (`--trace-opt` shows one still completing as the script ends), and
  `LowMemoryNotification` flushes the compile queues without waiting for a
  job that is running (`FreeCachesOnMemoryPressure` →
  `AbortConcurrentOptimization(kDontBlock)`). Releasing the array with
  `retainedEvents=undefined` therefore did not free it while such a job was
  in flight. Evidence: with V8's worker threads created under `SCHED_IDLE`
  and 48 busy loops on 32 cores, the engine code of abe95a2 (built at
  0396517, which adds only the build tooling and lock updates) fails 46 and
  42 times out of 150 on the same assertion, and a third test of the same
  file without a hot loop fails 0 of 150; the same build with the array
  emptied first (`retainedEvents.length=0`) fails 0 of 150 and 0 of 150.
  That the holder is the compile job's handle to the array is inferred from
  the trace and the V8 source, not observed directly; a
  `--concurrent-recompilation-delay` run does not show it, because that
  delay sits before the job builds its graph. The tests now empty the array
  before dropping it. A third test of the same shape,
  `refused_standalone_constructors_leave_exactly_the_published_records`,
  failed once in a workspace run on 2026-11-16 and got the same change: 30
  of 150 before it and 0 of 150 after, under the same starvation.
- **Open:** preflight cache; `Response.type` `cors`;
  `Origin: null` after a tainted redirect chain; `data:` and `blob:` URLs in
  `fetch()`; QuickJS has neither `signal` nor the provenance correction.

## MOD1: module scripts and `import()` (P2 package)

The sweep's most common remaining failure after WIN1 was `Uncaught (in
promise) Error: Not supported` (svelte.dev, crates.io): V8's answer to an
`import()` with no host behind it. Sites built with Vite, SvelteKit or Astro
ship their whole program as modules and rendered nothing.

- **Engine (V8).** `crates/tenta-js-v8/src/modules.rs` keeps the realm's
  module map: a module is compiled under a key (its URL, or a name of the
  page's own for an inline script) and the engine names the URLs it imports;
  nothing links until the root runs, by which time every dependency is in the
  map. `import()` hands script a promise and leaves a request for the page;
  the promise settles with the namespace once the module and its top-level
  `await`s have run. `import.meta.url` is the module's URL. Modules are
  compiled with a resource name, so their errors carry `url:line`.
- **Shared.** `tenta_js::modules`: specifier resolution
  (`./`, `../`, `/`, absolute URLs; a bare specifier only through the import
  map), the import map's `imports` with exact and prefix keys, the JavaScript
  MIME test. `PageEngine` gained `supports_modules`, `add_import_map`,
  `compile_module`, `run_module`, `take_dynamic_imports`,
  `finish_dynamic_import`, all defaulting to "not supported".
- **Page.** `crates/tenta-page/src/page/modules.rs` loads a graph level by
  level over `tenta-net`; one load per URL is shared by every graph that
  needs it. A module's fetch is a CORS request (`cors` mode, so it carries
  `Origin`) with `same-origin` credentials (a cross-origin module must be
  shared with the document), its response
  must have a JavaScript MIME type, and its text is UTF-8.
  `<script type="module">` (inline or `src`) is deferred and runs in document
  order with the `defer` scripts before `DOMContentLoaded`; with `async` it
  runs when its graph has loaded. `<script type="importmap">` is applied when
  the parser reaches it. A `nomodule` script is not run on an engine that has
  modules. An `import()` holds the settle while its graph loads and has 30 s
  to do so once the navigation is over.
- **QuickJS** has no modules: it leaves `type="module"` unrun and runs the
  `nomodule` fallback, as before.
- **Measured** (release, Fedora 43 x86_64, against CO1): `bench/js_xhr.py`
  (`S-js-xhr`, three sets of 25 interleaved pairs) navigation p50
  16.97/17.34/17.01 ms before and 17.12/17.47/17.53 ms after (+0.9%, +0.7%,
  +3.1%), RSS p50 29.9 to 30.1 MiB before and 30.2 to 30.5 MiB after.
  `bench/dom_calls.py`: `has` 27.91 and 28.21 ns per call, `get` 83.27 and
  84.18 ns. Inside the noise floor. Across today's packages the same
  scenario has drifted from 16.7 to 17.0 ms at HEAD to 17.1 to 17.5 ms, each
  step below the floor and all in one direction; that sum has not been taken
  apart.
- **Checks:** `tenta-js` modules 6; `tenta-js-v8::dom_modules` 6 (graph order
  and single evaluation, inline modules, import map, link and evaluation
  failures, `import()`, top-level `await`); `tenta-browser::modules_js` 5
  over loopback (document order with `defer` and `DOMContentLoaded`, shared
  module fetched once, QuickJS `nomodule` fallback, import map with
  `import()` and top-level `await`, load failures, a module another origin
  shares). Workspace 4786 and the
  feature set 669 passed.
- **On live sites** svelte.dev, crates.io, vite.dev and astro.build now load
  and run their module graphs; what stops them next is named in their
  consoles: `customElements`, `ResizeObserver`, `Request`, `Worker`,
  `HTMLMediaElement`, `SVGAElement`.
- **Open:** import map `scopes` and `integrity`; JSON and CSS modules (import
  attributes are refused); `import.meta.resolve`; `<link rel="modulepreload">`
  is ignored; a module script that fails to load fires no `error` event and
  leaves no console message; module workers; the classic-script order
  guarantee across a module with top-level `await` (the next deferred script
  does not wait for it).

## OB1 and IF1: observers, and the media, canvas and SVG interfaces (P2 packages)

What the module-built sites named next in their consoles once MOD1 let them
run: `ResizeObserver` (astro.build), `HTMLMediaElement` (crates.io),
`SVGAElement` (svelte.dev), and on the earlier sweep `getContext is not a
function` and `play is not a function`.

- **`ResizeObserver` and `IntersectionObserver` (OB1, V8).**
  `crates/tenta-js-v8/src/host/observers.rs`. Each is a lazy data property of
  the global, so the interface is built the first time a page names it and a
  realm that never does pays for neither. **A model decision to record:**
  nothing scrolls and there is no viewport to leave, so the whole document
  counts as in view. An `IntersectionObserver` tells its callback once, in a
  task after `observe()`, that each target is intersecting
  (`isIntersecting` true, ratio 1), which is the notification a page that
  loads content "when it scrolls into view" waits for; `root`, `rootMargin`
  and `threshold` are kept and reported back and change nothing. A
  `ResizeObserver` reports each target once with the box
  `getBoundingClientRect()` gives and, as the standard has it, only when that
  box is not empty. Neither observes later changes. Entries are plain objects
  with the standard's members; `ResizeObserverEntry` and
  `IntersectionObserverEntry` are not defined. QuickJS has neither.
- **Media, canvas, SVG (IF1, both engines, generated).**
  `HTMLMediaElement`, `HTMLVideoElement`, `HTMLAudioElement`,
  `HTMLCanvasElement`, `SVGGraphicsElement`, `SVGSVGElement`, `SVGAElement`
  (`idl/dom/dom.webidl`, `tenta_js::bindings::host::media`). `<video>`,
  `<audio>`, `<canvas>`, SVG `<svg>` and SVG `<a>` get their interface in the
  prototype chain. A media element is one that has loaded nothing: paused,
  `readyState` 0, `duration` NaN, `canPlayType()` "", and `play()` rejects
  with `NotSupportedError` ("The element has no supported sources."), as a
  browser does for an element it has no source for. `volume`, `currentTime`
  and `playbackRate` accept a value and keep the default (a throwing setter
  would stop a strict-mode script that only configures the element); `muted`
  follows the content attribute. A canvas has `width`/`height` (300 by 150 by
  default), `getContext()` is null for every id and `toDataURL()` is
  `data:,`.
- **Checks:** `tenta-js-v8::dom_observers` 3, `dom_media` 4;
  `tenta-js-qjs::runtime` 1 (the IF1 interfaces). Workspace 4794 passed.
- **Measured, all of 2026-11-16's packages against HEAD abe95a2** (release,
  Fedora 43 x86_64): `bench/js_xhr.py` (`S-js-xhr`, three sets of 25
  interleaved pairs) navigation p50 16.68/16.53/16.68 ms at HEAD and
  17.65/17.71/17.66 ms now (+5.8%, +7.1%, +5.9%), p95 17.10/17.47/17.59 and
  18.47/18.31/19.42 ms, RSS p50 29.1 to 29.2 MiB and 30.3 to 30.4 MiB.
  Below the 10% floor in each set, but the same sign and size in all three,
  so it is a real cost of about 1 ms and 1.2 MiB per cold navigation.
  `Engine::with_document` (60 engines per process, three runs) read 1.92 to
  2.33 ms at HEAD and 2.07 to 2.49 ms now, so engine construction is about a
  fifth of it; the rest has not been located. Steady state is unchanged:
  `bench/dom_calls.py` `has` 28.00 and 27.78 ns per call, `get` 83.30 and
  82.74 ns. Against MOD1 alone, OB1 and IF1 read 16.92 and 17.09 ms.
- **What a probe of 340 commonly used members still finds missing** (V8,
  after these packages; a diagnostic, not an inventory of the platform):
  `customElements`, `attachShadow`/`ShadowRoot`, `window.postMessage`,
  `MessageChannel`, `Request`, `ReadableStream` and the other streams,
  `navigator.sendBeacon`, `document.write`, `crypto.randomUUID` and
  `crypto.subtle`, `URL.createObjectURL`, `getSelection`,
  `PerformanceObserver`, `AbortSignal.timeout`/`any`, `CSS.escape`,
  `reportError`, `window.event`, the event interfaces (`ErrorEvent`,
  `PromiseRejectionEvent`, `MessageEvent`, `PointerEvent`, `InputEvent`,
  `PopStateEvent`, ...), `WebSocket`, `EventSource`, `Worker`, `indexedDB`,
  `navigator.userAgentData`, and most per-tag HTML interfaces
  (`HTMLBodyElement`, `HTMLDialogElement`, ...).
- **A regression to know about.** Running module scripts means running
  hydration code that used to fail at its first `import()`. nytimes.com kept
  its server-rendered text before MOD1 and now empties `#app` and stops on
  an uncaught `TypeError` (`reading 'apply'`); its snapshot went from 18
  elements to none. On the 35-site sweep, sites with no snapshot elements
  still went from 7 to 5 (crates.io, onet.pl and search.brave.com gained
  content), and uncaught errors went from 22 to 47 because more code runs. Eleven of
  the 47 are `Failed to fetch` for analytics hosts that the DNS of the
  machine the sweep ran on answers with `::`, which the private-network
  policy refuses; they say nothing about the engine.
- **Open:** observation of later changes (resize, real intersection);
  `MutationObserver`-style entry interfaces; a 2D canvas context; media
  events (`loadedmetadata`, `error`) and `<source>` selection; the other SVG
  interfaces (`SVGPathElement`, ...) and `SVGAElement.href`; the remaining
  per-tag HTML interfaces. Still named by live sites: `customElements`,
  `Request`, `Worker`.

## CE1: custom elements (P2 package, V8)

`customElements is not defined` was the most common uncaught error left on
the 35-site sweep (7 sites), and it ends the bundle that raises it.

- **What exists** (`crates/tenta-js-v8/src/host/custom_elements.rs`):
  `customElements` and `CustomElementRegistry`, both lazy data properties of
  the global, with `define()`, `get()`, `getName()`, `whenDefined()` and
  `upgrade()`; the HTML element constructor, so `class X extends HTMLElement`
  can call `super()` and `new X()` makes an element of the defined name;
  upgrade of elements of a defined name; `connectedCallback()`,
  `disconnectedCallback()` and `attributeChangedCallback()` with
  `observedAttributes`. `define()` validates in HTML's order: the name
  (`SyntaxError`), a name or constructor already used
  (`NotSupportedError`), the prototype and the reaction members
  (`TypeError`).
- **How the document tells the engine** (`tenta_dom::LifecycleRecord`,
  `Document::enable_lifecycle_log`). `Document::mutate` is the one mutation
  path, so the log is written there: an HTML element with a hyphen in its
  name was created; an element became connected or stopped being connected
  (one record for the root of the subtree); an attribute of an element with
  a hyphen in its name changed. The log is off until the first `define()`,
  so a page without custom elements records nothing and pays one flag read
  per binding call (`DocumentSet::custom_elements_used`).
- **When reactions run.** `V8CallbackAdmission::after_callback` acts on the
  records after every binding call, which is where HTML runs the reactions
  of a `[CEReactions]` operation: before the script that made the call sees
  its result. `createElement()`, cloning and fragment parsing therefore give
  an upgraded element, and elements in `<template>` contents stay inert
  until they are cloned out. `define()` only notes the elements it finds and
  they are upgraded when its call returns: inside the call every wrapper
  their reactions made would count against one binding call's limit of 4096
  (700 elements appending ten children each failed that way before the
  change, and a test keeps it). **One ordering difference:** elements the
  parser creates are upgraded when script is next entered (a script, a
  timer, `DOMContentLoaded`/`load`), not while the parser creates them, so a
  constructor can see children HTML would not have parsed yet.
- **`[HTMLConstructor]`** (`tenta-idl`, `docs/dev/idl.md`): the attribute
  stands on the `HTMLElement` interface and makes the generated V8
  constructor ask the world (`V8CallbackAdmission::html_constructor`) instead
  of throwing. Every other interface without a constructor still throws
  `Illegal constructor`, and so does QuickJS.
- **State.** Definitions are script objects kept by the registry's private
  properties; an element's wrapper holds its definition and whether it was
  told it is connected. The host keeps one root on the registry, the pending
  `whenDefined()` promises and the construction stack.
- **Checks:** `tenta-dom::lifecycle` 2; `tenta-idl::codegen` 31 (one new);
  `tenta-js-v8::dom_custom_elements` 14 (validation, reading the reactions,
  upgrade order, creation paths, connection, attributes, `whenDefined`,
  failed constructors, `<template>`, elements added outside script, state
  across a collection, 700 elements each building a subtree, 6000 elements
  in one insertion, child replacement); `tenta-browser::custom_elements_js`
  2 over loopback (a page whose elements stand before and after the
  defining script, and a guarded page on QuickJS, which has no registry).
- **Measured against 03babbd** (release, Fedora 43 x86_64, 25 interleaved
  pairs per set): `bench/dom_calls.py` (`S-dom-calls`) `has` p50 27.72 and
  27.53 ns before, 28.47 and 28.16 ns now (+2.7%, +2.3%), `get` 82.39 and
  82.62 ns before, 82.27 and 82.56 ns now; `bench/js_xhr.py` (`S-js-xhr`)
  navigation p50 17.68 and 17.84 ms before, 17.40 and 17.57 ms now, RSS p50
  30.3 to 30.4 MiB both. All below the 10% floor; the `has` difference has
  the same sign in both sets and is the size one more flag read per call
  would have (0.7 ns).
- **Open:** customized built-in elements (`extends`, `is=`), which
  `define()` refuses with `NotSupportedError`; form-associated custom
  elements and `ElementInternals`; `adoptedCallback()`; the `:defined`
  pseudo-class still matches every element; QuickJS has no registry. Shadow
  trees came with SD1, and the upgrade walks include them.

## SD1: shadow trees for script, events and extraction (P2 package)

A component library's constructor calls `attachShadow()` first, so custom
elements without it upgraded nothing that such a library defines.

- **The tree** (`tenta-dom`). A shadow root is a container owned by its
  host, outside the document tree, as `<template>` contents are; that part
  existed and nothing used it. New: the mode
  (`attach_shadow_root_with_mode`, `shadow_root_mode`), `shadow_host`,
  `containing_shadow_root`, the shadow-including order
  (`shadow_including_subtree`), and named slot assignment computed when
  asked (`assigned_nodes`, `assigned_slot`): the first slot of a name takes
  the host's children that ask for it, text goes to the default slot.
- **Two meanings of connected.** `Document::is_connected` stays what the
  indexes need, "in the document tree", so `getElementById`,
  `getElementsByTagName` and `querySelector` on the document do not see
  into a shadow tree. `is_composed_connected` is the standard's meaning
  (the shadow-including root is the document), computed by walking out
  through the hosts; `Node.isConnected`, the lifecycle log and the MCP ref
  table use it.
- **Script** (`idl/dom/dom.webidl`, `tenta_js::bindings::host::shadow`,
  generated for both engines): `Element.attachShadow()` with the standard's
  list of hosts and errors, `Element.shadowRoot` (null for a closed root),
  `ShadowRoot` as a `DocumentFragment` with `mode`, `host`, `innerHTML`
  (parsed in the context of the host), `delegatesFocus` and
  `slotAssignment`; `HTMLSlotElement` with `name`, `assignedNodes()` and
  `assignedElements()`; `Element.assignedSlot` and `Element.slot`;
  `getRootNode({composed: true})`.
- **Events** (`tenta_js::dom::events`). The propagation path follows DOM's
  "get the parent": a slotted node goes to its slot, a shadow root to its
  host unless the event is not composed and the root is that of the tree it
  was dispatched in. When the path leaves a shadow tree, each entry's
  listeners see the retargeted target (`Document::retarget`), and a host
  the path left a tree through is at target, so it hears an event that does
  not bubble. A path that crosses no boundary is built and walked as
  before.
- **Extraction** reads the flat tree: `extract::Walker` shows a host's
  shadow tree in place of its children and a slot's assigned nodes in place
  of the slot, so text, markdown, the snapshot and main-content extraction
  return what is rendered. `textContent` and `innerHTML` are unchanged.
- **Custom elements** walk shadow-including trees: `define()`, `upgrade()`
  and the connected and disconnected reactions reach elements in shadow
  trees, and the lifecycle log records an element that enters or leaves a
  shadow tree whose host is connected.
- **MCP.** The snapshot shows shadow tree elements, a ref inside an open
  shadow tree can be acted on (the page-side path has a step that enters
  `shadowRoot`), and a click there is dispatched without a point.
- **Not done, and it shows.** Styles and layout do not cover shadow trees:
  the cascade's node table and the layout are built over the document tree,
  so shadow content has no computed style and no box. A **screenshot does
  not show it**, `getBoundingClientRect()` inside a shadow tree is empty,
  the snapshot cannot apply stylesheet visibility to it (it uses the
  `hidden` attribute and the style attribute, as on a page with no layout),
  and `<style>` inside a shadow root does nothing. A host's children that
  are not slotted are still laid out as if there were no shadow tree.
- **Checks:** `tenta-dom::shadow` 4; `tenta-js-v8::dom_shadow` 6 (validation,
  the separate tree, slots, a custom element rendering into a shadow tree,
  the event path with retargeting, events at slotted nodes);
  `tenta-js-qjs::runtime` 1; `tenta-browser::custom_elements_js` 3 (one
  new, over loopback); `tenta-mcp::scenario` 10 (one new: a component is
  read and clicked through MCP at the render tier).
- **Measured** (release, Fedora 43 x86_64, 25 interleaved pairs per set).
  Against CE1 (cd44ad4): `bench/dom_calls.py` (`S-dom-calls`) `has` p50
  27.62 to 27.78 ns, `get` 82.01 to 82.60 ns; `bench/js_xhr.py` (`S-js-xhr`)
  navigation p50 17.68 to 17.36 ms, RSS p50 30.3 and 30.4 MiB.
  `bench/dom_mutation_hooks.py` (`S-dom-mutation-hooks`, V8), which CE1
  should have been measured with and was not, across three builds: before
  CE1 (03babbd) to CE1, detached p50 44.03 to 47.88 ms and 43.73 to 47.79 ms
  (+8.7%, +9.3%), connected 44.88 to 48.86 ms and 44.80 to 48.59 ms (+8.9%,
  +8.5%); CE1 to this tree, detached 47.68 to 44.95 ms (-5.7%), connected
  48.66 to 46.03 ms (-5.4%); before CE1 to this tree, detached 43.61 to
  45.66 ms and 44.23 to 44.51 ms (+4.7%, +0.6%), connected 45.02 to 46.04 ms
  and 44.92 to 45.87 ms (+2.3%, +2.1%). Everything is below the 10% floor.
  The CE1 build is repeatably about 9% slower on this scenario and this
  build, which contains all of CE1's work on the mutation path and more, is
  not, so the cause is not that work as such; it has not been located.
- **The 35-site sweep after CE1 and SD1** (2026-11-16, same list and
  driver as before). `customElements is not defined` is gone (8 messages
  before). Uncaught errors went from 47 to 53, again because more code runs:
  github.com now reaches `Unable to get document domain` (7) and
  `HTMLMetaElement is not defined` (3), nytimes.com `No randomUUID function`
  (5); 11 are still `Failed to fetch` for hosts the sweep machine's DNS
  answers with `::`. Sites with no snapshot elements went from 5 to 6.
- **A regression to know about: cloudflare.com no longer loads.** Its
  bundle used to stop at `customElements is not defined` and the page kept
  its server-rendered content (14 refs, 299 snapshot lines). The bundle now
  runs, and a timer-driven script reads used values (`getComputedStyle` of
  layout-dependent properties, element rectangles) after each change it
  makes. Every such read lays the page out again in full, one layout of
  that page takes 126 to 132 ms (95 layouts, 12.1 s, measured with a
  temporary timer in `RenderSession::refresh_layout`), and the page is
  closed for exceeding its script budget (15 s within 30 s), so the
  navigation fails and the agent gets nothing. Stack samples put the time
  in nested grid and flex sizing (container depth 4 on average, up to 9).
  Neither custom elements nor shadow trees are involved beyond letting the
  script run. What would address it: a cheaper layout for this shape of
  page, layout that does not start over after every mutation, or a page
  that survives its script budget with the document it has; the last is a
  policy the specification owns.
- **Open:** `slotchange`; manual slot assignment and `flatten`;
  `composedPath()` hiding closed trees; `ShadowRoot.activeElement`; whether
  scripts inside a shadow tree load has not been checked; the CDP DOM domain
  does not report shadow roots; a ref inside a closed shadow tree cannot be
  acted on. Styles, layout and paint are SD2, below.

## WIN2: members the sweep named after CE1 and SD1 (P2 package)

- **`document.domain`**: the host of the document's URL. Assigning the
  value it already has does nothing; any other value is a `SecurityError`,
  because nothing here relaxes the same-origin policy. github.com's bundle
  threw `Unable to get document domain` seven times without it.
- **`crypto.randomUUID()`** (`idl/webcrypto/Crypto.webidl`): a version 4
  UUID from the source `getRandomValues()` uses. Not limited to secure
  contexts, which are not modelled.
- **`HTMLMetaElement`, `HTMLSourceElement`, `HTMLDialogElement`**: the
  reflected attributes. A dialog's `show()`, `showModal()` and `close()` set
  and remove `open`; there is no top layer, no `returnValue` and no `close`
  event.
- **Checks:** `tenta-js-v8::dom_media` 6 (two new), `tenta-js-qjs::runtime`
  1 new. Workspace 4825 passed; the feature set 593 passed twice after one
  run in which `tenta-cdp::page
  a_refused_connection_is_an_error_text_and_an_error_document` failed. That
  test navigates to a port it bound and released, alone it passed 6 times
  out of 6, and its failure output was not kept; another test's server
  taking the released port would explain it and has not been shown.
- **Measured against SD1** (release, two sets of 25 interleaved pairs):
  `bench/js_xhr.py` (`S-js-xhr`) navigation p50 17.51 to 17.58 ms and 17.51
  to 17.55 ms, RSS p50 30.4 to 30.6 MiB both; below the 10% floor.
- **Still named by the sweep:** `ReadableStream` (3), `Worker`,
  `MessageChannel`, `Request`, `getSelection`, `DOMMatrix`,
  `SVGAnimatedString`, `createNodeIterator`, an unsupported CSS rule type in
  `insertRule`.

## SD2: shadow trees are styled, laid out and painted (P3 package)

- **Sheets** (`tenta-css`): a `StyleSource` carries the shadow root its
  owner is in. The engine builds the document's cascade data from the
  sources without one and, for each shadow root that has any, a Stylo
  `AuthorStyles` set of its own. `tenta-page` finds `<style>` and
  `<link rel=stylesheet>` in shadow trees once the document has one, and the
  signature that decides when sheets are collected again covers them, so a
  sheet a script adds to a shadow root applies.
- **The adapter Stylo walks**: `ShadowRootHandle` is a real node
  (`TShadowRoot` with its host and style data), `TElement::shadow_root` and
  `containing_shadow` answer, the traversal and inheritance follow the flat
  tree (`Document::flat_parent`, the shadow root's children under the host,
  assigned nodes under their slot), and selector matching stays within one
  node tree (`parent_element` stops at a shadow root, `assigned_slot` and
  `containing_shadow_host` answer). With that Stylo does the scoping: a
  shadow sheet's rules reach only its tree, `:host` its host and
  `::slotted()` the nodes slotted into it, and document rules do not reach
  into a shadow tree.
- **Boxes** (`tenta-layout`): the box builder takes an element's children
  from `Document::flat_children`. A host child that no slot takes and the
  fallback of a slot that has assigned nodes are not in the flat tree, are
  not styled and get no box. `slot` is `display: contents` in the
  user-agent sheet, so slotted nodes are items of the container their slot
  is in.
- **What reads the layout**: images inside shadow trees are planned and
  loaded; `getBoundingClientRect`, `getComputedStyle`, `offset*` and
  `innerText` answer for an element in a shadow tree;
  `document.elementFromPoint` gives the host; the per-element geometry the
  MCP snapshot uses lists shadow elements (`Document::composed_elements`),
  so one hidden by its shadow sheet is left out of the snapshot; an MCP
  click on a ref inside a shadow tree is aimed at its box and the check for
  a covering element follows the flat tree.
- **Cost on documents without a shadow tree, and what was done about it.**
  Every new question is behind `Document::has_shadow_trees`, a flag set when
  the first shadow root is attached and never cleared, read once per flush
  view. That was not enough: with a shadow root type that has values Stylo
  keeps its shadow paths, its style-sharing test asks six more questions per
  candidate, and the larger functions are inlined differently. `S-style-10k`
  (initial traversal of 10 000 elements, the scenario most dominated by
  style sharing) went from 14.5 to 15.8 ms, +7% and +9% in two runs of 25
  interleaved pairs and 1108 to 1221 million instructions under callgrind;
  on the copy of a real page from LP1 the style flush moved 1 to 3%.
  Keeping the flat-tree iterator free of a destructor, and its slot branch
  out of line, changed nothing measurable. What the same profile showed is
  that a third of that scenario was `element_state`, recomputed from
  attributes and ancestors for every sharing candidate; it is now computed
  once per element per document generation (`StoreEntry::state`).
- **Measured** (release, `bench/render.py`, 25 interleaved pairs against
  68e3a5f, final code): `S-style-10k` total p50/p95 14.60/16.49 to
  12.43/14.02 ms (-14.9%), 873 million instructions. `S-layout-5k`,
  `S-layout-nested`, `S-screenshot-inproc` and `S-fixtures` inside the 10%
  floor (one `S-fixtures` group of ten read -10.2% once). A document with
  shadow trees has not been benchmarked.
- **Not built or not checked:** `adoptedStyleSheets` and constructable
  sheets on a shadow root; `shadowRoot.styleSheets`; `::part()` and
  `exportparts` (the adapter answers `is_part`; nothing tests it);
  `:host()` and `:host-context()` are Stylo's and untested here; `:defined`;
  focus inside a shadow tree; `getComputedStyle` of a node outside the flat
  tree resolves without a parent style; slot assignment is recomputed by a
  walk of the shadow tree for every question (`assigned_slot`,
  `containing_shadow_root`), which has not been measured on a page with
  many components. An element's state is cached per document generation,
  and `input.checked = …` or a typed value does not move the generation, so
  as before this package `:checked` and `:placeholder-shown` restyle only
  with the next mutation.
- **Checks:** `tenta-css::cascade_shadow` 4 (scoping both ways, `:host`,
  `::slotted()`, flat-tree inheritance, nothing styled outside the flat
  tree), `tenta-css::cascade_flush` 1 new (state recomputed after a
  mutation; with the cache never invalidated it and the focus test fail),
  `tenta-layout::shadow` 3 (a component's boxes, slotted flex items, no box
  outside the flat tree), `tenta-browser::geometry_js` 1 new (computed
  style, rectangles, a sheet added by script, `elementFromPoint`),
  `tenta-mcp::scenario` shadow scenario extended (the shadow sheet hides a
  paragraph from the snapshot, the element screenshot is 200 px wide and
  green, the click reports a point). Workspace 4844 passed; the feature set
  595 passed; fmt, both Clippy runs, `lint-deps`, `idl-manifest` and
  `cargo deny` clean.
- **The 35-site sweep after DSD1 and SD2** (one run, 2026-11-17, same list
  and driver): no navigation failed, the same three sites have no snapshot
  elements (youtube.com, amazon.com, imdb.com), the 35 navigations took
  54.8 s against 56.8 s and the console held 59 errors against 60. No site's
  snapshot was compared line by line except developer.mozilla.org's, whose
  first lines differ between the two runs: a promotion banner and menu
  buttons stand where a list of links was. Whether the page served
  something else or this package shows what it did not before has not
  been determined.

## DSD1: declarative shadow roots (P2 package)

- **Parser** (`tenta-html`): a `<template shadowrootmode="open|closed">`
  start tag asks the sink to attach a shadow root to the adjusted current
  node (HTML "in head" insertion mode). When the sink does, the template
  goes on the stack of open elements only, with the shadow root in place of
  its template contents, and never into the tree. It stays an ordinary
  template when the parse does not allow declarations
  (`ParserOptions::declarative_shadow_roots`, off by default and on for a
  navigated document, so `innerHTML` and `DOMParser` declare nothing), when
  the mode is neither value, when the adjusted current node is the root
  element, or when the sink refuses. A sink without shadow trees keeps the
  default and refuses.
- **Document** (`tenta-dom`): `can_host_shadow_root` holds the list of
  valid hosts (moved from the binding), `attach_declarative_shadow_root`
  refuses an invalid host and a host that already has a root, and
  `claim_declarative_shadow_root` is `attachShadow()` on such a host: a
  declared root of the mode asked for is emptied and returned once, as DOM
  "attach a shadow root" has it, which is what lets a component hydrate
  over server-rendered markup.
- **Not built:** `shadowrootdelegatesfocus`, `shadowrootclonable` and
  `shadowrootserializable` are ignored; `setHTMLUnsafe()` and
  `parseHTMLUnsafe()` do not exist; cloning a host does not clone a
  declared root.
- **Checks:** `tenta-dom::shadow` 3 new, `tenta-js-v8::dom_shadow` 1 new,
  `tenta-browser::custom_elements_js` 1 new (a server-rendered component is
  there before script and its constructor gets the declared root).
  Workspace 4836 passed. The feature set passed 594 of 594 twice after a
  first run in which `tenta-browser::css_loading
  a_stylesheet_is_requested_while_the_body_is_still_streaming` failed. That
  test asserts on wall-clock time (the sheet request at least 800 ms before
  the end of a body the server holds for 1200 ms), alone it passed 6 times
  of 6, and its failure output was not kept; load from the parallel run
  delaying the request would explain it and has not been shown.

## LP1 and GC1: what a page that animates costs (performance)

After CE1 cloudflare.com stopped loading: its bundle ran, kept changing the
document from timers and animation frames, and the page was closed for
spending 15 s of script time within 30 s. Two costs were behind it, and
neither is specific to that site.

### LP1: nested flex and grid containers share their answers

- **Where the time went.** A copy of that page without its scripts (2672
  fragments, 445 flex and grid containers, nested 4 deep on average and up
  to 9) took 126 ms to lay out. Counters in `tenta-layout`'s Taffy view
  showed 94 544 runs of a container algorithm for those 445 containers, up
  to 1566 for one of them, in 2890 views. A nested container is solved
  inside its parent's view while the parent sizes its items, and again as
  the root of its own view when its fragments are made, and the containers
  below it once more under each of those; every view started with an empty
  cache, so the work multiplied with the depth.
- **The change** (`crates/tenta-layout/src/taffy_tree.rs`). Taffy's answers
  about the boxes below a view are kept in a memo for the outermost layout
  in progress, the same span as the existing scratch-layout memo, and
  shared by every view inside it. An answer is keyed by the box, the parent
  size it was asked under and the direction of the view. An answer that
  read the view's own inner width (a ratio-only image measured before any
  width is known) is shared only with views of the same inner width. The
  view's maps hash box ids with a multiply-and-rotate hasher; SipHash was
  13% of the profile.
- **Result.** The same page: 7728 container runs, layout 126 to 24 ms, and
  the fragment tree is identical fragment for fragment (node, rectangles,
  edges, overflow, flags, children; 2672 of 2672). On the live page a
  layout with its style flush went from about 127 to about 40 ms.
  `bench/render.py`, release, 25 interleaved pairs: the new
  `S-layout-nested` (about 430 containers nested eight deep, text warm)
  total p50/p95 57.39/60.17 to 33.07/35.53 ms (-42.4%); `S-layout-5k` warm
  18.56 to 18.39 ms and cold 39.95 to 39.99 ms, `S-screenshot-inproc`
  relayout 9.16 to 9.14 ms, `S-fixtures` within 5%: all inside the 10%
  floor. RSS deltas did not move.
- **Checks:** `tenta-layout::flex` three new tests. With the inner-width
  tracking removed, `a_ratio_only_image_fills_the_container_it_is_in_not_an_outer_one`
  fails. With the parent's block size taken out of the key no test fails
  and the page above lays out identically but 5% slower (60.7 against
  57.8 ms with its style flush), because the answers for different parents
  then evict one another; that part of the key is covered by measurement
  only. The direction in the key mirrors the scratch-layout memo's and has
  no test of its own.
- **Open, with what was measured.** Layout is still redone in full after
  every mutation, and so is style: `StyleEngine::flush` restyles the whole
  tree whenever the document generation moved, 19 ms per flush on that
  page, which is now half of what a forced layout costs there. Sizing a
  container's inline size under min-content and max-content lays its items
  out to learn block sizes nothing reads (31% of the remaining layout on
  that page, 44% of `S-layout-nested`); skipping that is exact only where
  no box derives an inline size from a block size (aspect ratios, wrapping
  columns), which has not been built. A container is still entered once
  per ancestor level when its fragments are made (2818 entries for 445
  containers).

### GC1: collecting the wrapper graph at task ends is amortised (V8)

- **Where the time went.** With LP1 in, the page still ran out of script
  time. Timing every engine entry showed 371 animation-frame entries of
  12 to 13 ms that laid nothing out, and stack samples of the page thread
  put them in `Engine::graph_checkpoint`: a full `LowMemoryNotification`
  after every timer callback and every animation-frame callback. The ledger
  already listed "empty or duplicate task checkpoints" as an optimisation
  not yet accepted, and image events were already amortised (1024 events or
  250 ms) for the same reason.
- **The change** (`Engine::collect_after_task`). A task end collects when
  the wrappers published since the last collection have reached the count
  at which the next binding call would collect anyway
  (`SOFT_WRAPPER_MATERIALIZATIONS`, 1024), or when the last collection is
  250 ms old. It applies to the microtask checkpoint behind timer callbacks,
  animation frames and `run_microtasks`, and replaces the separate
  image-event counter. The end of a script run, fetch and XHR settlement,
  lifecycle events and `trim_memory` collect as before. Admission inside a
  task is unchanged, so the bound on wrappers between two collections is
  the one that already held within a task.
- **What it changes for a caller.** Wrappers and native records that became
  unreachable in a timer or frame are reclaimed at the next collection, not
  at the end of that task. A page that goes idle keeps up to 1023 of them
  until its next entry or `trim_memory`. No existing test depended on the
  earlier timing.
- **Result.** cloudflare.com loads again: three navigations of three with
  the release CLI, 7.7 to 7.8 s each to the busy cap, title and snapshot
  present. Release, 25 interleaved pairs against 538ead7:
  `bench/js_xhr.py` (`S-js-xhr`) navigation p50/p95 17.81/19.06 to
  12.94/14.17 ms (-27.3%), RSS p50 30.5 MiB both. `bench/listener_gc.py`
  whole-process wall p50 60.30 to 52.01 ms (-13.7%) and CPU 168.08 to
  132.82 ms (-21.0%); its five explicit collection phases moved between
  -1.9% and +4.3%, every phase freed or retained what it did before, peak
  RSS 23.6 MiB both. `bench/dom_calls.py`: `has` 27.77 to 27.73 ns, `get`
  83.88 both. `bench/dom_mutation_hooks.py --backend v8`: detached 48.82 to
  49.09 ms, connected 49.79 to 50.43 ms (+0.5%, +1.3%, inside the floor).
  `bench/wrapper_pressure.py` (candidate-only driver, run on each build):
  task p50 within 3.4% for all four shapes, nodes after the task and GC
  epochs identical, peak RSS within 0.7%. The `S-js-xhr`, listener and
  `S-layout-nested` figures are from the committed code; the others here
  and in LP1 were taken one edit earlier, before the time of a collection
  was recorded at its end instead of its start.
- **Checks:** four tests in `tenta-js-v8` (`task_collection_tests.inc.rs`).
  With a collection at every task end all four fail; with the wrapper count
  ignored `a_task_that_reaches_the_wrapper_bound_collects_at_its_end` fails.
- **Not measured:** the resident memory of a long-lived animated page under
  the new timing, and QuickJS, which has no such collection at task ends.

### The 35-site sweep after LP1 and GC1

One run, 2026-11-17, same list and driver as before; the earlier run
predates WIN2, so the two differ by WIN2 as well and a change cannot be
pinned on one package. Navigations that failed went from 1 to 0
(cloudflare.com). Sites with no snapshot elements went from 6 to 3:
cloudflare.com now has 12 refs, nytimes.com 18 and microsoft.com 5;
youtube.com, amazon.com and imdb.com are still empty. The 35 navigations
together took 56.8 s against 81.6 s; microsoft.com alone went from 23.6 to
3.7 s. Console errors went from 53 to 60, again with more code running: 12
are one page reading `nextSibling` of null, 11 are `Failed to fetch` for
hosts the sweep machine's DNS answers with `::`, and the names still
missing are `Request` (3), `ReadableStream` (3), `MessageChannel`, `Worker`,
`getSelection`, `DOMMatrix`, `SVGAnimatedString`, `CSS.escape`,
`PerformanceNavigationTiming` and `createNodeIterator`; cloudflare.com
reads `.ready` of an object that is undefined here, probably
`document.fonts`, which has not been confirmed.

Workspace 4832 passed; the feature set 593 passed; fmt, both Clippy runs,
`lint-deps`, `idl-manifest` and `cargo deny` clean.

## MSG1: messaging in one realm, and the window as an `EventTarget` receiver (P2 package, V8)

youtube.com returned an empty snapshot. Logging where its uncaught
exceptions were thrown (a temporary print in the inspector's exception
report) gave two causes.

- **`Illegal invocation`.** The page's ShadyDOM copies
  `EventTarget.prototype.addEventListener` to another name and calls it on
  the window. The generated operations carry the interface's signature and
  the global object is not made from the `EventTarget` template, so V8
  refused the window as receiver, and a bare call of a stored reference
  with it (Web IDL substitutes the global object for a missing receiver).
  `EventTarget.prototype`'s three operations are now functions that hand
  the call to the window's own operation for that receiver and to the
  generated one for any other. A node's `addEventListener` therefore goes
  through one more native call; its cost has not been measured. QuickJS
  already behaved correctly and has a test saying so.
- **`MessageChannel is not defined`.** `MessageChannel`, `MessagePort`,
  `MessageEvent` and `window.postMessage()` are in
  `crates/tenta-js-v8/src/host/message_channel.rs`. A posted value is
  cloned at once with `structuredClone`, and each message is delivered as a
  `message` event from a task of its own, in order. A port holds what
  arrives until `start()` or an `onmessage` assignment and delivers nothing
  after `close()`. `MessagePort` is a real `EventTarget` and `MessageEvent`
  a real `Event`, each with its own prototype in front, as `MediaQueryList`
  is built. `window.postMessage()` delivers to the window with the
  document's origin and the window as source; the target origin is not
  compared, there being no other browsing context.
- **Not built:** transfer lists are accepted and ignored, so a port or a
  buffer is never moved and `MessageEvent.ports` is empty; no
  `messageerror`; `BroadcastChannel`; `initMessageEvent`; nothing on
  QuickJS.
- **Found on the way and open:** an uncaught exception fires no `error`
  event, so `window.onerror` and `addEventListener('error', …)` never run
  and there is no `ErrorEvent`; youtube.com still throws once reading
  `startsWith` of undefined. amazon.com and imdb.com answer 202 with
  `x-amzn-waf-action: challenge`: the operator's bot check. A local page
  answered 202 with an external script runs here, so it is that script's
  own logic that fails (`checkForceRefresh` of undefined), and it was not
  taken apart: getting past a site's bot check is a decision for the
  project owner, not a gap in the engine.
- **Checks:** `tenta-js-v8::dom_messaging` 6 (the receiver, order and
  cloning, start and close, refusals, `MessageEvent`, `window.postMessage`),
  `tenta-js-qjs::runtime` 1 new. Workspace 4851 passed. The feature set
  passed 595 of 595 in two runs of four; in the other two one test failed
  each, and each passed alone afterwards: `tenta-browser::wpt_driver
  a_live_harness_reports_from_its_own_timeout_before_the_driver_gives_up`
  (a race of a 1 000 ms against a 1 250 ms clock; 4 of 4 alone) and
  `tenta-mcp::reading network_requests_list_the_current_page_and_filter`
  (a page without script; 6 of 6 alone). Their failure output was not kept
  and no cause is established. Together with the two earlier one-off
  failures recorded above, four different tests of the feature set have now
  failed once each under the parallel run.
- **Result:** youtube.com's snapshot has its banner, search form, main
  region and consent dialog (68 lines, one navigation by hand).

## TW1: tree traversal and `CSS.escape()` (P2 package)

developer.mozilla.org and microsoft.com threw `Cannot read properties of
null (reading 'nextSibling')` at `nextNode`. Both render with Lit, which
keeps one `TreeWalker` rooted at the document and points `currentNode` at
the content of each template. The only `createTreeWalker` there was was a
forward-only stand-in in the CDP interaction script, present only for pages
driven through CDP or MCP, and it walked off the end of a tree its root did
not contain.

- **Traversal** (`crates/tenta-js/src/traversal.rs`, run by
  `Engine::install_preludes` on V8): `NodeFilter` with its constants,
  `TreeWalker` (all seven movements and a settable `currentNode`),
  `NodeIterator` (`nextNode`, `previousNode`, the reference node and which
  side of it the pointer is on), `Document.createTreeWalker()` and
  `createNodeIterator()`, each following the algorithm of DOM §6, with the
  filter given as a function or as an object with `acceptNode`. It is a
  script and not a binding because every step may call the page's filter;
  it reads the tree through `Node`'s accessors taken before page script
  runs. A walk whose current node is outside the root ends with `null` when
  it leaves that node's tree, as browsers do.
- **`CSS.escape()`** (`idl/cssom/CSS.webidl`): cssparser's identifier
  serializer. MDN called it once the walker let it get that far.
- **Not built:** a `NodeIterator` is not adjusted when its reference node is
  removed; the functions are script, so their `toString()` shows source and
  not `[native code]`; QuickJS keeps the stand-in, and only when driven
  through CDP or MCP.
- **Checks:** `tenta-js-v8::dom_traversal` 6 (order both ways, `whatToShow`
  and both filter forms, the seven movements, the template pattern, the
  iterator, the interfaces' shape and errors), `tenta-js-v8::dom_style` 1
  new. The feature set 595 passed. The workspace passed 4858 of 4858 on its
  second run; on the first `tenta-js-qjs::runtime
  recursive_microtasks_are_bounded` failed, a test that gives its script
  10 ms of wall-clock time and passed 5 times of 5 alone. Output not kept,
  cause not established.
- **Result** (one navigation each, by hand): the error is gone from both
  sites; developer.mozilla.org's snapshot has 469 lines where it had about
  20.

### The 35-site sweep after MSG1 and TW1

One run, 2026-11-17, same list and driver. youtube.com has snapshot
elements now. Console errors went from 59 to 52. The 35 navigations took
58.5 s against 54.8 s.

**microsoft.com's navigation failed in this run, and the cause is the
script CPU budget, not the page.** The page was closed with "ran out of
script time: 15017 ms spent of a 15000 ms budget within 30 s" 5.7 s into
its navigation. The driver uses one page for all 35 sites, the budget is
the page's and not the navigation's (D-033, `crates/tenta-page/src/
script_cpu.rs`), and the sites before it in the list (getbootstrap.com,
cloudflare.com and apple.com directly before) share its 30 s window; how
much each of them spent was not measured. Alone, in a
new session, microsoft.com loaded twice of twice in 5.7 s with its title
and snapshot. Each package that lets more page script run (GC1, MSG1, TW1)
makes this more likely for an agent that goes from one heavy site to the
next in one tab. Changing whose budget it is would reopen D-033 and is not
done here; a navigation the host asks for, as opposed to one the page
starts, could begin a new window without giving a page that reloads itself
a fresh budget.

Still named by pages: `Request` (3), `ReadableStream` (3), `Worker`,
`SVGAnimatedString`, `Element.animate`, an object whose `load` and one whose
`ready` is read (probably `document.fonts`), an unsupported CSS rule type
in `insertRule`; 11 errors are `Failed to fetch` for hosts the sweep
machine's DNS answers with `::`.

### A host navigation starts a new script CPU window (SW1)

D-033 made the script CPU budget the page's and not the navigation's, so
that a page reloading itself cannot spend a new budget each time. The sweep
above showed the other side of that: an agent that goes from one heavy site
to the next in one tab has the last one crashed for what the earlier ones
ran. The owner agreed on 2026-11-17 to change it; the entry for
`docs/spec/18-decisions.md` is still to be written there.

- **What changed:** `Page::begin_navigation` is told who asked (`Requester::
  Host` for `goto`, `goto_with_eval` and `begin_goto`; `Requester::Document`
  for a navigation the document's script requested). When a host navigation
  commits its document — the fetched one or the error document —
  `ScriptCpu::restart` empties the window. The crash latch is not reset: a
  crashed page stays crashed.
- **What did not change:** the budget, the window, the per-entry deadline,
  and a navigation the document starts.
- **What a host can now do that it could not:** navigate one page in a loop
  and have each document spend a full budget. The limit was never a limit on
  the host, which can also open pages.
- **Not covered:** a navigation that follows a click the host dispatched is
  the document's (a link activation or a script handler), so an agent that
  moves between heavy sites by clicking links still shares one window.
  Telling a host-caused activation from a script's own would need user
  activation to be tracked, which the engine does not do.
- **Checks:** `tenta-page::script_cpu` 1 new unit test;
  `tenta-page::runtime_watchdog
  a_navigation_the_host_asks_for_starts_a_new_script_window` (four
  navigations of 60 ms against a 150 ms budget; fails with the restart
  disabled); `tenta-page::persistent_engine
  a_page_that_reloads_itself_spends_one_budget` replaces
  `the_script_budget_spans_navigations`, which drove the old behaviour
  through host navigations: the document now reloads itself and is crashed
  on its third load.
  The workspace passed 4860 of 4860. The feature set passed 594 of 595:
  `tenta-browser::css_loading
  a_stylesheet_is_requested_while_the_body_is_still_streaming` failed once
  and then passed 5 times of 5 alone; it is the test that failed once after
  DSD1 as well, it times a server that waits 1.2 s of real time, and why it
  sometimes fails beside other tests was not established.
- **Result:** one sweep run, same list and driver, 2026-11-17:
  microsoft.com navigated in 6.0 s with its title and snapshot; no page was
  closed for script time. Console errors 53 (52 before), the 35 navigations
  59.7 s (58.5 s before); neither difference is outside what one run varies
  by. amazon.com and imdb.com are still empty behind the operator's
  challenge page.

## Implementation queue and dependencies

- P2 CSSOM uses Stylo per D-003. Inline declarations and constructed
  stylesheets have runtime bindings; native rule handles preserve identity,
  including nested style-rule mutations. Host-owned wrapper state now has
  the directed graph above; other nested rule kinds, import/namespace insertion,
  keyframe child snapshots and HTML style/link integration remain open.
  Inline declarations and CSS.supports now use Stylo on both engines;
  document quirks-mode parsing remains to verify.
- Web Storage now has context-owned origin stores, page session leases and
  local-storage state import/export wiring. Generated named-property bindings
  are under integration testing. StorageEvent delivery, full DOMString fidelity
  and combined context storage/cache accounting remain open.
- XML parsing and DOM serialization already exist. DOMParser,
  XMLSerializer bindings now have focused V8 coverage. Attr-as-Node and
  XMLSerializer attribute handling now have focused coverage; cross-document
  Attr adoption, live Range Attr endpoints and XHR `responseXML` remain open.
  Native `Document::head` now checks both the HTML root and each child's
  namespace, matching the existing body selection discipline. Three new
  regressions failed before integration; the accepted Luna/Sol/root patch is
  under broad runtime validation.
- Constructor length metadata now uses the shortest required arity, but
  generated constructor dispatch still selects the first declared overload.
  Full constructor overload selection and complete DOMString UTF-16 storage
  remain binding gaps; the metadata test does not claim either capability.
- FsPolicy still needs handle-relative protection against directory swaps and
  hard-link creation races. Moving fallible validation before truncation
  does not establish race-free file access.
- Fetch/XHR currently buffer response bodies. Streams, WebSocket,
  EventSource and sendBeacon remain separate implementation work; CORS is
  CO1 above.
- `window.location` has its read side (below). Navigation through
  `location`, `document.location`, History and session history, workers,
  Web Crypto, CSP/frame origin rules, remote objects and Google Full search
  remain open P2 packages.
- The in-process `S-js-xhr` navigation time now has a first baseline.
  Last-mutation-to-settle gap, page RSS delta and reference comparison
  remain unmeasured; they are distinct from the correctness assertions.
- P3 advances through Stylo integration, block/inline layout, remaining
  layout, then paint. P4 needs the page/runtime and geometry foundation.
  P5 follows P2 and can overlap P3/P4; P6 needs audio and compositing;
  P7 needs audio/video and still has the open D-024 crypto decision.
- P8 needs actual device/release evidence. CI currently cross-compiles
  `tenta-core` only for mobile targets. It does not run Android emulators,
  iOS simulators or device tests. Local `adb` and `coturn` were absent at
  inventory time; RTC infrastructure and device access remain to arrange.

The WebIDL manifest is an inventory, not implementation or conformance
evidence. Current CI tests the WPT runner's judge, not the full P2 WPT
directories. Full engine runs need the pinned WPT checkout and drivers.

## Completion rules

- Keep each package's implementation, meaningful checks and independent Sol
  review together; root reviews integration and unused/duplicated code.
- Record unexecuted or failing gates honestly. Never replace conformance
  with stubbed success or weaken a budget to declare completion.
- Use local fixtures and controlled scheduling for regressions, and
  `cargo nextest` for Rust tests.
- Device, multi-OS, fuzz-duration and real-site gates require their stated
  evidence. Missing environment access does not justify marking them done;
  continue other implementation and validation while resolving access.
