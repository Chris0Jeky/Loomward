//! `loomward-desktop`: the Tauri 2 shell around the built app (`app/dist`). Two IPC commands,
//! `lw_call` and `lw_events`; the capability file grants nothing else. Until lane L8 lands the
//! engine service it answers from the contract examples (`FixtureService`, synthetic only).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use loomward_desktop::dialogs::DesktopDialogs;
use loomward_desktop::{selftest, shell, Desktop};
use loomward_http::fixture::FixtureService;
use loomward_protocol::DatasetClass;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

const USAGE: &str =
    "usage: loomward-desktop [--self-test] [--dataset synthetic|personal] [--allow-personal]";

/// Console output for `--self-test` from a release build (GUI subsystem has no console).
fn attach_console() {
    #[cfg(windows)]
    // SAFETY: plain Win32 call with a constant argument; failure just leaves no console.
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(
            windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS,
        );
    }
}

fn main() -> ExitCode {
    let mut dataset = DatasetClass::Synthetic;
    let (mut allow_personal, mut self_test) = (false, false);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--self-test" => self_test = true,
            "--allow-personal" => allow_personal = true,
            "--dataset" => match args.next().as_deref() {
                Some("synthetic") => dataset = DatasetClass::Synthetic,
                Some("personal") => dataset = DatasetClass::Personal,
                _ => return usage("--dataset is synthetic or personal"),
            },
            _ => return usage(&format!("unknown argument {a}")),
        }
    }
    if self_test {
        attach_console();
        let ok = selftest::run(&mut |line| println!("{line}"));
        println!("self-test {}", if ok { "passed" } else { "FAILED" });
        return if ok {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    if dataset == DatasetClass::Personal && !allow_personal {
        return usage("--dataset personal also requires --allow-personal");
    }
    // Personal mode is refused here exactly as by loomward-serve (#143): the fixture serves
    // synthetic examples only.
    // An installed build has no checkout: its examples are bundled next to the exe
    // (`bundle.resources` in tauri.conf.json). Without them the compiled-in checkout path is used.
    let service = match installed_examples(dataset) {
        Some(dir) => FixtureService::load(dataset, &dir),
        None => FixtureService::new(dataset),
    };
    let service = match service {
        Ok(s) => s,
        Err(e) => {
            attach_console();
            eprintln!("loomward-desktop: {e}");
            return ExitCode::FAILURE;
        }
    };
    let dialogs = Arc::new(DesktopDialogs::new(dataset));
    let desktop = Arc::new(Desktop::new(Arc::new(service), dataset, dialogs.clone()));
    let result = shell::configure(tauri::Builder::default().manage(dialogs), desktop)
        .run(tauri::generate_context!());
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            attach_console();
            eprintln!("loomward-desktop: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The contract examples bundled beside the executable by the installer, synthetic sessions only
/// (`FixtureService::new` keeps refusing every other dataset).
fn installed_examples(dataset: DatasetClass) -> Option<PathBuf> {
    if dataset != DatasetClass::Synthetic {
        return None;
    }
    bundled_examples_dir(std::env::current_exe().ok()?.parent()?)
}

fn bundled_examples_dir(exe_dir: &Path) -> Option<PathBuf> {
    let dir = exe_dir.join("contracts/v3/examples");
    dir.join("commands").is_dir().then_some(dir)
}

fn usage(message: &str) -> ExitCode {
    attach_console();
    eprintln!("loomward-desktop: {message}\n{USAGE}");
    ExitCode::from(2)
}

#[cfg(test)]
mod tests {
    use super::bundled_examples_dir;

    #[test]
    fn bundled_examples_are_found_only_when_installed_beside_the_exe() {
        let exe_dir = tempfile::tempdir().expect("temp dir");
        assert_eq!(bundled_examples_dir(exe_dir.path()), None);
        let commands = exe_dir.path().join("contracts/v3/examples/commands");
        std::fs::create_dir_all(&commands).expect("create");
        assert_eq!(
            bundled_examples_dir(exe_dir.path()),
            Some(exe_dir.path().join("contracts/v3/examples"))
        );
    }
}
