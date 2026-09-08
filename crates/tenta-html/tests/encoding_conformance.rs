//! html5lib encoding-sniffing conformance
//! (`tests/html5lib-tests/encoding/*.dat`).

mod common;

use std::path::{Path, PathBuf};

use common::tree::Tree;
use tenta_html::{Encoding, ParseStatus, Parser, ParserOptions, sniff};

struct EncodingCase {
    file: String,
    index: usize,
    bytes: Vec<u8>,
    expected: String,
}

/// Splits an encoding `.dat` file. The data is raw bytes in the encoding
/// under test, so the file is never decoded as UTF-8.
fn parse_encoding_dat(path: &Path) -> Vec<EncodingCase> {
    let file = path.file_name().unwrap().to_string_lossy().into_owned();
    let bytes = std::fs::read(path).expect("readable .dat file");
    let mut cases = Vec::new();
    let mut data: Vec<u8> = Vec::new();
    let mut expected: Vec<u8> = Vec::new();
    let mut in_data = false;
    let mut index = 0usize;

    let flush =
        |data: &mut Vec<u8>, expected: &mut Vec<u8>, cases: &mut Vec<EncodingCase>, index| {
            if expected.is_empty() {
                return;
            }
            // The newline before the next heading is a separator, not content.
            if data.last() == Some(&b'\n') {
                data.pop();
            }
            cases.push(EncodingCase {
                file: String::new(),
                index,
                bytes: std::mem::take(data),
                expected: String::from_utf8_lossy(expected).trim().to_owned(),
            });
            expected.clear();
        };

    for line in bytes.split_inclusive(|&byte| byte == b'\n') {
        let trimmed: &[u8] = line.strip_suffix(b"\n").unwrap_or(line);
        if trimmed == b"#data" {
            flush(&mut data, &mut expected, &mut cases, index);
            index += 1;
            data.clear();
            in_data = true;
        } else if trimmed == b"#encoding" {
            in_data = false;
        } else if in_data {
            data.extend_from_slice(line);
        } else {
            expected.extend_from_slice(line);
        }
    }
    flush(&mut data, &mut expected, &mut cases, index);
    for case in &mut cases {
        case.file = file.clone();
    }
    cases
}

fn dat_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(path) = stack.pop() {
        for entry in std::fs::read_dir(&path).expect("readable directory") {
            let entry = entry.expect("readable entry").path();
            if entry.is_dir() {
                stack.push(entry);
            } else if entry.extension().is_some_and(|ext| ext == "dat") {
                files.push(entry);
            }
        }
    }
    files.sort();
    files
}

/// The encoding the parser settles on, including a late `<meta charset>`
/// found past the pre-scan window.
fn detected_encoding(bytes: &[u8]) -> &'static Encoding {
    let sniffed = sniff(bytes, None, "en");
    let mut parser = Parser::new(Tree::new(), ParserOptions::default());
    match parser.feed(bytes) {
        ParseStatus::EncodingChanged(encoding) => encoding,
        _ => sniffed.encoding,
    }
}

#[test]
fn html5lib_encoding() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/html5lib-tests/encoding");
    if !dir.is_dir() {
        eprintln!(
            "skipping: {} is missing; run tests/fetch-html5lib-tests.sh",
            dir.display()
        );
        return;
    }

    let mut total = 0usize;
    let mut passed = 0usize;
    for path in dat_files(&dir) {
        let cases = parse_encoding_dat(&path);
        let mut file_passed = 0usize;
        for case in &cases {
            total += 1;
            let expected = Encoding::for_label(case.expected.as_bytes())
                .unwrap_or_else(|| panic!("unknown label {}", case.expected));
            let actual = detected_encoding(&case.bytes);
            if actual == expected {
                file_passed += 1;
                passed += 1;
            } else {
                println!(
                    "{}:{} expected {} got {}\n  {:?}",
                    case.file,
                    case.index,
                    expected.name(),
                    actual.name(),
                    String::from_utf8_lossy(&case.bytes[..case.bytes.len().min(120)])
                );
            }
        }
        println!(
            "{:<32} {file_passed:>4}/{}",
            path.file_name().unwrap().to_string_lossy(),
            cases.len()
        );
    }

    let ratio = passed as f64 / total as f64 * 100.0;
    println!("encoding: {passed}/{total} = {ratio:.2}%");
    assert!(
        ratio >= 95.0,
        "encoding pass rate {ratio:.2}% is below the 95% gate ({passed}/{total})"
    );
}
