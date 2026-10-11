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
[--allow-personal] [--grant-root DIR]... [--allow-identity-unverified] [--port N] [--allow-origin http://localhost:PORT]... [--static DIR]";

struct Args {
    dataset: DatasetClass,
    allow_personal: bool,
    fixtures: bool,
    state_dir: Option<PathBuf>,
    grant_roots: Vec<PathBuf>,
    /// #219: list a FAT, exFAT or unknown-filesystem grant root read-only instead of refusing it.
    allow_identity_unverified: bool,
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
        fixtures: false,
        state_dir: None,
        grant_roots: Vec::new(),
        allow_identity_unverified: false,
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
            "--allow-identity-unverified" => a.allow_identity_unverified = true,
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

type Started = (Arc<dyn ViewService>, String, Vec<PathBuf>);

fn service(args: &Args) -> Result<Started, String> {
    if args.fixtures {
        let s = FixtureService::new(args.dataset).map_err(|e| e.to_string())?;
        return Ok((
            Arc::new(s),
            "fixture service (contract examples, no scanning)".into(),
            Vec::new(),
        ));
    }
    let state_dir = args.state_dir.clone().expect("checked by parse");
    paths::prospective_dir(&state_dir)?;
    // The canonical roots the rules approved are exactly the roots the service grants (#146).
    let roots = paths::validate_roots(&args.grant_roots, Some(&state_dir))?;
    let count = roots.len();
    let s = Service::open_with_static(
        Config {
            state_dir,
            dataset: args.dataset,
            allow_personal: args.allow_personal,
            allow_identity_unverified: args.allow_identity_unverified,
            grant_roots: roots.clone(),
        },
        args.static_dir.as_deref(),
    )?;
    Ok((
        Arc::new(s),
        format!("engine service; {count} grant root(s) granted"),
        roots,
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
    let (service, summary, roots) = match service(&args) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("loomward-serve: {e}");
            return ExitCode::from(2);
        }
    };
    // The static root must not overlap the roots the service granted or its state (#146).
    let options = args.http_options(&roots);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardening_server_rejects_network_state_before_creating_it() {
        let args = parse(
            ["--state-dir", r"\\?\GLOBALROOT\Device\fixture\state"]
                .map(String::from)
                .into_iter(),
        )
        .unwrap();
        let error = service(&args).err().unwrap();
        assert_eq!(error, "network_or_removable_unsupported");
        let tmp = tempfile::tempdir_in(".").unwrap();
        let state = tmp.path().join("state");
        let missing = tmp.path().join("missing");
        let args = parse(
            [
                "--state-dir".into(),
                state.to_str().unwrap().into(),
                "--grant-root".into(),
                missing.to_str().unwrap().into(),
            ]
            .into_iter(),
        )
        .unwrap();
        assert!(service(&args).is_err());
        assert!(!state.exists());
    }

    /// #219: identity-unverified grant roots need the owner's explicit flag; it is off by default.
    #[test]
    fn identity_unverified_grants_need_an_explicit_flag() {
        let args = |extra: &[&str]| {
            parse(
                ["--state-dir", "state"]
                    .iter()
                    .chain(extra)
                    .map(|s| s.to_string()),
            )
            .unwrap()
        };
        assert!(!args(&[]).allow_identity_unverified);
        assert!(args(&["--allow-identity-unverified"]).allow_identity_unverified);
        assert!(USAGE.contains("[--allow-identity-unverified]"));
    }

    #[test]
    fn http_options_consume_exactly_the_validated_canonical_roots() {
        let base = std::env::current_dir()
            .unwrap()
            .join(format!("lw-http-startup-{}", std::process::id()));
        let grant = base.join("grant");
        let state = base.join("state");
        std::fs::create_dir_all(&grant).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        let raw = grant.join("..").join("grant");
        let args = parse(
            [
                "--dataset".into(),
                "personal".into(),
                "--allow-personal".into(),
                "--grant-root".into(),
                raw.to_str().unwrap().into(),
                "--state-dir".into(),
                state.to_str().unwrap().into(),
            ]
            .into_iter(),
        )
        .unwrap();
        #[cfg(windows)]
        if loomward_windows::enumerate::running_elevated() {
            // Elevated hosts (hosted CI) refuse every grant: the volume query opens nothing elevated.
            let refused = paths::validate_roots(&args.grant_roots, Some(&state)).unwrap_err();
            assert!(refused.contains("identity_unavailable"), "{refused}");
            std::fs::remove_dir_all(base).unwrap();
            return;
        }
        let roots = paths::validate_roots(&args.grant_roots, Some(&state)).unwrap();
        assert_ne!(roots, args.grant_roots);
        let options = args.http_options(&roots);
        assert_eq!(options.grant_roots, roots);
        assert_eq!(options.grant_roots, vec![grant.canonicalize().unwrap()]);
        std::fs::remove_dir_all(base).unwrap();
    }

    fn argv(items: &[&str]) -> impl Iterator<Item = String> {
        items
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn personal_dataset_is_refused_without_allow_personal() {
        let err = parse(argv(&["--fixtures", "--dataset", "personal"]))
            .err()
            .unwrap();
        assert_eq!(err, "--dataset personal also requires --allow-personal");
        // Control: the same request with the opt-in is accepted.
        let ok = parse(argv(&[
            "--fixtures",
            "--dataset",
            "personal",
            "--allow-personal",
        ]));
        assert!(ok.is_ok());
    }

    #[test]
    fn invalid_dataset_is_refused() {
        for bad in ["production", "Synthetic", "", "personal;synthetic"] {
            let err = parse(argv(&["--fixtures", "--dataset", bad]))
                .err()
                .unwrap();
            assert_eq!(err, "--dataset is synthetic or personal", "{bad:?}");
        }
    }

    #[test]
    fn invalid_port_is_refused() {
        for bad in ["65536", "-1", "http", "", "1.5"] {
            let err = parse(argv(&["--fixtures", "--port", bad])).err().unwrap();
            assert_eq!(err, "--port takes 0-65535", "{bad:?}");
        }
        // Boundary: the largest port is accepted.
        assert!(parse(argv(&["--fixtures", "--port", "65535"])).is_ok());
    }

    #[test]
    fn a_flag_without_its_value_is_refused() {
        for flag in [
            "--dataset",
            "--state-dir",
            "--grant-root",
            "--port",
            "--allow-origin",
            "--static",
        ] {
            let err = parse(argv(&["--fixtures", flag])).err().unwrap();
            assert_eq!(err, format!("{flag} needs a value"));
        }
    }
}
