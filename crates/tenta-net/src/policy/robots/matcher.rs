//! Path matching for `robots.txt`.
//!
//! Ported from Google's reference implementation
//! (<https://github.com/google/robotstxt>, `robots.cc`, Apache-2.0): the
//! wildcard matcher, the URL-to-path reduction and the pattern escaping are
//! the behaviour site owners actually test against, and RFC 9309 was
//! written from that implementation.

/// Whether `path` matches `pattern`, and if so the match priority, which is
/// the pattern's length: the longest matching rule wins (RFC 9309 §2.2.2).
///
/// The algorithm is an NFA simulation over the set of path offsets reachable
/// after consuming a prefix of the pattern, so a pattern with many `*` costs
/// O(pattern × path) rather than backtracking exponentially.
pub fn pattern_matches(path: &str, pattern: &str) -> Option<usize> {
    let priority = pattern.len();
    if matches(path.as_bytes(), pattern.as_bytes()) {
        Some(priority)
    } else {
        None
    }
}

fn matches(path: &[u8], pattern: &[u8]) -> bool {
    let path_len = path.len();
    let mut positions = vec![0usize; path_len + 1];
    let mut count = 1;

    let mut index = 0;
    while index < pattern.len() {
        let byte = pattern[index];
        if byte == b'$' && index + 1 == pattern.len() {
            return positions[count - 1] == path_len;
        }
        if byte == b'*' {
            // Every offset at or after the earliest reachable one becomes
            // reachable.
            count = path_len - positions[0] + 1;
            for i in 1..count {
                positions[i] = positions[i - 1] + 1;
            }
        } else {
            let mut next = 0;
            for i in 0..count {
                let at = positions[i];
                if at < path_len && path[at] == byte {
                    positions[next] = at + 1;
                    next += 1;
                }
            }
            if next == 0 {
                return false;
            }
            count = next;
        }
        index += 1;
    }
    true
}

/// Reduces a URL to the path, parameters and query used for matching.
/// Anything that is already a path is returned unchanged; a fragment is
/// dropped because it never reaches the server.
pub fn path_params_query(url: &str) -> String {
    let bytes = url.as_bytes();
    // Leading "//" is a scheme-relative URL, so the authority starts there.
    let search_start = if bytes.len() >= 2 && bytes[0] == b'/' && bytes[1] == b'/' {
        2
    } else {
        0
    };
    let early_path = find_first_of(url, search_start, b"/?;");
    let mut protocol_end = url[search_start..].find("://").map(|at| at + search_start);
    if let (Some(early), Some(proto)) = (early_path, protocol_end)
        && early < proto
    {
        // A '/', '?' or ';' before "://" means the "://" is not a scheme.
        protocol_end = None;
    }
    let protocol_end = match protocol_end {
        Some(at) => at + 3,
        None => search_start,
    };

    let Some(path_start) = find_first_of(url, protocol_end, b"/?;") else {
        return "/".to_string();
    };
    let hash = url[search_start..].find('#').map(|at| at + search_start);
    if let Some(hash) = hash
        && hash < path_start
    {
        return "/".to_string();
    }
    let path_end = hash.unwrap_or(url.len());
    let path = &url[path_start..path_end];
    if !path.starts_with('/') {
        return format!("/{path}");
    }
    path.to_string()
}

fn find_first_of(text: &str, from: usize, set: &[u8]) -> Option<usize> {
    if from > text.len() {
        return None;
    }
    text.as_bytes()[from..]
        .iter()
        .position(|b| set.contains(b))
        .map(|at| at + from)
}

/// Normalises a rule pattern so it can be compared with an escaped path:
/// existing percent escapes are uppercased and non-ASCII bytes are escaped.
pub fn escape_pattern(pattern: &str) -> String {
    let bytes = pattern.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%'
            && index + 2 < bytes.len()
            && bytes[index + 1].is_ascii_hexdigit()
            && bytes[index + 2].is_ascii_hexdigit()
        {
            out.push('%');
            out.push(bytes[index + 1].to_ascii_uppercase() as char);
            out.push(bytes[index + 2].to_ascii_uppercase() as char);
            index += 3;
        } else if byte & 0x80 != 0 {
            out.push_str(&format!("%{byte:02X}"));
            index += 1;
        } else {
            out.push(byte as char);
            index += 1;
        }
    }
    out
}

/// The product token of a user-agent value, lowercased.
///
/// RFC 9309 §2.2.1 defines the product token as the leading run of letters,
/// `_` and `-`, so `Googlebot/2.1 (+http://...)` and `Googlebot` are the same
/// crawler. `*` is the global group and is returned verbatim.
pub fn extract_product_token(value: &str) -> String {
    let trimmed = value.trim_start();
    let bytes = trimmed.as_bytes();
    if bytes.first() == Some(&b'*')
        && (bytes.len() == 1 || bytes[1].is_ascii_whitespace() || bytes[1] == b'/')
    {
        return "*".to_string();
    }
    let end = bytes
        .iter()
        .position(|b| !(b.is_ascii_alphabetic() || *b == b'_' || *b == b'-'))
        .unwrap_or(bytes.len());
    trimmed[..end].to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards_and_anchors() {
        assert!(pattern_matches("/foo/bar", "/foo").is_some());
        assert!(pattern_matches("/foo/bar", "/foo/*/bar").is_none());
        assert!(pattern_matches("/foo/x/bar", "/foo/*/bar").is_some());
        assert!(pattern_matches("/foo.php", "/*.php$").is_some());
        assert!(pattern_matches("/foo.php?x=1", "/*.php$").is_none());
        assert!(pattern_matches("/foo.php?x=1", "/*.php").is_some());
        assert!(pattern_matches("/anything", "").is_some());
        assert_eq!(pattern_matches("/anything", ""), Some(0));
        assert_eq!(pattern_matches("/a", "/a"), Some(2));
    }

    #[test]
    fn many_wildcards_do_not_blow_up() {
        let path = format!("/{}", "a".repeat(200));
        let pattern = format!("/{}b", "*a".repeat(50));
        assert!(pattern_matches(&path, &pattern).is_none());
    }

    #[test]
    fn url_reduction() {
        let cases: &[(&str, &str)] = &[
            ("", "/"),
            ("http://www.example.com", "/"),
            ("http://www.example.com/", "/"),
            ("http://www.example.com/a", "/a"),
            ("http://www.example.com/a/", "/a/"),
            (
                "http://www.example.com/a/b?c=http://d.e/",
                "/a/b?c=http://d.e/",
            ),
            (
                "http://www.example.com/a/b?c=d&e=f#fragment",
                "/a/b?c=d&e=f",
            ),
            ("example.com", "/"),
            ("example.com/", "/"),
            ("example.com/a", "/a"),
            ("a", "/"),
            ("a/", "/"),
            ("a/b", "/b"),
            ("example.com?a", "/?a"),
            ("example.com/a;b#c", "/a;b"),
            ("//a/b/c", "/b/c"),
        ];
        for (input, expected) in cases {
            assert_eq!(path_params_query(input), *expected, "{input}");
        }
    }

    #[test]
    fn pattern_escaping() {
        assert_eq!(escape_pattern("/foo"), "/foo");
        assert_eq!(escape_pattern("/a%2fb"), "/a%2Fb");
        assert_eq!(escape_pattern("/a%2Fb"), "/a%2Fb");
        assert_eq!(escape_pattern("/ツ"), "/%E3%83%84");
        assert_eq!(escape_pattern("/%zz"), "/%zz");
    }

    #[test]
    fn product_tokens() {
        assert_eq!(extract_product_token("Googlebot/2.1"), "googlebot");
        assert_eq!(extract_product_token("  FooBot  "), "foobot");
        assert_eq!(
            extract_product_token("Googlebot-Images"),
            "googlebot-images"
        );
        assert_eq!(extract_product_token("*"), "*");
        assert_eq!(extract_product_token("* Foo"), "*");
        assert_eq!(extract_product_token("Foo Bar"), "foo");
        assert_eq!(extract_product_token(""), "");
    }
}
