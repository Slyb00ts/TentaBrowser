// Golden tests for the generated output of both fixtures.
//
// Refresh the golden files with `UPDATE_GOLDEN=1 cargo test -p tenta-idl`.

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

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
fn toy_output_matches_the_golden_files() {
    check_fixture("toy", &common::toy());
}

#[test]
fn dom_output_matches_the_golden_files() {
    check_fixture("dom", &common::dom());
}

#[test]
fn every_generated_file_is_valid_rust() {
    for model in [common::toy(), common::dom()] {
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

    assert!(
        file.contents
            .contains("https://docs.rs/rquickjs/0.12.2/rquickjs/")
    );
    for call in [
        "impl<'js, T: NodeOps",
        "rquickjs::class::JsClass<'js>",
        "rquickjs::Class::<NodeClass<T>>::define(",
        "rquickjs::function::Constructor::new_class::<",
        "rquickjs::object::Accessor::new",
        "rquickjs::function::Func::new",
        "set_prototype(",
        "rquickjs::atom::PredefinedAtom::SymbolToStringTag",
    ] {
        assert!(
            file.contents.contains(call),
            "node_qjs.rs never calls {call}"
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
    assert!(
        v8.contents.matches(".length(1)").count() == 1,
        "only `forEach` reports a required argument"
    );
    assert!(
        v8.contents.matches(".length(0)").count() == 3,
        "the three views take no arguments"
    );

    let ops = generated_file(&model, "pair_box_ops.rs");
    assert!(
        ops.contents
            .contains("fn iterable_entries(&self) -> Result<Vec<(String, String)>, JsError>;")
    );
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
        .find("if has_iterator(scope, value) {")
        .expect("the sequence arm is selected by its `@@iterator`");
    let record_at = v8
        .find("if value.is_object() && !value.is_array() {")
        .expect("the record arm is selected as an object that is not an array");
    let fallback_at = v8
        .find("Ok::<String, JsError>(string_from_v8(scope, value))")
        .expect("the unguarded `DOMString` arm is the fallback");
    assert!(
        sequence_at < record_at && record_at < fallback_at,
        "the arms are not emitted in declaration order"
    );

    // The arms are not merely guarded: each one converts through the conversion
    // its test selects it for.
    assert!(
        v8.contains("sequence_from_v8(scope, value, |scope, item| Ok::<String, JsError>(string_from_v8(scope, item)))"),
        "the sequence arm does not use the `GetIterator` conversion"
    );
    assert!(
        v8.contains("record_from_v8(scope, value, |scope, item| Ok::<String, JsError>(string_from_v8(scope, item)))"),
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

/// The QuickJS impl carries a record test too, because it converts every arm
/// through the same table; its sequence arm stays an array check because
/// `rquickjs`'s `Vec<T>: FromJs` reads an array and has no `@@iterator` step.
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
        qjs.contains("if value.is_object() && !value.is_array() {"),
        "the QuickJS record arm has no test"
    );
    assert!(
        qjs.contains("if value.is_array() {"),
        "the QuickJS sequence arm is still an array check"
    );
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
