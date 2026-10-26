#![forbid(unsafe_code)]

mod cli;
mod json;
mod protocol;
mod stdio;
mod text;
mod tools;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args = match cli::Args::parse(
        std::env::args_os().skip(1),
        std::env::var_os("LOCALAPPDATA"),
    ) {
        Ok(args) => args,
        Err(error) => {
            eprintln!(
                "loomward-mcp: {}\n{}",
                text::escape_string(&error),
                cli::USAGE
            );
            return ExitCode::from(2);
        }
    };
    match stdio::serve(protocol::Protocol::new(tools::Host::new(args))) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!(
                "loomward-mcp: stdio: {}",
                text::escape_string(&error.to_string())
            );
            ExitCode::FAILURE
        }
    }
}
