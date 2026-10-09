//! Choosing the image an `<img>` shows: `srcset` density and width candidates,
//! `sizes`, and `<picture>` sources (HTML, "select an image source").

use tenta_core::NodeId;
use tenta_core::atom::atoms;
use tenta_dom::Document;
use url::Url;

use super::media::{self, MediaEnv, split_top_level};

/// The candidate an element loads: an absolute URL and the image pixels per
/// CSS pixel it was authored for.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Source {
    pub url: Url,
    pub density: f32,
}

#[derive(Clone, Debug, PartialEq)]
struct Candidate {
    url: String,
    descriptor: Descriptor,
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum Descriptor {
    Density(f32),
    Width(f32),
}

/// What the element's source attributes select, or `None` when it has no
/// usable source.
pub(crate) fn select(doc: &Document, img: NodeId, base: &Url, env: &MediaEnv) -> Option<Source> {
    let (srcset, sizes) = srcset_and_sizes(doc, img, env);
    let src = doc.attribute(img, atoms::SRC).map_or("", str::trim);
    let (text, density) = choose(srcset.as_deref().unwrap_or(""), sizes.as_deref(), src, env)?;
    let url = base.join(&text).ok()?;
    Some(Source { url, density })
}

/// The `srcset` and `sizes` that apply: those of the first matching
/// `<source>` before the `<img>` inside a `<picture>`, else the image's own.
fn srcset_and_sizes(
    doc: &Document,
    img: NodeId,
    env: &MediaEnv,
) -> (Option<String>, Option<String>) {
    let parent = doc.parent(img);
    if parent != NodeId::NONE && doc.tag(parent) == Some(atoms::PICTURE) {
        for sibling in doc.children(parent) {
            if sibling == img {
                break;
            }
            if doc.tag(sibling) != Some(atoms::SOURCE) {
                continue;
            }
            let Some(srcset) = doc
                .attribute(sibling, atoms::SRCSET)
                .filter(|s| !s.trim().is_empty())
            else {
                continue;
            };
            if let Some(kind) = doc.attribute(sibling, atoms::TYPE)
                && !supported_type(kind)
            {
                continue;
            }
            if let Some(query) = doc.attribute(sibling, atoms::MEDIA)
                && !media::matches(query, env)
            {
                continue;
            }
            return (
                Some(srcset.to_owned()),
                doc.attribute_str(sibling, "sizes").map(str::to_owned),
            );
        }
    }
    (
        doc.attribute(img, atoms::SRCSET).map(str::to_owned),
        doc.attribute_str(img, "sizes").map(str::to_owned),
    )
}

/// Whether the engine decodes a `<source type>`.
pub(crate) fn supported_type(kind: &str) -> bool {
    let essence = kind
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    matches!(
        essence.as_str(),
        "image/png"
            | "image/apng"
            | "image/jpeg"
            | "image/jpg"
            | "image/pjpeg"
            | "image/webp"
            | "image/gif"
            | "image/bmp"
            | "image/x-icon"
            | "image/vnd.microsoft.icon"
            | "image/svg+xml"
    )
}

/// The URL text and density of the best candidate of `srcset` for the device
/// pixel ratio in `env`, with `src` as the fallback candidate.
fn choose(srcset: &str, sizes: Option<&str>, src: &str, env: &MediaEnv) -> Option<(String, f32)> {
    let mut candidates = parse_srcset(srcset);
    let has_width = candidates
        .iter()
        .any(|c| matches!(c.descriptor, Descriptor::Width(_)));
    let has_unit_density = candidates
        .iter()
        .any(|c| c.descriptor == Descriptor::Density(1.0));
    if !src.is_empty() && !has_width && !has_unit_density {
        candidates.push(Candidate {
            url: src.to_owned(),
            descriptor: Descriptor::Density(1.0),
        });
    }
    let size = if has_width {
        source_size(sizes.unwrap_or(""), env)
    } else {
        env.width
    };
    let mut resolved: Vec<(f32, &str)> = candidates
        .iter()
        .filter_map(|c| match c.descriptor {
            Descriptor::Density(d) => Some((d, c.url.as_str())),
            Descriptor::Width(w) if size > 0.0 => Some((w / size, c.url.as_str())),
            Descriptor::Width(_) => None,
        })
        .collect();
    // Stable, so a tie keeps the author's order.
    resolved.sort_by(|a, b| a.0.total_cmp(&b.0));
    let dpr = env.dpr.max(0.01);
    let pick = resolved
        .iter()
        .find(|(density, _)| *density >= dpr)
        .or_else(|| resolved.last())?;
    Some((pick.1.to_owned(), pick.0))
}

fn parse_srcset(text: &str) -> Vec<Candidate> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        while at < bytes.len() && (bytes[at].is_ascii_whitespace() || bytes[at] == b',') {
            at += 1;
        }
        if at >= bytes.len() {
            break;
        }
        let start = at;
        while at < bytes.len() && !bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        let mut url = &text[start..at];
        let mut descriptors = "";
        if url.ends_with(',') {
            url = url.trim_end_matches(',');
        } else {
            let tail = at;
            let mut depth = 0_i32;
            while at < bytes.len() {
                match bytes[at] {
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    b',' if depth <= 0 => break,
                    _ => {}
                }
                at += 1;
            }
            descriptors = &text[tail..at];
        }
        if url.is_empty() {
            continue;
        }
        if let Some(descriptor) = parse_descriptors(descriptors) {
            out.push(Candidate {
                url: url.to_owned(),
                descriptor,
            });
        }
    }
    out
}

/// `None` for an invalid descriptor list, which discards the candidate.
fn parse_descriptors(text: &str) -> Option<Descriptor> {
    let mut found: Option<Descriptor> = None;
    for token in text.split_ascii_whitespace() {
        let (number, unit) = token.split_at(token.len().saturating_sub(1));
        let value: f32 = number
            .parse()
            .ok()
            .filter(|v: &f32| v.is_finite() && *v > 0.0)?;
        let descriptor = match unit {
            "w" if number.bytes().all(|b| b.is_ascii_digit()) => Descriptor::Width(value),
            "x" => Descriptor::Density(value),
            _ => return None,
        };
        if found.is_some() {
            return None;
        }
        found = Some(descriptor);
    }
    Some(found.unwrap_or(Descriptor::Density(1.0)))
}

/// The CSS width in pixels the `sizes` attribute gives the image: the first
/// entry whose condition matches, else `100vw`.
fn source_size(sizes: &str, env: &MediaEnv) -> f32 {
    for entry in split_top_level(sizes, ',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (condition, length) = split_entry(entry);
        let Some(size) = size_length(length, env).filter(|s| *s >= 0.0) else {
            continue;
        };
        if condition.is_empty() || media::matches(condition, env) {
            return size;
        }
    }
    env.width
}

/// Splits `(media) length` into its condition and its length, which may be a
/// function with spaces inside.
fn split_entry(entry: &str) -> (&str, &str) {
    if entry.ends_with(')') {
        let mut depth = 0_i32;
        for (at, c) in entry.char_indices().rev() {
            match c {
                ')' => depth += 1,
                '(' => {
                    depth -= 1;
                    if depth == 0 {
                        // Include the function name in front of the paren.
                        let name_start = entry[..at]
                            .rfind(|c: char| c.is_whitespace() || c == ')')
                            .map_or(0, |i| i + 1);
                        if name_start == at {
                            // A bare parenthesis is a media condition only.
                            return (entry, "");
                        }
                        return (entry[..name_start].trim(), &entry[name_start..]);
                    }
                }
                _ => {}
            }
        }
        return (entry, "");
    }
    match entry.rfind(char::is_whitespace) {
        Some(at) => (entry[..at].trim(), entry[at..].trim()),
        None => ("", entry),
    }
}

/// A `<source-size-value>`: a length, a viewport unit, or a flat `calc()`.
fn size_length(text: &str, env: &MediaEnv) -> Option<f32> {
    let text = text.trim();
    if let Some(inner) = text
        .strip_prefix("calc(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        return calc(inner, env);
    }
    term(text, env)
}

/// Terms joined by `+` and `-` with spaces around them, as `calc()` requires.
fn calc(inner: &str, env: &MediaEnv) -> Option<f32> {
    let mut total = 0.0;
    let mut sign = 1.0;
    let mut expect_term = true;
    for token in inner.split_ascii_whitespace() {
        if expect_term {
            total += sign * term(token, env)?;
            expect_term = false;
        } else {
            sign = match token {
                "+" => 1.0,
                "-" => -1.0,
                _ => return None,
            };
            expect_term = true;
        }
    }
    (!expect_term).then_some(total)
}

fn term(token: &str, env: &MediaEnv) -> Option<f32> {
    let token = token.trim();
    for (suffix, basis) in [
        ("vmin", env.width.min(env.height) / 100.0),
        ("vmax", env.width.max(env.height) / 100.0),
        ("vw", env.width / 100.0),
        ("vh", env.height / 100.0),
    ] {
        if let Some(number) = token.strip_suffix(suffix) {
            return number.parse::<f32>().ok().map(|n| n * basis);
        }
    }
    media::length(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENV: MediaEnv = MediaEnv {
        width: 800.0,
        height: 600.0,
        dpr: 1.0,
        dark: false,
    };

    fn pick(srcset: &str, sizes: Option<&str>, src: &str, env: MediaEnv) -> Option<(String, f32)> {
        choose(srcset, sizes, src, &env)
    }

    #[test]
    fn density_descriptors_pick_the_smallest_that_covers_the_device() {
        let set = "a.png, b.png 2x, c.png 3x";
        assert_eq!(pick(set, None, "", ENV), Some(("a.png".into(), 1.0)));
        let dpr2 = MediaEnv { dpr: 2.0, ..ENV };
        assert_eq!(pick(set, None, "", dpr2), Some(("b.png".into(), 2.0)));
        let dpr5 = MediaEnv { dpr: 5.0, ..ENV };
        assert_eq!(pick(set, None, "", dpr5), Some(("c.png".into(), 3.0)));
    }

    #[test]
    fn src_is_the_one_x_candidate_when_the_set_has_none() {
        let dpr1 = pick("big.png 2x", None, "small.png", ENV);
        assert_eq!(dpr1, Some(("small.png".into(), 1.0)));
        assert_eq!(
            pick("", None, "only.png", ENV),
            Some(("only.png".into(), 1.0))
        );
        assert_eq!(pick("", None, "", ENV), None);
    }

    #[test]
    fn width_descriptors_divide_by_the_source_size() {
        let set = "s.png 300w, m.png 600w, l.png 1200w";
        let at = |sizes: &str, dpr: f32| pick(set, Some(sizes), "", MediaEnv { dpr, ..ENV });
        assert_eq!(at("300px", 1.0), Some(("s.png".into(), 1.0)));
        assert_eq!(at("300px", 2.0), Some(("m.png".into(), 2.0)));
        assert_eq!(at("100vw", 1.0), Some(("l.png".into(), 1.5)));
        assert_eq!(
            at("(max-width: 500px) 100px, calc(50vw - 100px)", 1.0),
            Some(("s.png".into(), 1.0))
        );
        // Without `sizes` the source size is 100vw.
        assert_eq!(pick(set, None, "", ENV), Some(("l.png".into(), 1.5)));
    }

    #[test]
    fn a_url_with_commas_and_odd_spacing_parses() {
        let parsed = parse_srcset("  a,b.png   2x ,c.png,  d.png 1.5x");
        let urls: Vec<_> = parsed.iter().map(|c| c.url.as_str()).collect();
        assert_eq!(urls, ["a,b.png", "c.png", "d.png"]);
        assert_eq!(parsed[0].descriptor, Descriptor::Density(2.0));
        assert_eq!(parsed[1].descriptor, Descriptor::Density(1.0));
    }

    #[test]
    fn invalid_descriptors_discard_the_candidate() {
        assert!(parse_srcset("a.png 2y").is_empty());
        assert!(parse_srcset("a.png 2x 3w").is_empty());
        assert!(parse_srcset("a.png -1x").is_empty());
        assert!(parse_srcset("a.png 1.5w").is_empty());
    }

    #[test]
    fn source_types_the_engine_decodes() {
        assert!(supported_type("image/webp"));
        assert!(supported_type("image/svg+xml; charset=utf-8"));
        assert!(!supported_type("image/avif"));
        assert!(!supported_type("image/jxl"));
    }
}
