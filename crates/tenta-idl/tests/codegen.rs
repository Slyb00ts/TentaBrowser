// Golden tests for the generated output of both fixtures.
//
// Refresh the golden files with `UPDATE_GOLDEN=1 cargo nextest run -p tenta-idl --test codegen`.

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use tenta_idl::model::BufferType;
use tenta_idl::{GeneratedFile, Model};

fn generated_file(model: &Model, name: &str) -> GeneratedFile {
    tenta_idl::generate(model)
        .into_iter()
        .find(|file| file.name == name)
        .unwrap_or_else(|| panic!("{name} is generated"))
}

fn golden_dir(fixture: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(fixture)
}

fn updating() -> bool {
    std::env::var_os("UPDATE_GOLDEN").is_some()
}

fn check_fixture(fixture: &str, model: &Model) {
    let dir = golden_dir(fixture);
    let files = tenta_idl::generate(model);
    assert!(!files.is_empty());

    if updating() {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the golden directory is writable");
        for file in &files {
            std::fs::write(dir.join(&file.name), &file.contents)
                .expect("the golden file is written");
        }
    }

    let expected_names: BTreeSet<String> = files.iter().map(|f| f.name.clone()).collect();
    let actual_names: BTreeSet<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|entry| {
            entry
                .expect("a directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        actual_names,
        expected_names,
        "the golden directory {} is out of sync; rerun with UPDATE_GOLDEN=1",
        dir.display()
    );

    for file in &files {
        let path = dir.join(&file.name);
        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        assert_eq!(
            file.contents,
            expected,
            "{} differs from the golden file; rerun with UPDATE_GOLDEN=1",
            path.display()
        );
    }
}

fn every_file(model: &Model, mut check: impl FnMut(&GeneratedFile)) {
    for file in tenta_idl::generate(model) {
        check(&file);
    }
}

#[test]
fn constructor_length_is_the_shortest_required_arity_in_both_backends() {
    let model = tenta_idl::parse_webidl(
        "interface RequiredSample { constructor(DOMString type, optional long flags = 0); }; \
         interface OverloadedSample { constructor(DOMString type); constructor(); }; \
         interface AbstractSample {};",
    )
    .expect("constructor fixture parses");
    assert_eq!(
        model
            .interface("RequiredSample")
            .unwrap()
            .constructor_length(),
        Some(1)
    );
    assert_eq!(
        model
            .interface("OverloadedSample")
            .unwrap()
            .constructor_length(),
        Some(0)
    );
    assert_eq!(
        model
            .interface("AbstractSample")
            .unwrap()
            .constructor_length(),
        None
    );
    for (name, length) in [("required_sample", 1), ("overloaded_sample", 0)] {
        let v8 = generated_file(&model, &format!("{name}_v8.rs"));
        assert!(
            v8.contents
                .contains(&format!(".length({length}).build(scope)"))
        );
        let qjs = generated_file(&model, &format!("{name}_qjs.rs"));
        assert!(
            qjs.contents
                .contains(&format!("constructor.set_length({length})?;"))
        );
        let register = generated_file(&model, &format!("{name}_register.rs"));
        assert!(
            register
                .contents
                .contains(&format!("constructor_length: Some({length})"))
        );
    }
}

#[test]
fn toy_output_matches_the_golden_files() {
    check_fixture("toy", &common::toy());
}

#[test]
fn dom_output_matches_the_golden_files() {
    check_fixture("dom", &common::dom());
}

#[test]
fn callback_union_output_matches_the_golden_files() {
    let model = tenta_idl::parse_webidl(include_str!("fixtures/callback_union/union.webidl"))
        .expect("the callback union fixture parses");
    check_fixture("callback_union", &model);
}

#[test]
fn every_generated_file_is_valid_rust() {
    for model in [
        common::toy(),
        common::dom(),
        tenta_idl::parse_webidl(include_str!("fixtures/callback_union/union.webidl"))
            .expect("the callback union fixture parses"),
    ] {
        every_file(&model, |file| {
            if let Err(error) = syn::parse_file(&file.contents) {
                panic!("{} does not parse as Rust: {error}", file.name);
            }
        });
    }
}

#[test]
fn generated_output_contains_no_placeholders() {
    const FORBIDDEN: [&str; 5] = ["TODO", "FIXME", "unimplemented!", "todo!", "XXX"];
    for model in [common::toy(), common::dom()] {
        every_file(&model, |file| {
            for needle in FORBIDDEN {
                assert!(
                    !file.contents.contains(needle),
                    "{} contains the placeholder `{needle}`",
                    file.name
                );
            }
        });
    }
}

#[test]
fn event_listener_methods_lease_converted_callbacks_without_widening_other_operations() {
    let model = common::dom();
    let v8 = generated_file(&model, "event_target_v8.rs").contents;
    let quickjs = generated_file(&model, "event_target_qjs.rs").contents;

    // Each file has the constructor lease and exactly the two listener-method
    // leases. dispatchEvent and unrelated methods retain their ordinary path.
    assert_eq!(v8.matches("v8_begin_stored_value_lease(scope);").count(), 3);
    assert_eq!(
        quickjs
            .matches("quickjs_begin_stored_value_lease(&ctx)?;")
            .count(),
        3
    );
    for method in ["add_event_listener", "remove_event_listener"] {
        assert!(v8.contains(&format!(
            "v8_begin_stored_value_lease(scope);\n        body_call_event_target_{method}"
        )));
        assert!(v8.contains(&format!("host.{method}(&r#type, callback, options)")));
        assert!(quickjs.contains(&format!(
            "(*host).{method}(&r#type, callback, options).map_err"
        )));
    }
    assert_eq!(
        v8.matches("v8_transfer_stored_value_lease(scope);").count(),
        3
    );
    assert_eq!(
        quickjs
            .matches("quickjs_transfer_stored_value_lease(&ctx)?;")
            .count(),
        3
    );
}

#[test]
fn generation_is_deterministic() {
    let model = common::dom();
    let first = tenta_idl::generate(&model);
    let second = tenta_idl::generate(&model);
    assert_eq!(first.len(), second.len());
    for (a, b) in first.iter().zip(&second) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.contents, b.contents);
    }
}

#[test]
fn one_file_group_is_emitted_per_interface_and_namespace() {
    let model = common::dom();
    let names: Vec<String> = tenta_idl::generate(&model)
        .into_iter()
        .map(|f| f.name)
        .collect();
    assert_eq!(names[0], "prelude.rs");
    assert_eq!(names[1], "types.rs");
    assert_eq!(
        names.len(),
        2 + 4 * (model.interfaces.len() + model.namespaces.len())
    );
    for stem in [
        "event_target",
        "event",
        "abort_signal",
        "node",
        "node_list",
        "css",
    ] {
        for suffix in ["ops", "v8", "qjs", "register"] {
            let name = format!("{stem}_{suffix}.rs");
            assert!(names.contains(&name), "{name} is missing from {names:?}");
        }
    }
}

#[test]
fn the_v8_glue_names_the_engine_items_it_uses() {
    let model = common::dom();
    let file = tenta_idl::generate(&model)
        .into_iter()
        .find(|f| f.name == "node_v8.rs")
        .expect("node_v8.rs");

    assert!(file.contents.contains("https://docs.rs/v8/152.2.0/v8/"));
    for item in [
        "v8::FunctionTemplate::builder",
        "v8::fast_api::{CFunction, CFunctionInfo, CTypeInfo, Flags,",
        "v8::PropertyAttribute::{NONE, READ_ONLY, DONT_ENUM, DONT_DELETE}",
        "v8::Intrinsic::{ArrayProtoEntries, ArrayProtoForEach, ArrayProtoKeys,",
    ] {
        assert!(file.contents.contains(item), "the API note omits {item}");
    }
    // The documented items are the ones the file actually calls.
    for call in [
        "v8::FunctionTemplate::builder(",
        "constructor.set_class_name(",
        "constructor.inherit(",
        "prototype_template(scope)",
        "instance_template(scope)",
        "set_internal_field_count(",
        "set_accessor_property(",
        "v8::Signature::new(",
        "build_fast(",
        "v8::fast_api::CFunction::new(",
        "v8::Symbol::get_to_string_tag(scope)",
    ] {
        assert!(
            file.contents.contains(call),
            "node_v8.rs never calls {call}"
        );
    }
}

#[test]
fn the_quickjs_glue_names_the_engine_items_it_uses() {
    let model = common::dom();
    let file = tenta_idl::generate(&model)
        .into_iter()
        .find(|f| f.name == "node_qjs.rs")
        .expect("node_qjs.rs");

    for call in [
        "pub fn install_node<'js, T: NodeOps",
        "quickjs_host::<T>",
        "quickjs_callback_boundary",
        "rquickjs::function::Constructor::new_prototype",
        "rquickjs::object::Accessor::new",
        "rquickjs::Function::new",
        "set_prototype(",
        "rquickjs::atom::PredefinedAtom::SymbolToStringTag",
    ] {
        assert!(
            file.contents.contains(call),
            "node_qjs.rs never calls {call}"
        );
    }
}

/// WebIDL gives `@@iterator`, and nothing else, to an interface that has an
/// indexed getter and an integer `length` but declares no `iterable<>`.
#[test]
fn an_indexed_getter_with_a_length_iterates_without_an_iterable_declaration() {
    let source = "interface Rows { getter DOMString? item(unsigned long index); \
                  readonly attribute unsigned long length; }; \
                  interface Named { getter DOMString? item(DOMString name); \
                  readonly attribute unsigned long length; }; \
                  interface Unsized { getter DOMString? item(unsigned long index); }; \
                  interface Derived : Rows { };";
    let model = tenta_idl::parse_webidl(source).expect("the interfaces parse");
    for (file, expected) in [
        ("rows", true),
        ("named", false),
        ("unsized", false),
        ("derived", false),
    ] {
        let v8 = generated_file(&model, &format!("{file}_v8.rs"));
        assert_eq!(
            v8.contents.contains("v8::Symbol::get_iterator(scope)"),
            expected,
            "{file}_v8.rs"
        );
        assert!(
            !v8.contents.contains("v8_string(scope, \"forEach\")"),
            "{file}_v8.rs has the members only `iterable<>` brings"
        );
        let qjs = generated_file(&model, &format!("{file}_qjs.rs"));
        assert_eq!(
            qjs.contents.contains("PredefinedAtom::SymbolIterator"),
            expected,
            "{file}_qjs.rs"
        );
    }
}

/// The generator's own view of a value iterator: `%Array.prototype%` provides
/// all four members, and `@@iterator` is the same intrinsic as `values`.
#[test]
fn a_value_iterable_exposes_the_array_prototype_members() {
    let file = generated_file(&common::dom(), "node_list_v8.rs");

    for (member, intrinsic) in [
        ("entries", "ArrayProtoEntries"),
        ("forEach", "ArrayProtoForEach"),
        ("keys", "ArrayProtoKeys"),
        ("values", "ArrayProtoValues"),
    ] {
        assert!(
            file.contents
                .contains(&format!("v8_string(scope, \"{member}\")?")),
            "node_list_v8.rs never defines `{member}`"
        );
        assert!(
            file.contents
                .contains(&format!("v8::Intrinsic::{intrinsic}")),
            "node_list_v8.rs never uses {intrinsic}"
        );
    }
    assert!(
        file.contents.contains("v8::Symbol::get_iterator(scope)"),
        "`@@iterator` is missing"
    );
    assert_eq!(
        file.contents
            .matches("prototype.set_intrinsic_data_property(")
            .count(),
        5,
        "the four members and `@@iterator` are all intrinsic data properties"
    );
    assert_eq!(
        file.contents.matches("ArrayProtoValues,").count(),
        2,
        "`values` and `@@iterator` are the same intrinsic"
    );
}

/// A member the interface declares itself wins, as WebIDL requires.
///
/// `@@iterator` is the one exception: it stays `%Array.prototype.values%`, so it
/// is no longer the same function object as the declared `values`. Recorded as
/// a latent deviation in `docs/dev/idl.md`; no interface in the corpus declares
/// `values`.
#[test]
fn a_declared_member_shadows_the_generated_iterator_member() {
    let model =
        tenta_idl::parse_webidl("interface Sample { iterable<DOMString>; DOMString values(); };")
            .expect("the source parses");
    let file = generated_file(&model, "sample_v8.rs");

    // `values` is installed as the interface's own operation ...
    assert!(file.contents.contains("call_sample_values"));
    // ... so it is not also installed as the intrinsic, and only `@@iterator`
    // still refers to `%Array.prototype.values%`.
    assert_eq!(
        file.contents
            .matches("v8::Intrinsic::ArrayProtoValues")
            .count(),
        1
    );
    for member in ["entries", "keys", "forEach"] {
        assert!(
            file.contents
                .contains(&format!("v8_string(scope, \"{member}\")?")),
            "the undeclared member `{member}` was dropped too"
        );
    }
}

/// The pair form has no array-like surface, so every member is a callback over
/// one host snapshot rather than an intrinsic.
#[test]
fn a_pair_iterable_emits_three_views_and_a_for_each_over_one_host_hook() {
    let model = common::toy();
    let v8 = generated_file(&model, "pair_box_v8.rs");

    for member in ["entries", "keys", "values"] {
        assert!(
            v8.contents
                .contains(&format!("call_pair_box_{member}::<T>"))
        );
        assert!(
            v8.contents
                .contains(&format!("v8_string(scope, \"{member}\")?"))
        );
    }
    assert!(v8.contents.contains("host.iterable_entries()"));
    assert!(v8.contents.contains("array_iterator(scope, array)"));
    assert!(
        !v8.contents
            .contains("prototype.set_intrinsic_data_property"),
        "a pair iterator must not reuse a `%Array.prototype%` intrinsic"
    );
    // `@@iterator` is `entries`: both properties are installed from the one
    // template bound for `entries`, and V8 makes one function object out of it
    // per context — which is the identity WebIDL asks for. The runtime half of
    // that claim is asserted by
    // `tenta-js-v8`'s `one_template_is_one_function_object_per_context`.
    assert!(
        v8.contents.contains("v8::Symbol::get_iterator(scope)"),
        "`@@iterator` is missing"
    );
    assert_eq!(
        v8.contents
            .matches("let iterable_entries_template = ")
            .count(),
        1,
        "`entries` gets one template binding"
    );
    assert_eq!(
        v8.contents
            .matches("iterable_entries_template.into(),")
            .count(),
        2,
        "that one template is installed as both `entries` and `@@iterator`"
    );

    // `forEach` is the fourth member and the only one that is not a view: it
    // calls the page's callback with `(value, key, container)`, so it needs the
    // receiver-carrying helper and a required argument, where the three views
    // take none.
    assert!(
        v8.contents.contains("call_pair_box_for_each::<T>"),
        "the pair iterator has no `forEach` member"
    );
    assert!(
        v8.contents.contains("v8_string(scope, \"forEach\")?"),
        "`forEach` is not installed under its IDL name"
    );
    assert!(
        v8.contents
            .contains("call_js_function(scope, callback, this_arg, &[value, key, container])"),
        "`forEach` does not call the page's callback with `(value, key, container)`"
    );
    for (member, length) in [("for_each", 1), ("entries", 0), ("keys", 0), ("values", 0)] {
        let marker = format!("let iterable_{member}_template = ");
        let template = v8
            .contents
            .split_once(&marker)
            .expect("the iterable member template is present")
            .1
            .split_once(';')
            .expect("the template binding is terminated")
            .0;
        assert!(
            template.contains(&format!(".length({length})")),
            "the `{member}` view has the required argument count"
        );
    }

    let ops = generated_file(&model, "pair_box_ops.rs");
    assert!(ops.contents.contains(
        "fn iterable_entries(&self) -> Result<IterableEntries<String, String>, JsError>;"
    ));
}

/// A union arm is selected by the WebIDL conversion's own test, and the arms
/// are tried in declaration order.
///
/// The shape mirrors `URLSearchParams`' constructor argument, where getting the
/// order wrong turns an object into `"[object Object]"`: the sequence arm has to
/// claim anything with an `@@iterator`, the record arm has to claim the plain
/// object the sequence arm declined, and `DOMString` has to be left as the
/// fallback. `wpt/checkout/fetch/api/headers/headers-record.any.js` observes the
/// record half of this through `for (const [k, v] of new Headers(obj))`.
#[test]
fn a_union_arm_is_selected_by_its_own_test_in_declaration_order() {
    let model = tenta_idl::parse_webidl(
        "interface Sample {\n  \
           undefined probe((sequence<DOMString> or record<DOMString, DOMString> or DOMString) value);\n\
         };\n",
    )
    .expect("the source parses");
    assert_eq!(model.unions.len(), 1, "the union is interned once");
    let contents = generated_file(&model, "types.rs").contents;
    // Slice out one impl, or the assertions below would be satisfied by the
    // other engine's block: both impls are emitted into this one file.
    let v8 = impl_body(&contents, &model.unions[0].name, "v8");

    let sequence_at = v8
        .find("if value.is_object() && has_iterator(scope, value)? {")
        .expect("the sequence arm is selected by its `@@iterator`, on an object");
    let record_at = v8
        .find("if value.is_object() && !value.is_array() {")
        .expect("the record arm is selected as an object that is not an array");
    let fallback_at = v8
        .find("string_from_v8(scope, value)")
        .expect("the unguarded `DOMString` arm is the fallback");
    assert!(
        sequence_at < record_at && record_at < fallback_at,
        "the arms are not emitted in declaration order"
    );

    // The arms are not merely guarded: each one converts through the conversion
    // its test selects it for.
    assert!(
        v8.contains("sequence_from_v8(scope, value, string_from_v8)"),
        "the sequence arm does not use the `GetIterator` conversion"
    );
    assert!(
        v8.contains("record_from_v8(scope, value, string_from_v8, string_from_v8)"),
        "the record arm does not use the `es-to-record` conversion"
    );
}

/// One engine's `impl` block for a union, without the other engine's.
fn impl_body(contents: &str, name: &str, engine: &str) -> String {
    let head = match engine {
        "v8" => format!("#[cfg(feature = \"v8\")]\nimpl {name} {{"),
        // The QuickJS side is a trait impl, so its header carries the trait.
        _ => {
            format!("#[cfg(feature = \"{engine}\")]\nimpl<'js> rquickjs::FromJs<'js> for {name} {{")
        }
    };
    let start = contents
        .find(&head)
        .unwrap_or_else(|| panic!("{engine} has no impl for {name}"));
    let rest = &contents[start + head.len()..];
    let end = rest.find("#[cfg(feature = ").unwrap_or(rest.len());
    rest[..end].to_string()
}

/// The QuickJS impl tests records and the sequence iterator protocol before
/// falling back to a primitive conversion.
#[test]
fn the_quickjs_union_arms_carry_the_same_tests() {
    let model = tenta_idl::parse_webidl(
        "interface Sample {\n  \
           undefined probe((sequence<DOMString> or record<DOMString, DOMString> or DOMString) value);\n\
         };\n",
    )
    .expect("the source parses");
    let contents = generated_file(&model, "types.rs").contents;
    let qjs = impl_body(&contents, &model.unions[0].name, "quickjs");

    assert!(
        qjs.contains("if value.is_object() {"),
        "the QuickJS record arm has no test"
    );
    assert!(
        qjs.contains("if quickjs_has_iterator(ctx, &value)? {"),
        "the QuickJS sequence arm must use the iterator protocol"
    );
}

#[test]
fn callback_interface_union_arms_follow_callable_iterable_and_platform_tests() {
    let model = tenta_idl::parse_webidl(include_str!("fixtures/callback_union/union.webidl"))
        .expect("the callback union fixture parses");
    let contents = generated_file(&model, "types.rs").contents;
    for engine in ["v8", "quickjs"] {
        let callback = impl_body(&contents, "ListenerOrHandler", engine);
        let callable = callback
            .find("if value.is_function()")
            .expect("callable test");
        let object = callback
            .find("if value.is_object() {")
            .expect("object test");
        assert!(
            callable < object,
            "{engine} must select the function arm first"
        );
        assert!(
            callback.contains("the value is not of type '("),
            "{engine} must reject unrelated primitives"
        );

        let iterable = impl_body(&contents, "ListenerOrSequenceOfDomString", engine);
        let iterator = if engine == "v8" {
            "if value.is_object() && has_iterator(scope, value)?"
        } else {
            "if quickjs_has_iterator(ctx, &value)?"
        };
        assert!(
            iterable.find(iterator).expect("iterable test")
                < iterable
                    .find("if value.is_object() {")
                    .expect("listener object test"),
            "{engine} must select an iterable before a callback interface"
        );

        let platform = impl_body(&contents, "ListenerOrCallbackUnionProbe", engine);
        let native = if engine == "v8" {
            "if let Some(inner) = interface_handle_from_v8::<CallbackUnionProbe>"
        } else {
            "if quickjs_is_interface(ctx, &value, \"CallbackUnionProbe\")"
        };
        assert!(
            platform.find(native).expect("native interface test")
                < platform
                    .find("if value.is_object() {")
                    .expect("listener object test"),
            "{engine} must select the branded platform object first"
        );
    }
}

/// Every buffer source arm of a union is guarded by the test its conversion
/// brand-checks, for all thirteen `BufferType` members and not only the
/// `BufferSource` arm the fixture declares.
///
/// The typed arrays are matched per class — WebIDL selects the `Uint8Array` arm
/// only for an object whose `[[TypedArrayName]]` is `"Uint8Array"` — and the
/// two spellings that name an `ArrayBuffer` admit a `SharedArrayBuffer`, which
/// has the `[[ArrayBufferData]]` slot WebIDL's `ArrayBuffer` type is. An arm
/// with no test becomes the union's fallback, which is how
/// `typedef (BufferSource or USVString) BodyInit;` used to stringify a typed
/// array instead of taking its bytes.
#[test]
fn every_buffer_union_arm_is_guarded_by_the_class_it_converts() {
    let cases = [
        (
            BufferType::ArrayBuffer,
            "value.is_array_buffer() || value.is_shared_array_buffer()",
        ),
        (BufferType::DataView, "value.is_data_view()"),
        (BufferType::Int8Array, "value.is_int8_array()"),
        (BufferType::Int16Array, "value.is_int16_array()"),
        (BufferType::Int32Array, "value.is_int32_array()"),
        (BufferType::Uint8Array, "value.is_uint8_array()"),
        (BufferType::Uint16Array, "value.is_uint16_array()"),
        (BufferType::Uint32Array, "value.is_uint32_array()"),
        (
            BufferType::Uint8ClampedArray,
            "value.is_uint8_clamped_array()",
        ),
        (BufferType::Float32Array, "value.is_float32_array()"),
        (BufferType::Float64Array, "value.is_float64_array()"),
        (BufferType::ArrayBufferView, "value.is_array_buffer_view()"),
        (
            BufferType::BufferSource,
            "value.is_array_buffer() || value.is_shared_array_buffer() || value.is_array_buffer_view()",
        ),
    ];

    for (kind, test) in cases {
        let name = kind.idl_name();
        let model = tenta_idl::parse_webidl(&format!(
            "interface Sample {{ undefined probe(({name} or USVString) value); }};\n"
        ))
        .expect("the source parses");
        let contents = generated_file(&model, "types.rs").contents;
        let union = &model.unions[0].name;

        let v8 = impl_body(&contents, union, "v8");
        assert!(
            v8.contains(&format!("if {test} {{")),
            "the `{name}` arm is not selected by `{test}`"
        );
        let arm_at = v8
            .find(&format!(
                "{}_from_v8(scope, value)",
                kind.conversion_suffix()
            ))
            .unwrap_or_else(|| panic!("the `{name}` arm does not use its own conversion"));
        let fallback_at = v8
            .find("string_from_v8(scope, value)")
            .expect("the unguarded `USVString` arm is the fallback");
        assert!(
            arm_at < fallback_at,
            "the `{name}` arm is emitted after the string fallback, which shadows it"
        );

        // Native buffer brands must be checked before the string fallback.
        let qjs = impl_body(&contents, union, "quickjs");
        let qjs_arm_at = qjs
            .find("if quickjs_is_buffer(&value,")
            .unwrap_or_else(|| panic!("the QuickJS `{name}` arm has no test"));
        let qjs_fallback_at = qjs
            .find("rquickjs::Coerced::<rquickjs::String>::from_js")
            .expect("the unguarded QuickJS `USVString` arm is the fallback");
        assert!(
            qjs_arm_at < qjs_fallback_at,
            "the QuickJS `{name}` arm is emitted after the string fallback"
        );
    }
}

/// A `ByteString` is a byte string, not text: its conversion is isomorphic, so a
/// byte above `0x7F` survives the round trip and a code unit above `0xFF` is an
/// error rather than a truncation. `Headers` is the first interface to make this
/// observable (`new Headers({ "x-ÿ": "v" })`).
#[test]
fn a_byte_string_is_converted_isomorphically() {
    let model =
        tenta_idl::parse_webidl("interface Sample { ByteString probe(ByteString value); };")
            .expect("the source parses");
    let file = generated_file(&model, "sample_v8.rs");

    assert!(
        file.contents.contains("byte_string_from_v8(scope, raw)"),
        "the argument does not use the isomorphic conversion"
    );
    assert!(
        file.contents.contains("byte_string_to_v8(scope, &value)"),
        "the return value does not use the isomorphic conversion"
    );
    assert!(
        !file.contents.contains("from_utf8_lossy"),
        "the `ByteString` path decoded the bytes as UTF-8"
    );
}

/// A buffer source is converted by its bytes, never as a host object.
///
/// This is the assertion the opaque-interface lowering failed: routing one of
/// these names through `handle_from_v8` compiled, generated an
/// `InterfaceMarker` for a type V8 owns, and then rejected every real typed
/// array the page passed in.
#[test]
fn a_buffer_source_is_converted_by_its_bytes() {
    let model = tenta_idl::parse_webidl(
        "interface Sample {\n  \
           Uint8Array read(ArrayBuffer data);\n  \
           undefined write(DataView view, Float64Array numbers);\n\
         };\n",
    )
    .expect("the source parses");

    let v8 = generated_file(&model, "sample_v8.rs").contents;
    for call in [
        "array_buffer_from_v8(scope, raw)",
        "data_view_from_v8(scope, raw)",
        "float64_array_from_v8(scope, raw)",
        "uint8_array_to_v8(scope, value)",
    ] {
        assert!(v8.contains(call), "the generated glue never calls {call}");
    }
    for name in ["ArrayBuffer", "DataView", "Uint8Array", "Float64Array"] {
        for call in [
            format!("handle_from_v8::<{name}>"),
            format!("handle_to_v8::<{name}>"),
        ] {
            assert!(
                !v8.contains(&call),
                "`{name}` still goes through `{call}`, which a native V8 type \
                 never satisfies"
            );
        }
    }

    // No opaque interface is invented for the name, so no `InterfaceMarker` is
    // generated for it either.
    for name in ["ArrayBuffer", "DataView", "Uint8Array", "Float64Array"] {
        assert!(
            !model.opaque_interfaces.contains(&name.to_string()),
            "`{name}` was recorded as an undefined interface"
        );
        assert!(
            !generated_file(&model, "types.rs")
                .contents
                .contains(&format!("pub enum {name} {{}}")),
            "`{name}` was declared as a host-interface marker"
        );
    }

    // The host boundary carries bytes, not handles.
    let ops = generated_file(&model, "sample_ops.rs").contents;
    assert!(ops.contains("fn read(&self, data: Vec<u8>) -> Result<Vec<u8>, JsError>;"));
    assert!(
        ops.contains("fn write(&self, view: Vec<u8>, numbers: Vec<u8>) -> Result<(), JsError>;")
    );
}

/// Every buffer type the model can name has both of its conversions defined in
/// the prelude, under the name the generator calls it by.
///
/// The two halves are generated from the type: `codegen::convert` builds the
/// call from `BufferType::conversion_suffix` and the prelude builds the
/// definition from the same, so this test is what catches a member added to one
/// side only.
#[test]
fn every_buffer_type_has_the_conversion_the_generator_calls() {
    let prelude = generated_file(&common::toy(), "prelude.rs").contents;

    for kind in BufferType::ALL {
        let name = kind.idl_name();
        let suffix = kind.conversion_suffix();
        for (direction, declaration) in [
            ("from_v8", format!("undefined probe({name} value);")),
            ("to_v8", format!("{name} probe();")),
        ] {
            let model =
                tenta_idl::parse_webidl(&format!("interface Sample {{ {declaration} }};\n"))
                    .expect("the source parses");
            let v8 = generated_file(&model, "sample_v8.rs").contents;
            let call = match direction {
                "from_v8" => format!("{suffix}_from_v8(scope, raw)"),
                _ => format!("{suffix}_to_v8(scope, value)"),
            };
            assert!(
                v8.contains(&call),
                "`{name}` is not converted through `{call}`"
            );
        }
        for function in [
            format!("pub fn {suffix}_from_v8("),
            // The writer is the arm that returns a value tied to the scope.
            format!("pub fn {suffix}_to_v8<'s>("),
        ] {
            assert!(
                prelude.contains(&function),
                "the prelude does not define `{function}`, which `{name}` is converted by"
            );
        }
    }
}

/// `[LegacyTreatNonObjectAsNull]` widens the conversion of a **nullable**
/// callback attribute and nothing else. The HTML Standard reaches the loose
/// rule through `typedef EventHandlerNonNull? EventHandler`, the WebIDL
/// standard's §3.4.8 pair names the annotated callback directly under `?` next
/// to an unannotated one, and every other callback position keeps its gate.
#[test]
fn a_nullable_callback_attribute_converts_loosely_only_when_annotated() {
    let model = common::dom();

    // The annotated attribute, reached through the HTML Standard's typedef.
    let onabort = generated_file(&model, "abort_signal_v8.rs").contents;
    assert!(
        onabort.contains(
            "if !raw.is_object() { Ok(None) } else { (value_from_v8(scope, raw).map(EventHandlerNonNull::from_bits)).map(Some) }"
        ),
        "the annotated nullable callback attribute does not take the loose conversion"
    );

    // The WebIDL standard's pair: `handler2` annotated, `handler1` not. The
    // unannotated one keeps the strict gate inside its nullable wrapper.
    let manager = generated_file(&model, "manager_v8.rs").contents;
    assert!(
        manager.contains(
            "if !raw.is_object() { Ok(None) } else { (value_from_v8(scope, raw).map(ErrorHandler::from_bits)).map(Some) }"
        ),
        "`Manager.handler2` does not take the loose conversion"
    );
    assert!(
        manager.contains(
            "if raw.is_null_or_undefined() { Ok(None) } else { (if raw.is_function() { value_from_v8(scope, raw).map(OccurrenceHandler::from_bits) } else { Err::<OccurrenceHandler, JsError>(JsError::type_error(\"the value is not callable, so it is not an `OccurrenceHandler`\")) }).map(Some) }"
        ),
        "`Manager.handler1` did not keep the strict gate"
    );

    // A callback interface in an argument position is gated on being an
    // object; there is no callable test, because `handleEvent` is looked up
    // when the callback is invoked.
    let event_target = generated_file(&model, "event_target_v8.rs").contents;
    assert!(
        event_target.contains(
            "if raw.is_object() { value_from_v8(scope, raw).map(EventListener::from_bits) } else { Err::<EventListener, JsError>(JsError::type_error(\"the value is not an object, so it is not an `EventListener`\")) }"
        ),
        "the callback interface conversion has no object gate"
    );

    let manager_qjs = generated_file(&model, "manager_qjs.rs").contents;
    assert!(
        manager_qjs.contains("if value.is_function() { <OccurrenceHandler as rquickjs::FromJs>::from_js(&ctx, value) }"),
        "the unannotated QuickJS callback setter must test native callability"
    );
    assert!(
        manager_qjs.contains("if !value.is_object() { Ok(None) } else { <ErrorHandler as rquickjs::FromJs>::from_js(&ctx, value).map(Some) }"),
        "only the annotated nullable QuickJS callback setter accepts a non-callable object"
    );
    let onabort_qjs = generated_file(&model, "abort_signal_qjs.rs").contents;
    assert!(
        onabort_qjs.contains("if !value.is_object() { Ok(None) } else { <EventHandlerNonNull as rquickjs::FromJs>::from_js(&ctx, value).map(Some) }"),
        "the HTML callback typedef must reach the same setter-only legacy rule"
    );
    let event_target_qjs = generated_file(&model, "event_target_qjs.rs").contents;
    assert!(
        event_target_qjs.contains(
            "if value.is_object() { <EventListener as rquickjs::FromJs>::from_js(&ctx, value) }"
        ),
        "the QuickJS callback interface argument accepts objects without requiring callability"
    );
}

#[test]
fn quickjs_callback_argument_wrappers_keep_their_inner_gate() {
    let model = tenta_idl::parse_webidl(
        "callback Handler = undefined (); interface Target { undefined call(optional Handler? callback); undefined many(Handler... callbacks); };",
    )
    .expect("callback argument fixture parses");
    let qjs = generated_file(&model, "target_qjs.rs").contents;
    assert!(
        qjs.contains("if __raw_value.is_undefined() { None } else { Some({ let value = __raw_value; if value.is_null() || value.is_undefined() { Ok(None) } else { { let value = value; if value.is_function()"),
        "optional nullable callback conversion must keep the strict gate for a present value"
    );
    assert!(
        qjs.contains(".map(|__raw_value| { let value = __raw_value; if value.is_function()"),
        "each variadic callback element must be checked for callability"
    );
}

#[test]
fn the_registry_entry_point_carries_the_interface_description() {
    let model = common::dom();
    let file = tenta_idl::generate(&model)
        .into_iter()
        .find(|f| f.name == "node_register.rs")
        .expect("node_register.rs");

    assert!(
        file.contents
            .contains("pub fn register_node(registry: &mut dyn InterfaceRegistry)")
    );
    assert!(file.contents.contains("NODE_DESCRIPTOR"));
    assert!(file.contents.contains("parent: Some(\"EventTarget\")"));
    assert!(file.contents.contains("\"ELEMENT_NODE\""));
    assert!(file.contents.contains("Symbol.toStringTag"));
}

/// A nullable *optional* string argument binds as `Option<Option<String>>`, so
/// the borrowed form the trait method takes has to reach through the outer
/// `Option` **by reference**.
///
/// `.map(|inner| inner.as_deref())` moves the inner `Option` into the closure and
/// returns a `&str` borrowed from the closure's own parameter, which does not
/// outlive the call: `rustc` rejects it with E0515. `XMLHttpRequest.open`'s
/// `username`/`password` are the first arguments in the corpus to reach this
/// branch, and they are why the outer `.as_ref()` exists.
#[test]
fn a_nullable_optional_string_argument_is_borrowed_through_its_outer_option() {
    let model = tenta_idl::parse_webidl(
        "interface Sample { undefined probe(optional DOMString? value = null); };",
    )
    .expect("the sample lowers");

    // The trait's own spelling, so the emission below is checked against the
    // parameter type it has to produce rather than in isolation.
    assert!(
        generated_file(&model, "sample_ops.rs")
            .contents
            .contains("fn probe(&self, value: Option<Option<&str>>) -> Result<(), JsError>;")
    );

    let v8 = generated_file(&model, "sample_v8.rs").contents;
    assert!(
        v8.contains("host.probe(value.as_ref().map(|inner| inner.as_deref()))"),
        "the V8 argument does not borrow the outer `Option` by reference"
    );
    assert!(
        !v8.contains("value.map(|inner| inner.as_deref())"),
        "the V8 argument moves the inner `Option` into the closure"
    );

    // The QuickJS generator must borrow the same optional nullable string.
    let qjs = generated_file(&model, "sample_qjs.rs").contents;
    assert!(
        qjs.contains("probe(value.as_ref().map(|v| v.as_deref()))"),
        "the QuickJS argument does not borrow the outer `Option` by reference"
    );
}

#[test]
fn generated_numeric_conversions_use_restricted_helpers_and_preserve_unrestricted_values() {
    let model = tenta_idl::parse_webidl(
        "dictionary NumberInit { float narrow; unrestricted float wideFloat; double restricted; unrestricted double wide; }; \
         interface NumberSample { undefined accept(float narrow, unrestricted float wideFloat, double restricted, unrestricted double wide); };",
    )
    .expect("numeric fixture parses");
    let types = generated_file(&model, "types.rs");
    let v8 = generated_file(&model, "number_sample_v8.rs");
    let qjs = generated_file(&model, "number_sample_qjs.rs");
    for output in [&types.contents, &v8.contents] {
        assert!(output.contains("and_then(restricted_f32)"));
        assert!(output.contains("and_then(restricted_f64)"));
    }
    for output in [&types.contents, &qjs.contents] {
        assert!(output.contains("restricted_f32(v.0).map_err"));
    }
    assert!(
        types
            .contents
            .contains("result.wide = Some((number_from_v8(scope, member))?);")
    );
    assert!(types.contents.contains("number as f32"));
    assert!(qjs.contents.contains("v.0 as f32"));
}

#[test]
fn quickjs_derived_interface_constructor_links_its_parent_function() {
    let model = tenta_idl::parse_webidl(
        "interface Base { constructor(); }; interface Derived : Base { constructor(); };",
    )
    .expect("inheritance fixture parses");
    let parent = generated_file(&model, "base_qjs.rs");
    let child = generated_file(&model, "derived_qjs.rs");
    assert!(!parent.contents.contains("parent_constructor"));
    assert!(
        child
            .contents
            .contains("let parent_constructor: rquickjs::Function = global.get(\"Base\")?;")
    );
    assert!(
        child
            .contents
            .contains("constructor.set_prototype(Some(&parent_constructor))?;")
    );
}

#[test]
fn buffer_reference_annotation_changes_only_the_annotated_operation() {
    let model = tenta_idl::parse_webidl(
        "interface BufferReferenceProbe { \
         [TentaBufferReferences] ArrayBufferView echoView(ArrayBufferView value); \
         [TentaBufferReferences] BufferSource echoSource(BufferSource value); \
         undefined copied(BufferSource value); };",
    )
    .expect("the annotated toy interface parses");
    let ops = generated_file(&model, "buffer_reference_probe_ops.rs").contents;
    assert!(ops.contains(
        "fn echo_view(&self, value: ArrayBufferViewRef) -> Result<ArrayBufferViewRef, JsError>"
    ));
    assert!(ops.contains(
        "fn echo_source(&self, value: BufferSourceRef) -> Result<BufferSourceRef, JsError>"
    ));
    assert!(ops.contains("fn copied(&self, value: Vec<u8>) -> Result<(), JsError>"));
    let v8 = generated_file(&model, "buffer_reference_probe_v8.rs").contents;
    let quickjs = generated_file(&model, "buffer_reference_probe_qjs.rs").contents;
    assert!(v8.contains("v8_checked_begin_stored_value_lease(scope)"));
    assert!(v8.contains("array_buffer_view_ref_from_v8(scope, raw)"));
    assert!(v8.contains("buffer_source_ref_to_v8(scope, value)"));
    assert!(quickjs.contains("quickjs_begin_buffer_reference_lease(&ctx)?"));
    assert!(!quickjs.contains("quickjs_transfer_stored_value_lease(&ctx)?"));
    assert!(quickjs.contains("buffer_source_ref_from_quickjs(&ctx, __raw_value)"));
    assert!(quickjs.contains("array_buffer_view_ref_to_quickjs(&ctx, result)"));
}

#[test]
fn buffer_reference_annotation_preserves_other_argument_shapes() {
    let model = tenta_idl::parse_webidl(
        "interface BufferReferenceProbe { \
         [TentaBufferReferences] undefined optionalCount(BufferSource value, optional unsigned long count); \
         [TentaBufferReferences] undefined variadicCount(BufferSource value, unsigned long... counts); };",
    )
    .expect("ordinary optional and variadic arguments remain supported");
    let ops = generated_file(&model, "buffer_reference_probe_ops.rs").contents;
    assert!(ops.contains("fn optional_count(&self, value: BufferSourceRef, count: Option<u32>)"));
    assert!(ops.contains("fn variadic_count(&self, value: BufferSourceRef, counts: Vec<u32>)"));
    let qjs = generated_file(&model, "buffer_reference_probe_qjs.rs").contents;
    assert!(!qjs.contains("quickjs_transfer_stored_value_lease(&ctx)?"));
}

#[test]
fn buffer_reference_annotation_refuses_unsupported_shapes() {
    for declaration in [
        "[TentaBufferReferences] undefined wrong(optional BufferSource source);",
        "[TentaBufferReferences] undefined wrong(BufferSource... sources);",
        "[TentaBufferReferences] undefined wrong(BufferSource? source);",
        "[TentaBufferReferences] undefined wrong(sequence<BufferSource> sources);",
        "[TentaBufferReferences] undefined wrong(ArrayBuffer buffer);",
        "[TentaBufferReferences] undefined wrong(long value);",
        "[TentaBufferReferences] Promise<BufferSource> wrong(BufferSource source);",
    ] {
        let source = format!("interface BufferReferenceProbe {{ {declaration} }};");
        let error = tenta_idl::parse_webidl(&source).expect_err(declaration);
        assert!(
            error.to_string().contains("TentaBufferReferences"),
            "{declaration}: {error}"
        );
    }
    for source in [
        "[TentaBufferReferences] interface Wrong { undefined call(); };",
        "interface Wrong { [TentaBufferReferences] readonly attribute long value; };",
        "interface Wrong { [TentaBufferReferences] constructor(); };",
        "interface Wrong { undefined call([TentaBufferReferences] long value); };",
        "[TentaBufferReferences] dictionary Wrong { long value; };",
        "namespace Wrong { [TentaBufferReferences] undefined call(BufferSource value); };",
    ] {
        assert!(
            tenta_idl::parse_webidl(source)
                .expect_err("annotation is operation-only")
                .to_string()
                .contains("TentaBufferReferences"),
            "{source}",
        );
    }
    let mixed = "interface BufferReferenceProbe { \
        [TentaBufferReferences] undefined mixed(BufferSource source); \
        undefined mixed(DOMString ordinary, long other); };";
    assert!(
        tenta_idl::parse_webidl(mixed)
            .expect_err("mixed annotation overloads are unsupported")
            .to_string()
            .contains("TentaBufferReferences")
    );
}

/// An interface declared `[HTMLConstructor]` hands construction to the world
/// on V8; every other interface without a constructor throws, and so does the
/// QuickJS glue, which has no custom elements.
#[test]
fn an_html_constructor_interface_defers_to_the_world_on_v8() {
    let dom = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../idl/dom");
    let model = tenta_idl::parse_directory(&dom).expect("the DOM interfaces parse");

    let element = generated_file(&model, "html_element_v8.rs").contents;
    assert!(
        element.contains("v8_html_constructor(scope, args, rv);"),
        "`HTMLElement` does not call the HTML element constructor"
    );
    assert!(
        !element.contains("Illegal constructor"),
        "`HTMLElement` still throws on construction"
    );

    let div = generated_file(&model, "html_div_element_v8.rs").contents;
    assert!(
        div.contains("throw_type_error(scope, \"Illegal constructor\");"),
        "an interface without the attribute no longer throws"
    );
    assert!(!div.contains("v8_html_constructor"));

    let quickjs = generated_file(&model, "html_element_qjs.rs").contents;
    assert!(
        quickjs.contains("Illegal constructor"),
        "the QuickJS `HTMLElement` no longer throws on construction"
    );
}
