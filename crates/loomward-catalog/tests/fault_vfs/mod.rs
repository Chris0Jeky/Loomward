//! I/O faults below SQLite (docs/45 row 5): a VFS that wraps the platform one (`win32` on
//! Windows) and fails the Nth read, write, fsync or truncate of this process the way the OS
//! would. Installed as the default VFS only inside a sweep child (`install`), so no other test
//! of the binary sees it. `LOOMWARD_FAULT=<kind>:<N>` arms one fault; without it the VFS only
//! counts calls, which is how a clean run enumerates them.
//!
//! Faults, with the Win32 failure each stands for (`winRead`/`winWrite` in SQLite's `os_win.c`
//! map them to the same codes):
//! - `read`: ReadFile fails (`ERROR_CRC`, a bad sector): `SQLITE_IOERR_READ`.
//! - `short-read` (`SHORT_READ`, recovery only): ReadFile stops early mid-file
//!   (`ERROR_HANDLE_EOF`): half the bytes, the rest zeroed, `SQLITE_IOERR_SHORT_READ`.
//! - `write`: WriteFile fails (`ERROR_IO_DEVICE`): `SQLITE_IOERR_WRITE`, nothing written.
//! - `full`: the volume is full (`ERROR_DISK_FULL`): `SQLITE_FULL`, nothing written, and every
//!   later write fails the same way until the space is freed (`clear`).
//! - `short-write`: the disk fills part way through one WriteFile: half the bytes land, then
//!   `SQLITE_FULL`, sticky like `full`.
//! - `sync`: FlushFileBuffers fails: `SQLITE_IOERR_FSYNC`.
//! - `truncate`: SetEndOfFile fails: `SQLITE_IOERR_TRUNCATE`.
//! - `lost-write` (`LOST_WRITE`, the checker's mutant, never swept as survivable): a lying disk.
//!
//! The wrapper keeps the platform file object as is and only swaps its method table for a copy
//! whose read, write, sync and truncate check the plan first. Shared-memory (`-shm`) mapping,
//! locking and the VFS-level calls (delete, access) are not faulted.
#![allow(unsafe_code)]
use rusqlite::ffi;
use std::{
    ffi::{c_char, c_int, c_void},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering::SeqCst},
        OnceLock,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Read = 0,
    Write = 1,
    Sync = 2,
    Truncate = 3,
}
pub const CLASSES: [Class; 4] = [Class::Read, Class::Write, Class::Sync, Class::Truncate];

/// Every fault the store must survive honestly, with the call class it counts and fails.
pub const KINDS: [(&str, Class); 6] = [
    ("read", Class::Read),
    ("write", Class::Write),
    ("full", Class::Write),
    ("short-write", Class::Write),
    ("sync", Class::Sync),
    ("truncate", Class::Truncate),
];
/// Not a fault the sweep expects to survive: a disk that reports success for every write from
/// the Nth on and stores none of them. It proves the sweep's checker can fail.
pub const LOST_WRITE: (&str, Class) = ("lost-write", Class::Write);
/// A read that stops short mid-file. SQLite's VFS contract treats a short read as end of file
/// and zero-fills the rest (`pager.c`), so a lie here reaches SQL as zeroed data: the error that
/// follows (if any) cannot name the disk. Swept for recovery only, not for an honest code.
pub const SHORT_READ: (&str, Class) = ("short-read", Class::Read);

static COUNTS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
static FIRED: AtomicBool = AtomicBool::new(false);
static CLEARED: AtomicBool = AtomicBool::new(false);
static INNER: OnceLock<usize> = OnceLock::new();
static AT_OPEN: OnceLock<[u64; 4]> = OnceLock::new();
/// The platform's method table and the faulting copy of it.
static METHODS: OnceLock<(usize, ffi::sqlite3_io_methods)> = OnceLock::new();

fn plan() -> Option<(&'static str, Class, u64)> {
    static PLAN: OnceLock<Option<(&'static str, Class, u64)>> = OnceLock::new();
    *PLAN.get_or_init(|| {
        let spec = std::env::var("LOOMWARD_FAULT").ok()?;
        let (kind, n) = spec.split_once(':').expect("LOOMWARD_FAULT=<kind>:<N>");
        let (kind, class) = KINDS
            .iter()
            .chain([&LOST_WRITE, &SHORT_READ])
            .copied()
            .find(|(k, _)| *k == kind)
            .unwrap_or_else(|| panic!("unknown fault kind {kind}"));
        Some((kind, class, n.parse().expect("fault call number")))
    })
}

/// Calls of each class so far, in `CLASSES` order.
pub fn counts() -> [u64; 4] {
    CLASSES.map(|c| COUNTS[c as usize].load(SeqCst))
}
/// Records the counts when the store first opened; later calls keep the first.
pub fn mark_open() {
    let _ = AT_OPEN.set(counts());
}
/// The counts when the store first opened (all of them if it never did).
pub fn at_open() -> [u64; 4] {
    AT_OPEN.get().copied().unwrap_or_else(counts)
}
/// The armed fault has been injected.
pub fn fired() -> bool {
    FIRED.load(SeqCst)
}
/// The fault clears: a transient error has passed, or space was freed on a full disk.
pub fn clear() {
    CLEARED.store(true, SeqCst);
}

/// The fault to apply to this call, if any.
fn inject(class: Class) -> Option<&'static str> {
    let n = COUNTS[class as usize].fetch_add(1, SeqCst) + 1;
    let (kind, armed, at) = plan()?;
    if armed != class || CLEARED.load(SeqCst) {
        return None;
    }
    if n == at {
        FIRED.store(true, SeqCst);
        return Some(kind);
    }
    match kind {
        "full" | "short-write" if n > at => Some("full"),
        "lost-write" if n > at => Some(kind),
        _ => None,
    }
}

fn orig() -> &'static ffi::sqlite3_io_methods {
    let (ptr, _) = METHODS.get().expect("a file was opened");
    // SAFETY: the platform VFS's method tables are static for the life of the process.
    unsafe { &*(*ptr as *const ffi::sqlite3_io_methods) }
}

unsafe extern "C" fn read(
    f: *mut ffi::sqlite3_file,
    buf: *mut c_void,
    amt: c_int,
    off: i64,
) -> c_int {
    let fault = inject(Class::Read);
    if fault == Some("read") {
        return ffi::SQLITE_IOERR_READ;
    }
    let rc = unsafe { orig().xRead.unwrap()(f, buf, amt, off) };
    if fault == Some("short-read") && rc == ffi::SQLITE_OK {
        let half = (amt / 2) as usize;
        // SAFETY: SQLite passed a buffer of `amt` bytes.
        unsafe { std::ptr::write_bytes((buf as *mut u8).add(half), 0, amt as usize - half) };
        return ffi::SQLITE_IOERR_SHORT_READ;
    }
    rc
}

unsafe extern "C" fn write(
    f: *mut ffi::sqlite3_file,
    buf: *const c_void,
    amt: c_int,
    off: i64,
) -> c_int {
    match inject(Class::Write) {
        Some("write") => ffi::SQLITE_IOERR_WRITE,
        Some("full") => ffi::SQLITE_FULL,
        Some("lost-write") => ffi::SQLITE_OK,
        Some("short-write") => {
            let rc = unsafe { orig().xWrite.unwrap()(f, buf, amt / 2, off) };
            if rc == ffi::SQLITE_OK {
                ffi::SQLITE_FULL
            } else {
                rc
            }
        }
        _ => unsafe { orig().xWrite.unwrap()(f, buf, amt, off) },
    }
}

unsafe extern "C" fn sync(f: *mut ffi::sqlite3_file, flags: c_int) -> c_int {
    if inject(Class::Sync).is_some() {
        return ffi::SQLITE_IOERR_FSYNC;
    }
    unsafe { orig().xSync.unwrap()(f, flags) }
}

unsafe extern "C" fn truncate(f: *mut ffi::sqlite3_file, size: i64) -> c_int {
    if inject(Class::Truncate).is_some() {
        return ffi::SQLITE_IOERR_TRUNCATE;
    }
    unsafe { orig().xTruncate.unwrap()(f, size) }
}

unsafe extern "C" fn open(
    _vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    file: *mut ffi::sqlite3_file,
    flags: c_int,
    out: *mut c_int,
) -> c_int {
    let inner = *INNER.get().unwrap() as *mut ffi::sqlite3_vfs;
    let rc = unsafe { (*inner).xOpen.unwrap()(inner, name, file, flags, out) };
    let methods = unsafe { (*file).pMethods };
    if !methods.is_null() {
        let (platform, faulting) = METHODS.get_or_init(|| {
            let mut copy = unsafe { *methods };
            copy.xRead = Some(read);
            copy.xWrite = Some(write);
            copy.xSync = Some(sync);
            copy.xTruncate = Some(truncate);
            (methods as usize, copy)
        });
        // One platform table is all `win32` (and `unix`) hand out; a second would be unwrapped.
        assert_eq!(
            *platform, methods as usize,
            "a second platform method table"
        );
        unsafe { (*file).pMethods = faulting };
    }
    rc
}

/// Makes the faulting VFS this process's default. Call before the first connection opens.
pub fn install() {
    // SAFETY: SQLite owns the default VFS for the process; the copy is leaked, so it outlives
    // every connection that uses it.
    unsafe {
        let inner = ffi::sqlite3_vfs_find(std::ptr::null());
        assert!(!inner.is_null(), "no default VFS");
        INNER.set(inner as usize).expect("installed twice");
        let mut vfs = *inner;
        vfs.zName = c"loomward-fault".as_ptr();
        vfs.pNext = std::ptr::null_mut();
        vfs.xOpen = Some(open);
        let vfs = Box::leak(Box::new(vfs));
        assert_eq!(ffi::sqlite3_vfs_register(vfs, 1), ffi::SQLITE_OK);
    }
}
