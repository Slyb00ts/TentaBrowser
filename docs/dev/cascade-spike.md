# Cascade spike (package C0): decision record

Status: complete. Date: 2026-10-09. Evidence program:
`crates/tenta-css/examples/cascade_spike.rs` (throwaway; package C1a deletes it).
Run it with

```bash
CARGO_INCREMENTAL=0 cargo run --release -p tenta-css --example cascade_spike --features stylo
```

All `stylo:` references are to `stylo-0.21.0` in
`~/.cargo/registry/src/*/stylo-0.21.0/`, `path:line`. `selectors:` is
`selectors-0.40.0`.

## 1. Decision

**GO for full restyle per flush: a live adapter over `&Document`, with corrections to plan 4.1.**

Scope of the GO: every flush restyles the whole tree from scratch, which is
what M1 does (plan F6). Snapshots and restyle hints are not exercised
(`has_snapshot() == false`, `SnapshotMap` empty). Incremental restyle (C3a/C3b)
needs a different ownership: a **persistent `StyleStore`** owned by the
`StyleEngine` (`ElementDataWrapper` + has-data flag + hint and dirty bits),
keyed by arena slot **and generation** (freed slots are recycled with a bumped
generation, `arena.rs:455` and `:572`; a slot index alone would hand a new node
the old node's `ElementData`), with a per-flush view of one-word handles into
it. The spike's per-flush `Slot` table is the degenerate case of that design.

The spike parses a 5-element document with `tenta-html` into a `tenta-dom`
`Document`, implements `TDocument`/`TNode`/`TElement`/`TShadowRoot` and
`selectors::Element` over it, runs `style::driver::traverse_dom` sequentially
with a `Stylist` holding one UA rule and one author rule, and reads the
computed values back. Output (debug and release identical):

```
<div #5>: color = rgb(0, 128, 0), display = block
<p #6>: color = rgb(0, 128, 0), display = block
```

`div` matches the author rule `body > div.x { color: rgb(0, 128, 0) }` and the
UA rule `div, p, body, html { display: block }`. `p` matches only the UA rule
for `display`, and inherits `color` through the cascade. So: the UA origin, the
author origin, a child combinator, a class selector, inheritance and
blockification all work through the adapter.

The adapter reads tags, attributes, classes, ids, text and tree structure live
from `&Document`. Nothing is snapshotted from the DOM. What is built per flush
is Stylo's own state (one record per arena slot) and small atom caches.

Corrections to plan 4.1 (both are hard requirements found by running it):

1. **A `TElement` must be exactly one machine word.** The plan's
   `ArenaElement<'a>` over `(&Document, &StyleStore)` is two references (or
   `(&ctx, NodeId)` is 16 bytes). That compiles and then panics at the first
   traversal: `assertion left == right failed, left: 9744, right: 9488` at
   `stylo: sharing/mod.rs:611`. The style sharing cache stores candidate
   elements in a thread-local typed as `usize`
   (`FakeCandidate._element: usize`, `sharing/mod.rs:324`;
   `TypelessSharingCache`, `:561`; size and alignment asserts, `:611-618`).
   These are `assert_eq!`, not `debug_assert!`, so they fire in release too.
   The spike's handle is `struct Element<'a>(&'a Slot<'a>)` and a
   compile-time `const _: () = assert!(size_of::<Element>() == size_of::<usize>())`
   keeps it so.
2. **`Vec<AtomicRefCell<Option<ElementData>>>` does not satisfy the trait.**
   See 3.2. The side table holds `style::data::ElementDataWrapper`.

Neither correction changes the `StyleEngine` API in 4.1. They change `mod dom`
internals (C1a/C1b). A third finding (5.3, the single `SharedRwLock`) **does**
change the public `cssom.rs` API: `CssStyleSheet` constructors take the
engine's lock, so JS-held constructed sheets must be created against it.

## 2. Trait bounds (answers 1, 2 and 7)

### 2.1 `TElement`, `TNode`, `TDocument`, `TShadowRoot`

| Trait | Supertraits (stylo) |
| --- | --- |
| `TDocument` (`dom.rs:119`) | `Sized + Copy + Clone` |
| `TNode` (`dom.rs:153`) | `Sized + Copy + Clone + Debug + NodeInfo + PartialEq` |
| `TShadowRoot` (`dom.rs:364`) | `Sized + Copy + Clone + Debug + PartialEq` |
| `TElement` (`dom.rs:409-419`) | `Eq + PartialEq + Debug + Hash + Sized + Copy + Clone + selectors::Element<Impl = SelectorImpl> + ElementContext` |

- **No `Send` or `Sync` on any of them.** `dom.rs:1124-1128` says so
  explicitly: "TNode and TElement aren't Send because we want to be careful and
  explicit about our parallel traversal". The parallel path wraps handles in
  `SendNode`/`SendElement`, whose constructors are `unsafe` and whose `Send`
  impls are `unsafe impl` (`dom.rs:1131-1132`, `:1149-1150`). So `Send`/`Sync`
  is a proof obligation on the adapter's data, not a bound on the types.
- `Copy + Clone + Hash + Eq` are needed by sequential traversal too (the
  sharing cache and bloom filter hold elements by value; `Hash` is used by
  `TElement` consumers such as `HashSet<E>` in invalidation).
- `ElementContext` has a blanket impl for every `TElement` (`dom.rs:1035`).
- `selectors::Element` (`selectors: tree.rs:46`) requires only
  `Sized + Clone + Debug`.
- `TElement` has about 45 required methods. The set I had to implement is in 7.

### 2.2 Does the traversal need `Sync`? Is `&Document` accepted?

- `DomTraversal<E>: Sync` (`stylo: traversal.rs:57`). That bounds the
  *traversal struct* (here `RecalcStyle { shared: SharedStyleContext }`), not
  `E`. The same applies to the sequential path: `with_pool_in_place_scope`
  takes `impl FnOnce + Send + 'scope` (`driver.rs:49-53`) and is called with
  `pool = None` too, and `ScopedTLS<T>: Sync` needs `T: Send`
  (`scoped_tls.rs:28`). All of these compile with a handle that holds `&Document`
  (`Document` is `Send` but not `Sync`, plan F7), because
  `StyleSharingCache` stores `PhantomData<SendElement<E>>`
  (`sharing/mod.rs:578`) and the traversal queue stores `SendNode` (`driver.rs`,
  `parallel.rs:149`).
- `SharedStyleContext` (`context.rs:126`) is `Sync` as long as the
  `&dyn RegisteredSpeculativePainters` field (a `Sync` trait, `context.rs:704`)
  is. The spike supplies a unit struct returning `None`.
- **Verdict: `&Document` (not `Sync`) is accepted for sequential traversal**,
  with no `unsafe impl`. The spike compiles and runs with
  `Slot { ctx: &Ctx, .. }`, `Ctx { doc: &Document, Cell/OnceCell/RefCell .. }`.
- Parallel traversal would additionally need: every `Cell`/`OnceCell`/`RefCell`
  in the context replaced with atomics or `OnceLock`, and an
  `unsafe impl Sync` wrapper asserting that `Document` is only read through
  methods that do not touch its `RefCell` indexes. That is package C3b.

## 3. Where `ElementData` lives (answer 3)

### 3.1 The type

`ElementData` is `style::data::ElementData` (`data.rs:288`, 24 bytes,
`size_of_test!` just after the borrow impls). It is not stored in an `AtomicRefCell` by the style
crate. `TElement` hands out `ElementDataRef<'_>` and `ElementDataMut<'_>`
(`dom.rs:739-753`), which are opaque structs with private fields
(`data.rs:315-330`). The only constructors are
`ElementDataWrapper::borrow()` and `borrow_mut()` (`data.rs:332-353`), and
`ElementDataWrapper` (`data.rs:306`) is an `UnsafeCell<ElementData>` plus a
debug-only `AtomicRefCell<()>` used for aliasing checks.

### 3.2 Does `Vec<AtomicRefCell<Option<ElementData>>>` work?

**No, not as written.** An `AtomicRef<ElementData>` cannot be turned into an
`ElementDataRef` because its fields are private. The side table must hold
`ElementDataWrapper` values, plus a "has data" flag because
`has_data`/`borrow_data`/`mutate_data` return `false`/`None` until
`ensure_data` has been called (`dom.rs:739-753`).

What the spike does (and what satisfies all five methods):

```text
struct Slot<'a> { ctx: &'a Ctx<'a>, id: NodeId, data: ElementDataWrapper,
                  has_data: Cell<bool>, dirty_descendants: Cell<bool>,
                  selector_flags: Cell<ElementSelectorFlags>,
                  children_to_process: Cell<isize> }
ensure_data  : has_data.set(true); data.borrow_mut()
clear_data   : if has_data.replace(false) { *data.borrow_mut() = ElementData::default() }
has_data     : has_data.get()
borrow_data  : has_data().then(|| data.borrow())
mutate_data  : has_data().then(|| data.borrow_mut())
```

`ElementData` is not reset between flushes by Stylo; C2a either keeps the
`Slot` table across flushes (needed for incremental restyle, C3a/C3b) or
rebuilds it per flush for M1.

### 3.3 One word per handle: the slot table

Because the handle must be one word (section 1), it is a reference to its own
`Slot`. Slots are built after the context and installed once:

```text
let ctx = Ctx::new(doc);
let slot_table: Vec<Slot<'_>> = (0..doc.slots()).map(|i| Slot::new(&ctx, i)).collect();
ctx.slots.set(&slot_table);          // OnceCell<&[Slot]>
```

`Slot` -> `Ctx` -> `[Slot]` is a reference cycle through a `OnceCell`. It
compiles in safe Rust (no `unsafe`, no `Rc`) because `Slot` has no `Drop` impl
and `Vec`'s destructor is `#[may_dangle]`. The declaration order
(`ctx` before `slot_table`) matters. Navigation is
`ctx.slot(doc.parent(id))`: an index, not a stored link.

## 4. `thread_state` (answer 4)

Stylo's `thread_state` (`thread_state.rs`) is a thread-local `Cell<Option<ThreadState>>`.
It is consulted only by `debug_assert!`s:
`SequentialTask::execute` (`context.rs:466`) and
`SequentialTaskList::drop` (`context.rs:527`), both of which require
`ThreadState::LAYOUT`. Measured: running the debug build with the spike's
`--no-thread-state` flag panics with
`assertion failed: thread_state::get().contains(ThreadState::LAYOUT)` at
`context.rs:527`; the release build runs without it (no-op).

Use `thread_state::enter(ThreadState::LAYOUT)` / `exit(ThreadState::LAYOUT)`
around the traversal (`thread_state.rs:72-86`), not `initialize`. The spike
calls them bare; the engine needs an RAII guard so `exit` runs on unwind, plus
`catch_unwind` around the traversal. After a caught panic the thread-local
style sharing cache must be reset: `StyleSharingCache::new` has
`debug_assert!(cache.is_empty())` (`sharing/mod.rs:619`) and the leaked
thread-local cache is reused, so a panic that skips its `Drop` would trip it
(and leave stale elements) on the next flush. Clear it explicitly after a panic.
`initialize` (`:50`) is permanent and panics if the thread is later initialised
to a different state; the page thread is shared with other work. `enter` asserts
(in debug) that the flag is not already set, so it is not reentrant: wrap it in
one RAII guard in `StyleEngine::flush`.

Process-global preferences are separate and unchanged: `configure_stylo()`
(`cssom.rs:1516-1527`) must have run before the first parse. The spike calls
`stylo_static_prefs::set_pref!` directly for two prefs; C2a must call the shared
function. `traverse_dom` also reads `layout.css.stylo-work-unit-size` and
`layout.css.stylo-local-work-queue.*` prefs (`work_unit_max()` in `driver.rs`, `style_trees` in `parallel.rs:149`),
which have defaults.

## 5. `Device`, `Stylist` and the sheets (answers 5 and 6)

### 5.1 Constructing a Servo-flavour `Device`

`Device::new` (`device/servo.rs:85`) takes ten arguments; the spike passes:

| Argument | Spike value | Note |
| --- | --- | --- |
| `media_type` | `MediaType::screen()` | `media_queries/media_query.rs:36`; print uses another |
| `quirks_mode` | from `Document::quirks_mode()` | `style::context::QuirksMode` = `selectors::matching::QuirksMode` |
| `viewport_size` | `euclid::Size2D<f32, CSSPixel>` (800, 600) | CSS px |
| `device_size` | `Size2D<f32, DevicePixel>` | same as viewport here |
| `device_pixel_ratio` | `Scale<f32, CSSPixel, DevicePixel>` 1.0 | |
| `font_metrics_provider` | `Box<dyn FontMetricsProvider>` | see 5.2 |
| `default_values` | `ComputedValues::initial_values_with_font_override(Font::initial_values())` | `properties.mako.rs:1873`, `:1415`; initial `font-size` is `medium` = 16px, initial `font-family` is the `serif` generic |
| `prefers_color_scheme` | `PrefersColorScheme::Light` | `style::queries::values` |
| `primary_pointer_capabilities`, `all_pointer_capabilities` | `PointerCapabilities::default()` | `style::servo::media_features` |

`medium = 16px` is the constant `FONT_MEDIUM_PX` (`values/specified/font.rs:797`)
that seeds `root_font_size`; the root element's real font size replaces it when
the root is styled (`matching.rs:1023-1045`, driven by
`ComputedValueFlags::IS_ROOT_ELEMENT_STYLE`, which the cascade sets when
`selectors::Element::is_root()` is true).

`euclid` is not re-exported by `style`; building these arguments needs a direct
`euclid` dependency (the spike adds it as a dev-dependency). `app_units::Au` is
re-exported as `style::values::computed::Au`.

Later changes go through `Device::set_viewport_size`/`set_device_pixel_ratio`/
`set_color_scheme`/`set_media_type` (`device/servo.rs`, each documented as not
updating the `Stylist`), then `Stylist::set_device` (`stylist.rs:1655`), which
returns the origins to invalidate.

### 5.2 What the font-metrics hook must answer

`FontMetricsProvider: Debug + Sync` (`device/servo.rs:43`):

- `query_font_metrics(vertical, &Font, base_size, QueryFontMetricsFlags) -> FontMetrics`.
  Called for `ex`, `ch`, `cap`, `ic` units and `font-size-adjust`. The returned
  `FontMetrics` (`font_metrics.rs:14`) has `x_height`, `zero_advance_measure`
  (`ch`), `cap_height`, `ic_width` as `Option<Length>` (None means "use the
  fallback ratio") and a mandatory `ascent`. `QueryFontMetricsFlags::NEEDS_CH`/
  `NEEDS_IC` say which are required (`values/specified/font.rs:2073`).
  The spike answers fixed proportions of `base_size`; the real provider asks
  the font system for the face resolved from `font.font_family`, weight and
  style, which is exactly the `FontMetrics` trait in plan 4.1.
- `base_size_for_generic(GenericFontFamily) -> Length`: 16px for everything but
  `monospace` (13px), matching plan 4.1's "default monospace size 13px".

The bound is `Sync`, so the provider must be an `Arc`/`Box` of a `Sync` type;
plan 4.1's `Arc<dyn FontMetrics>` with `FontMetrics: Send + Sync` fits.

### 5.3 UA and author sheets into a `Stylist`

```text
Stylist::new(device, quirks)                                   // stylist.rs:860
stylist.append_stylesheet(DocumentStyleSheet(Arc<Stylesheet>), &guard)   // :1117
stylist.flush(&StylesheetGuards::same(&guard))                 // :1055
```

- `StylistSheet = DocumentStyleSheet` in the Servo flavour (`stylist.rs:103`,
  `stylesheet.rs:289`). The origin is a property of the `Stylesheet`, set at
  parse time: `Stylesheet::from_str(css, UrlExtraData, Origin::UserAgent | Origin::Author, media, lock, loader, reporter, quirks, AllowImportRules)`
  (`stylesheets/stylesheet.rs:499`). Order of `append_stylesheet` calls is the
  document order within an origin; `insert_stylesheet_before` (`:1100`) and
  `remove_stylesheet` exist for the incremental case.
- `flush` is idempotent: it returns early when `!stylesheets.has_changed()`
  (`stylist.rs:1056`) and otherwise rebuilds cascade data and returns a
  `StylesheetInvalidationSet`. The spike's flush cost is in 6.
- **One `SharedRwLock` for everything, owned by `StyleEngine` and outliving
  every flush** (a new lock per flush would invalidate every parsed sheet and
  `style=""` block). `Locked::read_with` is a release
  `assert!` that the guard belongs to the same lock (`shared_lock.rs:138-143`).
  `StylesheetGuards { author, ua_or_user }` (`:239-262`) lets the two origins
  use different locks, but every sheet inside one origin, the `style=""` blocks,
  and `TDocument::shared_lock()` (`dom.rs:148`) must share it. The spike owns one
  lock in `Ctx` and parses both sheets with it.
- **Conflict with plan 4.1:** `CssStyleSheet::parse_with_loader`
  (`crates/tenta-css/src/cssom.rs:900`) creates `SharedRwLock::new()` per
  sheet. Two such sheets in one `Stylist` panic at the first read. C2a (or C1b)
  must give the constructor a lock parameter owned by the `StyleEngine`.
  `CssStyleSheet::parse_with_loader`, `TDocument::shared_lock` and
  style-attribute parsing all take that lock. **This is a public `cssom.rs`
  API change**: JS-held constructed sheets (`CSSStyleSheet`) must be created
  against the engine lock, and the JS bindings (`tenta-js` cssom host) are
  affected.

## 6. Measurements (answer 8)

Machine: Mac16,10, 10 cores, macOS, release build (`opt-level` as the
workspace release profile, debug info on). **The machine was heavily loaded by
other agents' builds (load average 17-23 on 10 cores) during every run.**
**All timings in this document were taken at load about 21 and must be
re-measured on an idle machine** before being quoted or used as a baseline. I
report the median and the minimum over many separate process runs; the minimum
is only a best-case bound.

### 6.1 Atom mapping cost per selector hook

Methodology: `ns_per_op` in the example runs the operation 20,000,000 times
(5,000,000 for the dynamic-atom rows), cycling over 8 tag atoms (5 class
strings), `black_box`-ing each result, and divides wall time by iterations.
11 process runs. The "mapped" value is a `style::LocalName` built from the tenta
`Atom` and compared with a selector-side `LocalName` (`has_local_name` shape).

| Operation | median ns | min ns |
| --- | ---: | ---: |
| tenta `Atom == Atom` (baseline, no mapping) | 0.46 | 0.34 |
| tenta `Atom::as_str()` (RwLock read + index) | 4.6 | 4.1 |
| `as_str()` then string compare with style name | 6.7 | 4.9 |
| **naive map** `LocalName::from(tag.as_str())` + `==` | **14.0** | 10.5 |
| **table map** `Vec<OnceCell<LocalName>>[atom.raw()]` + `==` | **1.25** | 0.68 |
| `style::Atom::from(&str)`, static atom | 9.0 | 5.2 |
| `style::Atom::from(&str)`, dynamic (the atom is dropped each iteration, so the set insert and remove are measured) | 89.5 | 70.8 |
| dynamic atom `clone()` | 3.8 | 2.3 |
| class string compare (`has_class` path, no atoms) | 7.6 | 4.3 |

So mapping costs about **1.3 ns per hook with a per-flush table** and about
14 ns naive. The table is a `Vec<OnceCell<LocalName>>` sized to
`Atom::count()` and filled on first use, because `TElement::local_name()` and
`namespace()` return references (`dom.rs:955-959`), which rules out converting
on every call.

### 6.2 Whole-traversal cost and hook counts

4,003 elements (generated `div.card-container-N.item-title-link#nI > p.body-paragraph-text[data-k]`),
42 author rules plus the UA rule, sequential, one warm-up run then one measured
run, 21 process runs:

| | median | min |
| --- | ---: | ---: |
| traversal (all hooks, table mapping) | 5.0 ms (1.25 us/element) | 2.77 ms (0.69 us/element) |
| context + slot table build | 39 us | 36 us |
| `Stylist::flush` | 15 us | 8 us |

5-element document, 15 runs: context 6.5 us, flush 18.5 us, traversal 39 us.
(The first run in a fresh process is 5-10x slower: 150-260 us / 120-340 us /
600-760 us; static atom tables and the allocator are cold.)

Hook calls in that traversal: `local_name` 98,758 (about 25 per element: bloom
filter and sharing cache), `id` 98,756, `each_class` 18,656, `has_local_name`
4,109, `attr_matches` 2,306, `has_class`/`has_id` 636 (the style sharing cache
resolves most elements without matching).

Mapping cost in the traversal: `local_name` x 1.25 ns = 0.12 ms, about 4% of
the 2.77-5.0 ms. With the naive 14 ns mapping it would be 1.4 ms, 28-50%.
Per selector match (`has_local_name`) the table mapping is the 1.25 ns above.

Dynamic class atoms: building an `AtomIdent` per `each_class` call versus
caching by string in a `HashMap` showed no difference in my runs (medians 5.0 vs
5.1 ms, minima 2.78 vs 2.77 ms), but a reviewer's single run showed the opposite
(2.9 ms uncached vs 6.3 ms cached) under the same load. The measurement is
inconclusive, so **this document makes no class-atom cache recommendation**;
the spike's per-call path is the default until an idle-machine benchmark
decides. The 90 ns micro-benchmark row is a worst case (atom dropped each
iteration).

Note on "context + slot table build" (36-43 µs for 4,003 slots): this is the
measured run after a warm-up run in the same process, so the roughly 2,000 id
atoms were already in the dynamic atom set (lookup, about 14 ns each, not
insert) and the slot table is one allocation of plain structs. It is not the
cold cost: the first run in a fresh process took 150-260 µs for only five
elements. Treat the 36-43 µs as a warm lower bound, not a budget.

Budget context: plan 8 item 7 budgets 8 ms initial traversal on 4 threads for the
M1 pages; this single-threaded figure for 4k elements is well under it.

## 7. What C1a, C1b and C2a must implement

### 7.1 Required by the traits (all done in the spike unless marked)

`TDocument`: `as_node`, `is_html_document`, `quirks_mode`, `shared_lock`.
`elements_with_id` is optional (default `Err(())`); implementing it needs a
`&'a [Element]` slice per id and is a later optimisation.

`TNode`: `parent_node`, `first_child`, `last_child`, `prev_sibling`,
`next_sibling`, `owner_doc`, `is_in_document`, `traversal_parent`, `opaque`
(`OpaqueNode(usize)`), `debug_id`, `as_element`, `as_document`,
`as_shadow_root`; plus `NodeInfo` (`is_element`, `is_text_node`).

`TShadowRoot`: `as_node`, `host`, `style_data`. The spike uses an uninhabited
enum. `Node::ConcreteShadowRoot` must exist even with no shadow DOM.

`TElement` (required methods only): `as_node`, `traversal_children`
(`LayoutIterator<I>`), `is_html_element`, `is_mathml_element`, `is_svg_element`,
`style_attribute`, `animation_rule`, `transition_rule`, `state` (returns
`dom::ElementState`, see item 4), `has_part_attr`, `exports_any_part`, `id`,
`each_class`, `each_custom_state`, `each_attr_name`, `has_dirty_descendants`,
`has_snapshot`, `handled_snapshot`, `set_handled_snapshot`*,
`set_dirty_descendants`*, `unset_dirty_descendants`*, `store_children_to_process`,
`did_process_child`, `ensure_data`*, `clear_data`*, `has_data`, `borrow_data`,
`mutate_data`, `skip_item_display_fixup`, `may_have_animations`,
`has_animations`, `has_css_animations`, `has_css_transitions`, `shadow_root`,
`containing_shadow`, `lang_attr`, `match_element_lang`,
`is_html_document_body_element`,
`synthesize_presentational_hints_for_legacy_attributes`, `local_name`,
`namespace`, `query_container_size`, `has_selector_flags`,
`relative_selector_search_direction`, `get_attr`. (* = `unsafe fn`, forced by
the trait.)

`selectors::Element`: `opaque`, `parent_element`, `parent_node_is_shadow_root`,
`containing_shadow_host`, `is_pseudo_element`, `prev_sibling_element`,
`next_sibling_element`, `first_element_child`, `is_html_element_in_html_document`,
`has_local_name`, `has_namespace`, `is_same_type`, `attr_matches`,
`match_non_ts_pseudo_class`, `match_pseudo_element`, `apply_selector_flags`,
`is_link`, `is_html_slot_element`, `has_id`, `has_class`, `has_custom_state`,
`imported_part`, `is_part`, `is_empty`, `is_root`, `add_element_unique_hashes`.

`DomTraversal<E>`: `process_preorder` (ensure data, call `recalc_style_at`,
unset the dirty-descendants bit), `process_postorder` (no-op),
`needs_postorder_traversal() -> false`, `shared_context`. Entry:
`DomTraversal::pre_traverse(root, shared)` then `traverse_dom(&traversal, token, None)`.

### 7.2 Stylo-facing types the packages own

- **C1a (DOM adapter, tree and selectors):** the `Slot`/handle design (one word),
  `StyleStore` of slots, the `Ctx` caches (`Vec<OnceCell<LocalName>>` keyed by
  `Atom::raw()`, per-slot `id` atoms, namespace table), all
  `TNode`/`TDocument`/`TShadowRoot`/`selectors::Element` methods, the
  non-tree-structural pseudo-classes via `NonTSPseudoClass::state_flag()`
  (`servo/selector_parser.rs:494`) over `dom::ElementState`, `:link`/`:any-link`
  via `is_link`, and `:lang`. It also deletes `StyleHandle`/`Elements.style_data`
  (plan F7) and this example.
  - The `local_names` table and the id-atom table should be **persistent and
    grow-only** (owned by the engine, extended as new atoms/ids appear, never
    rebuilt per flush); the spike rebuilds them per flush only for simplicity.
    Per-slot id atoms are keyed by slot and generation like the rest of the store.
- **C1b (style store, `style=""`, presentational hints):** `style_attribute()`
  returns `Option<ArcBorrow<'_, Locked<PropertyDeclarationBlock>>>` tied to
  `&self`, so parsed blocks (from `parse_style_attribute`,
  `properties/declaration_block.rs:1418`, using the engine's one lock) live in
  the store keyed by (node, attribute revision). `synthesize_presentational_hints_for_legacy_attributes<V: Push<ApplicableDeclarationBlock>>`
  (`dom.rs:938`). `lang_attr`/`match_element_lang` (`AtomString`, `Lang = Box<str>`).
  `get_attr` for `attr()`.
- **C2a (`StyleEngine`):** `Device`/`FontMetricsProvider` (5.1, 5.2), `Stylist`
  ownership, the single `SharedRwLock`, `StylesheetGuards::same`, flush ordering
  (`append_stylesheet` x N, `Stylist::flush`, `SharedStyleContext`, `pre_traverse`,
  `traverse_dom`), `thread_state` guard (4), `configure_stylo()` before the first
  parse, `SnapshotMap::new()` (`servo/selector_parser.rs:797`),
  `DocumentAnimationSet::default()`, `RegisteredSpeculativePainters` stub,
  reading `data.styles.primary()` into `Style`, `set_device` on env changes.

### 7.3 Smaller facts the spike pinned down

1. `Servo` flavour atoms: `style::Atom = stylo_atoms::Atom` (a
   `string_cache::Atom<AtomStaticSet>`); `LocalName`, `Namespace`, `Prefix` are
   `GenericAtomIdent<web_atoms::...StaticSet>` (`lib.rs:149-159`);
   `SelectorImpl::BorrowedLocalName = web_atoms::LocalName` (`servo/selector_parser.rs:578`),
   which is `Deref` target of `style::LocalName`. `TElement::id()` returns
   `Option<&style::Atom>` (`WeakAtom` is an alias, `lib.rs:200`), which forces a
   per-element id-atom table.
2. `AttrValue` for attribute selectors is `AtomString` (`values/mod.rs:164`),
   used through `AsRef<str>`; `lang_attr` uses the same type, not
   `style::attr::AttrValue`.
3. `ElementSelectorFlags` must be stored per element and returned by
   `has_selector_flags`/`relative_selector_search_direction`; the spike keeps a
   `Cell<ElementSelectorFlags>` per slot.
4. `dom::ElementState` (crate `stylo_dom`, import name `dom`) is not re-exported
   by `style`; naming it in the `state()` signature needs a direct dependency
   (the spike adds `dom = { package = "stylo_dom", version = "=0.21.0" }`).
5. `ElementState` has `empty()` but no `Default` impl.

## 8. Unsafe required

Under AGENTS.md and `docs/spec/13-performance.md` 5:

- **Sequential M1 path: no `unsafe` blocks of our own except two call sites.**
  The trait forces five `unsafe fn` signatures
  (`set_handled_snapshot`, `set_dirty_descendants`, `unset_dirty_descendants`,
  `ensure_data`, `clear_data`; `dom.rs:648,675,680,739,744`). Their bodies are
  type-safe (Cell/`ElementDataWrapper`), but not free of aliasing risk: in
  release builds the `AtomicRefCell` borrow checks inside `ElementDataWrapper`
  are compiled out (`data.rs:306-310`, `#[cfg(debug_assertions)]`), so
  `borrow_data`/`mutate_data`/`clear_data` are unchecked `&`/`&mut` over an
  `UnsafeCell`. If Stylo ever held a `borrow_data` result across `clear_data`
  (or two `mutate_data`s) that would be undefined behaviour in release and only
  a panic in debug. Sequential traversal does not do this (`clear_data` is
  reached from `clear_descendant_data` in `recalc_style_at` and clears only
  descendants, never the element whose `ElementData` borrow is held),
  which is the safety argument; keep debug-assertion CI runs of the traversal
  tests to catch violations. The two call sites
  (`process_preorder` calling `ensure_data` and `unset_dirty_descendants`) are
  `unsafe { }` with the justification "sequential traversal, no other borrow of
  this element's data".
- **No `unsafe impl Send/Sync`, no raw pointers, no transmute.** The
  self-referential slot table (3.3) is safe Rust.
- **Parallel traversal (C3b) needs real `unsafe`:** an `unsafe impl Sync` wrapper
  over `&Document` (reading only methods that skip its `RefCell`s), atomics for
  every `Cell` in `Slot`/`Ctx`, `OnceLock` for the name tables, and
  `SendNode::new`/`SendElement::new` calls are inside stylo. The scalar
  reference is the sequential path, which stays; the differential test is
  sequential-vs-parallel computed values; the benchmark is the 4k-element
  traversal above.
- Wrap `traverse_dom` in `catch_unwind` at the engine boundary (AGENTS.md native
  boundary rule) since stylo asserts abort the page thread otherwise.

## 9. Verification performed

- `cargo run -p tenta-css --example cascade_spike --features stylo` (debug,
  stylo debug assertions on): printed the values above; `--no-thread-state`
  panics as described.
- `cargo run --release ...`: same values; numbers in section 6 (many runs, loaded
  machine).
- `cargo clippy -p tenta-css --example cascade_spike --features stylo -- -D warnings`: clean.
- `rustfmt --edition 2024 --check` on the example: clean.
- Not run: `cargo nextest` (the spike adds no tests), the full workspace
  clippy/fmt gate, `cargo deny`, `cargo xtask lint-deps` (the new dev-dependencies
  `euclid` and `stylo_dom` are already in `Cargo.lock`; `tenta-html` is a
  workspace path crate), QuickJS arm, Linux/Windows/Android/iOS builds, a
  multi-threaded traversal, `:has()`/relative-selector invalidation, shadow DOM,
  `::before`/`::after` pseudo-elements, and any idle-machine benchmark.
