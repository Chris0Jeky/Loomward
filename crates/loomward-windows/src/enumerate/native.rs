use super::*;
use std::{
    fs::File,
    mem::{size_of, zeroed},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
    time::Duration,
};
use windows_sys::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{
            NtCreateFile, FILE_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
            FILE_SYNCHRONOUS_IO_NONALERT,
        },
    },
    Win32::{
        Foundation::*,
        Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY},
        Storage::FileSystem::*,
        System::{
            Threading::*,
            IO::{CancelSynchronousIo, IO_STATUS_BLOCK},
        },
    },
};

/// Cancellation owns thread handles, never closes handles used by an outstanding call.
#[derive(Clone, Default, Debug)]
pub struct IoCancellation {
    cancelled: Arc<AtomicBool>,
    threads: Arc<Mutex<Vec<Weak<OwnedHandle>>>>,
}
impl IoCancellation {
    /// Requests cancellation immediately; blocked synchronous calls are interrupted after 500 ms.
    pub fn cancel(&self) {
        if self.cancelled.swap(true, Ordering::AcqRel) {
            return;
        }
        let threads = self.threads.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            let handles: Vec<_> = threads
                .lock()
                .unwrap()
                .iter()
                .filter_map(Weak::upgrade)
                .collect();
            for handle in handles {
                unsafe {
                    CancelSynchronousIo(handle.as_raw_handle());
                }
            }
        });
    }
    /// Whether cancellation was requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
    fn register(&self) -> Result<Arc<OwnedHandle>, SourceError> {
        let mut handle = null_mut();
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                GetCurrentThread(),
                GetCurrentProcess(),
                &mut handle,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            return Err(last_error());
        }
        let handle = Arc::new(unsafe { OwnedHandle::from_raw_handle(handle) });
        let mut threads = self.threads.lock().unwrap();
        threads.retain(|h| h.strong_count() != 0);
        threads.push(Arc::downgrade(&handle));
        Ok(handle)
    }
}

/// Read-only native source. Capability probing is per opened directory.
#[derive(Debug, Clone, Default)]
pub struct NativeSource {
    cancel: IoCancellation,
    preferred: Option<Strategy>,
}
impl NativeSource {
    /// A source sharing this run's cancellation.
    pub fn new(cancel: IoCancellation) -> Self {
        Self {
            cancel,
            preferred: None,
        }
    }
    /// Select a starting class for fallback fixture tests and measurements.
    pub fn with_strategy(strategy: Strategy) -> Self {
        Self {
            cancel: IoCancellation::default(),
            preferred: Some(strategy),
        }
    }
}
/// Validated directory. The file owns the native handle until every call has returned.
#[derive(Debug)]
pub struct NativeDir {
    file: File,
    refs: bool,
    strategy: Mutex<Strategy>,
}
impl NativeDir {
    /// Actual class last used for this directory.
    pub fn strategy(&self) -> Strategy {
        *self.strategy.lock().unwrap()
    }
}
fn last_error() -> SourceError {
    match unsafe { GetLastError() } {
        5 => SourceError::AccessDenied,
        c => SourceError::Io(c as i32),
    }
}
fn checked_child(name: &[u16]) -> Result<(), SourceError> {
    if name.is_empty()
        || name == [46]
        || name == [46, 46]
        || name.len() > 32767
        || name.iter().any(|c| matches!(*c, 0 | 47 | 58 | 92))
    {
        Err(SourceError::Refused)
    } else {
        Ok(())
    }
}
fn information<T>(handle: HANDLE, class: FILE_INFO_BY_HANDLE_CLASS) -> Option<T> {
    let mut info: T = unsafe { zeroed() };
    (unsafe {
        GetFileInformationByHandleEx(
            handle,
            class,
            (&mut info as *mut T).cast(),
            size_of::<T>() as u32,
        )
    } != 0)
        .then_some(info)
}
fn opened(
    file: File,
    listed: Option<FileIdObs>,
    preferred: Strategy,
) -> Result<(NativeDir, OpenedIdentity), SourceError> {
    let h = file.as_raw_handle();
    let attr: FILE_ATTRIBUTE_TAG_INFO =
        information(h, FileAttributeTagInfo).ok_or_else(last_error)?;
    if refused_attributes(attr.FileAttributes)
        || attr.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
    {
        return Err(SourceError::Refused);
    }
    let mut fs_name = [0u16; 64];
    if unsafe {
        GetVolumeInformationByHandleW(
            h,
            null_mut(),
            0,
            null_mut(),
            null_mut(),
            null_mut(),
            fs_name.as_mut_ptr(),
            fs_name.len() as u32,
        )
    } == 0
    {
        return Err(last_error());
    }
    let refs = String::from_utf16_lossy(
        &fs_name[..fs_name
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(fs_name.len())],
    )
    .eq_ignore_ascii_case("ReFS");
    let mut basic: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
    let basic_ok = unsafe { GetFileInformationByHandle(h, &mut basic) } != 0;
    let ext: Option<FILE_ID_INFO> = information(h, FileIdInfo);
    // A 64-bit listing must be compared with the compatible 64-bit query, never truncation.
    let legacy = basic_ok
        .then_some(FileIdObs::Id64(
            (u64::from(basic.nFileIndexHigh) << 32) | u64::from(basic.nFileIndexLow),
        ))
        .and_then(FileIdObs::nonzero);
    let extended = ext
        .as_ref()
        .map(|v| FileIdObs::Id128(v.FileId.Identifier))
        .and_then(FileIdObs::nonzero);
    let id = match listed {
        Some(FileIdObs::Id64(_)) => legacy,
        Some(FileIdObs::Id128(_)) => extended,
        None => extended.or(legacy),
    };
    let (id, basis) = validate_identity(listed, id, refs)?;
    Ok((
        NativeDir {
            file,
            refs,
            strategy: Mutex::new(preferred),
        },
        OpenedIdentity {
            id,
            basis,
            volume_serial: ext
                .map(|i| i.VolumeSerialNumber)
                .or_else(|| basic_ok.then_some(u64::from(basic.dwVolumeSerialNumber))),
            attributes: attr.FileAttributes,
            reparse_tag: (attr.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0)
                .then_some(attr.ReparseTag),
        },
    ))
}
fn non_elevated() -> Result<(), SourceError> {
    let mut token = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(last_error());
    }
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut elevation: TOKEN_ELEVATION = unsafe { zeroed() };
    let mut size = 0;
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        )
    } == 0
    {
        return Err(last_error());
    }
    if elevation.TokenIsElevated != 0 {
        Err(SourceError::Refused)
    } else {
        Ok(())
    }
}
impl DirSource for NativeSource {
    type Dir = NativeDir;
    fn strategy_for(&self, dir: &NativeDir) -> Strategy {
        dir.strategy()
    }
    fn is_refs(&self, dir: &NativeDir) -> bool {
        dir.refs
    }
    fn strategy(&self) -> Strategy {
        self.preferred.unwrap_or(Strategy::Extended)
    }
    fn open_root(&self, path: &Path) -> Result<(NativeDir, OpenedIdentity), SourceError> {
        non_elevated()?;
        let _thread = self.cancel.register()?;
        if self.cancel.is_cancelled() {
            return Err(SourceError::Io(ERROR_OPERATION_ABORTED as i32));
        }
        let mut name: Vec<u16> = path.as_os_str().encode_wide().collect();
        if !path.is_absolute() || name.contains(&0) {
            return Err(SourceError::Refused);
        }
        name.push(0);
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS
                    | FILE_FLAG_OPEN_REPARSE_POINT
                    | FILE_FLAG_OPEN_NO_RECALL,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(last_error());
        }
        opened(
            unsafe { File::from_raw_handle(handle) },
            None,
            self.strategy(),
        )
    }
    fn open_child(
        &self,
        parent: &NativeDir,
        entry: &RawEntry<'_>,
    ) -> Result<(NativeDir, OpenedIdentity), SourceError> {
        checked_child(entry.name)?;
        if refused_attributes(entry.attributes) {
            return Err(SourceError::Refused);
        }
        let _thread = self.cancel.register()?;
        if self.cancel.is_cancelled() {
            return Err(SourceError::Io(ERROR_OPERATION_ABORTED as i32));
        }
        let mut name = entry.name.to_vec();
        let unicode = UNICODE_STRING {
            Length: (name.len() * 2) as u16,
            MaximumLength: (name.len() * 2) as u16,
            Buffer: name.as_mut_ptr(),
        };
        let attributes = OBJECT_ATTRIBUTES {
            Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
            RootDirectory: parent.file.as_raw_handle(),
            ObjectName: &unicode as *const _ as *mut _,
            Attributes: 0,
            SecurityDescriptor: null_mut(),
            SecurityQualityOfService: null_mut(),
        };
        let mut status: IO_STATUS_BLOCK = unsafe { zeroed() };
        let mut h = null_mut();
        let code = unsafe {
            NtCreateFile(
                &mut h,
                FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                &attributes,
                &mut status,
                null(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                FILE_OPEN,
                FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
                null(),
                0,
            )
        };
        if code < 0 {
            return Err(if code == STATUS_ACCESS_DENIED {
                SourceError::AccessDenied
            } else {
                SourceError::Io(code)
            });
        }
        opened(
            unsafe { File::from_raw_handle(h) },
            entry.file_id,
            self.strategy(),
        )
    }
    fn list(&self, dir: &NativeDir, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
        let _thread = match self.cancel.register() {
            Ok(h) => h,
            Err(_) => return ListOutcome::Incomplete(IncompleteReason::Io(-1)),
        };
        // Revalidate immediately before listing, including a directory that acquired recall flags.
        let attr: Option<FILE_ATTRIBUTE_TAG_INFO> =
            information(dir.file.as_raw_handle(), FileAttributeTagInfo);
        match attr {
            Some(a)
                if !refused_attributes(a.FileAttributes)
                    && a.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 => {}
            _ => return ListOutcome::Incomplete(IncompleteReason::AccessDenied),
        }
        let mut strategy = *dir.strategy.lock().unwrap();
        let mut buffer = vec![0u64; 2048]; // 16 KiB, aligned for native structures.
        let mut emitted = false;
        let mut first = true;
        loop {
            if self.cancel.is_cancelled() {
                return ListOutcome::Incomplete(IncompleteReason::Cancelled);
            }
            if strategy == Strategy::Find {
                return self.find(dir, sink);
            }
            let class = match (strategy, first) {
                (Strategy::Extended, true) => FileIdExtdDirectoryRestartInfo,
                (Strategy::Extended, false) => FileIdExtdDirectoryInfo,
                (Strategy::Both, true) => FileIdBothDirectoryRestartInfo,
                (Strategy::Both, false) => FileIdBothDirectoryInfo,
                _ => return ListOutcome::Incomplete(IncompleteReason::Malformed),
            };
            buffer.fill(0);
            let ok = unsafe {
                GetFileInformationByHandleEx(
                    dir.file.as_raw_handle(),
                    class,
                    buffer.as_mut_ptr().cast(),
                    (buffer.len() * 8) as u32,
                )
            };
            if ok == 0 {
                let code = unsafe { GetLastError() };
                if !emitted
                    && matches!(
                        code,
                        ERROR_INVALID_PARAMETER | ERROR_INVALID_FUNCTION | ERROR_NOT_SUPPORTED
                    )
                {
                    strategy = if strategy == Strategy::Extended {
                        Strategy::Both
                    } else {
                        Strategy::Find
                    };
                    *dir.strategy.lock().unwrap() = strategy;
                    first = true;
                    continue;
                }
                if code == ERROR_MORE_DATA || code == ERROR_INSUFFICIENT_BUFFER {
                    if buffer.len() * 8 < 1048576 {
                        buffer.resize(buffer.len() * 2, 0);
                        continue;
                    }
                    return ListOutcome::Incomplete(IncompleteReason::Malformed);
                }
                return query_stop(code, self.cancel.is_cancelled());
            }
            // This Win32 API has no byte-count output; validate within the caller-owned buffer.
            let bytes = unsafe {
                std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), buffer.len() * 8)
            };
            if bytes[..88].iter().all(|b| *b == 0) {
                if buffer.len() * 8 < 1048576 {
                    buffer.resize(buffer.len() * 2, 0);
                    continue;
                }
                return ListOutcome::Incomplete(IncompleteReason::Malformed);
            }
            let mut callback = |e: RawEntry<'_>| {
                emitted = true;
                if self.cancel.is_cancelled() {
                    Flow::Stop
                } else {
                    sink(e)
                }
            };
            if let Err(reason) = decode_buffer(bytes, strategy, &mut callback) {
                return ListOutcome::Incomplete(reason);
            }
            first = false;
        }
    }
}
struct FindHandle(HANDLE);
impl Drop for FindHandle {
    fn drop(&mut self) {
        unsafe {
            FindClose(self.0);
        }
    }
}
impl NativeSource {
    fn find(&self, dir: &NativeDir, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
        let mut path = vec![0u16; 32768];
        let n = unsafe {
            GetFinalPathNameByHandleW(
                dir.file.as_raw_handle(),
                path.as_mut_ptr(),
                path.len() as u32,
                0,
            )
        } as usize;
        if n == 0 || n >= path.len() {
            return ListOutcome::Incomplete(IncompleteReason::Io(unsafe { GetLastError() } as i32));
        }
        path.truncate(n);
        let directory = std::path::PathBuf::from(std::ffi::OsString::from_wide(&path));
        let mut pins = Vec::new();
        // Find is path-based: pin and validate every ancestor while its search handle lives.
        for ancestor in directory.ancestors().collect::<Vec<_>>().into_iter().rev() {
            let mut name: Vec<_> = ancestor.as_os_str().encode_wide().chain(Some(0)).collect();
            let handle = unsafe {
                CreateFileW(
                    name.as_mut_ptr(),
                    FILE_READ_ATTRIBUTES,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    null(),
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS
                        | FILE_FLAG_OPEN_REPARSE_POINT
                        | FILE_FLAG_OPEN_NO_RECALL,
                    null_mut(),
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                return ListOutcome::Incomplete(IncompleteReason::Io(
                    unsafe { GetLastError() } as i32
                ));
            }
            let pin = unsafe { File::from_raw_handle(handle) };
            let attr: Option<FILE_ATTRIBUTE_TAG_INFO> =
                information(pin.as_raw_handle(), FileAttributeTagInfo);
            if !attr.is_some_and(|a| {
                !refused_attributes(a.FileAttributes)
                    && a.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0
            }) {
                return ListOutcome::Incomplete(IncompleteReason::AccessDenied);
            }
            pins.push(pin);
        }
        let original: Option<FILE_ID_INFO> = information(dir.file.as_raw_handle(), FileIdInfo);
        let pinned: Option<FILE_ID_INFO> =
            information(pins.last().unwrap().as_raw_handle(), FileIdInfo);
        if original
            .as_ref()
            .map(|i| (i.VolumeSerialNumber, i.FileId.Identifier))
            != pinned
                .as_ref()
                .map(|i| (i.VolumeSerialNumber, i.FileId.Identifier))
        {
            return ListOutcome::Incomplete(IncompleteReason::Io(ERROR_FILE_INVALID as i32));
        }
        let mut current = vec![0u16; 32768];
        let length = unsafe {
            GetFinalPathNameByHandleW(
                dir.file.as_raw_handle(),
                current.as_mut_ptr(),
                current.len() as u32,
                0,
            )
        } as usize;
        if length == 0 || length >= current.len() || current[..length] != path {
            return ListOutcome::Incomplete(IncompleteReason::Io(ERROR_FILE_INVALID as i32));
        }
        path.extend([92, 42, 0]);
        let mut data: WIN32_FIND_DATAW = unsafe { zeroed() };
        let h = unsafe {
            FindFirstFileExW(
                path.as_ptr(),
                FindExInfoBasic,
                (&mut data as *mut WIN32_FIND_DATAW).cast(),
                FindExSearchNameMatch,
                null(),
                FIND_FIRST_EX_LARGE_FETCH,
            )
        };
        if h == INVALID_HANDLE_VALUE {
            let c = unsafe { GetLastError() };
            return if c == ERROR_FILE_NOT_FOUND {
                ListOutcome::Complete
            } else {
                query_stop(c, self.cancel.is_cancelled())
            };
        }
        let _find = FindHandle(h);
        loop {
            if self.cancel.is_cancelled() {
                return ListOutcome::Incomplete(IncompleteReason::Cancelled);
            }
            let n = data
                .cFileName
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(data.cFileName.len());
            if n == 0 || n == data.cFileName.len() {
                return ListOutcome::Incomplete(IncompleteReason::Malformed);
            }
            let name = &data.cFileName[..n];
            if name != [46] && name != [46, 46] {
                let time = |t: FILETIME| {
                    ((u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)) as i64
                };
                if sink(RawEntry {
                    name,
                    file_id: None,
                    attributes: data.dwFileAttributes,
                    reparse_tag: (data.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0)
                        .then_some(data.dwReserved0),
                    end_of_file: (u64::from(data.nFileSizeHigh) << 32)
                        | u64::from(data.nFileSizeLow),
                    allocation_size: None,
                    creation: Some(time(data.ftCreationTime)),
                    last_access: Some(time(data.ftLastAccessTime)),
                    last_write: Some(time(data.ftLastWriteTime)),
                    change: None,
                }) == Flow::Stop
                {
                    return ListOutcome::Incomplete(IncompleteReason::Cancelled);
                }
            }
            if unsafe { FindNextFileW(h, &mut data) } == 0 {
                return query_stop(unsafe { GetLastError() }, self.cancel.is_cancelled());
            }
        }
    }
}
#[cfg(test)]
mod cancellation_tests {
    use super::*;
    #[test]
    fn cancels_blocked_native_io_without_closing_its_handle() {
        use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW};
        let cancel = IoCancellation::default();
        let io = cancel.clone();
        let entered = Arc::new(AtomicBool::new(false));
        let ready = entered.clone();
        let worker = std::thread::spawn(move || {
            let name: Vec<u16> = format!(
                r"\\.\pipe\loomward-scan-cancel-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )
            .encode_utf16()
            .chain(Some(0))
            .collect();
            let h = unsafe {
                CreateNamedPipeW(name.as_ptr(), PIPE_ACCESS_INBOUND, 0, 1, 0, 0, 0, null())
            };
            assert_ne!(h, INVALID_HANDLE_VALUE);
            let handle = unsafe { File::from_raw_handle(h) };
            let _thread = io.register().unwrap();
            ready.store(true, Ordering::Release);
            let result = unsafe { ConnectNamedPipe(handle.as_raw_handle(), null_mut()) };
            let error = unsafe { GetLastError() };
            assert_eq!(result, 0);
            assert_eq!(error, ERROR_OPERATION_ABORTED);
            // The handle remained valid through return; only now may its owning File drop.
            let mut flags = 0;
            assert_ne!(
                unsafe { GetHandleInformation(handle.as_raw_handle(), &mut flags) },
                0
            );
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !entered.load(Ordering::Acquire) {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        let started = std::time::Instant::now();
        cancel.cancel();
        assert!(started.elapsed() < Duration::from_millis(250));
        worker.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
