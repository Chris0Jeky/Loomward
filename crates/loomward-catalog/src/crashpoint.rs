//! Crash injection (q-39): every durable write boundary of the stores calls `hit`; when the
//! process is started with `LOOMWARD_CRASH_AT=N` the Nth boundary ends the process on the spot
//! (`exit(CRASH_EXIT)`: no destructors, no graceful close, no checkpoint), the closest portable
//! stand-in for a kill. `LOOMWARD_CRASH_TRACE=<file>` appends `N<TAB>label` per boundary so a
//! failing N can be named. Compiled only with the `test-hooks` feature; production builds get the
//! no-op stubs below and read no environment variable.
use rusqlite::Connection;

#[cfg(feature = "test-hooks")]
mod armed {
    use rusqlite::{hooks::Action, Connection};
    use std::{
        io::Write,
        sync::{
            atomic::{AtomicU64, Ordering},
            OnceLock,
        },
    };
    pub const CRASH_EXIT: i32 = 86;
    static COUNT: AtomicU64 = AtomicU64::new(0);
    fn config() -> &'static (Option<u64>, Option<std::path::PathBuf>) {
        static CONFIG: OnceLock<(Option<u64>, Option<std::path::PathBuf>)> = OnceLock::new();
        CONFIG.get_or_init(|| {
            (
                std::env::var("LOOMWARD_CRASH_AT")
                    .ok()
                    .and_then(|v| v.parse().ok()),
                std::env::var_os("LOOMWARD_CRASH_TRACE").map(Into::into),
            )
        })
    }
    pub fn hit(label: &str) {
        let n = COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        let (target, trace) = config();
        if let Some(path) = trace {
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                let _ = writeln!(f, "{n}\t{label}");
            }
        }
        if *target == Some(n) {
            std::process::exit(CRASH_EXIT);
        }
    }
    /// Whether this process was started to trace or to crash.
    pub fn configured() -> bool {
        let (target, trace) = config();
        target.is_some() || trace.is_some()
    }
    pub fn arm(conn: &Connection) {
        let _ = conn.update_hook(Some(|action: Action, db: &str, table: &str, _row: i64| {
            hit(&format!("row {action:?} {db}.{table}"));
        }));
        let _ = conn.commit_hook(Some(|| {
            hit("pre-commit");
            false
        }));
        conn.wal_hook(Some(|_, _| {
            hit("post-commit");
            Ok(())
        }));
    }
}
#[cfg(feature = "test-hooks")]
pub use armed::CRASH_EXIT;

/// A boundary outside SQLite's hooks: a rename, a delete, a checkpoint, a VACUUM.
#[inline]
pub fn hit(label: &str) {
    #[cfg(feature = "test-hooks")]
    armed::hit(label);
    #[cfg(not(feature = "test-hooks"))]
    let _ = label;
}
/// Hooks the row, pre-commit and post-commit boundaries of one connection. A connection that sets
/// its own `wal_hook` afterwards must call `hit("post-commit")` there.
#[inline]
pub(crate) fn arm(conn: &Connection) {
    #[cfg(feature = "test-hooks")]
    armed::arm(conn);
    #[cfg(not(feature = "test-hooks"))]
    let _ = conn;
}
/// [`arm`] for another crate's store connection (the service's and the learning store's), only in
/// a process started to trace or to crash: elsewhere their SQLite auto-checkpoint stays as built.
#[inline]
pub fn arm_external(conn: &Connection) {
    #[cfg(feature = "test-hooks")]
    if armed::configured() {
        armed::arm(conn);
    }
    #[cfg(not(feature = "test-hooks"))]
    let _ = conn;
}
