use loomward_protocol::DatasetClass;
use loomward_service::Config;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

pub const USAGE: &str = "usage: loomward-mcp --client NAME [--state-dir DIR] [--dataset synthetic|personal] [--allow-personal] [--idle-release-secs N]";

pub struct Args {
    pub principal: String,
    pub config: Config,
    pub idle: Duration,
}

impl Args {
    pub fn parse(
        mut args: impl Iterator<Item = OsString>,
        local_app_data: Option<OsString>,
    ) -> Result<Self, String> {
        let mut client = None;
        let mut state = None;
        let mut dataset = DatasetClass::Personal;
        let mut allow_personal = false;
        let mut idle = 10;
        while let Some(flag) = args.next() {
            let flag = flag.to_str().ok_or("argument is not UTF-8")?;
            let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
            match flag {
                "--client" => {
                    let name = value()?.into_string().map_err(|_| "invalid client name")?;
                    if !(1..=32).contains(&name.len())
                        || !name
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                    {
                        return Err("--client must match [a-z0-9-]{1,32}".into());
                    }
                    client = Some(name);
                }
                "--state-dir" => {
                    let dir = value()?;
                    if dir.is_empty() {
                        return Err("--state-dir needs a nonempty path".into());
                    }
                    state = Some(PathBuf::from(dir));
                }
                "--dataset" => {
                    dataset = match value()?.to_str() {
                        Some("synthetic") => DatasetClass::Synthetic,
                        Some("personal") => DatasetClass::Personal,
                        _ => return Err("--dataset is synthetic or personal".into()),
                    }
                }
                "--allow-personal" => allow_personal = true,
                "--idle-release-secs" => {
                    let n = value()?;
                    idle = n
                        .to_str()
                        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                        .and_then(|s| s.parse::<u64>().ok())
                        .ok_or("--idle-release-secs takes a nonnegative integer")?;
                }
                _ => return Err(format!("unknown argument {flag}")),
            }
        }
        let client = client.ok_or("--client is required")?;
        if state.is_some() && dataset == DatasetClass::Personal && !allow_personal {
            return Err("a personal --state-dir also requires --allow-personal".into());
        }
        // Same class-separated default as the desktop, with personal as the launch default.
        let state_dir = match state {
            Some(dir) => dir,
            None => default_state_dir(local_app_data, dataset)?,
        };
        Ok(Self {
            principal: format!("mcp:{client}"),
            config: Config {
                state_dir,
                dataset,
                allow_personal: dataset == DatasetClass::Personal,
                allow_identity_unverified: false,
                grant_roots: vec![],
            },
            idle: Duration::from_secs(idle),
        })
    }
}

fn default_state_dir(base: Option<OsString>, dataset: DatasetClass) -> Result<PathBuf, String> {
    let base = base
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or("LOCALAPPDATA is not an absolute path; pass --state-dir")?;
    let class = match dataset {
        DatasetClass::Personal => "personal",
        DatasetClass::Synthetic => "synthetic",
    };
    Ok(base.join("Loomward Data").join("v3").join(class))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_home_and_personal_opt_in_match_the_launch_policy() {
        let base = std::env::current_dir()
            .unwrap()
            .join("synthetic-local-app-data");
        let parse = |args: &[&str]| {
            Args::parse(
                args.iter().map(OsString::from),
                Some(base.clone().into_os_string()),
            )
        };
        let a = parse(&["--client", "sample-1"]).unwrap();
        assert_eq!(a.principal, "mcp:sample-1");
        assert_eq!(a.config.state_dir, base.join("Loomward Data/v3/personal"));
        assert_eq!(a.config.dataset, DatasetClass::Personal);
        assert!(a.config.allow_personal);
        assert_eq!(a.idle, Duration::from_secs(10));
        assert!(parse(&["--client", "test", "--state-dir", "fixture"]).is_err());
        assert!(parse(&[
            "--client",
            "test",
            "--state-dir",
            "fixture",
            "--allow-personal"
        ])
        .is_ok());
        let a = parse(&[
            "--client",
            "test",
            "--dataset",
            "synthetic",
            "--idle-release-secs",
            "0",
        ])
        .unwrap();
        assert_eq!(a.config.state_dir, base.join("Loomward Data/v3/synthetic"));
        assert!(!a.config.allow_personal);
        assert_eq!(a.idle, Duration::ZERO);
        assert!(Args::parse(["--client", "test"].map(OsString::from).into_iter(), None).is_err());
    }
}
