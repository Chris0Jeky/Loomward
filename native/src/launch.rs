//! What the window hosts: the command line and the [`ViewService`] behind `lw_call`/`lw_events`.
//!
//! By default the engine service (`loomward_service::Service`) in a personal session, with its
//! state under `%LOCALAPPDATA%\Loomward\v3\personal` (docs/41 section 3). `loomward-serve` gates
//! personal mode behind `--allow-personal` because it is launched from a command line and takes
//! roots from `--grant-root`, with no person in the loop. The desktop accepts no root argument: a
//! personal session starts with no new root, and every root comes from the owner choosing it in
//! the Rust-owned native picker, which the webview cannot open or drive (`dialogs.rs`). That choice
//! is the deliberate opt-in, made per folder. Agents and tests pass `--dataset synthetic` or
//! `--fixtures`, and a scratch `--state-dir`, so they never open the owner's state.

use loomward_http::fixture::{FixtureService, EXAMPLES_DIR};
use loomward_protocol::{DatasetClass, ViewService};
use loomward_service::{Config, Service};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const USAGE: &str =
    "usage: loomward-desktop [--self-test] [--dataset personal|synthetic] [--state-dir DIR] [--fixtures]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub self_test: bool,
    pub dataset: DatasetClass,
    /// The contract examples (`FixtureService`): synthetic, no state, no scanning.
    pub fixtures: bool,
    /// Overrides the per-user default state directory.
    pub state_dir: Option<PathBuf>,
}

impl Launch {
    pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Launch, String> {
        let mut l = Launch {
            self_test: false,
            dataset: DatasetClass::Personal,
            fixtures: false,
            state_dir: None,
        };
        let mut dataset = None;
        while let Some(a) = args.next() {
            match a.as_str() {
                "--self-test" => l.self_test = true,
                "--fixtures" => l.fixtures = true,
                "--dataset" => match args.next().as_deref() {
                    Some("synthetic") => dataset = Some(DatasetClass::Synthetic),
                    Some("personal") => dataset = Some(DatasetClass::Personal),
                    _ => return Err("--dataset is synthetic or personal".into()),
                },
                "--state-dir" => match args.next() {
                    Some(dir) => l.state_dir = Some(dir.into()),
                    None => return Err("--state-dir needs a value".into()),
                },
                _ => return Err(format!("unknown argument {a}")),
            }
        }
        if l.fixtures {
            if dataset == Some(DatasetClass::Personal) {
                return Err("--fixtures serves synthetic contract examples only".into());
            }
            if l.state_dir.is_some() {
                return Err("--fixtures keeps no state; drop --state-dir".into());
            }
            dataset = Some(DatasetClass::Synthetic);
        }
        l.dataset = dataset.unwrap_or(DatasetClass::Personal);
        Ok(l)
    }
}

/// `<local app data>\Loomward\v3\<dataset class>`: one directory per class, so a synthetic session
/// never opens a personal store (the catalogue also refuses a store of the other class).
pub fn default_state_dir(
    local_app_data: Option<OsString>,
    dataset: DatasetClass,
) -> Result<PathBuf, String> {
    let base = local_app_data
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or("LOCALAPPDATA is not an absolute path; pass --state-dir")?;
    let class = match dataset {
        DatasetClass::Synthetic => "synthetic",
        DatasetClass::Personal => "personal",
    };
    Ok(base.join("Loomward").join("v3").join(class))
}

/// Opens the service the window hosts. `local_app_data` is `%LOCALAPPDATA%` (injected for tests);
/// it is read only when no `--state-dir` was given.
pub fn open(
    launch: &Launch,
    local_app_data: Option<OsString>,
) -> Result<Arc<dyn ViewService>, String> {
    if launch.fixtures {
        let fixture = FixtureService::load(DatasetClass::Synthetic, &examples_dir())
            .map_err(|e| e.to_string())?;
        return Ok(Arc::new(fixture));
    }
    let state_dir = match &launch.state_dir {
        Some(dir) => dir.clone(),
        None => default_state_dir(local_app_data, launch.dataset)?,
    };
    let service = Service::open(Config {
        state_dir,
        dataset: launch.dataset,
        // See the module comment: launching the desktop is the personal session; roots stay
        // empty here and arrive only through the owner's native picker.
        allow_personal: launch.dataset == DatasetClass::Personal,
        grant_roots: Vec::new(),
    })?;
    Ok(Arc::new(service))
}

/// The contract examples: bundled beside an installed exe (`bundle.resources` in tauri.conf.json),
/// else the checkout path compiled into the binary.
pub fn examples_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| bundled_examples_dir(exe.parent()?))
        .unwrap_or_else(|| PathBuf::from(EXAMPLES_DIR))
}

pub fn bundled_examples_dir(exe_dir: &Path) -> Option<PathBuf> {
    let dir = exe_dir.join("contracts/v3/examples");
    dir.join("commands").is_dir().then_some(dir)
}
