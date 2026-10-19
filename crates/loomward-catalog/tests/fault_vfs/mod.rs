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
//!
//! Power cut (docs/45 row 8), `LOOMWARD_POWER_CUT=<mode>:<N>[:<seed>]`: every write and truncate
//! of a store file (database, WAL, rollback journal; not temporary files, not `-shm`) is kept in
//! memory per file name until a successful xSync of that file makes it durable. At the entry of
//! sync N+1 of the process (or at `cut_now`) the power fails: `cut_now` writes each file's image
//! as the disk would hold it to the armed directory and ends the process (`exit(CRASH_EXIT)`).
//! - `drop`: every write since the file's last sync is lost.
//! - `tear`: a prefix of them survives, split at 512-byte sector boundaries, its length drawn per
//!   file from `seed` (deterministic).
//! - `lie` (the checker's mutant): a disk whose flush does nothing, so nothing written in this
//!   process is durable.
//!
//! Model limits: a delete (xDelete) is durable at once, creating a file is durable (its content is
//! not), a rename is not seen, and `-shm` is volatile (the parent removes it: SQLite rebuilds it).
#![allow(unsafe_code)]
use rusqlite::ffi;
use std::{
    collections::{BTreeMap, HashMap},
    ffi::{c_char, c_int, c_void, CStr},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering::SeqCst},
        Mutex, OnceLock,
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
    if cut_plan().is_some() {
        // SAFETY: SQLite passed a buffer of `amt` bytes.
        let bytes = unsafe { std::slice::from_raw_parts(buf as *const u8, amt as usize) };
        record(f, Op::Write(off as u64, bytes.to_vec()));
    }
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
    if let Some(plan) = cut_plan() {
        if SYNCS.fetch_add(1, SeqCst) + 1 > plan.after {
            cut_now();
        }
    }
    if inject(Class::Sync).is_some() {
        return ffi::SQLITE_IOERR_FSYNC;
    }
    let rc = unsafe { orig().xSync.unwrap()(f, flags) };
    if rc == ffi::SQLITE_OK && cut_plan().is_some_and(|p| p.mode != CutMode::Lie) {
        durable(f);
    }
    rc
}

unsafe extern "C" fn truncate(f: *mut ffi::sqlite3_file, size: i64) -> c_int {
    if cut_plan().is_some() {
        record(f, Op::Truncate(size as u64));
    }
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
        // SQLite reuses `sqlite3_file` memory: a stale entry must not claim this file's writes.
        if cut_plan().is_some() {
            handles().remove(&(file as usize));
        }
        let tracked = ffi::SQLITE_OPEN_MAIN_DB
            | ffi::SQLITE_OPEN_MAIN_JOURNAL
            | ffi::SQLITE_OPEN_WAL
            | ffi::SQLITE_OPEN_SUPER_JOURNAL;
        if rc == ffi::SQLITE_OK
            && cut_plan().is_some()
            && !name.is_null()
            && flags & tracked != 0
            && flags & ffi::SQLITE_OPEN_DELETEONCLOSE == 0
        {
            // SAFETY: SQLite passes a NUL-terminated path.
            let path = unsafe { CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned();
            track(file as usize, Path::new(&path));
        }
    }
    rc
}

unsafe extern "C" fn delete(
    _vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    dir_sync: c_int,
) -> c_int {
    let inner = *INNER.get().unwrap() as *mut ffi::sqlite3_vfs;
    let rc = unsafe { (*inner).xDelete.unwrap()(inner, name, dir_sync) };
    if cut_plan().is_some() && !name.is_null() {
        // SAFETY: SQLite passes a NUL-terminated path.
        let path = unsafe { CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned();
        if let Some(key) = file_key(Path::new(&path)) {
            files().remove(&key);
        }
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
        vfs.xDelete = Some(delete);
        let vfs = Box::leak(Box::new(vfs));
        assert_eq!(ffi::sqlite3_vfs_register(vfs, 1), ffi::SQLITE_OK);
    }
}

// ---- power cut (docs/45 row 8) --------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CutMode {
    Drop,
    Tear,
    Lie,
}
#[derive(Clone, Copy, Debug)]
pub struct Cut {
    pub mode: CutMode,
    /// The power fails at the entry of sync `after + 1`.
    pub after: u64,
    pub seed: u64,
}
/// Parses `<mode>:<N>[:<seed>]`.
pub fn parse_cut(spec: &str) -> Option<Cut> {
    let mut parts = spec.split(':');
    let mode = match parts.next()? {
        "drop" => CutMode::Drop,
        "tear" => CutMode::Tear,
        "lie" => CutMode::Lie,
        _ => return None,
    };
    let after = parts.next()?.parse().ok()?;
    let seed = parts.next().map_or(Some(0), |s| s.parse().ok())?;
    parts.next().is_none().then_some(Cut { mode, after, seed })
}
fn cut_plan() -> Option<Cut> {
    static PLAN: OnceLock<Option<Cut>> = OnceLock::new();
    *PLAN.get_or_init(|| {
        let spec = std::env::var("LOOMWARD_POWER_CUT").ok()?;
        Some(parse_cut(&spec).unwrap_or_else(|| panic!("LOOMWARD_POWER_CUT={spec}")))
    })
}

enum Op {
    Write(u64, Vec<u8>),
    Truncate(u64),
}
impl Op {
    fn apply(&self, image: &mut Vec<u8>) {
        match self {
            Op::Write(off, bytes) => {
                let (start, end) = (*off as usize, *off as usize + bytes.len());
                if image.len() < end {
                    image.resize(end, 0);
                }
                image[start..end].copy_from_slice(bytes);
            }
            Op::Truncate(size) => image.resize(*size as usize, 0),
        }
    }
    /// The pieces a torn write can stop between: the 512-byte sectors of the file it touches.
    fn sectors(&self) -> Vec<Op> {
        match self {
            Op::Truncate(size) => vec![Op::Truncate(*size)],
            Op::Write(off, bytes) => {
                let mut out = Vec::new();
                let (mut at, mut rest) = (*off, bytes.as_slice());
                while !rest.is_empty() {
                    let take = ((512 - at % 512) as usize).min(rest.len());
                    out.push(Op::Write(at, rest[..take].to_vec()));
                    at += take as u64;
                    rest = &rest[take..];
                }
                out
            }
        }
    }
}
/// A store file as the disk holds it (`durable`) and the writes still only in the OS cache.
struct Track {
    durable: Vec<u8>,
    pending: Vec<Op>,
}
static SYNCS: AtomicU64 = AtomicU64::new(0);
static OUT: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();
fn files() -> std::sync::MutexGuard<'static, BTreeMap<String, Track>> {
    static FILES: Mutex<BTreeMap<String, Track>> = Mutex::new(BTreeMap::new());
    FILES.lock().unwrap_or_else(|e| e.into_inner())
}
fn handles() -> std::sync::MutexGuard<'static, HashMap<usize, String>> {
    static HANDLES: OnceLock<Mutex<HashMap<usize, String>>> = OnceLock::new();
    HANDLES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}
/// Files are keyed by name (one store directory per child): every handle on a file shares the
/// OS cache, so a sync through any handle flushes them all.
fn file_key(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    (!name.ends_with("-shm")).then_some(name)
}
fn track(handle: usize, path: &Path) {
    let Some(key) = file_key(path) else { return };
    handles().insert(handle, key.clone());
    // First sight of the file in this process: what is on disk now survived earlier processes.
    files().entry(key).or_insert_with(|| Track {
        durable: std::fs::read(path).unwrap_or_default(),
        pending: Vec::new(),
    });
}
fn record(f: *mut ffi::sqlite3_file, op: Op) {
    let Some(key) = handles().get(&(f as usize)).cloned() else {
        return;
    };
    if let Some(t) = files().get_mut(&key) {
        t.pending.push(op);
    }
}
fn durable(f: *mut ffi::sqlite3_file) {
    let Some(key) = handles().get(&(f as usize)).cloned() else {
        return;
    };
    if let Some(t) = files().get_mut(&key) {
        for op in std::mem::take(&mut t.pending) {
            op.apply(&mut t.durable);
        }
    }
}
/// Arms the power cut: `images` gets each store file as the disk holds it after the cut,
/// `report` the syncs reached, the files with unsynced writes and the files torn mid-way.
pub fn arm_power_cut(images: PathBuf, report: PathBuf) {
    OUT.set((images, report)).expect("armed twice");
}
fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}
/// The power fails now: write what the disk holds and end the process.
pub fn cut_now() -> ! {
    let plan = cut_plan().expect("no power cut armed");
    let (images, report) = OUT.get().expect("arm_power_cut first");
    std::fs::create_dir_all(images).unwrap();
    let files = files();
    let mut torn = 0;
    for (name, t) in files.iter() {
        let mut image = t.durable.clone();
        if plan.mode == CutMode::Tear {
            let pieces: Vec<Op> = t.pending.iter().flat_map(Op::sectors).collect();
            let mut rng = plan.seed ^ plan.after.wrapping_mul(0x100_0000_01b3);
            for b in name.bytes() {
                rng = (rng ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
            }
            let keep = (splitmix(&mut rng) % (pieces.len() as u64 + 1)) as usize;
            torn += usize::from(keep > 0 && keep < pieces.len());
            for p in &pieces[..keep] {
                p.apply(&mut image);
            }
        }
        std::fs::write(images.join(name), image).unwrap();
    }
    let unsynced = files.values().filter(|t| !t.pending.is_empty()).count();
    let syncs = SYNCS.load(SeqCst);
    std::fs::write(report, format!("{syncs} {unsynced} {torn}\n")).unwrap();
    std::process::exit(loomward_catalog::CRASH_EXIT);
}
