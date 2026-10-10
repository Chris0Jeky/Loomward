//! `loomward-serve`: the browser adapter over the engine service (`loomward-service`). Roots come
//! only from `--grant-root`, checked by the shared path rules and then by the session's provenance
//! policy (a synthetic session grants only registered lab roots). `--fixtures` serves the contract
//! examples instead (`FixtureService`): no state, no roots, no scanning.

use loomward_http::fixture::FixtureService;
use loomward_http::{serve, Options};
use loomward_protocol::{DatasetClass, ViewService};
use loomward_service::{paths, Config, Service};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

const USAGE: &str = "usage: loomward-serve (--state-dir DIR | --fixtures) [--dataset synthetic|personal] \
[--allow-personal] [--grant-root DIR]... [--port N] [--allow-origin http://localhost:PORT]... [--static DIR]";

struct Args {
    dataset: DatasetClass,
    allow_personal: bool,
    fixtures: bool,
    state_dir: Option<PathBuf>,
    grant_roots: Vec<PathBuf>,
    port: u16,
    allow_origins: Vec<String>,
    static_dir: Option<PathBuf>,
}

fn parse(mut it: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut a = Args {
        dataset: DatasetClass::Synthetic,
        allow_personal: false,
        fixtures: false,
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
            "--fixtures" => a.fixtures = true,
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
    match (a.fixtures, &a.state_dir) {
        (true, Some(_)) => return Err("--fixtures keeps no state; drop --state-dir".into()),
        (true, None) if !a.grant_roots.is_empty() => {
            return Err("--fixtures serves contract examples and grants no roots".into())
        }
        (false, None) => return Err("--state-dir is required (or --fixtures)".into()),
        _ => {}
    }
    Ok(a)
}

fn service(args: &Args) -> Result<(Arc<dyn ViewService>, String), String> {
    if args.fixtures {
        let s = FixtureService::new(args.dataset).map_err(|e| e.to_string())?;
        return Ok((
            Arc::new(s),
            "fixture service (contract examples, no scanning)".into(),
        ));
    }
    let state_dir = args.state_dir.clone().expect("checked by parse");
    std::fs::create_dir_all(&state_dir).map_err(|_| "cannot create --state-dir".to_string())?;
    // The canonical roots the rules approved are exactly the roots the service grants (#146).
    let roots = paths::validate_roots(&args.grant_roots, Some(&state_dir))?;
    let count = roots.len();
    let s = Service::open(Config {
        state_dir,
        dataset: args.dataset,
        allow_personal: args.allow_personal,
        grant_roots: roots,
    })?;
    Ok((
        Arc::new(s),
        format!("engine service; {count} grant root(s) granted"),
    ))
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
    let (service, summary) = match service(&args) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("loomward-serve: {e}");
            return ExitCode::from(2);
        }
    };
    let options = Options {
        bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), args.port),
        allow_origins: args.allow_origins,
        static_dir: args.static_dir,
        ..Options::default()
    };
    let handle = match serve(service, options) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("loomward-serve: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("loomward-serve: {summary}");
    // The one place the token is shown. Open this URL; the app moves the token out of the address bar.
    println!("{}", handle.url());
    handle.wait();
    ExitCode::SUCCESS
}
