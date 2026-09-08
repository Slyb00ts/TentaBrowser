//! `tentabrowser fetch`: one URL, one extraction
//! (`docs/spec/16-roadmap.md` P1; `docs/spec/13-performance.md` §1).
//!
//! Tier 1 only. Nothing above `tenta-net`, `tenta-html` and `tenta-dom` is
//! reached: no script engine, no style, no fonts.

use std::io::Write;

use tentabrowser::{Document, Goto, PageOptions, Settle};

use crate::cli::{Dump, FetchArgs, Global, Wait};
use crate::error::{CliError, CliResult};
use crate::output;
use crate::phase::Phases;
use crate::setup::{self, Session};

pub async fn run(global: &Global, args: &FetchArgs) -> CliResult<()> {
    let mut phases = Phases::start();
    let session = setup::open(global, 1)?;
    phases.mark("engine");
    let outcome = fetch(&session, global, args, &mut phases).await;
    session.close();
    phases.mark("close");
    phases.report();
    outcome
}

async fn fetch(
    session: &Session,
    global: &Global,
    args: &FetchArgs,
    phases: &mut Phases,
) -> CliResult<()> {
    // Pre-check the output path if specified to fail early (before the request)
    // if it's outside the policy roots. The actual file opening is deferred
    // until we have the response ready to write.
    if let Some(path) = &args.output {
        session.engine.fs_policy().check_write(path)?;
    }

    let page = session.context.new_page(PageOptions::tier1()).await?;
    let goto = Goto::default()
        .wait_until(settle(args.wait))
        .timeout(std::time::Duration::from_millis(global.timeout));
    tracing::debug!(url = %crate::logging::url_str(&args.url), "fetch");
    let navigation = page.goto(&args.url, goto).await;
    let document = match navigation {
        Ok(_) => page.take_document(),
        Err(error) => {
            let _ = page.close().await;
            return Err(CliError::from(error));
        }
    };
    let _ = page.close().await;
    let document = document?;
    phases.mark("navigate");

    tracing::debug!(
        status = document.status(),
        url = %crate::logging::url(document.final_url()),
        "response"
    );
    if args.headers {
        print_headers(&document);
    }

    let body = render(&document, dump(global, args), args)?;
    phases.mark("extract");
    match &args.output {
        Some(path) => output::write_file(session.engine.fs_policy(), path, body.as_bytes())?,
        None => output::write_stdout(body.as_bytes())?,
    }

    phases.mark("write");
    if document.status() >= 400 && !args.allow_http_errors {
        return Err(CliError::http(
            document.status(),
            document.final_url().as_str(),
        ));
    }
    Ok(())
}

fn settle(wait: Wait) -> Settle {
    match wait {
        Wait::Load => Settle::Load,
        Wait::DomContent => Settle::DomContent,
        Wait::NetworkIdle => Settle::NetworkIdle,
    }
}

/// The status line and the headers, on stderr.
///
/// Never on stdout: stdout carries the extraction and nothing else, so that
/// `fetch --dump text | wc -c` measures the page and a `-o` run writes the
/// same bytes as a piped one.
fn print_headers(document: &Document) {
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{} {}", document.status(), document.final_url());
    for (name, value) in document.headers() {
        let _ = writeln!(stderr, "{name}: {value}");
    }
    let _ = writeln!(stderr);
}

/// The global `--json` is the shorthand a host reaches for; it wins over
/// `--dump`, which defaults to text.
fn dump(global: &Global, args: &FetchArgs) -> Dump {
    if global.json { Dump::Json } else { args.dump }
}

fn render(document: &Document, dump: Dump, args: &FetchArgs) -> CliResult<String> {
    let main = || {
        document
            .main_content()
            .ok_or_else(|| CliError::usage("--main-content: the page has no main article"))
    };
    Ok(match dump {
        Dump::Text if args.main_content => main()?.text,
        Dump::Text => document.text(),
        Dump::Markdown if args.main_content => {
            tentabrowser::dom::markdown::markdown(document.dom(), main()?.node)
        }
        Dump::Markdown => document.markdown(),
        Dump::Html => document.html(),
        Dump::Links => output::to_json_lines(&document.links())?,
        Dump::Json => output::to_json(&document.to_json())?,
        Dump::Metadata => output::to_json(&document.metadata())?,
        Dump::Forms => output::to_json(&document.forms())?,
        Dump::Snapshot => output::to_json(&document.snapshot())?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_wait_maps_to_a_settle_condition() {
        assert_eq!(settle(Wait::Load), Settle::Load);
        assert_eq!(settle(Wait::DomContent), Settle::DomContent);
        assert_eq!(settle(Wait::NetworkIdle), Settle::NetworkIdle);
    }
}
