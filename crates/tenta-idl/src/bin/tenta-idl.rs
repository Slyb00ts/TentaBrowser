//! CLI front end for `tenta-idl`.
//!
//! ```text
//! tenta-idl gen --idl <idl-dir> --out <out-dir>
//! ```
//!
//! `gen` parses every `.webidl` file of `<idl-dir>` as one compilation unit
//! and writes the generated Rust files into `<out-dir>`. A feature crate's
//! `build.rs` calls [`tenta_idl::generate_directory`] instead of shelling out;
//! the CLI exists for inspecting the output by hand and for the round-trip
//! test.

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: tenta-idl gen --idl <idl-dir> --out <out-dir>";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("tenta-idl: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1);
    match arguments.next().as_deref() {
        Some("gen") => {}
        Some("--help") | Some("-h") => {
            println!("{USAGE}");
            return Ok(());
        }
        Some(other) => return Err(format!("unknown command `{other}`\n{USAGE}")),
        None => return Err(format!("no command given\n{USAGE}")),
    }

    let mut idl_dir: Option<PathBuf> = None;
    let mut out_dir: Option<PathBuf> = None;
    while let Some(flag) = arguments.next() {
        let value = arguments
            .next()
            .ok_or_else(|| format!("`{flag}` needs a value\n{USAGE}"))?;
        match flag.as_str() {
            "--idl" => idl_dir = Some(PathBuf::from(value)),
            "--out" => out_dir = Some(PathBuf::from(value)),
            other => return Err(format!("unknown option `{other}`\n{USAGE}")),
        }
    }

    let idl_dir = idl_dir.ok_or_else(|| format!("--idl is required\n{USAGE}"))?;
    let out_dir = out_dir.ok_or_else(|| format!("--out is required\n{USAGE}"))?;

    let written = tenta_idl::generate_directory(&idl_dir, &out_dir).map_err(|e| e.to_string())?;
    for path in &written {
        println!("{}", path.display());
    }
    eprintln!(
        "tenta-idl: wrote {} files to {}",
        written.len(),
        out_dir.display()
    );
    Ok(())
}
