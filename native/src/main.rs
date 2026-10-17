//! `loomward-desktop`: the Tauri 2 shell around the built app (`app/dist`). Two IPC commands,
//! `lw_call` and `lw_events`; the capability file grants nothing else. It hosts the engine service
//! in a personal session by default (`launch.rs` explains the opt-in); `--dataset synthetic` and
//! `--fixtures` are the demo and test modes.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use loomward_desktop::dialogs::{error_box, DesktopDialogs};
use loomward_desktop::launch::{self, Launch, USAGE};
use loomward_desktop::updates::{self, Updates};
use loomward_desktop::{selftest, shell, Desktop};
use std::process::ExitCode;
use std::sync::Arc;

/// Console output for `--self-test` from a release build (GUI subsystem has no console). True when
/// stderr goes somewhere: a parent console, this build's own console, or a redirected handle.
fn attach_console() -> bool {
    #[cfg(windows)]
    // SAFETY: plain Win32 calls with constant arguments; failure just leaves no console.
    unsafe {
        use windows_sys::Win32::System::Console::{
            AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
        };
        AttachConsole(ATTACH_PARENT_PROCESS);
        let handle = GetStdHandle(STD_ERROR_HANDLE);
        !handle.is_null() && handle != windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE
    }
    #[cfg(not(windows))]
    true
}

/// A start that fails: printed where there is a console, else a native error box, so an installed
/// build started from the Start menu never exits silently (#373).
fn fail(message: &str, code: u8) -> ExitCode {
    let console = attach_console();
    eprintln!("loomward-desktop: {message}");
    if !console {
        error_box("Loomward could not start", message);
    }
    ExitCode::from(code)
}

fn main() -> ExitCode {
    let launch = match Launch::parse(std::env::args().skip(1)) {
        Ok(l) => l,
        Err(e) => return fail(&format!("{e}\n{USAGE}"), 2),
    };
    if launch.self_test {
        attach_console();
        let ok = selftest::run(&mut |line| println!("{line}"));
        println!("self-test {}", if ok { "passed" } else { "FAILED" });
        return if ok {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    // Holds the state directory's lock until the process exits.
    let service = match launch::open(&launch, std::env::var_os("LOCALAPPDATA")) {
        Ok(s) => s,
        Err(e) => return fail(&e, 1),
    };
    if let Some(notice) = launch::legacy_state_notice(&launch, std::env::var_os("LOCALAPPDATA")) {
        if attach_console() {
            eprintln!("loomward-desktop: {notice}");
        } else {
            error_box("Loomward: older state retained", &notice);
        }
    }
    let dialogs = Arc::new(DesktopDialogs::new(launch.dataset));
    let desktop = Arc::new(Desktop::new(service, launch.dataset, dialogs.clone()));
    let context = tauri::generate_context!();
    // The update prefs live beside the state (none in --fixtures mode); off by default.
    let updates = Arc::new(Updates::new(
        launch::state_dir(&launch, std::env::var_os("LOCALAPPDATA")),
        updates::configured(context.config()),
        &context.package_info().version.to_string(),
    ));
    let builder = tauri::Builder::default().manage(dialogs).manage(updates);
    let result = shell::configure(builder, desktop).run(context);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(&e.to_string(), 1),
    }
}
