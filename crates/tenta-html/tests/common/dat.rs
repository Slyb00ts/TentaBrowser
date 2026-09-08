//! Reader for the html5lib `.dat` tree-construction format
//! (`tests/html5lib-tests/tree-construction/README.md`).

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scripting {
    On,
    Off,
    Both,
}

#[derive(Debug, Clone)]
pub struct DatCase {
    pub file: String,
    /// 1-based index of the `#data` section inside the file.
    pub index: usize,
    pub input: String,
    /// Context element for a fragment case, as written in the file
    /// (`"body"`, `"svg path"`, `"math ms"`).
    pub fragment_context: Option<String>,
    pub scripting: Scripting,
    pub expected: String,
}

const HEADINGS: [&str; 7] = [
    "data",
    "errors",
    "new-errors",
    "document-fragment",
    "document",
    "script-on",
    "script-off",
];

/// Splits one `.dat` file into cases.
pub fn parse_dat(path: &Path) -> Vec<DatCase> {
    let file = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("?")
        .to_owned();
    let text = std::fs::read_to_string(path).expect("readable .dat file");

    let mut cases = Vec::new();
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut index = 0usize;

    let flush = |sections: &mut Vec<(String, String)>, cases: &mut Vec<DatCase>, index: usize| {
        if sections.is_empty() {
            return;
        }
        let get = |key: &str| {
            sections
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
        };
        let scripting = match (get("script-on").is_some(), get("script-off").is_some()) {
            (true, _) => Scripting::On,
            (_, true) => Scripting::Off,
            _ => Scripting::Both,
        };
        cases.push(DatCase {
            file: String::new(),
            index,
            input: get("data").unwrap_or("").to_owned(),
            fragment_context: get("document-fragment").map(|value| value.trim().to_owned()),
            scripting,
            expected: get("document").unwrap_or("").to_owned(),
        });
        sections.clear();
    };

    for line in text.split_inclusive('\n') {
        let heading = line
            .strip_prefix('#')
            .map(|rest| rest.trim_end_matches('\n'))
            .filter(|rest| HEADINGS.contains(rest));
        match heading {
            Some(name) => {
                if name == "data" {
                    flush(&mut sections, &mut cases, index);
                    index += 1;
                }
                sections.push((name.to_owned(), String::new()));
            }
            None => {
                if let Some((_, value)) = sections.last_mut() {
                    value.push_str(line);
                }
            }
        }
    }
    flush(&mut sections, &mut cases, index);

    // Every section keeps the newline that separates it from the next
    // heading; the format says that newline is not part of the data.
    for case in &mut cases {
        case.file = file.clone();
        strip_final_newline(&mut case.input);
        strip_final_newline(&mut case.expected);
    }
    cases
}

fn strip_final_newline(text: &mut String) {
    if text.ends_with('\n') {
        text.pop();
    }
}

/// All `.dat` files in `dir`, sorted by name.
pub fn dat_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("readable corpus directory")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "dat"))
        .collect();
    files.sort();
    files
}
