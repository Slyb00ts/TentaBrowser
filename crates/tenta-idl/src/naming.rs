//! Identifier conversions shared by the model and the generators.

/// Rust keywords that cannot be written as raw identifiers.
const NON_RAW_KEYWORDS: &[&str] = &["crate", "self", "Self", "super"];

/// Rust keywords (2015 + 2018 + 2024 reserved) that need escaping.
const KEYWORDS: &[&str] = &[
    "as", "async", "await", "become", "box", "break", "const", "continue", "crate", "do", "dyn",
    "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl", "in", "let",
    "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref", "return",
    "self", "static", "struct", "super", "trait", "true", "try", "type", "typeof", "unsafe",
    "unsized", "use", "virtual", "where", "while", "yield", "Self",
];

/// Escapes a Rust identifier that collides with a keyword.
///
/// Keywords that support the raw form become `r#keyword`; the four that do not
/// (`crate`, `self`, `Self`, `super`) get a trailing underscore.
pub fn escape_ident(name: &str) -> String {
    if !KEYWORDS.contains(&name) {
        return name.to_string();
    }
    if NON_RAW_KEYWORDS.contains(&name) {
        format!("{name}_")
    } else {
        format!("r#{name}")
    }
}

/// Splits a WebIDL identifier into lowercase words.
///
/// Runs of capitals are kept together up to the last capital that starts a new
/// word, so `DOMString` yields `dom`, `string` and `querySelectorAll` yields
/// `query`, `selector`, `all`.
pub fn words(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut previous: Option<char> = None;
    for (index, &c) in chars.iter().enumerate() {
        if c == '_' || c == '-' || c == ' ' {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            previous = None;
            continue;
        }
        let starts_word = match previous {
            None => false,
            // Digits continue the current word: `base64Data` -> `base64_data`.
            Some(_) if c.is_ascii_digit() => false,
            // `aB` always starts a word; inside a run of capitals only the last
            // capital starts one, so `DOMString` splits as `dom` + `string`.
            Some(prev) if c.is_uppercase() => {
                !prev.is_uppercase() || chars.get(index + 1).is_some_and(|n| n.is_lowercase())
            }
            Some(_) => false,
        };
        if starts_word && !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        current.push(c.to_ascii_lowercase());
        previous = Some(c);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// `nodeValue` -> `node_value`, `DOMString` -> `dom_string`, `type` -> `r#type`.
pub fn snake_case(name: &str) -> String {
    escape_ident(&words(name).join("_"))
}

/// The snake-case form without keyword escaping.
///
/// Use it when the result is spliced into a longer generated identifier or a
/// file name, where `r#type` would be a syntax error but `type` is fine:
/// `get_event_type`, `event_ops.rs`.
pub fn snake_stem(name: &str) -> String {
    words(name).join("_")
}

/// Removes the `r#` of an escaped identifier so it can be spliced into a
/// longer generated identifier.
pub fn ident_stem(name: &str) -> &str {
    name.strip_prefix("r#").unwrap_or(name)
}

/// `nodeValue` -> `NODE_VALUE`.
pub fn screaming_snake_case(name: &str) -> String {
    words(name).join("_").to_uppercase()
}

/// `content-box` -> `ContentBox`, `` -> `EmptyString`.
pub fn pascal_case(name: &str) -> String {
    let mut out = String::new();
    for word in words(name) {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    if out.is_empty() {
        "EmptyString".to_string()
    } else if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        format!("V{out}")
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_acronyms_and_camel_case() {
        assert_eq!(snake_case("nodeValue"), "node_value");
        assert_eq!(snake_case("DOMString"), "dom_string");
        assert_eq!(snake_case("querySelectorAll"), "query_selector_all");
        assert_eq!(snake_case("isTrusted"), "is_trusted");
        assert_eq!(snake_case("URL"), "url");
        assert_eq!(snake_case("type"), "r#type");
        assert_eq!(snake_stem("type"), "type");
        assert_eq!(ident_stem("r#type"), "type");
        assert_eq!(ident_stem("node_value"), "node_value");
        assert_eq!(snake_case("Self"), "self_");
        assert_eq!(escape_ident("Self"), "Self_");
        assert_eq!(screaming_snake_case("ELEMENT_NODE"), "ELEMENT_NODE");
        assert_eq!(screaming_snake_case("MAX_COUNT"), "MAX_COUNT");
        assert_eq!(snake_case("innerHTML"), "inner_html");
        assert_eq!(snake_case("HTMLElement"), "html_element");
        assert_eq!(snake_case("base64Data"), "base64_data");
        assert_eq!(snake_case("Float32Array"), "float32_array");
        assert_eq!(pascal_case("content-box"), "ContentBox");
        assert_eq!(pascal_case(""), "EmptyString");
    }
}
