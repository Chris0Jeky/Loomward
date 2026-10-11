//! What the window hosts: the command line and the [`ViewService`] behind `lw_call`/`lw_events`.
//!
//! By default the engine service (`loomward_service::Service`) in a personal session, with its
//! state under `%LOCALAPPDATA%\Loomward Data\v3\personal`, beside (not inside) the per-user install
//! directory `%LOCALAPPDATA%\Loomward`, so uninstalling cannot reach it (#373). `loomward-serve` gates
//! personal mode behind `--allow-personal` because it is launched from a command line and takes
//! roots from `--grant-root`, with no person in the loop. The desktop accepts no root argument: a
//! personal session starts with no new root, and every root comes from the owner choosing it in
//! the Rust-owned native picker, which the webview cannot open or drive (`dialogs.rs`). That choice
//! is the deliberate opt-in, made per folder. Agents and tests pass `--dataset synthetic` or
//! `--fixtures`, and a scratch `--state-dir`, so they never open the owner's state.

use loomward_http::fixture::{FixtureService, EXAMPLES_DIR};
use loomward_protocol::{
    CallContext, DatasetClass, EventStream, RequestEnvelope, ResponseEnvelope, ViewService,
};
use loomward_service::{Config, Service};
use std::ffi::OsString;
use std::fs::{File, TryLockError};
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

/// The state's home under `%LOCALAPPDATA%`. A sibling of the install directory `Loomward`.
pub const DATA_DIR: &str = "Loomward Data";

fn local_base(local_app_data: Option<OsString>) -> Result<PathBuf, String> {
    local_app_data
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or_else(|| "LOCALAPPDATA is not an absolute path; pass --state-dir".into())
}

/// `<local app data>\Loomward Data\v3\<dataset class>`: one directory per class, so a synthetic
/// session never opens a personal store (the catalogue also refuses a store of the other class).
pub fn default_state_dir(
    local_app_data: Option<OsString>,
    dataset: DatasetClass,
) -> Result<PathBuf, String> {
    let class = match dataset {
        DatasetClass::Synthetic => "synthetic",
        DatasetClass::Personal => "personal",
    };
    Ok(local_base(local_app_data)?
        .join(DATA_DIR)
        .join("v3")
        .join(class))
}

/// One-time move of the state that builds before #373 kept inside the install directory
/// (`<base>\Loomward\v3`) to `<base>\Loomward Data\v3`. Nothing happens when there is no old state
/// or the new home already exists (the old copy is then left untouched, never merged). A failed
/// move refuses to start rather than open an empty store over the owner's real one.
pub fn migrate_legacy_state(base: &Path) -> Result<(), String> {
    let (old, new) = (
        base.join("Loomward").join("v3"),
        base.join(DATA_DIR).join("v3"),
    );
    if !old.is_dir() || new.exists() {
        return Ok(());
    }
    let moved =
        std::fs::create_dir_all(base.join(DATA_DIR)).and_then(|()| std::fs::rename(&old, &new));
    match moved {
        Ok(()) => Ok(()),
        // Another launch moved it first.
        Err(_) if new.is_dir() && !old.exists() => Ok(()),
        Err(e) => Err(format!(
            "could not move Loomward's state from {} to {}: {e}. Close every Loomward window and start it again.",
            old.display(),
            new.display()
        )),
    }
}

/// The exclusive lock file of docs/41 section 3, created in `state_dir` and held until the service
/// drops. It is an OS lock on an open handle, so a crashed instance leaves no stale lock behind.
pub fn lock_state_dir(state_dir: &Path) -> Result<File, String> {
    let dir = loomward_service::paths::prospective_dir(state_dir)?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("cannot create {}: {e}", state_dir.display()))?;
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("lock"))
        .map_err(|e| format!("cannot open the state lock in {}: {e}", state_dir.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(format!(
            "busy: Loomward is already open on {}. Switch to that window, or close it first.",
            state_dir.display()
        )),
        Err(TryLockError::Error(e)) => Err(format!(
            "cannot lock the state in {}: {e}",
            state_dir.display()
        )),
    }
}

/// The service with its state lock: the lock is released only after the service has dropped.
struct Locked {
    service: Service,
    _lock: File,
}

impl ViewService for Locked {
    fn call(&self, request: RequestEnvelope, ctx: &CallContext) -> ResponseEnvelope {
        self.service.call(request, ctx)
    }
    fn subscribe(&self, resume: Option<(String, u64)>) -> Box<dyn EventStream> {
        self.service.subscribe(resume)
    }
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
        None => {
            migrate_legacy_state(&local_base(local_app_data.clone())?)?;
            default_state_dir(local_app_data, launch.dataset)?
        }
    };
    // Before the stores open: a second instance must not touch them (docs/41 section 3).
    let lock = lock_state_dir(&state_dir)?;
    let service = Service::open(Config {
        state_dir,
        dataset: launch.dataset,
        // See the module comment: launching the desktop is the personal session; roots stay
        // empty here and arrive only through the owner's native picker.
        allow_personal: launch.dataset == DatasetClass::Personal,
        grant_roots: Vec::new(),
        // Unverified (FAT/exFAT) volumes are confirmed per root in the native picker (#219); the
        // startup flag only covers CLI-supplied roots, and the desktop takes none.
        allow_identity_unverified: false,
    })?;
    Ok(Arc::new(Locked {
        service,
        _lock: lock,
    }))
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
