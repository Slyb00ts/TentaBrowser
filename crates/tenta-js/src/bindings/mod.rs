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
// A dictionary's `from_v8` reads each member with `self.member.clone()` for
// uniformity across member types; for a `Copy` member (a `boolean` or an `any`)
// clippy reads the clone as redundant.
#![allow(clippy::clone_on_copy)]
// A nullable member wrapped in an optional (`Option<Option<T>>`: present, then
// nullable) serializes through a uniform outer/inner `match`; clippy reads the
// inner `match` as collapsible into the outer one.
#![allow(clippy::collapsible_match)]

include!(concat!(env!("OUT_DIR"), "/prelude.rs"));
include!(concat!(env!("OUT_DIR"), "/types.rs"));

include!(concat!(env!("OUT_DIR"), "/character_data_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/event_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/custom_event_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/ui_event_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/mouse_event_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/htmli_frame_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/svg_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/document_fragment_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_template_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/event_target_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/comment_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/document_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_collection_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/node_list_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/dom_token_list_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/css_style_declaration_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_div_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_span_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_paragraph_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_anchor_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_image_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_label_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_form_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_input_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_button_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_select_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_text_area_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_option_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_script_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/node_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/text_ops.rs"));

include!(concat!(env!("OUT_DIR"), "/character_data_register.rs"));
include!(concat!(env!("OUT_DIR"), "/event_register.rs"));
include!(concat!(env!("OUT_DIR"), "/custom_event_register.rs"));
include!(concat!(env!("OUT_DIR"), "/ui_event_register.rs"));
include!(concat!(env!("OUT_DIR"), "/mouse_event_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/htmli_frame_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/svg_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/document_fragment_register.rs"));
include!(concat!(
    env!("OUT_DIR"),
    "/html_template_element_register.rs"
));
include!(concat!(env!("OUT_DIR"), "/event_target_register.rs"));
include!(concat!(env!("OUT_DIR"), "/comment_register.rs"));
include!(concat!(env!("OUT_DIR"), "/document_register.rs"));
include!(concat!(env!("OUT_DIR"), "/element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_collection_register.rs"));
include!(concat!(env!("OUT_DIR"), "/node_list_register.rs"));
include!(concat!(env!("OUT_DIR"), "/dom_token_list_register.rs"));
include!(concat!(
    env!("OUT_DIR"),
    "/css_style_declaration_register.rs"
));
include!(concat!(env!("OUT_DIR"), "/html_div_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_span_element_register.rs"));
include!(concat!(
    env!("OUT_DIR"),
    "/html_paragraph_element_register.rs"
));
include!(concat!(env!("OUT_DIR"), "/html_anchor_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_image_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_label_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_form_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_input_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_button_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_select_element_register.rs"));
include!(concat!(
    env!("OUT_DIR"),
    "/html_text_area_element_register.rs"
));
include!(concat!(env!("OUT_DIR"), "/html_option_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_script_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/node_register.rs"));
include!(concat!(env!("OUT_DIR"), "/text_register.rs"));

#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/character_data_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/event_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/custom_event_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/ui_event_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/mouse_event_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/htmli_frame_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/svg_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/document_fragment_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_template_element_v8.rs"));
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
include!(concat!(env!("OUT_DIR"), "/dom_token_list_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/css_style_declaration_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_div_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_span_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_paragraph_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_anchor_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_image_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_label_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_form_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_input_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_button_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_select_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_text_area_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_option_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_script_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/node_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/text_v8.rs"));

pub mod host;

/// Runtime coverage for the mechanisms the generated pair-iterator glue rests
/// on: the two prelude helpers a `iterable<K, V>` declaration is built from,
/// and the V8 template behaviour that makes `@@iterator` identical to
/// `entries`.
///
/// The generated `entries`/`keys`/`values` callbacks need an interface with a
/// host behind it, and nothing wires a pair-iterable interface up yet, so these
/// tests reach the helpers directly. Without them `array_iterator` and the
/// rethrow contract of `call_js_function` would only ever be compiled, never
/// run.
#[cfg(all(test, feature = "v8"))]
mod prelude_helpers {
    use super::*;

    /// Runs `check` on a fresh isolate with one entered context.
    fn with_context(check: impl FnOnce(&mut v8::PinScope<'_, '_>)) {
        // The V8 platform is process-global and may be initialised only once.
        static PLATFORM: std::sync::Once = std::sync::Once::new();
        PLATFORM.call_once(|| {
            let platform = v8::new_default_platform(0, false).make_shared();
            v8::V8::initialize_platform(platform);
            v8::V8::initialize();
        });

        let mut isolate = v8::Isolate::new(v8::CreateParams::default());
        v8::scope!(let handle_scope, &mut isolate);
        let context = v8::Context::new(handle_scope, Default::default());
        let mut context_scope = v8::ContextScope::new(handle_scope, context);
        v8::scope!(let scope, &mut context_scope);
        check(scope);
    }

    /// Two properties set from one `FunctionTemplate` hold *one* function
    /// object, which is how the generated pair iterator makes `@@iterator` the
    /// same function object as `entries` without giving up the template API for
    /// the other three members.
    ///
    /// WebIDL asks for that identity, and `docs/dev/idl.md` records it as
    /// satisfied. If a future V8 stops instantiating a template once per
    /// context, that record is what has to change, so this asserts both halves:
    /// one template is one function, two templates are two.
    #[test]
    fn one_template_is_one_function_object_per_context() {
        with_context(|scope| {
            let prototype = v8::ObjectTemplate::new(scope);
            let shared = v8::FunctionTemplate::builder(
                |_: &mut v8::PinScope<'_, '_>,
                 _: v8::FunctionCallbackArguments<'_>,
                 _: v8::ReturnValue<v8::Value>| {},
            )
            .length(0)
            .build(scope);
            let other = v8::FunctionTemplate::builder(
                |_: &mut v8::PinScope<'_, '_>,
                 _: v8::FunctionCallbackArguments<'_>,
                 _: v8::ReturnValue<v8::Value>| {},
            )
            .length(1)
            .build(scope);
            for (name, template) in [("entries", shared), ("keys", shared), ("values", other)] {
                let key = v8_string(scope, name).expect("the name fits a V8 string");
                prototype.set_with_attr(key.into(), template.into(), v8::PropertyAttribute::NONE);
            }
            let iterator = v8::Symbol::get_iterator(scope);
            prototype.set_with_attr(
                iterator.into(),
                shared.into(),
                v8::PropertyAttribute::DONT_ENUM,
            );

            let instance = prototype
                .new_instance(scope)
                .expect("the template instantiates");
            let entries_key = v8_string(scope, "entries").expect("a V8 string");
            let keys_key = v8_string(scope, "keys").expect("a V8 string");
            let values_key = v8_string(scope, "values").expect("a V8 string");
            let entries = instance.get(scope, entries_key.into()).expect("set");
            let keys = instance.get(scope, keys_key.into()).expect("set");
            let values = instance.get(scope, values_key.into()).expect("set");
            let symbol = instance.get(scope, iterator.into()).expect("set");

            assert!(entries.strict_equals(keys), "one template, one function");
            assert!(
                entries.strict_equals(symbol),
                "`@@iterator` is `entries`, not a copy of it"
            );
            assert!(
                !entries.strict_equals(values),
                "a second template is a second function, so the asserts above are not vacuous"
            );
        });
    }

    /// Evaluates `source` in the entered context and returns its result as a
    /// string.
    fn eval(scope: &mut v8::PinScope<'_, '_>, source: &str) -> String {
        let source = v8_string(scope, source).expect("the source fits a V8 string");
        let script = v8::Script::compile(scope, source, None).expect("the source compiles");
        script
            .run(scope)
            .expect("the source runs without throwing")
            .to_rust_string_lossy(scope)
    }

    /// Reads a global by name.
    fn global<'s>(scope: &mut v8::PinScope<'s, '_>, name: &str) -> v8::Local<'s, v8::Value> {
        let key = v8_string(scope, name).expect("the name fits a V8 string");
        scope
            .get_current_context()
            .global(scope)
            .get(scope, key.into())
            .unwrap_or_else(|| panic!("`{name}` is a global"))
    }

    /// Reads a global that holds a function.
    fn global_function<'s>(
        scope: &mut v8::PinScope<'s, '_>,
        name: &str,
    ) -> v8::Local<'s, v8::Function> {
        v8::Local::<v8::Function>::try_from(global(scope, name))
            .unwrap_or_else(|_| panic!("`{name}` is a function"))
    }

    #[test]
    fn array_iterator_yields_the_arrays_values() {
        with_context(|scope| {
            let mut items: Vec<v8::Local<v8::Value>> = Vec::new();
            for item in ["a", "b", "c"] {
                items.push(
                    v8_string(scope, item)
                        .expect("the item fits a V8 string")
                        .into(),
                );
            }
            let array = v8::Array::new_with_elements(scope, &items);
            let iterator = array_iterator(scope, array).expect("the iterator is built");

            // `%Array.prototype.values%` is cached in an isolate slot the first
            // time, so a second call exercises the cached path.
            let empty = v8::Array::new(scope, 0);
            let cached = array_iterator(scope, empty).expect("the cached intrinsic still works");

            let global = scope.get_current_context().global(scope);
            for (name, value) in [("__iterator", iterator), ("__cached", cached)] {
                let key = v8_string(scope, name).expect("the name fits a V8 string");
                global.set(scope, key.into(), value);
            }

            // An Array Iterator, not an interface-branded one.
            assert_eq!(
                eval(
                    scope,
                    "{
                        const seen = [...__iterator];
                        [Object.prototype.toString.call(__iterator),
                         seen.join(','), seen.length].join('|');
                    }"
                ),
                "[object Array Iterator]|a,b,c|3"
            );
            assert_eq!(
                eval(scope, "[...__cached].length.toString()"),
                "0",
                "an empty array still yields an Array Iterator"
            );
        });
    }

    #[test]
    fn call_js_function_passes_the_receiver_as_this() {
        with_context(|scope| {
            eval(
                scope,
                "globalThis.__holder = { label: 'ctx', read: function () { return this.label; } };",
            );
            let holder = global(scope, "__holder");
            let key = v8_string(scope, "read").expect("the key fits a V8 string");
            let read = v8::Local::<v8::Object>::try_from(holder)
                .ok()
                .and_then(|object| object.get(scope, key.into()))
                .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
                .expect("`read` is a function");

            let value = call_js_function(scope, read, holder, &[]).expect("the call succeeds");
            assert_eq!(value.to_rust_string_lossy(scope), "ctx");
        });
    }

    /// The helper must rethrow, not merely report: an exception that reaches
    /// only the returned `JsError` would be lost to the page. Relying on the
    /// `TryCatch` destructor would swallow it instead, which is what the
    /// event-listener path in `tenta-js-v8` wants and therefore what this
    /// helper may not depend on.
    #[test]
    fn call_js_function_rethrows_into_the_enclosing_try_catch() {
        with_context(|scope| {
            eval(
                scope,
                "globalThis.__boom = function () { throw new RangeError('boom'); };",
            );
            let boom = global_function(scope, "__boom");

            v8::tc_scope!(let tc, scope);
            let receiver = v8::undefined(tc).into();
            let message = match call_js_function(tc, boom, receiver, &[]) {
                Ok(_) => panic!("the call throws"),
                Err(error) => {
                    assert_eq!(error.kind(), JsErrorKind::Type);
                    error.message().to_owned()
                }
            };
            assert!(message.contains("boom"), "unexpected message: {message}");
            assert!(
                tc.has_caught(),
                "the exception never reached the enclosing `TryCatch`"
            );
        });
    }

    /// WebIDL converts a sequence with `GetMethod(V, @@iterator)` followed by
    /// `GetIterator`, so anything iterable is a sequence and not only an array.
    /// `new URLSearchParams(new Map([["a", "b"]]))` is the caller that needs
    /// this; an array takes the same path, with no special case to get wrong.
    #[test]
    fn a_sequence_is_read_through_the_iterator_protocol() {
        with_context(|scope| {
            eval(scope, "globalThis.__set = new Set(['a', 'b']);");
            let set = global(scope, "__set");
            let items = sequence_from_v8(scope, set, |scope, item| Ok(string_from_v8(scope, item)))
                .expect("a `Set` is iterable, so it is a sequence");
            assert_eq!(items, ["a", "b"]);

            eval(scope, "globalThis.__items = ['x'];");
            let array = global(scope, "__items");
            let items =
                sequence_from_v8(scope, array, |scope, item| Ok(string_from_v8(scope, item)))
                    .expect("an array is a sequence");
            assert_eq!(items, ["x"]);
        });
    }

    /// A value without a usable `@@iterator` is the `TypeError` the spec
    /// prescribes, including a primitive, which is not an object at all.
    #[test]
    fn a_value_without_an_iterator_is_not_a_sequence() {
        with_context(|scope| {
            eval(scope, "globalThis.__plain = { a: '1' };");
            let plain = global(scope, "__plain");
            let error =
                sequence_from_v8(scope, plain, |scope, item| Ok(string_from_v8(scope, item)))
                    .expect_err("an object without `@@iterator` is not a sequence");
            assert_eq!(error.kind(), JsErrorKind::Type);

            // A primitive string *is* iterable in JavaScript, but WebIDL's
            // conversion rejects a non-object before it ever looks for the
            // method, so this is a `TypeError` too.
            let value = v8_string(scope, "abc")
                .expect("the text fits a V8 string")
                .into();
            let error =
                sequence_from_v8(scope, value, |scope, item| Ok(string_from_v8(scope, item)))
                    .expect_err("a primitive is not a sequence");
            assert_eq!(error.kind(), JsErrorKind::Type);
        });
    }

    /// `es-to-record` filters on the property descriptor: an own property that
    /// is not enumerable never reaches the record, an accessor is read rather
    /// than treated as data, and an inherited enumerable property is not an own
    /// key at all. `headers-record.any.js` observes the descriptor half.
    ///
    /// The integer-like key is the one that catches the descriptor read out: a
    /// key reported as a Number has no `Name` form, so an entry that is not
    /// converted to its string name is dropped instead of read.
    #[test]
    fn a_record_reads_only_the_enumerable_own_properties() {
        with_context(|scope| {
            eval(
                scope,
                "globalThis.__source = { visible: 'v' };
                 Object.defineProperty(__source, 'hidden', { value: 'h', enumerable: false });
                 Object.defineProperty(__source, 'computed', { get: () => 'g', enumerable: true });
                 globalThis.__numbered = { 0: 'zero', a: 'a' };
                 globalThis.__inherited = Object.create({ inherited: 'i' });
                 __inherited.own = 'o';",
            );
            let source = global(scope, "__source");
            let entries =
                record_from_v8(scope, source, |scope, item| Ok(string_from_v8(scope, item)))
                    .expect("the source is a record");
            assert_eq!(
                entries,
                [
                    ("visible".to_string(), "v".to_string()),
                    ("computed".to_string(), "g".to_string())
                ],
                "the non-enumerable own property leaked into the record"
            );

            let source = global(scope, "__numbered");
            let entries =
                record_from_v8(scope, source, |scope, item| Ok(string_from_v8(scope, item)))
                    .expect("the source is a record");
            assert_eq!(
                entries,
                [
                    ("0".to_string(), "zero".to_string()),
                    ("a".to_string(), "a".to_string())
                ],
                "an integer-like own key was dropped"
            );

            let source = global(scope, "__inherited");
            let entries =
                record_from_v8(scope, source, |scope, item| Ok(string_from_v8(scope, item)))
                    .expect("the source is a record");
            assert_eq!(
                entries,
                [("own".to_string(), "o".to_string())],
                "an inherited enumerable property leaked into the record"
            );
        });
    }

    /// A `ByteString` is one code unit per byte: `0xFF` survives the round
    /// trip, where a UTF-8 conversion would have rewritten it, and a code unit
    /// above `0xFF` is the `TypeError` WebIDL prescribes rather than a
    /// truncation to its low byte.
    #[test]
    fn a_byte_string_round_trips_isomorphically() {
        with_context(|scope| {
            eval(scope, "globalThis.__byte = '\\u00ff';");
            let value = global(scope, "__byte");
            let bytes = byte_string_from_v8(scope, value).expect("`0xFF` is a valid `ByteString`");
            assert_eq!(bytes, [0xFF]);

            let back = byte_string_to_v8(scope, &bytes).expect("the bytes fit a V8 string");
            assert_eq!(back.to_rust_string_lossy(scope), "\u{FF}");

            eval(scope, "globalThis.__too_big = '\\u0100';");
            let value = global(scope, "__too_big");
            let error = byte_string_from_v8(scope, value)
                .expect_err("a code unit above `0xFF` is not a `ByteString`");
            assert_eq!(error.kind(), JsErrorKind::Type);
        });
    }
}
