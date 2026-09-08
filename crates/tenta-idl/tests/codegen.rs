// Golden tests for the generated output of both fixtures.
//
// Refresh the golden files with `UPDATE_GOLDEN=1 cargo test -p tenta-idl`.

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use tenta_idl::{GeneratedFile, Model};

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
