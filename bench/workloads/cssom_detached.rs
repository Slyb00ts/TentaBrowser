//! Candidate-only detached nested CSSOM lifecycle workload.

use std::error::Error;
use std::hint::black_box;
use std::time::Instant;

use tenta_css::CssStyleSheet;
use url::Url;

const DROPPED_HANDLES: usize = 4_096;

pub fn run() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = std::env::args().collect();
    if arguments.len() != 3 || arguments[1] != "--size" {
        return Err("expected --size 16|64|256".into());
    }
    let size: usize = arguments[2].parse()?;
    if ![16, 64, 256].contains(&size) {
        return Err("size must be 16, 64 or 256".into());
    }
    let base = Url::parse("https://cssom.test/style.css")?;
    let cycles = DROPPED_HANDLES / size;
    let mut mutation_elapsed_ns = 0_u128;
    let mut drop_elapsed_ns = 0_u128;

    for _ in 0..cycles {
        let mut sheet = CssStyleSheet::parse(".parent { color: red }", base.clone());
        let parent = sheet.rule_at(0).ok_or("missing parent rule")?;
        drop(sheet.delete_rule(0)?);
        if parent.has_parent_style_sheet() {
            return Err("removed parent retained a sheet link".into());
        }

        let started = Instant::now();
        for index in 0..size {
            let css = format!("& .child{index} {{ color: blue }}");
            if sheet.insert_nested_rule(&parent, &css, index)?.is_some() {
                return Err("detached insertion invalidated a sheet".into());
            }
        }
        let children: Vec<_> = (0..size)
            .map(|index| parent.child_at(index).ok_or("missing child rule"))
            .collect::<Result<_, _>>()?;
        if children.iter().any(|child| child.has_parent_style_sheet()) {
            return Err("detached child unexpectedly has a sheet link".into());
        }
        for _ in 0..size {
            if sheet.delete_nested_rule(&parent, 0)?.is_some() {
                return Err("detached deletion invalidated a sheet".into());
            }
        }
        mutation_elapsed_ns += started.elapsed().as_nanos();
        if parent.child_count() != 0 || children.iter().any(|child| child.parent_rule().is_some()) {
            return Err("deleted child still belongs to its parent list".into());
        }

        black_box(&children);
        let started = Instant::now();
        drop(children);
        drop_elapsed_ns += started.elapsed().as_nanos();
    }
    println!(
        "{{\"scenario\":\"S-cssom-detached\",\"rules\":{size},\"cycles\":{cycles},\"dropped_handles\":{DROPPED_HANDLES},\"mutation_elapsed_ns\":{mutation_elapsed_ns},\"drop_elapsed_ns\":{drop_elapsed_ns}}}"
    );
    Ok(())
}
