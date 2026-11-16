//! Module scripts: what both backends and the page loader share
//! (`docs/spec/05-javascript.md`; HTML "module scripts" and "import maps").
//!
//! A module graph is loaded by the page and evaluated by the engine, and the
//! two meet over a *module map* keyed by URL. Everything that decides which
//! URL a specifier means lives here so the two sides cannot disagree: the
//! engine resolves the specifiers of a module it has compiled, and the page
//! fetches exactly the URLs the engine named.

use std::collections::BTreeMap;

use url::Url;

/// A dynamic `import()` the engine is waiting on the page to load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DynamicImport {
    /// Names the import in [`crate::engine::PageEngine::finish_dynamic_import`].
    pub id: u64,
    /// The module to load, already resolved.
    pub url: Url,
}

/// The `imports` of a document's import map
/// (<https://html.spec.whatwg.org/multipage/webappapis.html#import-maps>).
///
/// Scopes and integrity metadata are not kept: a specifier is remapped by the
/// top-level `imports` only.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportMap {
    /// Specifier key to address, `None` for an entry that maps to nothing.
    /// Ordered so the longest prefix is found by walking backwards.
    imports: BTreeMap<String, Option<Url>>,
}

impl ImportMap {
    /// Parses the JSON of a `<script type="importmap">` whose document's base
    /// URL is `base`.
    pub fn parse(json: &str, base: &Url) -> Result<ImportMap, String> {
        let value: serde_json::Value = serde_json::from_str(json)
            .map_err(|error| format!("the import map is not JSON: {error}"))?;
        let Some(top) = value.as_object() else {
            return Err("the import map is not a JSON object".to_owned());
        };
        let mut imports = BTreeMap::new();
        match top.get("imports") {
            None => {}
            Some(serde_json::Value::Object(entries)) => {
                for (key, address) in entries {
                    if key.is_empty() {
                        continue;
                    }
                    // A key that is itself a URL is compared in its
                    // serialized form, which is how a specifier that names
                    // the same URL will look once resolved.
                    let key = match url_like(key, base) {
                        Some(url) => url.to_string(),
                        None => key.clone(),
                    };
                    let address = address
                        .as_str()
                        .and_then(|address| url_like(address, base))
                        // A prefix key maps a whole subtree, so its address has
                        // to be one too.
                        .filter(|url| !key.ends_with('/') || url.as_str().ends_with('/'));
                    imports.insert(key, address);
                }
            }
            Some(_) => return Err("the import map's \"imports\" is not an object".to_owned()),
        }
        Ok(ImportMap { imports })
    }

    /// Adds the entries of `other` that this map does not already have, which
    /// is how a later import map merges into an earlier one.
    pub fn merge(&mut self, other: ImportMap) {
        for (key, address) in other.imports {
            self.imports.entry(key).or_insert(address);
        }
    }

    /// The address `normalized` (a specifier, or the serialization of the URL
    /// it parsed to) is mapped to. `None` when no entry applies;
    /// `Some(Err(..))` when one applies and blocks the specifier.
    fn resolve(&self, normalized: &str) -> Option<Result<Url, String>> {
        if let Some(address) = self.imports.get(normalized) {
            return Some(address.clone().ok_or_else(|| blocked(normalized)));
        }
        // The longest prefix key wins; keys are sorted, so every prefix of
        // `normalized` sorts at or before it.
        for (key, address) in self
            .imports
            .range::<str, _>((
                std::ops::Bound::Unbounded,
                std::ops::Bound::Included(normalized),
            ))
            .rev()
        {
            if !key.ends_with('/') || !normalized.starts_with(key.as_str()) {
                continue;
            }
            let Some(address) = address else {
                return Some(Err(blocked(normalized)));
            };
            let rest = &normalized[key.len()..];
            return Some(
                address
                    .join(rest)
                    .ok()
                    // A remapped URL may not climb out of the prefix's address.
                    .filter(|url| url.as_str().starts_with(address.as_str()))
                    .ok_or_else(|| blocked(normalized)),
            );
        }
        None
    }
}

fn blocked(specifier: &str) -> String {
    format!("the module specifier \"{specifier}\" is blocked by the import map")
}

/// `specifier` as a URL when it is URL-like: an absolute URL, or a path that
/// starts with `/`, `./` or `../` and is resolved against `base`.
fn url_like(specifier: &str, base: &Url) -> Option<Url> {
    if specifier.starts_with('/') || specifier.starts_with("./") || specifier.starts_with("../") {
        return base.join(specifier).ok();
    }
    Url::parse(specifier).ok()
}

/// Resolves a module specifier written in the module at `base`
/// (<https://html.spec.whatwg.org/multipage/webappapis.html#resolve-a-module-specifier>).
///
/// A bare specifier (`"lodash"`) means something only through the import map.
pub fn resolve_specifier(specifier: &str, base: &Url, map: &ImportMap) -> Result<Url, String> {
    let as_url = url_like(specifier, base);
    let normalized = as_url
        .as_ref()
        .map_or_else(|| specifier.to_owned(), Url::to_string);
    if let Some(mapped) = map.resolve(&normalized) {
        return mapped;
    }
    as_url.ok_or_else(|| {
        format!(
            "Failed to resolve module specifier \"{specifier}\". Relative references must start with either \"/\", \"./\", or \"../\"."
        )
    })
}

/// Whether `content_type` is a JavaScript MIME type, which a module script's
/// response has to be
/// (<https://mimesniff.spec.whatwg.org/#javascript-mime-type>).
pub fn is_javascript_mime_type(content_type: &str) -> bool {
    let essence = content_type.split(';').next().unwrap_or("").trim();
    const ESSENCES: [&str; 16] = [
        "application/ecmascript",
        "application/javascript",
        "application/x-ecmascript",
        "application/x-javascript",
        "text/ecmascript",
        "text/javascript",
        "text/javascript1.0",
        "text/javascript1.1",
        "text/javascript1.2",
        "text/javascript1.3",
        "text/javascript1.4",
        "text/javascript1.5",
        "text/jscript",
        "text/livescript",
        "text/x-ecmascript",
        "text/x-javascript",
    ];
    ESSENCES
        .iter()
        .any(|known| essence.eq_ignore_ascii_case(known))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://app.example/dir/main.js").unwrap()
    }

    fn resolve(specifier: &str, map: &ImportMap) -> Result<String, String> {
        resolve_specifier(specifier, &base(), map).map(|url| url.to_string())
    }

    #[test]
    fn relative_and_absolute_specifiers_resolve_against_the_module() {
        let map = ImportMap::default();
        assert_eq!(
            resolve("./a.js", &map).unwrap(),
            "https://app.example/dir/a.js"
        );
        assert_eq!(
            resolve("../b.js", &map).unwrap(),
            "https://app.example/b.js"
        );
        assert_eq!(resolve("/c.js", &map).unwrap(), "https://app.example/c.js");
        assert_eq!(
            resolve("https://cdn.example/d.js", &map).unwrap(),
            "https://cdn.example/d.js"
        );
        assert_eq!(
            resolve("data:text/javascript,1", &map).unwrap(),
            "data:text/javascript,1"
        );
    }

    #[test]
    fn a_bare_specifier_needs_the_import_map() {
        let error = resolve("lodash", &ImportMap::default()).unwrap_err();
        assert!(
            error.contains("Failed to resolve module specifier \"lodash\""),
            "{error}"
        );
        assert!(resolve("lodash/fp.js", &ImportMap::default()).is_err());
        assert!(resolve("", &ImportMap::default()).is_err());
    }

    #[test]
    fn an_import_map_remaps_exact_and_prefix_keys() {
        let page = Url::parse("https://app.example/index.html").unwrap();
        let map = ImportMap::parse(
            r#"{"imports": {
                "lodash": "/vendor/lodash.js",
                "lodash/": "/vendor/lodash/",
                "lodash/deep/": "https://cdn.example/deep/",
                "./local.js": "./replaced.js",
                "https://cdn.example/old.js": "/new.js",
                "nothing": null,
                "bad/": "/not-a-directory"
            }, "scopes": {"/x/": {"lodash": "/ignored.js"}}}"#,
            &page,
        )
        .unwrap();
        assert_eq!(
            resolve("lodash", &map).unwrap(),
            "https://app.example/vendor/lodash.js"
        );
        assert_eq!(
            resolve("lodash/fp.js", &map).unwrap(),
            "https://app.example/vendor/lodash/fp.js"
        );
        assert_eq!(
            resolve("lodash/deep/x.js", &map).unwrap(),
            "https://cdn.example/deep/x.js",
            "the longest prefix wins"
        );
        // `./local.js` in the map is the page's; the module's `./local.js` is
        // another URL and is left alone.
        assert_eq!(
            resolve("./local.js", &map).unwrap(),
            "https://app.example/dir/local.js"
        );
        assert_eq!(
            resolve("/local.js", &map).unwrap(),
            "https://app.example/replaced.js"
        );
        assert_eq!(
            resolve("https://cdn.example/old.js", &map).unwrap(),
            "https://app.example/new.js"
        );
        assert!(resolve("nothing", &map).unwrap_err().contains("blocked"));
        assert!(resolve("bad/x.js", &map).unwrap_err().contains("blocked"));
        // A remapped specifier may not climb out of the address it maps to.
        assert!(
            resolve("lodash/../../escape.js", &map)
                .unwrap_err()
                .contains("blocked")
        );
        assert!(resolve("unknown", &map).is_err());
    }

    #[test]
    fn a_later_import_map_does_not_override_an_earlier_one() {
        let page = Url::parse("https://app.example/").unwrap();
        let mut map = ImportMap::parse(r#"{"imports": {"a": "/first.js"}}"#, &page).unwrap();
        map.merge(
            ImportMap::parse(r#"{"imports": {"a": "/second.js", "b": "/b.js"}}"#, &page).unwrap(),
        );
        assert_eq!(resolve("a", &map).unwrap(), "https://app.example/first.js");
        assert_eq!(resolve("b", &map).unwrap(), "https://app.example/b.js");
    }

    #[test]
    fn an_import_map_that_is_not_an_object_is_an_error() {
        let page = Url::parse("https://app.example/").unwrap();
        assert!(ImportMap::parse("[]", &page).is_err());
        assert!(ImportMap::parse("{", &page).is_err());
        assert!(ImportMap::parse(r#"{"imports": []}"#, &page).is_err());
        assert_eq!(ImportMap::parse("{}", &page).unwrap(), ImportMap::default());
    }

    #[test]
    fn javascript_mime_types_are_recognised_by_essence() {
        assert!(is_javascript_mime_type("text/javascript"));
        assert!(is_javascript_mime_type(
            "Application/JavaScript; charset=utf-8"
        ));
        assert!(is_javascript_mime_type(" text/ecmascript "));
        assert!(!is_javascript_mime_type("text/html"));
        assert!(!is_javascript_mime_type("application/json"));
        assert!(!is_javascript_mime_type(""));
    }
}
