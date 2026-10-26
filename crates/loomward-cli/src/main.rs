//! Native read-only CLI: metadata scans, tier simulation and admitted Windows tree verification.
#![forbid(unsafe_code)]
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::ExitCode;

mod verify_tree;

const USAGE: &str = "Usage: loomward-native capabilities | scan ROOT [MAX_ENTRIES] | plan SCENARIO.json | verify-tree SOURCE DESTINATION --state-dir DIR [--max-entries N] [--budget-bytes N] [--budget-secs N]";

fn execute(args: &[String]) -> Result<serde_json::Value, String> {
    match args.first().map(String::as_str) {
        Some("capabilities") if args.len() == 1 => Ok(
            serde_json::json!({"native_build":"local_build","metadata_scan":true,"tier_simulation":true,"file_mutation":false,"process_mutation":false,"native_content_hash":cfg!(windows)}),
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
/// Writes the JSON report plus a newline and flushes it. The computed `status` is returned only
/// when the whole report reached the writer; any write or flush error (including BrokenPipe)
/// exits 2, so a consumer that closed stdout early cannot read a bare status as a report.
fn emit_report(out: &mut impl Write, text: &str, status: u8) -> u8 {
    match writeln!(out, "{text}").and_then(|()| out.flush()) {
        Ok(()) => status,
        Err(e) => {
            let _ = writeln!(io::stderr(), "loomward-native: {e}");
            2
        }
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
        Ok((text, status)) => ExitCode::from(emit_report(&mut io::stdout().lock(), &text, status)),
        Err(e) => {
            eprintln!("loomward-native: {e}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stdout stand-in whose every write and flush fails with one error kind.
    struct FailingWriter(io::ErrorKind);

    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(self.0))
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::from(self.0))
        }
    }

    /// Accepts every write but fails only the final flush.
    struct FlushFailingWriter;

    impl Write for FlushFailingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("flush failed"))
        }
    }

    #[test]
    fn broken_pipe_on_report_write_exits_2_even_when_status_is_zero() {
        let mut out = FailingWriter(io::ErrorKind::BrokenPipe);
        assert_eq!(emit_report(&mut out, "{}", 0), 2);
        assert_eq!(emit_report(&mut out, "{}", 1), 2);
    }

    #[test]
    fn other_write_errors_exit_2() {
        let mut out = FailingWriter(io::ErrorKind::Other);
        assert_eq!(emit_report(&mut out, "{}", 0), 2);
    }

    #[test]
    fn flush_failure_exits_2_even_when_status_is_zero() {
        assert_eq!(emit_report(&mut FlushFailingWriter, "{}", 0), 2);
    }

    #[test]
    fn complete_write_keeps_the_computed_status() {
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(emit_report(&mut out, "{\"a\":1}", 1), 1);
        assert_eq!(out, b"{\"a\":1}\n");
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(emit_report(&mut out, "{}", 0), 0);
    }
}
