use loomward_lab::{
    bucket_path, decode_records, disagreements, expected, file_spec, invalid, real_root, root_name,
    CrossTotals, Manifest, Profile, Totals, MARKER, MAX_DEPTH, QUEUE_LIMIT,
};
use serde::Serialize;
use std::{
    collections::VecDeque,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    mem::{size_of, zeroed},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Condvar, Mutex,
    },
    thread,
    time::Instant,
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_FILES, FILETIME, HANDLE,
        INVALID_HANDLE_VALUE,
    },
    Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY},
    Storage::FileSystem::*,
    System::{
        Ioctl::FSCTL_SET_SPARSE,
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        Threading::{GetCurrentProcess, GetProcessTimes, OpenProcessToken},
        IO::DeviceIoControl,
    },
};

const MIN_FREE: u64 = 20_000_000_000;
const REJECT_ATTRS: u32 = FILE_ATTRIBUTE_REPARSE_POINT
    | FILE_ATTRIBUTE_OFFLINE
    | FILE_ATTRIBUTE_RECALL_ON_OPEN
    | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS;

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn plain_metadata(path: &Path) -> io::Result<fs::Metadata> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_attributes() & REJECT_ATTRS != 0 {
        return Err(invalid(format!(
            "reparse/offline component refused: {}",
            path.display()
        )));
    }
    Ok(meta)
}

/// Hold directories without write/delete sharing so checked ancestors cannot be replaced.
fn pin_directory(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let mut info: FILE_ATTRIBUTE_TAG_INFO = unsafe { zeroed() };
    let success = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileAttributeTagInfo,
            (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    };
    if success == 0 {
        return Err(io::Error::last_os_error());
    }
    if info.FileAttributes & REJECT_ATTRS != 0
        || info.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
    {
        return Err(invalid(
            "directory handle is reparse/offline or not a directory",
        ));
    }
    Ok(file)
}

struct Scope {
    root: PathBuf,
    // Pins and the marker handle live for the entire operation.
    pins: Vec<File>,
    marker: Option<File>,
}
impl Scope {
    fn open(input: &str, create: bool) -> io::Result<Self> {
        let name = root_name(input)?;
        let drive = &input[..1];
        let base = PathBuf::from(format!(r"\\?\{drive}:\loomward-lab\scale"));
        let root = base.join(name);
        // Check the drive ancestor without enumerating anything outside the lab.
        plain_metadata(Path::new(&format!(r"{drive}:\")))?;
        let mut pins = Vec::new();
        for path in [base.parent().unwrap(), base.as_path()] {
            if create && !path.try_exists()? {
                fs::create_dir(path)?;
            }
            pins.push(pin_directory(path)?);
        }
        let marker = if create {
            if free_bytes(&base)? < MIN_FREE {
                return Err(invalid("less than 20 GB free; generation refused"));
            }
            fs::create_dir(&root)?; // Fails even for an existing empty or marked tree.
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(root.join(".loomward-lab-marker"))?;
            file.write_all(MARKER.as_bytes())?;
            file.sync_all()?;
            None
        } else {
            plain_metadata(&root)?;
            let path = root.join(".loomward-lab-marker");
            plain_metadata(&path)?;
            let mut file = OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .open(&path)?;
            use std::io::Read;
            let mut contents = String::new();
            (&mut file).take(1024).read_to_string(&mut contents)?;
            if contents != MARKER {
                return Err(invalid("missing or incorrect lab marker"));
            }
            Some(file)
        };
        pins.push(pin_directory(&root)?);
        Ok(Self { root, pins, marker })
    }
    fn manifest(&self) -> io::Result<Manifest> {
        let path = self.root.join("manifest.json");
        plain_metadata(&path)?;
        let manifest: Manifest = serde_json::from_reader(File::open(path)?)?;
        if manifest.schema_version != 1 || !manifest.complete {
            return Err(invalid("incomplete or unsupported manifest"));
        }
        if manifest.expected != expected(manifest.expected.files, manifest.seed, manifest.profile)?
        {
            return Err(invalid("manifest disagrees with deterministic plan"));
        }
        Ok(manifest)
    }
}

struct Busy(PathBuf);
impl Busy {
    fn take(scope: &Scope) -> io::Result<Self> {
        let path = scope.root.join(".busy");
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Busy {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn free_bytes(path: &Path) -> io::Result<u64> {
    let mut bytes = 0;
    if unsafe { GetDiskFreeSpaceExW(wide(path).as_ptr(), &mut bytes, null_mut(), null_mut()) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(bytes)
}

fn make_sparse(path: &Path, size: u64) -> io::Result<()> {
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut returned = 0;
    if unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_SET_SPARSE,
            null(),
            0,
            null_mut(),
            0,
            &mut returned,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // std uses SetFileInformationByHandle(FileEndOfFileInfo) on Windows.
    file.set_len(size)
}

fn save_manifest(root: &Path, manifest: &Manifest) -> io::Result<()> {
    let mut file = File::create(root.join("manifest.json"))?;
    serde_json::to_writer_pretty(&mut file, manifest)?;
    file.write_all(b"\n")?;
    file.sync_all()
}

fn generate(
    input: &str,
    files: u64,
    seed: u64,
    profile: Profile,
    threads: usize,
) -> io::Result<()> {
    let totals = expected(files, seed, profile)?;
    let scope = Scope::open(input, true)?;
    let _busy = Busy::take(&scope)?;
    let before = free_bytes(&scope.root)?;
    let start = Instant::now();
    let mut manifest = Manifest {
        schema_version: 1,
        complete: false,
        seed,
        profile,
        expected: totals,
        file_data_allocation_bytes: 0,
        generation_seconds: 0.0,
        volume_free_before: before,
        volume_free_after: before,
    };
    save_manifest(&scope.root, &manifest)?;
    let data = scope.root.join("data");
    fs::create_dir(&data)?;
    let next = AtomicU64::new(0);
    let buckets = files.div_ceil(profile.bucket_size());
    thread::scope(|s| -> io::Result<()> {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| -> io::Result<()> {
                    loop {
                        let bucket = next.fetch_add(1, Ordering::Relaxed);
                        if bucket >= buckets {
                            return Ok(());
                        }
                        if bucket % 128 == 0 && free_bytes(&scope.root)? < MIN_FREE {
                            return Err(invalid(
                                "20 GB free-space reserve reached; partial marked tree retained",
                            ));
                        }
                        let directory = data.join(bucket_path(bucket, seed, profile));
                        fs::create_dir_all(&directory)?;
                        let begin = bucket * profile.bucket_size();
                        for index in begin..(begin + profile.bucket_size()).min(files) {
                            let (name, size) = file_spec(index, seed, profile);
                            make_sparse(&directory.join(name), size)?;
                        }
                    }
                })
            })
            .collect();
        let mut error = None;
        for h in handles {
            if let Err(e) = h
                .join()
                .map_err(|_| io::Error::other("generator worker panicked"))?
            {
                error.get_or_insert(e);
            }
        }
        error.map_or(Ok(()), Err)
    })?;
    let actual = walk(&data, Strategy::Handle, threads, 64)?;
    if actual.totals != totals || actual.allocation_bytes > MIN_FREE {
        return Err(invalid(format!(
            "generated tree failed oracle/allocation check: {actual:?}"
        )));
    }
    manifest.file_data_allocation_bytes = actual.allocation_bytes;
    manifest.complete = true;
    manifest.generation_seconds = start.elapsed().as_secs_f64();
    manifest.volume_free_after = free_bytes(&scope.root)?;
    if before.saturating_sub(manifest.volume_free_after) > MIN_FREE {
        return Err(invalid(
            "volume free-space delta exceeded 20 GB; partial marked tree retained",
        ));
    }
    save_manifest(&scope.root, &manifest)?;
    println!("{}", serde_json::to_string(&manifest)?);
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum Strategy {
    Std,
    Find,
    Handle,
}

#[derive(Debug, Default)]
struct Observed {
    totals: Totals,
    denied_directories: u64,
    allocation_bytes: u64,
    ids: u64,
}
impl Observed {
    fn add(&mut self, other: Self) {
        self.totals.add(other.totals);
        self.denied_directories += other.denied_directories;
        self.allocation_bytes += other.allocation_bytes;
        self.ids += other.ids;
    }
}

struct DirectoryPin {
    file: File,
    _parent: Option<Arc<DirectoryPin>>,
}

struct Job {
    path: PathBuf,
    depth: usize,
    parent: Option<Arc<DirectoryPin>>,
}

struct Queue {
    jobs: VecDeque<Job>,
    pending: usize,
    error: Option<String>,
}
struct Pool {
    state: Mutex<Queue>,
    wake: Condvar,
    real: bool,
}

fn walk(
    root: &Path,
    strategy: Strategy,
    threads: usize,
    buffer_kib: usize,
) -> io::Result<Observed> {
    walk_mode(root, strategy, threads, buffer_kib, false)
}

fn walk_mode(
    root: &Path,
    strategy: Strategy,
    threads: usize,
    buffer_kib: usize,
    real: bool,
) -> io::Result<Observed> {
    let pool = Pool {
        state: Mutex::new(Queue {
            jobs: VecDeque::from([Job {
                path: root.to_path_buf(),
                depth: 0,
                parent: None,
            }]),
            pending: 1,
            error: None,
        }),
        wake: Condvar::new(),
        real,
    };
    thread::scope(|s| -> io::Result<Observed> {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| -> Observed {
                    let mut total = Observed::default();
                    loop {
                        let job = {
                            let mut state = pool.state.lock().unwrap();
                            while state.jobs.is_empty()
                                && state.pending != 0
                                && state.error.is_none()
                            {
                                state = pool.wake.wait(state).unwrap();
                            }
                            if state.pending == 0 || state.error.is_some() {
                                break;
                            }
                            state.jobs.pop_front().unwrap()
                        };
                        let result = visit(job, strategy, buffer_kib, &pool, &mut total);
                        let mut state = pool.state.lock().unwrap();
                        state.pending -= 1;
                        if let Err(e) = result {
                            state.error = Some(e.to_string());
                        }
                        pool.wake.notify_all();
                    }
                    total
                })
            })
            .collect();
        let mut total = Observed::default();
        for h in handles {
            total.add(
                h.join()
                    .map_err(|_| io::Error::other("scan worker panicked"))?,
            );
        }
        if let Some(error) = &pool.state.lock().unwrap().error {
            return Err(io::Error::other(error.clone()));
        }
        Ok(total)
    })
}

fn visit(
    job: Job,
    strategy: Strategy,
    buffer_kib: usize,
    pool: &Pool,
    totals: &mut Observed,
) -> io::Result<()> {
    let result = visit_directory(job, strategy, buffer_kib, pool, totals);
    match result {
        Err(e) if pool.real && e.kind() == io::ErrorKind::PermissionDenied => {
            totals.denied_directories += 1;
            Ok(())
        }
        result => result,
    }
}

fn visit_directory(
    job: Job,
    strategy: Strategy,
    buffer_kib: usize,
    pool: &Pool,
    totals: &mut Observed,
) -> io::Result<()> {
    let Job {
        path,
        depth,
        parent,
    } = job;
    if depth > MAX_DEPTH {
        return Err(invalid("directory depth exceeds 128"));
    }
    let dir = Arc::new(DirectoryPin {
        file: pin_directory(&path)?,
        _parent: parent,
    });
    let mut entry = |name: OsString,
                     attributes: u32,
                     size: u64,
                     allocation: u64,
                     has_id: bool|
     -> io::Result<()> {
        if name == "." || name == ".." {
            return Ok(());
        }
        if attributes & REJECT_ATTRS != 0 {
            totals.totals.skipped_reparse += 1;
            return Ok(());
        }
        if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
            totals.totals.files += 1;
            totals.totals.logical_bytes += size;
            totals.allocation_bytes += allocation;
            totals.ids += u64::from(has_id);
            return Ok(());
        }
        totals.totals.directories += 1;
        let child = path.join(name);
        let queued = {
            let mut state = pool.state.lock().unwrap();
            if state.error.is_some() {
                return Err(io::Error::other("another worker failed"));
            }
            if state.jobs.len() < QUEUE_LIMIT {
                state.jobs.push_back(Job {
                    path: child.clone(),
                    depth: depth + 1,
                    parent: Some(Arc::clone(&dir)),
                });
                state.pending += 1;
                pool.wake.notify_one();
                true
            } else {
                false
            }
        };
        // Bounded shared queue: overflow is consumed inline, not accumulated in a local list.
        if !queued {
            visit(
                Job {
                    path: child,
                    depth: depth + 1,
                    parent: Some(Arc::clone(&dir)),
                },
                strategy,
                buffer_kib,
                pool,
                totals,
            )?;
        }
        Ok(())
    };
    match strategy {
        Strategy::Std => {
            for row in fs::read_dir(&path)? {
                let row = row?;
                // A denied per-entry stat is not a denied directory enumeration.
                let metadata = fs::symlink_metadata(row.path())
                    .map_err(|e| io::Error::other(format!("entry metadata failed: {e}")))?;
                entry(
                    row.file_name(),
                    metadata.file_attributes(),
                    metadata.len(),
                    0,
                    false,
                )?;
            }
        }
        Strategy::Find => find_entries(&path, &mut entry)?,
        Strategy::Handle => handle_entries(&dir.file, buffer_kib, &mut entry)?,
    }
    Ok(())
}

type Emit<'a> = dyn FnMut(OsString, u32, u64, u64, bool) -> io::Result<()> + 'a;

struct FindHandle(HANDLE);
impl Drop for FindHandle {
    fn drop(&mut self) {
        unsafe {
            FindClose(self.0);
        }
    }
}

fn find_entries(path: &Path, emit: &mut Emit<'_>) -> io::Result<()> {
    let mut data: WIN32_FIND_DATAW = unsafe { zeroed() };
    let raw = unsafe {
        FindFirstFileExW(
            wide(&path.join("*")).as_ptr(),
            FindExInfoBasic,
            (&mut data as *mut WIN32_FIND_DATAW).cast(),
            FindExSearchNameMatch,
            null(),
            FIND_FIRST_EX_LARGE_FETCH,
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) {
            return Ok(());
        }
        return Err(error);
    }
    let handle = FindHandle(raw);
    loop {
        let end = data
            .cFileName
            .iter()
            .position(|&c| c == 0)
            .ok_or_else(|| invalid("unterminated find name"))?;
        emit(
            OsString::from_wide(&data.cFileName[..end]),
            data.dwFileAttributes,
            ((data.nFileSizeHigh as u64) << 32) | data.nFileSizeLow as u64,
            0,
            false,
        )?;
        if unsafe { FindNextFileW(handle.0, &mut data) } == 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                Ok(())
            } else {
                Err(error)
            };
        }
    }
}

fn handle_entries(dir: &File, buffer_kib: usize, emit: &mut Emit<'_>) -> io::Result<()> {
    // u64 storage guarantees the documented eight-byte buffer alignment.
    let mut buffer = vec![0u64; buffer_kib * 1024 / 8];
    let mut class = FileIdExtdDirectoryRestartInfo;
    loop {
        if unsafe {
            GetFileInformationByHandleEx(
                dir.as_raw_handle(),
                class,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 8) as u32,
            )
        } == 0
        {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                Ok(())
            } else {
                Err(error)
            };
        }
        let bytes =
            unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), buffer.len() * 8) };
        decode_records(bytes, |row| {
            emit(
                OsString::from_wide(&row.name),
                row.attributes,
                row.logical_bytes,
                row.allocation_bytes,
                row.file_id != [0; 16],
            )
        })?;
        class = FileIdExtdDirectoryInfo;
    }
}

fn cpu_seconds() -> io::Result<f64> {
    let mut creation: FILETIME = unsafe { zeroed() };
    let mut exit = creation;
    let mut kernel = creation;
    let mut user = creation;
    if unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let ticks = |time: FILETIME| ((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64;
    Ok((ticks(kernel) + ticks(user)) as f64 / 10_000_000.0)
}

fn require_non_elevated() -> io::Result<()> {
    let mut token: HANDLE = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut elevation: TOKEN_ELEVATION = unsafe { zeroed() };
    let mut returned = 0;
    let success = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    };
    let error = io::Error::last_os_error();
    unsafe {
        CloseHandle(token);
    }
    if success == 0 {
        return Err(error);
    }
    if elevation.TokenIsElevated != 0 {
        return Err(invalid("non-elevated token required"));
    }
    Ok(())
}

#[derive(serde::Deserialize, Serialize)]
struct CrossRun {
    strategy: String,
    threads: usize,
    buffer_kib: usize,
    cache_label: String,
    observed: Option<CrossTotals>,
    wall_seconds: f64,
    files_per_second: Option<f64>,
    cpu_seconds: f64,
    peak_rss_bytes: usize,
    error: Option<String>,
}

fn cross_check(
    input: &str,
    name: Option<&str>,
    threads: usize,
    buffer_kib: usize,
    cache_label: &str,
    root_log: &str,
) -> io::Result<()> {
    real_root(input)?;
    if let Some(name) = name {
        let strategy = match name {
            "std-single" | "std-parallel" => Strategy::Std,
            "find" => Strategy::Find,
            "handle" => Strategy::Handle,
            _ => return Err(invalid("unknown cross-check strategy")),
        };
        let root = PathBuf::from(format!(r"\\?\{input}"));
        let log_path = Path::new(root_log);
        if !log_path.is_absolute() || log_path.starts_with(Path::new(input)) {
            return Err(invalid(
                "root-log must be absolute and outside the scan root",
            ));
        }
        let mut log = OpenOptions::new()
            .append(true)
            .create(true)
            .open(log_path)?;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_secs();
        writeln!(
            log,
            "{timestamp}\t{input}\towner-authorised metadata stress; {name}; {cache_label}"
        )?;
        log.sync_all()?;
        // Validate and retain every root component before traversal; no resolve() hiding links.
        let mut ancestors = Vec::new();
        for ancestor in root.ancestors().collect::<Vec<_>>().into_iter().rev() {
            plain_metadata(ancestor)?;
            ancestors.push(pin_directory(ancestor)?);
        }
        let threads = if name == "std-single" { 1 } else { threads };
        let cpu_start = cpu_seconds()?;
        let start = Instant::now();
        let actual = walk_mode(&root, strategy, threads, buffer_kib, true);
        let wall = start.elapsed().as_secs_f64();
        let cpu = cpu_seconds()? - cpu_start;
        let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { zeroed() };
        counters.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let (observed, error) = match actual {
            Ok(actual) => (
                Some(CrossTotals {
                    totals: actual.totals,
                    denied_directories: actual.denied_directories,
                }),
                None,
            ),
            Err(e) => (
                None,
                Some(format!("{:?}; os_code={:?}", e.kind(), e.raw_os_error())),
            ),
        };
        let result = CrossRun {
            strategy: name.into(),
            threads,
            buffer_kib,
            cache_label: cache_label.into(),
            observed,
            wall_seconds: wall,
            files_per_second: observed.map(|r| r.totals.files as f64 / wall),
            cpu_seconds: cpu,
            peak_rss_bytes: counters.PeakWorkingSetSize,
            error,
        };
        println!("{}", serde_json::to_string(&result)?);
        return Ok(());
    }
    let mut runs: Vec<CrossRun> = Vec::new();
    let mut differences = Vec::new();
    for strategy in ["std-single", "std-parallel", "find", "handle"] {
        let output = std::process::Command::new(std::env::current_exe()?)
            .args([
                "cross-check",
                "--root",
                input,
                "--root-log",
                root_log,
                "--strategy",
                strategy,
                "--threads",
                &threads.to_string(),
                "--buffer-kib",
                &buffer_kib.to_string(),
                "--cache-label",
                cache_label,
            ])
            .output()?;
        if !output.status.success() {
            differences.push(format!(
                "{strategy} failed before reporting aggregates; exit={}",
                output.status
            ));
            continue;
        }
        let run: CrossRun = serde_json::from_slice(&output.stdout)?;
        if let Some(error) = &run.error {
            differences.push(format!("{strategy} traversal failed: {error}"));
        }
        if let (Some(baseline), Some(actual)) =
            (runs.first().and_then(|r| r.observed), run.observed)
        {
            differences.extend(disagreements(&runs[0].strategy, baseline, strategy, actual));
        }
        runs.push(run);
    }
    let exact = differences.is_empty() && runs.len() == 4;
    println!(
        "{}",
        serde_json::to_string(
            &serde_json::json!({ "runs": runs, "agreement": exact, "disagreements": differences })
        )?
    );
    if !exact {
        return Err(invalid("strategy disagreement; see aggregate report"));
    }
    Ok(())
}

#[derive(Serialize)]
struct Measurement {
    strategy: String,
    root: String,
    threads: usize,
    buffer_kib: usize,
    cache_label: String,
    wall_seconds: f64,
    files_per_second: f64,
    cpu_seconds: f64,
    cpu_percent_one_core: f64,
    peak_rss_bytes: usize,
    expected: Totals,
    observed: Totals,
    exact: bool,
    allocation_bytes: Option<u64>,
    nonzero_file_ids: Option<u64>,
}

fn bench(
    input: &str,
    name: &str,
    threads: usize,
    buffer_kib: usize,
    cache_label: String,
) -> io::Result<()> {
    let strategy = match name {
        "std-single" | "std-parallel" => Strategy::Std,
        "find" => Strategy::Find,
        "handle" => Strategy::Handle,
        _ => {
            return Err(invalid(
                "strategy must be std-single, std-parallel, find or handle",
            ))
        }
    };
    let threads = if name == "std-single" { 1 } else { threads };
    let scope = Scope::open(input, false)?;
    let _busy = Busy::take(&scope)?;
    let manifest = scope.manifest()?;
    let cpu_start = cpu_seconds()?;
    let start = Instant::now();
    let actual = walk(&scope.root.join("data"), strategy, threads, buffer_kib)?;
    let wall = start.elapsed().as_secs_f64();
    let cpu = cpu_seconds()? - cpu_start;
    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { zeroed() };
    counters.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    if unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let measurement = Measurement {
        strategy: name.into(),
        root: input.into(),
        threads,
        buffer_kib,
        cache_label,
        wall_seconds: wall,
        files_per_second: actual.totals.files as f64 / wall,
        cpu_seconds: cpu,
        cpu_percent_one_core: cpu / wall * 100.0,
        peak_rss_bytes: counters.PeakWorkingSetSize,
        expected: manifest.expected,
        observed: actual.totals,
        exact: actual.totals == manifest.expected,
        allocation_bytes: matches!(strategy, Strategy::Handle).then_some(actual.allocation_bytes),
        nonzero_file_ids: matches!(strategy, Strategy::Handle).then_some(actual.ids),
    };
    println!("{}", serde_json::to_string(&measurement)?);
    if !measurement.exact {
        return Err(invalid("scanner totals disagree with manifest"));
    }
    Ok(())
}

fn destroy(input: &str) -> io::Result<()> {
    if !input.starts_with("G:") && !input.starts_with("g:") {
        return Err(invalid("destroy remains restricted to the G: lab"));
    }
    let mut scope = Scope::open(input, false)?;
    let busy = Busy::take(&scope)?;
    // Preflight the entire tree before deleting a single entry. Refuse all reparse points.
    let mut directories = vec![(scope.root.clone(), pin_directory(&scope.root)?)];
    let mut files = Vec::new();
    let mut index = 0;
    while index < directories.len() {
        for row in fs::read_dir(&directories[index].0)? {
            let path = row?.path();
            let metadata = plain_metadata(&path)?;
            if metadata.is_dir() {
                let pin = pin_directory(&path)?;
                directories.push((path, pin));
            } else {
                files.push(path);
            }
        }
        index += 1;
    }
    // Keep the marker until payload removal succeeds, leaving partial failures identifiable.
    let marker = scope.root.join(".loomward-lab-marker");
    for path in files.iter().filter(|p| **p != marker && **p != busy.0) {
        fs::remove_file(path)?;
    }
    while directories.len() > 1 {
        let (path, pin) = directories.pop().unwrap();
        drop(pin);
        fs::remove_dir(path)?;
    }
    drop(directories);
    drop(busy);
    scope.marker.take();
    fs::remove_file(marker)?;
    scope.pins.pop();
    fs::remove_dir(&scope.root)?;
    println!("destroyed marked synthetic tree {input}");
    Ok(())
}

pub fn run() -> io::Result<()> {
    require_non_elevated()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first() else {
        return Err(invalid("generate|bench|destroy --root G:\\loomward-lab\\scale\\NAME [--files N --seed S --profile dev|media|mixed] [--threads N] [--strategy std-single|std-parallel|find|handle --buffer-kib N --cache-label LABEL]"));
    };
    if args.len() % 2 != 1 {
        return Err(invalid("options require values"));
    }
    let mut options = std::collections::BTreeMap::new();
    let allowed: &[&str] = match command.as_str() {
        "generate" => &["--root", "--files", "--seed", "--profile", "--threads"],
        "bench" => &[
            "--root",
            "--strategy",
            "--threads",
            "--buffer-kib",
            "--cache-label",
        ],
        "cross-check" => &[
            "--root",
            "--root-log",
            "--strategy",
            "--threads",
            "--buffer-kib",
            "--cache-label",
        ],
        "destroy" => &["--root"],
        _ => return Err(invalid("unknown command")),
    };
    for pair in args[1..].chunks_exact(2) {
        if !allowed.contains(&pair[0].as_str())
            || options.insert(pair[0].as_str(), pair[1].as_str()).is_some()
        {
            return Err(invalid("unknown or duplicated option"));
        }
    }
    let required = |key| {
        options
            .get(key)
            .copied()
            .ok_or_else(|| invalid(format!("required option {key}")))
    };
    let root = required("--root")?;
    let number = |key, default| -> io::Result<u64> {
        options.get(key).map_or(Ok(default), |s| {
            s.parse()
                .map_err(|_| invalid(format!("invalid integer {key}")))
        })
    };
    let threads = number("--threads", 8)? as usize;
    let buffer = number("--buffer-kib", 64)? as usize;
    if !(1..=32).contains(&threads) || !(4..=1024).contains(&buffer) {
        return Err(invalid("threads must be 1..=32, buffer-kib 4..=1024"));
    }
    match command.as_str() {
        "generate" => generate(
            root,
            required("--files")?
                .parse()
                .map_err(|_| invalid("invalid files"))?,
            required("--seed")?
                .parse()
                .map_err(|_| invalid("invalid seed"))?,
            Profile::parse(required("--profile")?)?,
            threads,
        ),
        "bench" => bench(
            root,
            required("--strategy")?,
            threads,
            buffer,
            options
                .get("--cache-label")
                .unwrap_or(&"uncontrolled")
                .to_string(),
        ),
        "destroy" => destroy(root),
        "cross-check" => cross_check(
            root,
            options.get("--strategy").copied(),
            threads,
            buffer,
            options.get("--cache-label").unwrap_or(&"uncontrolled"),
            required("--root-log")?,
        ),
        _ => unreachable!(),
    }
}
