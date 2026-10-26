//! Native read-only source CLI. No Windows/native build was available in authoring.
#![forbid(unsafe_code)]
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::ExitCode;

mod verify_tree;

const USAGE: &str = "Usage: loomward-native capabilities | scan ROOT [MAX_ENTRIES] | plan SCENARIO.json | verify-tree SOURCE DESTINATION --state-dir DIR [--max-entries N] [--budget-bytes N] [--budget-secs N]";

fn execute(args: &[String]) -> Result<serde_json::Value, String> {
    match args.first().map(String::as_str) {
        Some("capabilities") if args.len() == 1 => Ok(
            serde_json::json!({"native_build":"local_build","metadata_scan":true,"tier_simulation":true,"file_mutation":false,"process_mutation":false,"native_content_hash":false}),
        ),
        Some("scan") if (2..=3).contains(&args.len()) => {
            let limit = if args.len() == 3 {
                args[2].parse::<usize>().map_err(|e| e.to_string())?
            } else {
                50_000
            };
            loomward_core::inventory::scan(Path::new(&args[1]), limit, 64)
        }
        Some("plan") if args.len() == 2 => {
            let file = std::fs::File::open(&args[1]).map_err(|e| e.to_string())?;
            let mut input = String::new();
            file.take(20 * 1024 * 1024 + 1)
                .read_to_string(&mut input)
                .map_err(|e| e.to_string())?;
            if input.len() > 20 * 1024 * 1024 {
                return Err("scenario exceeds 20 MiB".into());
            }
            let s = serde_json::from_str(&input).map_err(|e| e.to_string())?;
            serde_json::to_value(loomward_core::planner::plan(&s)?).map_err(|e| e.to_string())
        }
        _ => Err(USAGE.into()),
    }
}
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = if args.first().is_some_and(|arg| arg == "verify-tree") {
        verify_tree::parse(&args[1..]).and_then(verify_tree::run)
    } else {
        execute(&args).map(|v| (v, 0))
    }
    .and_then(|(v, status)| {
        serde_json::to_string_pretty(&v)
            .map(|text| (text, status))
            .map_err(|e| e.to_string())
    });
    match result {
        Ok((text, status)) => {
            if let Err(e) = writeln!(io::stdout(), "{text}") {
                if e.kind() != io::ErrorKind::BrokenPipe {
                    eprintln!("loomward-native: {e}");
                    return ExitCode::from(2);
                }
            }
            ExitCode::from(status)
        }
        Err(e) => {
            eprintln!("loomward-native: {e}");
            ExitCode::from(2)
        }
    }
}
