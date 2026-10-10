//! `loomward-serve`: the browser adapter. Until lane L8 lands the real service it answers from
//! the contract examples (`FixtureService`), so it scans nothing whatever roots are granted.

use loomward_http::fixture::FixtureService;
use loomward_http::{grants, serve, Options};
use loomward_protocol::DatasetClass;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

const USAGE: &str = "usage: loomward-serve [--dataset synthetic|personal] [--allow-personal] \
[--state-dir DIR] [--grant-root DIR]... [--port N] [--allow-origin http://localhost:PORT]... \
[--static DIR]";

struct Args {
    dataset: DatasetClass,
    allow_personal: bool,
    state_dir: Option<PathBuf>,
    grant_roots: Vec<PathBuf>,
    port: u16,
    allow_origins: Vec<String>,
    static_dir: Option<PathBuf>,
}

impl Args {
    fn http_options(self, roots: &[PathBuf]) -> Options {
        Options {
            bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), self.port),
            allow_origins: self.allow_origins,
            static_dir: self.static_dir,
            grant_roots: roots.to_vec(),
            state_dir: self.state_dir,
            ..Options::default()
        }
    }
}

fn parse(mut it: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut a = Args {
        dataset: DatasetClass::Synthetic,
        allow_personal: false,
        state_dir: None,
        grant_roots: Vec::new(),
        port: 0,
        allow_origins: Vec::new(),
        static_dir: None,
    };
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--dataset" => {
                a.dataset = match value()?.as_str() {
                    "synthetic" => DatasetClass::Synthetic,
                    "personal" => DatasetClass::Personal,
                    _ => return Err("--dataset is synthetic or personal".into()),
                }
            }
            "--allow-personal" => a.allow_personal = true,
            "--state-dir" => a.state_dir = Some(value()?.into()),
            "--grant-root" => a.grant_roots.push(value()?.into()),
            "--port" => a.port = value()?.parse().map_err(|_| "--port takes 0-65535")?,
            "--allow-origin" => a.allow_origins.push(value()?),
            "--static" => a.static_dir = Some(value()?.into()),
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if a.dataset == DatasetClass::Personal && !a.allow_personal {
        return Err("--dataset personal also requires --allow-personal".into());
    }
    Ok(a)
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("loomward-serve: {e}");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let roots =
        match grants::validate_roots(args.dataset, &args.grant_roots, args.state_dir.as_deref()) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("loomward-serve: {e}");
                return ExitCode::from(2);
            }
        };
    let service = match FixtureService::new(args.dataset) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("loomward-serve: {e}");
            return ExitCode::FAILURE;
        }
    };
    let options = args.http_options(&roots);
    let handle = match serve(Arc::new(service), options) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("loomward-serve: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!(
        "loomward-serve: fixture service (contract examples, no scanning); {} grant root(s) validated",
        roots.len()
    );
    // The one place the token is shown. Open this URL; the app moves the token out of the address bar.
    println!("{}", handle.url());
    handle.wait();
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_options_consume_exactly_the_validated_canonical_roots() {
        let base = std::env::temp_dir().join(format!("lw-http-startup-{}", std::process::id()));
        let grant = base.join("grant");
        std::fs::create_dir_all(&grant).unwrap();
        let raw = grant.join("..").join("grant");
        let args = parse(
            [
                "--dataset".into(),
                "personal".into(),
                "--allow-personal".into(),
                "--grant-root".into(),
                raw.to_str().unwrap().into(),
            ]
            .into_iter(),
        )
        .unwrap();
        let roots = grants::validate_roots(args.dataset, &args.grant_roots, None).unwrap();
        assert_ne!(roots, args.grant_roots);
        let options = args.http_options(&roots);
        assert_eq!(options.grant_roots, roots);
        assert_eq!(options.grant_roots, vec![grant.canonicalize().unwrap()]);
        std::fs::remove_dir_all(base).unwrap();
    }
}
