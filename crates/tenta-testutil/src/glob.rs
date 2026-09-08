//! Minimal path glob: `*` matches within one path segment, `**` matches across
//! segments, `?` matches one character. Enough for `--delay-path` rules.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Glob {
    pattern: Vec<char>,
}

impl Glob {
    pub fn new(pattern: &str) -> Self {
        Self {
            pattern: pattern.chars().collect(),
        }
    }

    pub fn pattern(&self) -> String {
        self.pattern.iter().collect()
    }

    pub fn matches(&self, path: &str) -> bool {
        let text: Vec<char> = path.chars().collect();
        matches_at(&self.pattern, &text)
    }
}

fn matches_at(pattern: &[char], text: &[char]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some(('*', rest)) if rest.first() == Some(&'*') => {
            // `**`: consume any number of characters including separators.
            let rest = &rest[1..];
            (0..=text.len()).any(|skip| matches_at(rest, &text[skip..]))
        }
        Some(('*', rest)) => {
            let segment_end = text.iter().position(|c| *c == '/').unwrap_or(text.len());
            (0..=segment_end).any(|skip| matches_at(rest, &text[skip..]))
        }
        Some(('?', rest)) => match text.split_first() {
            Some((c, tail)) if *c != '/' => matches_at(rest, tail),
            _ => false,
        },
        Some((literal, rest)) => match text.split_first() {
            Some((c, tail)) if c == literal => matches_at(rest, tail),
            _ => false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::Glob;

    #[test]
    fn star_stays_inside_a_segment() {
        let glob = Glob::new("/slow/*.js");
        assert!(glob.matches("/slow/a.js"));
        assert!(glob.matches("/slow/.js"));
        assert!(!glob.matches("/slow/nested/a.js"));
        assert!(!glob.matches("/fast/a.js"));
    }

    #[test]
    fn double_star_crosses_segments() {
        let glob = Glob::new("/api/**");
        assert!(glob.matches("/api/echo"));
        assert!(glob.matches("/api/v1/deep/path"));
        assert!(!glob.matches("/apix"));
        assert!(Glob::new("**/stage?.js").matches("/dynamic-scripts/stage2.js"));
    }

    #[test]
    fn literal_and_question_mark() {
        assert!(Glob::new("/a?c").matches("/abc"));
        assert!(!Glob::new("/a?c").matches("/a/c"));
        assert!(Glob::new("/exact").matches("/exact"));
        assert!(!Glob::new("/exact").matches("/exact/"));
    }
}
