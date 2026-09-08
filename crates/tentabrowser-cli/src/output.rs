//! Writing results: stdout, a `--allow-write` checked file, JSON shapes.

use std::io::Write;
use std::path::Path;

use serde::Serialize;
use tenta_core::FsPolicy;
use tenta_core::fs_policy::OpenMode;

use crate::error::{CliError, CliResult};

/// Writes `bytes` to stdout, adding the trailing newline a terminal expects
/// and a pipe does not mind.
pub fn write_stdout(bytes: &[u8]) -> CliResult<()> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    write_all(&mut out, bytes)?;
    out.flush().map_err(broken_pipe)
}

/// Writes `bytes` to a path using the filesystem policy for secure file opening.
pub fn write_file(policy: &FsPolicy, path: &Path, bytes: &[u8]) -> CliResult<()> {
    let mut file = policy
        .open_write(path, OpenMode::CreateTruncate)
        .map_err(|err| CliError::usage(format!("writing {}: {err}", path.display())))?;
    write_all(&mut file, bytes)?;
    file.flush()
        .map_err(|err| CliError::usage(format!("writing {}: {err}", path.display())))
}

fn write_all(sink: &mut impl Write, bytes: &[u8]) -> CliResult<()> {
    sink.write_all(bytes).map_err(broken_pipe)?;
    if !bytes.ends_with(b"\n") {
        sink.write_all(b"\n").map_err(broken_pipe)?;
    }
    Ok(())
}

/// A closed pipe (`| head`) is not a failure of the fetch, but the process
/// still cannot deliver what it was asked for.
fn broken_pipe(err: std::io::Error) -> CliError {
    CliError::new(
        crate::error::EXIT_TRANSPORT,
        format!("writing output: {err}"),
    )
}

pub fn to_json<T: Serialize>(value: &T) -> CliResult<String> {
    serde_json::to_string_pretty(value)
        .map_err(|err| CliError::new(crate::error::EXIT_TRANSPORT, format!("serialising: {err}")))
}

/// One JSON object per line: the shape agents stream.
pub fn to_json_lines<T: Serialize>(values: &[T]) -> CliResult<String> {
    let mut out = String::with_capacity(64 * values.len());
    for value in values {
        let line = serde_json::to_string(value).map_err(|err| {
            CliError::new(crate::error::EXIT_TRANSPORT, format!("serialising: {err}"))
        })?;
        out.push_str(&line);
        out.push('\n');
    }
    Ok(out)
}

/// Cuts `text` to `max` characters on a character boundary, with an ellipsis
/// when something was dropped. Used by the `search` table.
pub fn truncate(text: &str, max: usize) -> String {
    let collapsed = collapse_whitespace(text);
    if collapsed.chars().count() <= max {
        return collapsed;
    }
    let keep: String = collapsed.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", keep.trim_end())
}

/// Newlines and runs of spaces become one space: a table row is one line.
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            in_space = true;
            continue;
        }
        if in_space && !out.is_empty() {
            out.push(' ');
        }
        in_space = false;
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_keeps_character_boundaries() {
        assert_eq!(truncate("zażółć gęślą jaźń", 6), "zażół…");
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("one\n  two\tthree", 40), "one two three");
        assert_eq!(truncate("", 5), "");
    }

    #[test]
    fn json_lines_end_every_object_with_a_newline() {
        let lines = to_json_lines(&[1, 2, 3]).expect("json");
        assert_eq!(lines, "1\n2\n3\n");
        assert_eq!(to_json_lines::<u8>(&[]).expect("json"), "");
    }
}
