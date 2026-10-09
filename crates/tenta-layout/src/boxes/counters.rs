//! CSS counters and their textual forms, for `counter()` in generated content
//! and for list markers.

/// One live counter. `scope` is the number of open elements when it was
/// created, so a later sibling's reset replaces it instead of nesting.
struct Counter {
    name: Box<str>,
    value: i32,
    scope: u32,
    step: i32,
}

/// The counters in scope at the current point of the tree walk.
#[derive(Default)]
pub(super) struct Counters {
    live: Vec<Counter>,
}

impl Counters {
    pub(super) fn len(&self) -> usize {
        self.live.len()
    }

    /// Drops the counters created below an element that has just closed.
    pub(super) fn truncate(&mut self, len: usize) {
        self.live.truncate(len);
    }

    /// `counter-reset`: a new counter, replacing one a previous sibling made.
    pub(super) fn reset(&mut self, name: &str, value: i32, scope: u32, step: i32) {
        self.live
            .retain(|c| !(c.scope == scope && &*c.name == name));
        self.live.push(Counter {
            name: name.into(),
            value,
            scope,
            step,
        });
    }

    /// `counter-increment`: the nearest counter, or a fresh one when none is
    /// in scope.
    pub(super) fn increment(&mut self, name: &str, by: i32, scope: u32) {
        match self.live.iter_mut().rev().find(|c| &*c.name == name) {
            Some(c) => c.value = c.value.saturating_add(by),
            None => self.reset(name, by, scope, 1),
        }
    }

    /// Assigns the nearest counter, creating it when none is in scope.
    pub(super) fn set(&mut self, name: &str, value: i32, scope: u32) {
        match self.live.iter_mut().rev().find(|c| &*c.name == name) {
            Some(c) => c.value = value,
            None => self.reset(name, value, scope, 1),
        }
    }

    /// The nearest counter's value; an absent counter reads as zero.
    pub(super) fn value(&self, name: &str) -> i32 {
        self.live
            .iter()
            .rev()
            .find(|c| &*c.name == name)
            .map_or(0, |c| c.value)
    }

    /// How much the nearest counter moves per list item.
    pub(super) fn step(&self, name: &str) -> i32 {
        self.live
            .iter()
            .rev()
            .find(|c| &*c.name == name)
            .map_or(1, |c| c.step)
    }

    /// Every counter of that name in scope, outermost first (`counters()`).
    pub(super) fn all(&self, name: &str) -> impl Iterator<Item = i32> + '_ {
        let name = name.to_owned();
        self.live
            .iter()
            .filter(move |c| *c.name == *name)
            .map(|c| c.value)
    }
}

/// A counter value in a named counter style. Unknown styles and values a
/// style cannot represent fall back to decimal, as CSS Counter Styles do.
pub(super) fn format_counter(value: i32, style: &str) -> String {
    match style {
        "none" => String::new(),
        "disc" => "\u{2022}".into(),
        "circle" => "\u{25E6}".into(),
        "square" => "\u{25AA}".into(),
        "decimal-leading-zero" if (0..10).contains(&value) => format!("0{value}"),
        "lower-alpha" | "lower-latin" => {
            alphabetic(value, b'a').unwrap_or_else(|| value.to_string())
        }
        "upper-alpha" | "upper-latin" => {
            alphabetic(value, b'A').unwrap_or_else(|| value.to_string())
        }
        "lower-roman" => roman(value).map_or_else(|| value.to_string(), |r| r.to_lowercase()),
        "upper-roman" => roman(value).unwrap_or_else(|| value.to_string()),
        "lower-greek" => greek(value).unwrap_or_else(|| value.to_string()),
        _ => value.to_string(),
    }
}

fn alphabetic(value: i32, base: u8) -> Option<String> {
    if value <= 0 {
        return None;
    }
    let mut n = value as u32;
    let mut out = Vec::new();
    while n > 0 {
        n -= 1;
        out.push(base + (n % 26) as u8);
        n /= 26;
    }
    out.reverse();
    String::from_utf8(out).ok()
}

fn greek(value: i32) -> Option<String> {
    const LETTERS: [char; 24] = [
        'α', 'β', 'γ', 'δ', 'ε', 'ζ', 'η', 'θ', 'ι', 'κ', 'λ', 'μ', 'ν', 'ξ', 'ο', 'π', 'ρ', 'σ',
        'τ', 'υ', 'φ', 'χ', 'ψ', 'ω',
    ];
    if value <= 0 {
        return None;
    }
    let mut n = value as usize;
    let mut out = Vec::new();
    while n > 0 {
        n -= 1;
        out.push(LETTERS[n % LETTERS.len()]);
        n /= LETTERS.len();
    }
    Some(out.into_iter().rev().collect())
}

fn roman(value: i32) -> Option<String> {
    if !(1..=3999).contains(&value) {
        return None;
    }
    const TABLE: [(i32, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut n = value;
    let mut out = String::new();
    for (v, s) in TABLE {
        while n >= v {
            out.push_str(s);
            n -= v;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_styles_format_values() {
        assert_eq!(format_counter(3, "decimal"), "3");
        assert_eq!(format_counter(3, "decimal-leading-zero"), "03");
        assert_eq!(format_counter(27, "lower-alpha"), "aa");
        assert_eq!(format_counter(2, "upper-latin"), "B");
        assert_eq!(format_counter(1994, "upper-roman"), "MCMXCIV");
        assert_eq!(format_counter(4, "lower-roman"), "iv");
        assert_eq!(format_counter(2, "lower-greek"), "β");
        assert_eq!(format_counter(0, "lower-alpha"), "0");
        assert_eq!(format_counter(5000, "upper-roman"), "5000");
        assert_eq!(format_counter(1, "unknown-style"), "1");
    }

    #[test]
    fn sibling_resets_replace_and_nested_resets_stack() {
        let mut c = Counters::default();
        c.reset("n", 0, 1, 1);
        c.reset("n", 5, 1, 1);
        assert_eq!(c.all("n").collect::<Vec<_>>(), [5]);
        c.reset("n", 7, 2, 1);
        assert_eq!(c.all("n").collect::<Vec<_>>(), [5, 7]);
        c.increment("n", 1, 3);
        assert_eq!(c.value("n"), 8);
        let mark = c.len();
        c.reset("m", 1, 3, 1);
        c.truncate(mark);
        assert_eq!(c.value("m"), 0);
    }

    #[test]
    fn increment_without_a_counter_creates_one() {
        let mut c = Counters::default();
        c.increment("fresh", 2, 1);
        assert_eq!(c.value("fresh"), 2);
        c.set("fresh", 9, 1);
        assert_eq!(c.value("fresh"), 9);
    }
}
