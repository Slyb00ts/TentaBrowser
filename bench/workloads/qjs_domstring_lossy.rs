//! Core-only QuickJS DOMString conversion workload shared by both builds.

use std::error::Error;
use std::hint::black_box;
use std::io;
use std::time::Instant;

use rquickjs_core::{Context, Ctx, Runtime, String as JsString};

#[derive(Clone, Copy)]
enum Case {
    AsciiNames,
    Latin1,
    PairedUnicode,
    LoneSurrogate,
    Large,
}

impl Case {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "ascii_names" => Some(Self::AsciiNames),
            "latin1" => Some(Self::Latin1),
            "paired_unicode" => Some(Self::PairedUnicode),
            "lone_surrogate" => Some(Self::LoneSurrogate),
            "large" => Some(Self::Large),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::AsciiNames => "ascii_names",
            Self::Latin1 => "latin1",
            Self::PairedUnicode => "paired_unicode",
            Self::LoneSurrogate => "lone_surrogate",
            Self::Large => "large",
        }
    }

    fn operations(self) -> usize {
        match self {
            Self::AsciiNames => 2_000_000,
            Self::Latin1 | Self::PairedUnicode => 1_000_000,
            Self::LoneSurrogate => 250_000,
            Self::Large => 10_000,
        }
    }
}

fn inputs<'js>(ctx: Ctx<'js>, case: Case) -> rquickjs_core::Result<Vec<(JsString<'js>, String)>> {
    let texts: Vec<String> = match case {
        Case::AsciiNames => ["id", "class", "style", "title"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        Case::Latin1 => vec!["café".to_owned()],
        Case::PairedUnicode => vec!["A😀B".to_owned()],
        Case::LoneSurrogate => {
            return Ok(vec![(
                JsString::from_utf16(ctx, &[0x41, 0xD800, 0x42])?,
                "A�B".to_owned(),
            )]);
        }
        Case::Large => vec![format!("{}😀", "ab".repeat(8192))],
    };
    texts
        .into_iter()
        .map(|expected| Ok((JsString::from_str(ctx.clone(), &expected)?, expected)))
        .collect()
}

fn verify(inputs: &[(JsString<'_>, String)]) -> Result<(), Box<dyn Error>> {
    for (source, expected) in inputs {
        if source.to_string_lossy()? != *expected {
            return Err(io::Error::other("QuickJS DOMString conversion changed output").into());
        }
    }
    Ok(())
}

fn measured(case: Case) -> Result<(u128, u64, usize), Box<dyn Error>> {
    let runtime = Runtime::new()?;
    let context = Context::full(&runtime)?;
    context.with(|ctx| {
        let inputs = inputs(ctx, case)?;
        verify(&inputs)?;
        let operations = case.operations();
        let expected_checksum: u64 = inputs
            .iter()
            .enumerate()
            .map(|(index, (_, expected))| {
                let visits =
                    operations / inputs.len() + usize::from(index < operations % inputs.len());
                (visits as u64) * (expected.len() as u64)
            })
            .sum();
        let warmup = (operations / 100).max(64);
        for index in 0..warmup {
            let source = black_box(&inputs[index % inputs.len()].0);
            black_box(source.to_string_lossy()?);
        }
        let mut checksum = 0u64;
        let started = Instant::now();
        for index in 0..operations {
            let source = black_box(&inputs[index % inputs.len()].0);
            let output = black_box(source.to_string_lossy()?);
            checksum = checksum.wrapping_add(output.len() as u64);
            black_box(output);
        }
        let elapsed_ns = started.elapsed().as_nanos();
        verify(&inputs)?;
        if checksum != expected_checksum || elapsed_ns == 0 {
            return Err(io::Error::other("QuickJS DOMString checksum/timer mismatch").into());
        }
        Ok((elapsed_ns, checksum, warmup))
    })
}

pub fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 || args[1] != "--case" {
        return Err(
            "expected --case ascii_names|latin1|paired_unicode|lone_surrogate|large".into(),
        );
    }
    let case = Case::parse(&args[2]).ok_or("unknown DOMString benchmark case")?;
    let (elapsed_ns, checksum, warmup) = measured(case)?;
    println!(
        "{{\"scenario\":\"S-qjs-domstring-lossy\",\"case\":\"{}\",\"operations\":{},\"warmup_operations\":{},\"checksum\":{},\"elapsed_ns\":{}}}",
        case.name(),
        case.operations(),
        warmup,
        checksum,
        elapsed_ns
    );
    Ok(())
}
