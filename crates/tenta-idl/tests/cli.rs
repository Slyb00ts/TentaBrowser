// End-to-end test of the `tenta-idl gen` command.

mod common;

use std::path::Path;
use std::process::Command;

fn run(arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_tenta-idl"))
        .args(arguments)
        .output()
        .expect("the tenta-idl binary runs")
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "tenta-idl-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("the temporary directory is created");
    dir
}

#[test]
fn gen_writes_the_same_files_the_library_generates() {
    let out = temp_dir("gen");
    let idl = common::toy_dir();
    let output = run(&[
        "gen",
        "--idl",
        idl.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "the command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let expected = tenta_idl::generate(&common::toy());
    let listed: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| {
            Path::new(line)
                .file_name()
                .expect("a file name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        listed,
        expected.iter().map(|f| f.name.clone()).collect::<Vec<_>>()
    );

    for file in &expected {
        let written = std::fs::read_to_string(out.join(&file.name))
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", file.name));
        assert_eq!(
            written,
            tenta_idl::format_source(&file.contents),
            "{} differs from the library output",
            file.name
        );
    }

    // Without the `format` feature the CLI writes the golden bytes verbatim.
    if !cfg!(feature = "format") {
        let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/toy");
        for file in &expected {
            let written =
                std::fs::read_to_string(out.join(&file.name)).expect("the generated file");
            let stored = std::fs::read_to_string(golden.join(&file.name)).expect("the golden file");
            assert_eq!(
                written, stored,
                "{} differs from the golden file",
                file.name
            );
        }
    }

    std::fs::remove_dir_all(&out).expect("the temporary directory is removed");
}

#[test]
fn gen_creates_the_output_directory() {
    let parent = temp_dir("nested");
    let out = parent.join("a/b/c");
    let idl = common::dom_dir();
    let output = run(&[
        "gen",
        "--idl",
        idl.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(output.status.success());
    assert!(out.join("node_ops.rs").is_file());
    std::fs::remove_dir_all(&parent).expect("the temporary directory is removed");
}

#[test]
fn a_bad_invocation_fails_with_a_message() {
    let output = run(&["gen", "--idl", "/nonexistent/idl", "--out", "/tmp"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("tenta-idl:"));

    let output = run(&["build"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown command `build`"));

    let output = run(&["gen", "--idl"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("needs a value"));

    let output = run(&["--help"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("usage: tenta-idl gen"));
}
