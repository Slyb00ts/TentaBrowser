//! `tentabrowser screenshot`: one URL, one image
//! (`docs/spec/11-library-api.md` §1, `docs/dev/p3-plan.md` H2).
//!
//! The image needs a build with `render`. Without it the command refuses with
//! exit code 5 before making a request, as `fetch --eval` does without `v8`.

#[cfg(feature = "render")]
mod capture;

use std::io::IsTerminal;
use std::path::Path;

use crate::cli::{Global, JsMode, ScreenshotArgs, ScreenshotFormat};
use crate::error::{CliError, CliResult, EXIT_UNSUPPORTED};

#[cfg(feature = "render")]
pub async fn run(global: &Global, args: &ScreenshotArgs) -> CliResult<()> {
    validate(args)?;
    capture::run(global, args).await
}

#[cfg(not(feature = "render"))]
pub async fn run(_global: &Global, args: &ScreenshotArgs) -> CliResult<()> {
    validate(args)?;
    Err(CliError::new(
        EXIT_UNSUPPORTED,
        "screenshot needs the render backend; rebuild with `cargo build -p tentabrowser-cli --features render`",
    ))
}

/// `-o -` writes the image to stdout.
fn is_stdout(path: &Path) -> bool {
    path == Path::new("-")
}

fn validate(args: &ScreenshotArgs) -> CliResult<()> {
    if args.quality.is_some() && args.format != ScreenshotFormat::Jpeg {
        return Err(CliError::usage("--quality needs --format jpeg"));
    }
    if args.js == Some(JsMode::On) && !cfg!(feature = "v8") {
        return Err(CliError::new(
            EXIT_UNSUPPORTED,
            "--js on needs the V8 backend; rebuild with `cargo build -p tentabrowser-cli --features v8`",
        ));
    }
    if is_stdout(&args.output) && std::io::stdout().is_terminal() {
        return Err(CliError::usage(
            "refusing to write an image to a terminal; pass -o FILE or pipe stdout",
        ));
    }
    Ok(())
}
