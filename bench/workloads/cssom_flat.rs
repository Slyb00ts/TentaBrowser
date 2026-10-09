//! Baseline-compatible native CSSOM lifecycle workload.

use std::error::Error;
use std::hint::black_box;
use std::time::Instant;

use tenta_css::CssStyleSheet;
use url::Url;

const HANDLE_OPERATIONS: usize = 1_048_576;
const MUTATION_CYCLES: usize = 16_384;

pub fn run() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = std::env::args().collect();
    if arguments.len() != 3 || arguments[1] != "--size" {
        return Err("expected --size 64|256|1024".into());
    }
    let size: usize = arguments[2].parse()?;
    if ![64, 256, 1024].contains(&size) {
        return Err("size must be 64, 256 or 1024".into());
    }
    let base = Url::parse("https://cssom.test/style.css")?;
    let source: String = (0..size)
        .map(|index| format!(".rule{index} {{ color: red }}\n"))
        .collect();
    let mut sheet = CssStyleSheet::parse(&source, base);
    if sheet.rule_count() != size {
        return Err("Stylo did not parse every benchmark rule".into());
    }

    for index in 0..size {
        let rule = sheet.rule_at(index).ok_or("missing warm-up rule")?;
        drop(black_box(rule));
    }
    let rounds = HANDLE_OPERATIONS / size;
    let started = Instant::now();
    for _ in 0..rounds {
        for index in 0..size {
            let rule = sheet.rule_at(index).ok_or("missing measured rule")?;
            drop(black_box(rule));
        }
    }
    let handle_elapsed_ns = started.elapsed().as_nanos();

    for _ in 0..32 {
        drop(sheet.insert_rule(".temporary { color: blue }", size)?);
        drop(sheet.delete_rule(size)?);
    }
    let started = Instant::now();
    for _ in 0..MUTATION_CYCLES {
        drop(sheet.insert_rule(".temporary { color: blue }", size)?);
        drop(sheet.delete_rule(size)?);
    }
    let mutation_elapsed_ns = started.elapsed().as_nanos();
    if sheet.rule_count() != size {
        return Err("flat mutation changed the benchmark rule count".into());
    }
    println!(
        "{{\"scenario\":\"S-cssom-flat\",\"rules\":{size},\"handle_operations\":{HANDLE_OPERATIONS},\"mutation_cycles\":{MUTATION_CYCLES},\"handle_elapsed_ns\":{handle_elapsed_ns},\"mutation_elapsed_ns\":{mutation_elapsed_ns}}}"
    );
    Ok(())
}
