use super::*;
use std::{
    fs::File,
    mem::{size_of, zeroed},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Component, Prefix},
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
            NtCreateFile, FILE_OPEN, FILE_OPEN_NO_RECALL, FILE_OPEN_REPARSE_POINT,
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
    #[cfg(any(test, feature = "fixtures"))]
    path_query_error: Option<i32>,
}
impl NativeSource {
    /// A source sharing this run's cancellation.
    pub fn new(cancel: IoCancellation) -> Self {
        Self {
            cancel,
            preferred: None,
            #[cfg(any(test, feature = "fixtures"))]
            path_query_error: None,
        }
    }
    /// Select a starting class for fallback fixture tests and measurements.
    pub fn with_strategy(strategy: Strategy) -> Self {
        Self {
            cancel: IoCancellation::default(),
            preferred: Some(strategy),
            #[cfg(any(test, feature = "fixtures"))]
            path_query_error: None,
        }
    }
    /// Inject a normalised handle-path query failure in disposable fixture scans only.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn with_path_query_failure(mut self, code: i32) -> Self {
        self.path_query_error = Some(code);
        self
    }
}
/// Validated directory. The file owns the native handle until every call has returned.
#[derive(Debug)]
pub struct NativeDir {
    file: Arc<File>,
    root: Arc<File>,
    refs: bool,
    reopen_safe: bool,
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
fn directory_create_options() -> u32 {
    // FILE_DIRECTORY_FILE is incompatible with NO_RECALL; validate the opened directory instead.
    FILE_OPEN_REPARSE_POINT | FILE_OPEN_NO_RECALL | FILE_SYNCHRONOUS_IO_NONALERT
}
fn open_relative(
    parent: &File,
    name: &[u16],
    access: u32,
    share: u32,
) -> Result<File, SourceError> {
    open_relative_mode(parent, name, access, share, false)
}
pub(crate) fn open_relative_mode(
    parent: &File,
    name: &[u16],
    access: u32,
    share: u32,
    asynchronous: bool,
) -> Result<File, SourceError> {
    open_relative_with_attributes(parent, name, access, share, asynchronous, 0)
}
fn open_relative_with_attributes(
    parent: &File,
    name: &[u16],
    access: u32,
    share: u32,
    asynchronous: bool,
    object_attributes: u32,
) -> Result<File, SourceError> {
    non_elevated()?;
    checked_child(name)?;
    let mut name = name.to_vec();
    let unicode = UNICODE_STRING {
        Length: (name.len() * 2) as u16,
        MaximumLength: (name.len() * 2) as u16,
        Buffer: name.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle(),
        ObjectName: &unicode as *const _ as *mut _,
        Attributes: object_attributes,
        SecurityDescriptor: null_mut(),
        SecurityQualityOfService: null_mut(),
    };
    let mut status: IO_STATUS_BLOCK = unsafe { zeroed() };
    let mut handle = null_mut();
    let _profile = profile::OPEN.start();
    let code = unsafe {
        NtCreateFile(
            &mut handle,
            access,
            &attributes,
            &mut status,
            null(),
            0,
            share,
            FILE_OPEN,
            if asynchronous {
                FILE_OPEN_REPARSE_POINT | FILE_OPEN_NO_RECALL
            } else {
                directory_create_options()
            },
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
    Ok(unsafe { File::from_raw_handle(handle) })
}
fn directory_attributes(file: &File) -> Result<FILE_ATTRIBUTE_TAG_INFO, SourceError> {
    let _profile = profile::ATTRIBUTES.start();
    let attr: FILE_ATTRIBUTE_TAG_INFO =
        information(file.as_raw_handle(), FileAttributeTagInfo).ok_or_else(last_error)?;
    validate_directory_attributes(attr)
}

fn readonly_error(error: SourceError) -> std::io::Error {
    match error {
        SourceError::Io(code) => std::io::Error::from_raw_os_error(if code < 0 {
            (unsafe { RtlNtStatusToDosError(code) }) as i32
        } else {
            code
        }),
        SourceError::AccessDenied => std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "non-elevated guarded access required",
        ),
        _ => std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "placeholder, offline, reparse or invalid path refused",
        ),
    }
}

/// Open a local directory by a non-elevated, no-recall, no-follow handle walk.
/// Ancestor handles bind subsequent opens; attribute-only handles do not prevent renames.
pub fn open_readonly_directory(path: &Path) -> std::io::Result<File> {
    let mut pins = root_handles(path, FILE_READ_ATTRIBUTES | SYNCHRONIZE, FILE_SHARE_READ)
        .map_err(readonly_error)?;
    Ok(pins.pop().unwrap())
}

/// Open user-supplied shortcut path text for metadata only; no reparse following or recall.
pub fn open_metadata_child(parent: &File, name: &[u16]) -> std::io::Result<File> {
    open_readonly_child(parent, name, false)
}

pub(crate) fn open_readonly_child(
    parent: &File,
    name: &[u16],
    contents: bool,
) -> std::io::Result<File> {
    directory_attributes(parent).map_err(readonly_error)?;
    let file = open_relative_with_attributes(
        parent,
        name,
        FILE_READ_ATTRIBUTES | SYNCHRONIZE | if contents { FILE_READ_DATA } else { 0 },
        FILE_SHARE_READ | FILE_SHARE_DELETE,
        false,
        OBJ_CASE_INSENSITIVE,
    )
    .map_err(readonly_error)?;
    if unsafe { GetFileType(file.as_raw_handle()) } != FILE_TYPE_DISK {
        return Err(readonly_error(SourceError::Refused));
    }
    let attr: FILE_ATTRIBUTE_TAG_INFO = information(file.as_raw_handle(), FileAttributeTagInfo)
        .ok_or_else(|| readonly_error(last_error()))?;
    if refused_attributes(attr.FileAttributes) {
        return Err(readonly_error(SourceError::Refused));
    }
    Ok(file)
}

/// Read at most `cap` bytes from one regular, no-recall, no-follow shortcut child.
#[cfg(feature = "shortcut-read")]
pub fn read_capped_regular_file(
    parent: &File,
    name: &[u16],
    cap: usize,
) -> std::io::Result<Vec<u8>> {
    read_capped_regular_file_with(parent, name, cap, || {})
}

#[cfg(feature = "shortcut-read")]
fn read_capped_regular_file_with(
    parent: &File,
    name: &[u16],
    cap: usize,
    after_open: impl FnOnce(),
) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let limit = cap
        .checked_add(1)
        .ok_or_else(|| readonly_error(SourceError::Refused))?;
    let file = open_readonly_child(parent, name, true)?;
    let attr: FILE_ATTRIBUTE_TAG_INFO = information(file.as_raw_handle(), FileAttributeTagInfo)
        .ok_or_else(|| readonly_error(last_error()))?;
    if attr.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        return Err(readonly_error(SourceError::Refused));
    }
    if file.metadata()?.len() > cap as u64 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "file size exceeds cap",
        ));
    }
    after_open();
    let mut bytes = Vec::new();
    file.take(limit as u64).read_to_end(&mut bytes)?;
    if bytes.len() > cap {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "file size exceeds cap",
        ));
    }
    Ok(bytes)
}
fn validate_directory_attributes(
    attr: FILE_ATTRIBUTE_TAG_INFO,
) -> Result<FILE_ATTRIBUTE_TAG_INFO, SourceError> {
    if refused_attributes(attr.FileAttributes)
        || attr.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
    {
        return Err(SourceError::Refused);
    }
    Ok(attr)
}
pub(crate) fn root_handles(path: &Path, access: u32, share: u32) -> Result<Vec<File>, SourceError> {
    root_handles_mode(path, access, share, false)
}
fn root_handles_mode(
    path: &Path,
    access: u32,
    share: u32,
    asynchronous: bool,
) -> Result<Vec<File>, SourceError> {
    non_elevated()?;
    if !path.is_absolute() || path.as_os_str().encode_wide().any(|c| c == 0) {
        return Err(SourceError::Refused);
    }
    let mut components = path.components();
    // Only local drive roots have an anchor with no filesystem ancestors to traverse.
    let drive = match components.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => drive,
            _ => return Err(SourceError::Refused),
        },
        _ => return Err(SourceError::Refused),
    };
    if components.next() != Some(Component::RootDir) {
        return Err(SourceError::Refused);
    }
    let mut components = components.peekable();
    let metadata_access = FILE_READ_ATTRIBUTES | SYNCHRONIZE;
    let anchor: Vec<u16> = format!(r"\\?\{}:\", char::from(drive))
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let _profile = profile::OPEN.start();
    let handle = unsafe {
        CreateFileW(
            anchor.as_ptr(),
            if components.peek().is_none() {
                access
            } else {
                metadata_access
            },
            share,
            null(),
            OPEN_EXISTING,
            crate::win::metadata_open_flags()
                | if asynchronous && components.peek().is_none() {
                    FILE_FLAG_OVERLAPPED
                } else {
                    0
                },
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last_error());
    }
    let file = unsafe { File::from_raw_handle(handle) };
    directory_attributes(&file)?;
    let mut pins = vec![file];
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return Err(SourceError::Refused);
        };
        let name: Vec<u16> = name.encode_wide().collect();
        let file = open_relative_mode(
            pins.last().unwrap(),
            &name,
            if components.peek().is_none() {
                access
            } else {
                metadata_access
            },
            share,
            asynchronous && components.peek().is_none(),
        )?;
        directory_attributes(&file)?;
        pins.push(file);
    }
    Ok(pins)
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
    file: Arc<File>,
    listed: Option<FileIdObs>,
    preferred: Strategy,
    root: Option<Arc<File>>,
) -> Result<(NativeDir, OpenedIdentity), SourceError> {
    let _profile = profile::IDENTITY.start();
    let h = file.as_raw_handle();
    let attr = directory_attributes(&file)?;
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
    let fs_name = String::from_utf16_lossy(
        &fs_name[..fs_name
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(fs_name.len())],
    );
    let refs = fs_name.eq_ignore_ascii_case("ReFS");
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
    let root = root.unwrap_or_else(|| file.clone());
    Ok((
        NativeDir {
            file,
            root,
            refs,
            reopen_safe: reopen_safe_fs(&fs_name),
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
fn reopen_safe_fs(name: &str) -> bool {
    // FAT/exFAT IDs can be recycled when a directory entry is reused after handle release.
    name.eq_ignore_ascii_case("NTFS") || name.eq_ignore_ascii_case("ReFS")
}
/// Retain component-wise, no-recall pins while opening the final component asynchronously.
pub(crate) fn open_watch_root(
    path: &Path,
) -> Result<(Vec<File>, File, OpenedIdentity), SourceError> {
    non_elevated()?;
    let mut pins = root_handles_mode(
        path,
        FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        true,
    )?;
    let (dir, identity) = opened(
        Arc::new(pins.pop().unwrap()),
        None,
        Strategy::Extended,
        None,
    )?;
    drop(dir.root);
    Ok((pins, Arc::try_unwrap(dir.file).unwrap(), identity))
}
/// True when this process holds an elevated token, which every native open refuses.
pub fn running_elevated() -> bool {
    matches!(non_elevated(), Err(SourceError::Refused))
}
#[cfg(test)]
thread_local! {
    static TEST_ELEVATED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
#[cfg(test)]
pub(crate) fn force_elevation_for_test() -> impl Drop {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            TEST_ELEVATED.set(self.0);
        }
    }
    Reset(TEST_ELEVATED.replace(true))
}
pub(crate) fn non_elevated() -> Result<(), SourceError> {
    #[cfg(test)]
    if TEST_ELEVATED.get() {
        return Err(SourceError::Refused);
    }
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
        let mut pins = root_handles(
            path,
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        )?;
        opened(Arc::new(pins.pop().unwrap()), None, self.strategy(), None)
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
        let file = open_relative(
            &parent.file,
            entry.name,
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        )?;
        opened(
            Arc::new(file),
            entry.file_id,
            self.strategy(),
            Some(parent.root.clone()),
        )
    }
    fn can_reopen(&self, dir: &NativeDir, identity: OpenedIdentity) -> bool {
        dir.reopen_safe
            && matches!(identity.basis, IdBasis::Listed | IdBasis::PostOpen)
            && identity.id.and_then(FileIdObs::nonzero).is_some()
            && identity.volume_serial.is_some()
    }
    fn reopen(
        &self,
        root: &NativeDir,
        root_identity: OpenedIdentity,
        identity: OpenedIdentity,
    ) -> Result<(NativeDir, OpenedIdentity), SourceError> {
        non_elevated()?;
        if !self.can_reopen(root, identity) {
            return Err(SourceError::IdentityUnverified);
        }
        let (_, observed) = opened(
            root.file.clone(),
            None,
            self.strategy(),
            Some(root.root.clone()),
        )?;
        if observed.id != root_identity.id || observed.volume_serial != root_identity.volume_serial
        {
            return Err(SourceError::IdentityChanged);
        }
        let _thread = self.cancel.register()?;
        if self.cancel.is_cancelled() {
            return Err(SourceError::Io(ERROR_OPERATION_ABORTED as i32));
        }
        let mut descriptor: FILE_ID_DESCRIPTOR = unsafe { zeroed() };
        descriptor.dwSize = size_of::<FILE_ID_DESCRIPTOR>() as u32;
        match identity.id.unwrap() {
            FileIdObs::Id64(id) => {
                descriptor.Type = FileIdType;
                descriptor.Anonymous.FileId = id as i64;
            }
            FileIdObs::Id128(id) => {
                descriptor.Type = ExtendedFileIdType;
                descriptor.Anonymous.ExtendedFileId.Identifier = id;
            }
        }
        let handle = {
            let _profile = profile::OPEN.start();
            let handle = unsafe {
                OpenFileById(
                    root.file.as_raw_handle(),
                    &descriptor,
                    FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    null(),
                    FILE_FLAG_BACKUP_SEMANTICS
                        | FILE_FLAG_OPEN_REPARSE_POINT
                        | FILE_FLAG_OPEN_NO_RECALL,
                )
            };
            // Read the thread's last error before the timer's drop can run platform code.
            if handle == INVALID_HANDLE_VALUE {
                return Err(last_error());
            }
            handle
        };
        let result = opened(
            Arc::new(unsafe { File::from_raw_handle(handle) }),
            identity.id,
            self.strategy(),
            Some(root.root.clone()),
        )?;
        if result.1.volume_serial != identity.volume_serial {
            return Err(SourceError::IdentityChanged);
        }
        Ok(result)
    }
    fn list(&self, dir: &NativeDir, sink: &mut dyn FnMut(RawEntry<'_>) -> Flow) -> ListOutcome {
        let _profile = profile::LIST.start();
        let mut callback = |entry: RawEntry<'_>| {
            let _profile = profile::CALLBACK.start();
            sink(entry)
        };
        let sink = &mut callback as &mut dyn FnMut(RawEntry<'_>) -> Flow;
        let _thread = match self.cancel.register() {
            Ok(h) => h,
            Err(_) => return ListOutcome::Incomplete(IncompleteReason::Io(-1)),
        };
        // Find proves containment after taking its namespace pins.
        if dir.strategy() != Strategy::Find {
            // shortcut: handle enumeration observes containment before listing; atomic exclusion needs pins.
            if let Err(reason) = self.containment(dir) {
                return ListOutcome::Incomplete(reason);
            }
        }
        self.list_validated(dir, sink, directory_attributes(&dir.file).map(|_| ()))
    }
}

fn normalized_path(file: &File) -> Result<Vec<u16>, i32> {
    let mut path = vec![0u16; 256];
    let flags = FILE_NAME_NORMALIZED | VOLUME_NAME_NT;
    loop {
        #[cfg(test)]
        PATH_QUERIES.with_borrow_mut(|queries| queries.push((flags, path.len())));
        let query = profile::PATH_QUERY.start();
        let n = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle(),
                path.as_mut_ptr(),
                path.len() as u32,
                flags,
            )
        } as usize;
        let error = (n == 0).then(|| unsafe { GetLastError() } as i32);
        drop(query);
        if let Some(error) = error {
            return Err(error);
        }
        if n < path.len() {
            path.truncate(n);
            return Ok(path);
        }
        if n >= 32768 {
            return Err(ERROR_FILENAME_EXCED_RANGE as i32);
        }
        path.resize(n + 1, 0);
    }
}

fn path_beneath(root: &[u16], path: &[u16]) -> bool {
    let mut path = path.split(|c| *c == 92).filter(|c| !c.is_empty());
    root.split(|c| *c == 92)
        .filter(|c| !c.is_empty())
        .all(|component| path.next() == Some(component))
}

impl NativeSource {
    fn handle_path(&self, file: &File) -> Result<Vec<u16>, i32> {
        #[cfg(any(test, feature = "fixtures"))]
        if let Some(code) = self.path_query_error {
            return Err(code);
        }
        normalized_path(file)
    }

    fn containment(&self, dir: &NativeDir) -> Result<(), IncompleteReason> {
        let _profile = profile::CONTAINMENT.start();
        let root = self.handle_path(&dir.root).map_err(IncompleteReason::Io)?;
        if Arc::ptr_eq(&dir.root, &dir.file) {
            return Ok(());
        }
        let path = self.handle_path(&dir.file).map_err(IncompleteReason::Io)?;
        if path_beneath(&root, &path) {
            Ok(())
        } else {
            Err(IncompleteReason::ContainmentUnproven)
        }
    }
}

#[cfg(test)]
thread_local! {
    static PATH_QUERIES: std::cell::RefCell<Vec<(u32, usize)>> = const { std::cell::RefCell::new(Vec::new()) };
}
#[cfg(test)]
mod containment_tests {
    use super::*;

    #[test]
    fn normalized_containment_compares_exact_components() {
        for (root, path, expected) in [
            (
                r"\\?\X:\Synthetic\Root",
                r"\\?\x:\synthetic\ROOT\child",
                false,
            ),
            (
                r"\\?\X:\Synthetic\Root",
                r"\\?\X:\Synthetic\Root-sibling",
                false,
            ),
            (r"\\?\X:\Synthetic\Root", r"\\?\X:\Synthetic", false),
            (r"\\?\X:\Synthetic\Root", r"\\?\X:\Synthetic\Root", true),
            (r"\\?\X:\", r"\\?\X:\Synthetic", true),
            (r"\\?\X:\Synthetic\Å", r"\\?\X:\Synthetic\å\child", false),
            (r"\Device\Volume\proj", r"\Device\Volume\PROJ\child", false),
            (r"\\?\X:\Synthetic", r"\\?\Y:\Synthetic\child", false),
        ] {
            assert_eq!(
                path_beneath(
                    &root.encode_utf16().collect::<Vec<_>>(),
                    &path.encode_utf16().collect::<Vec<_>>()
                ),
                expected
            );
        }
    }

    #[test]
    fn native_containment_normalizes_verbatim_and_short_root_aliases() {
        let temp = super::super::tests::canonical_tempdir();
        let source = NativeSource::default();
        if running_elevated() {
            assert!(matches!(
                source.open_root(temp.path()),
                Err(SourceError::Refused)
            ));
            return;
        }
        let path = temp.path().join("synthetic-long-root-name");
        std::fs::create_dir_all(path.join("child")).unwrap();
        let canonical = std::fs::canonicalize(&path).unwrap();
        let (root, _) = source.open_root(&canonical).unwrap();
        let expected = normalized_path(&root.file).unwrap();
        let wide = canonical
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let mut short = vec![0u16; 32768];
        let n = unsafe { GetShortPathNameW(wide.as_ptr(), short.as_mut_ptr(), short.len() as u32) }
            as usize;
        assert!(n > 0 && n < short.len());
        short.truncate(n);
        for alias in [
            path.with_file_name("SYNTHETIC-LONG-ROOT-NAME"),
            path,
            canonical,
            std::path::PathBuf::from(std::ffi::OsString::from_wide(&short)),
        ] {
            let (dir, _) = source.open_root(&alias).unwrap();
            assert_eq!(normalized_path(&dir.root).unwrap(), expected);
            let mut children = 0;
            assert_eq!(
                source.list(&dir, &mut |e| {
                    let (child, _) = source.open_child(&dir, &e).unwrap();
                    assert_eq!(
                        source.list(&child, &mut |_| panic!("empty child")),
                        ListOutcome::Complete
                    );
                    children += 1;
                    Flow::Continue
                }),
                ListOutcome::Complete
            );
            assert_eq!(children, 1);
        }
    }

    #[test]
    fn native_containment_failed_path_query_refuses_listing() {
        let temp = super::super::tests::canonical_tempdir();
        let source = NativeSource::default();
        if running_elevated() {
            assert!(matches!(
                source.open_root(temp.path()),
                Err(SourceError::Refused)
            ));
            return;
        }
        std::fs::write(temp.path().join("sentinel"), b"synthetic").unwrap();
        let (mut dir, _) = source.open_root(temp.path()).unwrap();
        let mut read = null_mut();
        let mut write = null_mut();
        assert_ne!(
            unsafe {
                windows_sys::Win32::System::Pipes::CreatePipe(&mut read, &mut write, null(), 0)
            },
            0
        );
        let _writer = unsafe { File::from_raw_handle(write) };
        dir.root = Arc::new(unsafe { File::from_raw_handle(read) });
        assert!(normalized_path(&dir.root).is_err());
        assert!(matches!(
            source.list(&dir, &mut |_| panic!("failed containment emitted metadata")),
            ListOutcome::Incomplete(IncompleteReason::Io(_))
        ));
    }

    #[test]
    fn native_containment_queries_nt_paths_once_for_root_and_retries_long_paths() {
        let temp = super::super::tests::canonical_tempdir();
        let source = NativeSource::default();
        if running_elevated() {
            assert!(matches!(
                source.open_root(temp.path()),
                Err(SourceError::Refused)
            ));
            return;
        }
        std::fs::create_dir(temp.path().join("child")).unwrap();
        let (root, _) = source.open_root(temp.path()).unwrap();
        PATH_QUERIES.with_borrow_mut(Vec::clear);
        let mut child = None;
        assert_eq!(
            source.list(&root, &mut |e| {
                child = Some(source.open_child(&root, &e).unwrap().0);
                Flow::Continue
            }),
            ListOutcome::Complete
        );
        assert_eq!(
            PATH_QUERIES.with_borrow(Clone::clone),
            [(VOLUME_NAME_NT, 256)]
        );
        for _ in 0..2 {
            PATH_QUERIES.with_borrow_mut(Vec::clear);
            assert_eq!(
                source.list(child.as_ref().unwrap(), &mut |_| panic!("empty child")),
                ListOutcome::Complete
            );
            assert_eq!(
                PATH_QUERIES.with_borrow(Clone::clone),
                [(VOLUME_NAME_NT, 256); 2]
            );
        }
        let mut long = temp.path().to_path_buf();
        for _ in 0..6 {
            long.push("synthetic-long-component-for-path-query-buffer-retry");
        }
        std::fs::create_dir_all(&long).unwrap();
        let (long, _) = source.open_root(&long).unwrap();
        PATH_QUERIES.with_borrow_mut(Vec::clear);
        let path = normalized_path(&long.file).unwrap();
        assert!(path.len() > 256);
        let queries = PATH_QUERIES.with_borrow(Clone::clone);
        assert_eq!(queries.len(), 2);
        assert_eq!(queries[0], (VOLUME_NAME_NT, 256));
        assert!(queries[1].1 > path.len());
        assert_eq!(queries[1].0, VOLUME_NAME_NT);
    }
}
fn incomplete(error: SourceError) -> ListOutcome {
    ListOutcome::Incomplete(match error {
        SourceError::Refused => IncompleteReason::Refused,
        SourceError::IdentityUnverified => IncompleteReason::IdentityUnverified,
        SourceError::AccessDenied => IncompleteReason::AccessDenied,
        SourceError::IdentityChanged => IncompleteReason::Io(ERROR_FILE_INVALID as i32),
        SourceError::Io(code) => IncompleteReason::Io(code),
        SourceError::Unsupported => IncompleteReason::Io(ERROR_NOT_SUPPORTED as i32),
    })
}
impl NativeSource {
    fn list_validated(
        &self,
        dir: &NativeDir,
        sink: &mut dyn FnMut(RawEntry<'_>) -> Flow,
        validation: Result<(), SourceError>,
    ) -> ListOutcome {
        // Revalidate immediately before listing, including a directory that acquired recall flags.
        if let Err(error) = validation {
            return incomplete(error);
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
            let query = profile::ENUM_QUERY.start();
            let ok = unsafe {
                GetFileInformationByHandleEx(
                    dir.file.as_raw_handle(),
                    class,
                    buffer.as_mut_ptr().cast(),
                    (buffer.len() * 8) as u32,
                )
            };
            let error = (ok == 0).then(|| unsafe { GetLastError() });
            drop(query);
            if let Some(code) = error {
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
        self.find_with_identity(dir, sink, |handle| information(handle, FileIdInfo))
    }
    fn find_with_identity(
        &self,
        dir: &NativeDir,
        sink: &mut dyn FnMut(RawEntry<'_>) -> Flow,
        mut identity: impl FnMut(HANDLE) -> Option<FILE_ID_INFO>,
    ) -> ListOutcome {
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
        // Find is path-based: pin and validate every ancestor while its search handle lives.
        let pins = match root_handles(
            &directory,
            FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
        ) {
            Ok(pins) => pins,
            Err(error) => return incomplete(error),
        };
        let original = identity(dir.file.as_raw_handle());
        let pinned = identity(pins.last().unwrap().as_raw_handle());
        // Path pins must be bound to the original handle before Find opens by name.
        if original.is_none() || pinned.is_none() {
            return incomplete(SourceError::IdentityUnverified);
        }
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
        if let Err(reason) = self.containment(dir) {
            return ListOutcome::Incomplete(reason);
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
    fn handle_release_requires_ntfs_or_refs_and_verified_identity() {
        let source = NativeSource::default();
        let identity = OpenedIdentity {
            id: Some(FileIdObs::Id128([1; 16])),
            basis: IdBasis::Listed,
            volume_serial: Some(1),
            attributes: FILE_ATTRIBUTE_DIRECTORY,
            reparse_tag: None,
        };
        for name in [
            "NTFS", "ntfs", "ReFS", "refs", "FAT", "FAT32", "exFAT", "", "unknown",
        ] {
            let file = Arc::new(tempfile::tempfile().unwrap());
            let dir = NativeDir {
                file: file.clone(),
                root: file,
                refs: name.eq_ignore_ascii_case("ReFS"),
                reopen_safe: reopen_safe_fs(name),
                strategy: Mutex::new(Strategy::Extended),
            };
            let safe = matches!(name, "NTFS" | "ntfs" | "ReFS" | "refs");
            assert_eq!(
                source.can_reopen(&dir, identity),
                safe,
                "filesystem {name:?}"
            );
            for invalid in [
                OpenedIdentity {
                    basis: IdBasis::NonUnique,
                    ..identity
                },
                OpenedIdentity {
                    id: None,
                    ..identity
                },
                OpenedIdentity {
                    id: Some(FileIdObs::Id128([0; 16])),
                    ..identity
                },
                OpenedIdentity {
                    volume_serial: None,
                    ..identity
                },
            ] {
                assert!(!source.can_reopen(&dir, invalid));
            }
        }
    }
    #[test]
    fn shortcut_opens_ignore_case_and_listing_opens_stay_exact() {
        let source = include_str!("native.rs").replace("\r\n", "\n");
        let source = source
            .split("#[cfg(test)]\nmod cancellation_tests")
            .next()
            .unwrap();
        assert!(source.contains(
            "open_relative_with_attributes(parent, name, access, share, asynchronous, 0)"
        ));
        assert!(source.contains(
            "FILE_SHARE_READ | FILE_SHARE_DELETE,\n        false,\n        OBJ_CASE_INSENSITIVE,"
        ));
        assert!(source.contains("Attributes: object_attributes,"));
    }

    #[cfg(feature = "shortcut-read")]
    fn shortcut_fixture() -> (tempfile::TempDir, File, Vec<u16>) {
        use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt};
        let temp = super::super::tests::canonical_tempdir();
        std::fs::write(temp.path().join("synthetic.lnk"), b"original").unwrap();
        // Test setup may open metadata while elevated; the APIs under test must refuse it.
        let parent = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES | SYNCHRONIZE)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(temp.path())
            .unwrap();
        (temp, parent, "synthetic.lnk".encode_utf16().collect())
    }

    #[cfg(feature = "shortcut-read")]
    #[test]
    fn shortcut_reader_caps_bytes_and_metadata_handles_cannot_read() {
        use std::io::Read;
        let (temp, parent, name) = shortcut_fixture();
        if running_elevated() {
            assert!(read_capped_regular_file(&parent, &name, 8).is_err());
            assert!(open_metadata_child(&parent, &name).is_err());
            return;
        }
        let mut metadata = open_metadata_child(&parent, &name).unwrap();
        assert!(metadata.read(&mut [0; 1]).is_err());
        assert_eq!(
            read_capped_regular_file(&parent, &name, 8).unwrap(),
            b"original"
        );
        assert!(read_capped_regular_file(&parent, &name, 7).is_err());
        assert!(read_capped_regular_file(&parent, &name, 0).is_err());
        assert!(read_capped_regular_file(&parent, &name, usize::MAX).is_err());
        std::fs::create_dir(temp.path().join("directory")).unwrap();
        assert!(read_capped_regular_file(
            &parent,
            &"directory".encode_utf16().collect::<Vec<_>>(),
            8
        )
        .is_err());
    }

    #[cfg(feature = "shortcut-read")]
    #[test]
    fn reader_uses_final_pinned_handle_after_leaf_swap() {
        let (temp, parent, name) = shortcut_fixture();
        if running_elevated() {
            assert!(read_capped_regular_file(&parent, &name, 8).is_err());
            assert!(open_metadata_child(&parent, &name).is_err());
            return;
        }
        let path = temp.path().join("synthetic.lnk");
        let mut swapped = false;
        let bytes = read_capped_regular_file_with(&parent, &name, 8, || {
            std::fs::rename(&path, temp.path().join("moved.lnk")).unwrap();
            std::fs::write(&path, b"replaced").unwrap();
            swapped = true;
        })
        .unwrap();
        assert!(swapped);
        assert_eq!(bytes, b"original");
    }

    fn fixture() -> Option<(tempfile::TempDir, NativeSource, NativeDir)> {
        let temp = super::super::tests::canonical_tempdir();
        let source = NativeSource::with_strategy(Strategy::Find);
        if running_elevated() {
            assert!(matches!(
                source.open_root(temp.path()),
                Err(SourceError::Refused)
            ));
            return None;
        }
        std::fs::write(temp.path().join("file"), b"synthetic").unwrap();
        let (dir, _) = source.open_root(temp.path()).unwrap();
        Some((temp, source, dir))
    }
    #[test]
    fn find_refuses_two_missing_identity_queries_before_emitting() {
        let Some((_temp, source, dir)) = fixture() else {
            return;
        };
        let mut queries = 0;
        let mut emitted = 0;
        let outcome = source.find_with_identity(
            &dir,
            &mut |_| {
                emitted += 1;
                Flow::Continue
            },
            |_| {
                queries += 1;
                None
            },
        );
        assert_eq!(queries, 2);
        assert_eq!(emitted, 0);
        assert_eq!(
            outcome,
            ListOutcome::Incomplete(IncompleteReason::IdentityUnverified)
        );
        assert_eq!(
            source.list(&dir, &mut |_| Flow::Continue),
            ListOutcome::Complete
        );
    }
    #[test]
    fn list_revalidation_preserves_each_attribute_refusal() {
        let Some((_temp, source, dir)) = fixture() else {
            return;
        };
        for flag in [
            FILE_ATTRIBUTE_REPARSE_POINT,
            FILE_ATTRIBUTE_OFFLINE,
            FILE_ATTRIBUTE_RECALL_ON_OPEN,
            FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
        ] {
            let validation = validate_directory_attributes(FILE_ATTRIBUTE_TAG_INFO {
                FileAttributes: FILE_ATTRIBUTE_DIRECTORY | flag,
                ReparseTag: 0,
            })
            .map(|_| ());
            assert_eq!(validation, Err(SourceError::Refused));
            assert_eq!(
                source.list_validated(
                    &dir,
                    &mut |_| panic!("refused directory listed"),
                    validation
                ),
                ListOutcome::Incomplete(IncompleteReason::Refused)
            );
        }
        for (error, reason) in [
            (SourceError::AccessDenied, IncompleteReason::AccessDenied),
            (SourceError::Io(1234), IncompleteReason::Io(1234)),
        ] {
            assert_eq!(
                source.list_validated(&dir, &mut |_| panic!("failed query listed"), Err(error)),
                ListOutcome::Incomplete(reason)
            );
        }
        assert_eq!(
            source.list(&dir, &mut |_| Flow::Continue),
            ListOutcome::Complete
        );
    }
    #[test]
    fn native_opens_refuse_elevation_consistently() {
        let temp = super::super::tests::canonical_tempdir();
        std::fs::create_dir(temp.path().join("child")).unwrap();
        let parent = if running_elevated() {
            None
        } else {
            Some(
                root_handles(
                    temp.path(),
                    FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                )
                .unwrap()
                .pop()
                .unwrap(),
            )
        };
        let _reset = force_elevation_for_test();
        for time in [Duration::from_secs(1), Duration::ZERO] {
            assert_eq!(
                crate::duplicate_reader::inspect(
                    temp.path(),
                    &OpenedIdentity {
                        id: None,
                        basis: IdBasis::None,
                        volume_serial: None,
                        attributes: 0x10,
                        reparse_tag: None
                    },
                    &[],
                    crate::duplicate_reader::Budget { bytes: 100, time },
                    Default::default(),
                )
                .unwrap_err(),
                crate::duplicate_reader::Reason::Elevated
            );
        }
        assert!(matches!(crate::identity::observe(temp.path()),
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied));
        assert!(matches!(
            root_handles_mode(
                temp.path().ancestors().last().unwrap(),
                FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                false
            ),
            Err(SourceError::Refused)
        ));
        if let Some(parent) = parent {
            assert!(matches!(
                open_relative_mode(
                    &parent,
                    &[99, 104, 105, 108, 100],
                    FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    false
                ),
                Err(SourceError::Refused)
            ));
        }
        assert!(matches!(crate::volumes::enumerate(),
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied));
        assert!(matches!(
            open_watch_root(temp.path()),
            Err(SourceError::Refused)
        ));
    }
    #[test]
    fn directory_opens_always_disable_recall() {
        assert_ne!(directory_create_options() & 0x00400000, 0);
    }
    #[test]
    fn metadata_opens_always_disable_recall() {
        assert_ne!(
            crate::win::metadata_open_flags() & FILE_FLAG_OPEN_NO_RECALL,
            0
        );
    }
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
