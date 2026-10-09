//! The media conditions of `<source media>` and `sizes`: the width, height,
//! orientation, resolution and colour-scheme features, combined with `and`,
//! `or` and `not`. An unknown feature or a malformed query does not match,
//! which is what CSS says of an unrecognised media query.

/// What a condition is evaluated against.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct MediaEnv {
    /// Viewport width in CSS pixels.
    pub width: f32,
    /// Viewport height in CSS pixels.
    pub height: f32,
    /// Device pixels per CSS pixel.
    pub dpr: f32,
    /// `prefers-color-scheme: dark`.
    pub dark: bool,
}

/// Whether a media query list matches. An empty list matches.
pub(crate) fn matches(list: &str, env: &MediaEnv) -> bool {
    if list.trim().is_empty() {
        return true;
    }
    split_top_level(list, ',')
        .iter()
        .any(|query| Parser::new(query, env).query())
}

/// Splits at `separator` outside parentheses.
pub(crate) fn split_top_level(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0_i32, 0);
    for (at, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            c if c == separator && depth == 0 => {
                parts.push(&text[start..at]);
                start = at + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

struct Parser<'a> {
    text: &'a str,
    at: usize,
    env: &'a MediaEnv,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str, env: &'a MediaEnv) -> Self {
        Self { text, at: 0, env }
    }

    fn rest(&self) -> &'a str {
        &self.text[self.at..]
    }

    fn skip_space(&mut self) {
        self.at += self.rest().len() - self.rest().trim_start().len();
    }

    /// Consumes `word` (case-insensitive) when it is followed by a boundary.
    fn word(&mut self, word: &str) -> bool {
        self.skip_space();
        let rest = self.rest();
        let Some(head) = rest.get(..word.len()) else {
            return false;
        };
        let boundary = rest[word.len()..]
            .chars()
            .next()
            .is_none_or(|c| c.is_whitespace() || c == '(');
        if head.eq_ignore_ascii_case(word) && boundary {
            self.at += word.len();
            true
        } else {
            false
        }
    }

    /// `[not|only] type [and condition]` or a bare condition.
    fn query(&mut self) -> bool {
        self.skip_space();
        let negated = self.word("not");
        if !negated {
            self.word("only");
        }
        self.skip_space();
        let result = if self.rest().starts_with('(') {
            self.condition()
        } else {
            let end = self
                .rest()
                .find(|c: char| c.is_whitespace() || c == '(')
                .unwrap_or(self.rest().len());
            let kind = self.rest()[..end].to_ascii_lowercase();
            self.at += end;
            let kind_matches = match kind.as_str() {
                "all" | "screen" => Some(true),
                "print" | "speech" | "tty" | "tv" | "projection" | "handheld" => Some(false),
                _ => None,
            };
            let Some(kind_matches) = kind_matches else {
                return false;
            };
            if self.word("and") {
                kind_matches && self.condition()
            } else {
                kind_matches
            }
        };
        self.skip_space();
        if !self.rest().is_empty() {
            return false;
        }
        result != negated
    }

    /// `term (and term)*` or `term (or term)*`.
    fn condition(&mut self) -> bool {
        let mut value = self.term();
        loop {
            if self.word("and") {
                let next = self.term();
                value = value && next;
            } else if self.word("or") {
                let next = self.term();
                value = value || next;
            } else {
                return value;
            }
        }
    }

    fn term(&mut self) -> bool {
        self.skip_space();
        if self.word("not") {
            return !self.term();
        }
        if !self.rest().starts_with('(') {
            // Skip a token so the caller terminates.
            let end = self
                .rest()
                .find(char::is_whitespace)
                .unwrap_or(self.rest().len());
            self.at += end.max(1).min(self.rest().len());
            return false;
        }
        let start = self.at + 1;
        let mut depth = 0_i32;
        let mut end = None;
        for (offset, c) in self.rest().char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(self.at + offset);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else {
            self.at = self.text.len();
            return false;
        };
        let inner = self.text[start..end].trim();
        self.at = end + 1;
        if inner.starts_with('(')
            || inner
                .get(..4)
                .is_some_and(|w| w.eq_ignore_ascii_case("not "))
        {
            return Parser::new(inner, self.env).condition();
        }
        feature(inner, self.env)
    }
}

/// One `(name: value)`, `(name)` or range feature.
fn feature(inner: &str, env: &MediaEnv) -> bool {
    let inner = inner.to_ascii_lowercase();
    if let Some((name, value)) = inner.split_once(':') {
        return feature_value(name.trim(), value.trim(), env);
    }
    for op in ["<=", ">=", "<", ">", "="] {
        if let Some((name, value)) = inner.split_once(op) {
            let (name, value) = (name.trim(), value.trim());
            let (have, want) = match (number_feature(name, env), length(value)) {
                (Some(have), Some(want)) => (have, want),
                _ => return false,
            };
            return match op {
                "<=" => have <= want,
                ">=" => have >= want,
                "<" => have < want,
                ">" => have > want,
                _ => (have - want).abs() < 0.01,
            };
        }
    }
    match inner.trim() {
        "color" => true,
        "width" => env.width > 0.0,
        "height" => env.height > 0.0,
        "hover" | "any-hover" => false,
        _ => false,
    }
}

fn number_feature(name: &str, env: &MediaEnv) -> Option<f32> {
    match name {
        "width" => Some(env.width),
        "height" => Some(env.height),
        _ => None,
    }
}

fn feature_value(name: &str, value: &str, env: &MediaEnv) -> bool {
    let compare = |have: f32, want: Option<f32>, kind: Kind| match (want, kind) {
        (Some(want), Kind::Min) => have >= want,
        (Some(want), Kind::Max) => have <= want,
        (Some(want), Kind::Exact) => (have - want).abs() < 0.01,
        (None, _) => false,
    };
    match name {
        "width" => compare(env.width, length(value), Kind::Exact),
        "min-width" => compare(env.width, length(value), Kind::Min),
        "max-width" => compare(env.width, length(value), Kind::Max),
        "height" => compare(env.height, length(value), Kind::Exact),
        "min-height" => compare(env.height, length(value), Kind::Min),
        "max-height" => compare(env.height, length(value), Kind::Max),
        "resolution" => compare(env.dpr, resolution(value), Kind::Exact),
        "min-resolution" => compare(env.dpr, resolution(value), Kind::Min),
        "max-resolution" => compare(env.dpr, resolution(value), Kind::Max),
        "-webkit-min-device-pixel-ratio" => compare(env.dpr, value.parse().ok(), Kind::Min),
        "-webkit-max-device-pixel-ratio" => compare(env.dpr, value.parse().ok(), Kind::Max),
        "orientation" => match value {
            "portrait" => env.height >= env.width,
            "landscape" => env.width > env.height,
            _ => false,
        },
        "prefers-color-scheme" => match value {
            "dark" => env.dark,
            "light" => !env.dark,
            _ => false,
        },
        "prefers-reduced-motion" => value == "no-preference",
        "hover" | "any-hover" => value == "none",
        "pointer" | "any-pointer" => value == "none" || value == "fine",
        "color" => value.parse::<f32>().is_ok_and(|n| n <= 8.0),
        "min-color" => value.parse::<f32>().is_ok_and(|n| n <= 8.0),
        _ => false,
    }
}

#[derive(Copy, Clone)]
enum Kind {
    Min,
    Max,
    Exact,
}

/// A CSS length in pixels; `em` and `rem` use the initial 16 px font size,
/// which is what media queries use.
pub(crate) fn length(value: &str) -> Option<f32> {
    let value = value.trim();
    let end = value
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+')))
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(end);
    let number: f32 = number.parse().ok()?;
    let factor = match unit {
        "px" => 1.0,
        "" if number == 0.0 => 1.0,
        "em" | "rem" => 16.0,
        "pt" => 4.0 / 3.0,
        "pc" => 16.0,
        "in" => 96.0,
        "cm" => 96.0 / 2.54,
        "mm" => 96.0 / 25.4,
        _ => return None,
    };
    Some(number * factor)
}

/// A resolution in device pixels per CSS pixel.
fn resolution(value: &str) -> Option<f32> {
    let value = value.trim();
    let end = value
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(end);
    let number: f32 = number.parse().ok()?;
    match unit {
        "dppx" | "x" => Some(number),
        "dpi" => Some(number / 96.0),
        "dpcm" => Some(number * 2.54 / 96.0),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENV: MediaEnv = MediaEnv {
        width: 800.0,
        height: 600.0,
        dpr: 2.0,
        dark: false,
    };

    fn m(query: &str) -> bool {
        matches(query, &ENV)
    }

    #[test]
    fn width_features_compare_against_the_viewport() {
        assert!(m("(min-width: 600px)"));
        assert!(!m("(max-width: 600px)"));
        assert!(m("(min-width: 600px) and (max-width: 900px)"));
        assert!(m("screen and (min-width: 50em)"));
        assert!(m("(width >= 800px)"));
        assert!(!m("(width < 800px)"));
    }

    #[test]
    fn lists_or_nots_and_media_types_combine() {
        assert!(m("print, (min-width: 100px)"));
        assert!(!m("print"));
        assert!(m("not print"));
        assert!(m("not (max-width: 100px)"));
        assert!(m("(max-width: 100px) or (min-height: 500px)"));
        assert!(m(""));
    }

    #[test]
    fn resolution_orientation_and_scheme() {
        assert!(m("(min-resolution: 2dppx)"));
        assert!(m("(min-resolution: 192dpi)"));
        assert!(!m("(min-resolution: 3x)"));
        assert!(m("(orientation: landscape)"));
        assert!(m("(prefers-color-scheme: light)"));
        assert!(!m("(prefers-color-scheme: dark)"));
    }

    #[test]
    fn unknown_features_and_junk_do_not_match() {
        assert!(!m("(foo: bar)"));
        assert!(!m("garbage"));
        assert!(!m("(min-width: 1"));
    }
}
