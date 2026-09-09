//! DOM binding glue generated from `idl/dom` by `tenta-idl` (see `build.rs`).
//!
//! The engine-independent output (`prelude.rs`, `types.rs`, the `*Ops` traits
//! and the `register_*` functions) is always compiled. The V8 glue (`*_v8.rs`)
//! and QuickJS glue (`*_qjs.rs`) are compiled only behind their features, which
//! pull the corresponding engine crate.
//!
//! Everything is included into this one module so the generated files see each
//! other and the shared prelude. Dead code is expected until each interface is
//! wired to a host, so the generated items are allowed to be unused. A
//! member-less interface's V8 installer also builds a `Signature` it does not
//! use, so `unused_variables` is allowed for the generated glue too.
#![allow(dead_code)]
#![allow(unused_variables)]
// A member-less interface's generated `construct_*` throw stub carries the host
// type parameter for signature uniformity but never names it.
#![allow(clippy::extra_unused_type_parameters)]
// A fast getter/operation whose return type is already the V8 fast width (e.g.
// an `unsigned long` returning `u32`) still emits the widening `as` cast for
// uniformity, so the cast is a no-op the generator does not special-case.
#![allow(clippy::unnecessary_cast)]
// A void operation's receiver-dispatch glue is emitted as a uniform two-arm
// `match` (`Some(host) => host.op(..), None => ()`); the generator does not
// special-case the void arm into an `if let`.
#![allow(clippy::single_match)]
// A nullable-`DOMString` member wraps its value through `Ok(..).map(Some)` for
// uniformity with the fallible-conversion path, which clippy reads as a
// redundant map on the `Ok` constructor.
#![allow(clippy::unnecessary_map_on_constructor)]

include!(concat!(env!("OUT_DIR"), "/prelude.rs"));
include!(concat!(env!("OUT_DIR"), "/types.rs"));

include!(concat!(env!("OUT_DIR"), "/character_data_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/event_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/event_target_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/comment_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/document_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_collection_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/node_list_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/node_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/text_ops.rs"));

include!(concat!(env!("OUT_DIR"), "/character_data_register.rs"));
include!(concat!(env!("OUT_DIR"), "/event_register.rs"));
include!(concat!(env!("OUT_DIR"), "/event_target_register.rs"));
include!(concat!(env!("OUT_DIR"), "/comment_register.rs"));
include!(concat!(env!("OUT_DIR"), "/document_register.rs"));
include!(concat!(env!("OUT_DIR"), "/element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_collection_register.rs"));
include!(concat!(env!("OUT_DIR"), "/node_list_register.rs"));
include!(concat!(env!("OUT_DIR"), "/node_register.rs"));
include!(concat!(env!("OUT_DIR"), "/text_register.rs"));

#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/character_data_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/event_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/event_target_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/comment_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/document_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_collection_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/node_list_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/node_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/text_v8.rs"));

pub mod host;
