//! html5lib tree-construction conformance.
//!
//! The corpus is fetched by `tests/fetch-html5lib-tests.sh` and is not part
//! of the repository; the test reports a skip when it is missing.

pub mod common;

use std::path::{Path, PathBuf};

use common::dat::{DatCase, Scripting, dat_files, parse_dat};
use common::tree::{Tree, serialize};
use tenta_core::Atom;
use tenta_html::{Parser, ParserOptions, QualName};

pub fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/html5lib-tests/tree-construction")
}

/// Resolves a `#document-fragment` context string to a qualified name.
fn context_name(context: &str) -> QualName {
    let (ns, local) = match context.split_once(' ') {
        Some(("svg", local)) => ("http://www.w3.org/2000/svg", local),
        Some(("math", local)) => ("http://www.w3.org/1998/Math/MathML", local),
        _ => ("http://www.w3.org/1999/xhtml", context),
    };
    QualName::new(Atom::new(ns), Atom::new(local))
}

/// Parses one case and serialises the result in html5lib format.
pub fn run_case(case: &DatCase, scripting_enabled: bool) -> String {
    let fragment_context = case.fragment_context.as_deref().map(context_name);
    let is_fragment = fragment_context.is_some();
    let options = ParserOptions {
        scripting_enabled,
        fragment_context,
        ..ParserOptions::default()
    };
    let mut parser = Parser::new(Tree::new(), options);
    parser.feed_str(&case.input);
    let tree = parser.finish();

    if is_fragment {
        // The fragment parsing algorithm collects the children under a bare
        // <html> root; the expected tree lists that root's children.
        let root = tree.nodes[0].children[0];
        serialize(&tree, root, true)
    } else {
        serialize(&tree, 0, true)
    }
}

fn modes(case: &DatCase) -> &'static [bool] {
    match case.scripting {
        Scripting::On => &[true],
        Scripting::Off => &[false],
        Scripting::Both => &[true, false],
    }
}

#[test]
fn tree_construction() {
    let dir = corpus_dir();
    if !dir.is_dir() {
        eprintln!(
            "skipping: {} is missing; run tests/fetch-html5lib-tests.sh",
            dir.display()
        );
        return;
    }

    let mut total = 0usize;
    let mut passed = 0usize;
    let mut failures: Vec<String> = Vec::new();
    let mut per_file = Vec::new();

    for path in dat_files(&dir) {
        let cases = parse_dat(&path);
        let mut file_passed = 0usize;
        for case in &cases {
            total += 1;
            let mut ok = true;
            for &scripting in modes(case) {
                let actual = run_case(case, scripting);
                let actual = actual.trim_end_matches('\n');
                let expected = case.expected.trim_end_matches('\n');
                if actual != expected {
                    ok = false;
                    if failures.len() < 40 {
                        failures.push(format!(
                            "{}:{} (scripting={scripting})\ninput:\n{}\nexpected:\n{}\nactual:\n{}\n",
                            case.file, case.index, case.input, expected, actual
                        ));
                    }
                    break;
                }
            }
            if ok {
                file_passed += 1;
                passed += 1;
            }
        }
        per_file.push((
            path.file_name().unwrap().to_string_lossy().into_owned(),
            file_passed,
            cases.len(),
        ));
    }

    for (name, ok, count) in &per_file {
        if ok != count {
            println!("{name:<45} {ok:>5}/{count:<5} FAIL");
        } else {
            println!("{name:<45} {ok:>5}/{count:<5}");
        }
    }
    for failure in &failures {
        println!("---\n{failure}");
    }

    let ratio = passed as f64 / total as f64 * 100.0;
    println!("tree-construction: {passed}/{total} = {ratio:.2}%");
    assert!(
        ratio >= 99.5,
        "tree-construction pass rate {ratio:.2}% is below the 99.5% gate ({passed}/{total})"
    );
}
