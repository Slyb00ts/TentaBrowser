//! html5lib tokenizer conformance (`tests/html5lib-tests/tokenizer/*.test`).
//!
//! The corpus is fetched by `tests/fetch-html5lib-tests.sh`; the test reports
//! a skip when it is missing. It lives inside the crate because the tokenizer
//! is not part of the public API.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::interface::{Token, TokenSink, TokenSinkResult};
use super::states::{RawKind, State};
use super::{Tokenizer, TokenizerOpts, TokenizerResult};
use crate::input::Input;

/// One token in the html5lib output format.
#[derive(Debug, PartialEq, Eq)]
enum Emitted {
    Character(String),
    Comment(String),
    StartTag {
        name: String,
        attrs: BTreeMap<String, String>,
        self_closing: bool,
    },
    EndTag(String),
    Doctype {
        name: Option<String>,
        public_id: Option<String>,
        system_id: Option<String>,
        correct: bool,
    },
}

#[derive(Default)]
struct Collector {
    tokens: Vec<Emitted>,
}

impl Collector {
    fn push_char(&mut self, text: &str) {
        match self.tokens.last_mut() {
            Some(Emitted::Character(existing)) => existing.push_str(text),
            _ => self.tokens.push(Emitted::Character(text.to_owned())),
        }
    }
}

impl TokenSink for Collector {
    type Handle = ();

    fn process_token(&mut self, token: Token<'_>, _line: u64) -> TokenSinkResult<()> {
        match token {
            Token::Characters(text) => self.push_char(text),
            Token::NullCharacter => self.push_char("\0"),
            Token::Comment(text) => self.tokens.push(Emitted::Comment(text.to_owned())),
            Token::ProcessingInstruction { target, data } => {
                // Not an html5lib token: the corpus predates the processing
                // instruction rules the tree-construction tests require.
                self.tokens
                    .push(Emitted::Comment(format!("?{target} {data}")));
            }
            Token::Tag(tag) => {
                let name = tag.name.as_str().to_owned();
                match tag.kind {
                    super::interface::TagKind::StartTag => {
                        let attrs = tag
                            .attrs
                            .iter()
                            .map(|attr| {
                                let name = match attr.name.prefix {
                                    Some(prefix) => {
                                        format!("{}:{}", prefix.as_str(), attr.name.local.as_str())
                                    }
                                    None => attr.name.local.as_str().to_owned(),
                                };
                                (name, attr.value.clone())
                            })
                            .collect();
                        self.tokens.push(Emitted::StartTag {
                            name,
                            attrs,
                            self_closing: tag.self_closing,
                        });
                    }
                    super::interface::TagKind::EndTag => self.tokens.push(Emitted::EndTag(name)),
                }
            }
            Token::Doctype(doctype) => self.tokens.push(Emitted::Doctype {
                name: doctype.name.clone(),
                public_id: doctype.public_id.clone(),
                system_id: doctype.system_id.clone(),
                correct: !doctype.force_quirks,
            }),
            Token::Eof | Token::ParseError(_) => {}
        }
        TokenSinkResult::Continue
    }
}

fn tokenize(input_text: &str, state: State, last_start_tag: Option<String>) -> Vec<Emitted> {
    let mut tokenizer = Tokenizer::new(TokenizerOpts {
        discard_bom: false,
        initial_state: Some(state),
        last_start_tag_name: last_start_tag,
        scan_mode: crate::scan::ScanMode::default(),
    });
    let mut collector = Collector::default();
    let mut input = Input::new(input_text, 0, true);
    loop {
        match tokenizer.feed(&mut input, &mut collector) {
            TokenizerResult::Done => break,
            TokenizerResult::Stopped => break,
            _ => continue,
        }
    }
    let _ = tokenizer.end(&mut input, &mut collector);
    collector.tokens
}

/// Undoes the corpus' `doubleEscaped` encoding. Lone surrogates cannot exist
/// in a Rust string; they become U+FFFD, which is what decoding the same
/// bytes would produce anyway.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    let mut pending_high: Option<u32> = None;
    while let Some(c) = chars.next() {
        if c != '\\' {
            if pending_high.take().is_some() {
                out.push('\u{fffd}');
            }
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                let value = u32::from_str_radix(&hex, 16).expect("four hex digits");
                match (pending_high.take(), value) {
                    (Some(high), 0xDC00..=0xDFFF) => {
                        let combined = 0x10000 + ((high - 0xD800) << 10) + (value - 0xDC00);
                        out.push(char::from_u32(combined).expect("valid surrogate pair"));
                    }
                    (previous, 0xD800..=0xDBFF) => {
                        if previous.is_some() {
                            out.push('\u{fffd}');
                        }
                        pending_high = Some(value);
                    }
                    (previous, _) => {
                        if previous.is_some() {
                            out.push('\u{fffd}');
                        }
                        out.push(char::from_u32(value).unwrap_or('\u{fffd}'));
                    }
                }
            }
            Some(other) => {
                if pending_high.take().is_some() {
                    out.push('\u{fffd}');
                }
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    if pending_high.is_some() {
        out.push('\u{fffd}');
    }
    out
}

fn string_of(value: &Value, double_escaped: bool) -> String {
    let text = value.as_str().expect("string");
    if double_escaped {
        unescape(text)
    } else {
        text.to_owned()
    }
}

fn optional_string(value: &Value, double_escaped: bool) -> Option<String> {
    if value.is_null() {
        None
    } else {
        Some(string_of(value, double_escaped))
    }
}

fn expected_tokens(output: &[Value], double_escaped: bool) -> Vec<Emitted> {
    let mut tokens: Vec<Emitted> = Vec::new();
    for token in output {
        let parts = token.as_array().expect("token array");
        match parts[0].as_str().expect("token kind") {
            "Character" => {
                let text = string_of(&parts[1], double_escaped);
                match tokens.last_mut() {
                    Some(Emitted::Character(existing)) => existing.push_str(&text),
                    _ => tokens.push(Emitted::Character(text)),
                }
            }
            "Comment" => tokens.push(Emitted::Comment(string_of(&parts[1], double_escaped))),
            "StartTag" => tokens.push(Emitted::StartTag {
                name: string_of(&parts[1], double_escaped),
                attrs: parts[2]
                    .as_object()
                    .expect("attribute map")
                    .iter()
                    .map(|(name, value)| {
                        let name = if double_escaped {
                            unescape(name)
                        } else {
                            name.clone()
                        };
                        (name, string_of(value, double_escaped))
                    })
                    .collect(),
                self_closing: parts.get(3).and_then(Value::as_bool).unwrap_or(false),
            }),
            "EndTag" => tokens.push(Emitted::EndTag(string_of(&parts[1], double_escaped))),
            "DOCTYPE" => tokens.push(Emitted::Doctype {
                name: optional_string(&parts[1], double_escaped),
                public_id: optional_string(&parts[2], double_escaped),
                system_id: optional_string(&parts[3], double_escaped),
                correct: parts[4].as_bool().expect("correctness flag"),
            }),
            other => panic!("unknown expected token {other}"),
        }
    }
    tokens
}

fn state_named(name: &str) -> State {
    match name {
        "Data state" => State::Data,
        "PLAINTEXT state" => State::Plaintext,
        "RCDATA state" => State::RawData(RawKind::Rcdata),
        "RAWTEXT state" => State::RawData(RawKind::Rawtext),
        "Script data state" => State::RawData(RawKind::ScriptData),
        "CDATA section state" => State::CdataSection,
        other => panic!("unknown initial state {other}"),
    }
}

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/html5lib-tests/tokenizer")
}

#[test]
fn html5lib_tokenizer() {
    let dir = corpus_dir();
    if !dir.is_dir() {
        eprintln!(
            "skipping: {} is missing; run tests/fetch-html5lib-tests.sh",
            dir.display()
        );
        return;
    }

    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("readable corpus directory")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "test"))
        .collect();
    files.sort();

    let mut total = 0usize;
    let mut passed = 0usize;
    let mut shown = 0usize;

    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&path).expect("readable test file");
        let json: Value = serde_json::from_str(&text).expect("valid JSON");
        let Some(tests) = json.get("tests").and_then(Value::as_array) else {
            // xmlViolation.test drives an output mode that is not HTML
            // tokenization; html5lib's own runners skip it too.
            println!("{name:<32} skipped (no \"tests\" key)");
            continue;
        };

        let mut file_passed = 0usize;
        let mut file_total = 0usize;
        for test in tests {
            let double_escaped = test
                .get("doubleEscaped")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let input = string_of(&test["input"], double_escaped);
            let expected = expected_tokens(
                test["output"].as_array().expect("output array"),
                double_escaped,
            );
            let last_start_tag = test
                .get("lastStartTag")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let states: Vec<State> = match test.get("initialStates").and_then(Value::as_array) {
                Some(states) => states
                    .iter()
                    .map(|state| state_named(state.as_str().expect("state name")))
                    .collect(),
                None => vec![State::Data],
            };

            for state in states {
                file_total += 1;
                total += 1;
                let actual = tokenize(&input, state, last_start_tag.clone());
                if actual == expected {
                    file_passed += 1;
                    passed += 1;
                } else if shown < 30 {
                    shown += 1;
                    println!(
                        "{name}: {}\n  input:    {input:?}\n  expected: {expected:?}\n  actual:   {actual:?}",
                        test["description"].as_str().unwrap_or("")
                    );
                }
            }
        }
        println!("{name:<32} {file_passed:>5}/{file_total}");
    }

    let ratio = passed as f64 / total as f64 * 100.0;
    println!("tokenizer: {passed}/{total} = {ratio:.2}%");
    assert!(
        ratio >= 99.0,
        "tokenizer pass rate {ratio:.2}% is below the 99% target ({passed}/{total})"
    );
}
