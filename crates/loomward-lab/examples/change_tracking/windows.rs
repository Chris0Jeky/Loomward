use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io,
    mem::{size_of, zeroed},
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle},
    path::Path,
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, ERROR_IO_PENDING, ERROR_NOTIFY_ENUM_DIR, FILETIME, GENERIC_READ,
        HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY},
    Storage::FileSystem::{
        CreateFileW, ReadDirectoryChangesW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED,
        FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    },
    System::{
        Ioctl::{
            FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_UNPRIVILEGED_USN_JOURNAL, READ_USN_JOURNAL_DATA_V1,
            USN_JOURNAL_DATA_V1,
        },
        Threading::{
            CreateEventW, GetCurrentProcess, GetCurrentThread, GetProcessTimes, GetThreadTimes,
            OpenProcessToken, WaitForSingleObject,
        },
        IO::{CancelIoEx, DeviceIoControl, GetOverlappedResult, OVERLAPPED},
    },
};

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct PendingRead<'a> {
    dir: &'a File,
    buffer: &'a mut [u32],
    ov: &'a mut OVERLAPPED,
    error: Option<u32>,
}

impl<'a> PendingRead<'a> {
    fn start(dir: &'a File, buffer: &'a mut [u32], ov: &'a mut OVERLAPPED) -> io::Result<Self> {
        let ok = unsafe {
            ReadDirectoryChangesW(
                dir.as_raw_handle(),
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 4) as u32,
                1,
                FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME,
                null_mut(),
                ov,
                None,
            )
        };
        let error = (ok == 0).then(|| unsafe { GetLastError() });
        if let Some(code) = error {
            if code != ERROR_IO_PENDING && code != ERROR_NOTIFY_ENUM_DIR {
                return Err(io::Error::from_raw_os_error(code as i32));
            }
        }
        Ok(Self {
            dir,
            buffer,
            ov,
            error,
        })
    }
}

impl Drop for PendingRead<'_> {
    fn drop(&mut self) {
        if self.error != Some(ERROR_NOTIFY_ENUM_DIR) {
            let mut returned = 0;
            // Cancellation is only a request; storage must survive the completion too.
            unsafe {
                CancelIoEx(self.dir.as_raw_handle(), self.ov);
                GetOverlappedResult(self.dir.as_raw_handle(), self.ov, &mut returned, 1);
            }
        }
    }
}

struct Watcher {
    done: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<io::Result<Value>>>,
}

impl Watcher {
    fn join(mut self) -> io::Result<Value> {
        self.done.store(true, Ordering::Release);
        self.thread
            .take()
            .unwrap()
            .join()
            .map_err(|_| invalid("watcher panicked"))?
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn non_elevated() -> io::Result<()> {
    let mut raw = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Handle(raw);
    let mut elevation: TOKEN_ELEVATION = unsafe { zeroed() };
    let mut bytes = 0;
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut bytes,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if elevation.TokenIsElevated != 0 {
        return Err(invalid("elevated token refused"));
    }
    Ok(())
}

fn cpu(thread_only: bool) -> io::Result<f64> {
    let mut creation: FILETIME = unsafe { zeroed() };
    let mut exit = creation;
    let mut kernel = creation;
    let mut user = creation;
    let ok = unsafe {
        if thread_only {
            GetThreadTimes(
                GetCurrentThread(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        } else {
            GetProcessTimes(
                GetCurrentProcess(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        }
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let ticks = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
    Ok((ticks(kernel) + ticks(user)) as f64 / 10_000_000.0)
}

fn u32_at(bytes: &[u8], at: usize) -> io::Result<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(at..at + 4)
            .ok_or_else(|| invalid("short notification"))?
            .try_into()
            .unwrap(),
    ))
}
fn decode(bytes: &[u8]) -> io::Result<Vec<(u32, String)>> {
    let mut events = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let next = u32_at(bytes, at)? as usize;
        let action = u32_at(bytes, at + 4)?;
        let length = u32_at(bytes, at + 8)? as usize;
        if length % 2 != 0 || !(1..=5).contains(&action) {
            return Err(invalid("malformed notification"));
        }
        let end = at
            .checked_add(12)
            .and_then(|n| n.checked_add(length))
            .ok_or_else(|| invalid("notification length overflow"))?;
        let name = bytes
            .get(at + 12..end)
            .ok_or_else(|| invalid("short name"))?;
        let utf16: Vec<u16> = name
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        events.push((
            action,
            String::from_utf16(&utf16).map_err(|_| invalid("invalid UTF16 lab name"))?,
        ));
        if next == 0 {
            break;
        }
        if next % 4 != 0 || next < 12 + length || next >= bytes.len() - at {
            return Err(invalid("bad next offset"));
        }
        at += next;
    }
    Ok(events)
}

// Both completion forms lose the scope's history, regardless of how many hints arrived earlier.
fn overflow(error: Option<u32>, bytes: u32) -> bool {
    error == Some(ERROR_NOTIFY_ENUM_DIR) || (error.is_none() && bytes == 0)
}

fn usn() -> Vec<Value> {
    let mut rows = Vec::new();
    for drive in ['G', 'E'] {
        for (kind, path, access, flags) in [
            ("volume_metadata", format!(r"\\.\{drive}:"), 0, 0),
            ("volume_read", format!(r"\\.\{drive}:"), GENERIC_READ, 0),
            (
                "directory",
                format!(r"{drive}:\"),
                FILE_LIST_DIRECTORY,
                FILE_FLAG_BACKUP_SEMANTICS,
            ),
        ] {
            let name = wide(Path::new(&path));
            let raw = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    access,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    null(),
                    OPEN_EXISTING,
                    flags,
                    null_mut(),
                )
            };
            if raw == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
                rows.push(json!({"volume": drive.to_string(), "handle": kind, "open_error": unsafe {GetLastError()}, "query_error": null, "read_error": null}));
                continue;
            }
            let handle = Handle(raw);
            let mut journal: USN_JOURNAL_DATA_V1 = unsafe { zeroed() };
            let mut returned = 0;
            let ok = unsafe {
                DeviceIoControl(
                    handle.0,
                    FSCTL_QUERY_USN_JOURNAL,
                    null(),
                    0,
                    (&mut journal as *mut USN_JOURNAL_DATA_V1).cast(),
                    size_of::<USN_JOURNAL_DATA_V1>() as u32,
                    &mut returned,
                    null_mut(),
                )
            };
            let query_error = (ok == 0).then(|| unsafe { GetLastError() });
            let query_bytes = returned;
            let input = READ_USN_JOURNAL_DATA_V1 {
                StartUsn: journal.NextUsn,
                ReasonMask: u32::MAX,
                ReturnOnlyOnClose: 0,
                Timeout: 0,
                BytesToWaitFor: 0,
                UsnJournalID: journal.UsnJournalID,
                MinMajorVersion: 2,
                MaxMajorVersion: 3,
            };
            let mut out = vec![0u64; 8192];
            let start = Instant::now();
            let read_ok = unsafe {
                DeviceIoControl(
                    handle.0,
                    FSCTL_READ_UNPRIVILEGED_USN_JOURNAL,
                    (&input as *const READ_USN_JOURNAL_DATA_V1).cast(),
                    size_of::<READ_USN_JOURNAL_DATA_V1>() as u32,
                    out.as_mut_ptr().cast(),
                    (out.len() * 8) as u32,
                    &mut returned,
                    null_mut(),
                )
            };
            let read_error = (read_ok == 0).then(|| unsafe { GetLastError() });
            rows.push(json!({"volume": drive.to_string(), "handle": kind, "open_error": null, "query_error": query_error, "query_bytes": query_bytes, "read_error": read_error, "read_bytes": returned, "read_ms": start.elapsed().as_secs_f64()*1000.0,
                "journal": if query_error.is_none() { json!({"id_present": journal.UsnJournalID != 0, "retained_span_usn": journal.NextUsn - journal.FirstUsn, "lowest_valid_usn_is_zero": journal.LowestValidUsn == 0, "max_size": journal.MaximumSize, "versions": [journal.MinSupportedMajorVersion,journal.MaxSupportedMajorVersion]}) } else { Value::Null }}));
        }
    }
    rows
}

fn query(handle: HANDLE) -> io::Result<USN_JOURNAL_DATA_V1> {
    let mut journal = unsafe { zeroed::<USN_JOURNAL_DATA_V1>() };
    let mut returned = 0;
    if unsafe {
        DeviceIoControl(
            handle,
            FSCTL_QUERY_USN_JOURNAL,
            null(),
            0,
            (&mut journal as *mut USN_JOURNAL_DATA_V1).cast(),
            size_of::<USN_JOURNAL_DATA_V1>() as u32,
            &mut returned,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if returned < 56 {
        return Err(invalid("short journal query"));
    }
    Ok(journal)
}

fn read_usn(handle: HANDLE, input: &READ_USN_JOURNAL_DATA_V1) -> io::Result<Vec<u8>> {
    let mut output = vec![0u64; 8192];
    let mut returned = 0;
    if unsafe {
        DeviceIoControl(
            handle,
            FSCTL_READ_UNPRIVILEGED_USN_JOURNAL,
            (input as *const READ_USN_JOURNAL_DATA_V1).cast(),
            size_of::<READ_USN_JOURNAL_DATA_V1>() as u32,
            output.as_mut_ptr().cast(),
            (output.len() * 8) as u32,
            &mut returned,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if returned < 8 || returned as usize > output.len() * 8 {
        return Err(invalid("bad USN output length"));
    }
    Ok(
        unsafe { std::slice::from_raw_parts(output.as_ptr().cast::<u8>(), returned as usize) }
            .to_vec(),
    )
}

fn u64_at(bytes: &[u8], at: usize) -> io::Result<u64> {
    Ok(u64::from_le_bytes(
        bytes
            .get(at..at + 8)
            .ok_or_else(|| invalid("short USN record"))?
            .try_into()
            .unwrap(),
    ))
}

type UsnRecord = (u64, u64, u32, u16, u16);

fn usn_records(bytes: &[u8]) -> io::Result<Vec<UsnRecord>> {
    let mut at = 8;
    let mut records = Vec::new();
    while at < bytes.len() {
        let length = u32_at(bytes, at)? as usize;
        let version = u16::from_le_bytes(
            bytes
                .get(at + 4..at + 6)
                .ok_or_else(|| invalid("short USN version"))?
                .try_into()
                .unwrap(),
        );
        let (minimum, id_width, reason_offset, name_offset) = match version {
            2 => (60, 8, 40, 56),
            3 => (76, 16, 56, 72),
            _ => return Err(invalid("unsupported USN record version")),
        };
        if length < minimum || length % 8 != 0 {
            return Err(invalid("invalid USN record length"));
        }
        let end = at
            .checked_add(length)
            .ok_or_else(|| invalid("USN length overflow"))?;
        let record = bytes
            .get(at..end)
            .ok_or_else(|| invalid("truncated USN record"))?;
        let name_length =
            u16::from_le_bytes(record[name_offset..name_offset + 2].try_into().unwrap());
        let name_start =
            u16::from_le_bytes(record[name_offset + 2..name_offset + 4].try_into().unwrap())
                as usize;
        if name_length % 2 != 0
            || name_start + name_length as usize > length
            || (name_length != 0 && (name_start < minimum || name_start % 2 != 0))
        {
            return Err(invalid("invalid USN name bounds"));
        }
        // NTFS lab identity is 64-bit; do not silently truncate a 128-bit identifier.
        if id_width == 16 && (u64_at(record, 16)? != 0 || u64_at(record, 32)? != 0) {
            return Err(invalid("128-bit USN identity outside NTFS spike"));
        }
        records.push((
            u64_at(record, 8)?,
            u64_at(record, 8 + id_width)?,
            u32_at(record, reason_offset)?,
            name_length,
            version,
        ));
        at = end;
    }
    Ok(records)
}

fn directory(path: &Path, share: u32) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(
            FILE_LIST_DIRECTORY | windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES,
        )
        .share_mode(share)
        .custom_flags(
            FILE_FLAG_BACKUP_SEMANTICS
                | windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT,
        )
        .open(path)
}

fn pinned(path: &Path, share: u32) -> io::Result<File> {
    use windows_sys::Win32::Storage::FileSystem::*;
    let file = directory(path, share)?;
    let mut info = unsafe { zeroed::<FILE_ATTRIBUTE_TAG_INFO>() };
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileAttributeTagInfo,
            (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if info.FileAttributes
        & (FILE_ATTRIBUTE_REPARSE_POINT
            | FILE_ATTRIBUTE_OFFLINE
            | FILE_ATTRIBUTE_RECALL_ON_OPEN
            | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS)
        != 0
        || info.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
    {
        return Err(invalid(
            "lab ancestor is reparse/offline or not a directory",
        ));
    }
    Ok(file)
}

struct PinnedTree {
    path: std::path::PathBuf,
    pin: Option<File>,
}

impl PinnedTree {
    fn new(tree: tempfile::TempDir) -> io::Result<Self> {
        let pin = pinned(tree.path(), FILE_SHARE_READ | FILE_SHARE_WRITE)?;
        Ok(Self {
            path: tree.keep(),
            pin: Some(pin),
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn cleanup(&mut self) -> io::Result<()> {
        for entry in fs::read_dir(&self.path)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                fs::remove_dir_all(entry.path())?;
            } else {
                fs::remove_file(entry.path())?;
            }
        }
        // Denied delete sharing protects recursive cleanup; release only for empty-root removal.
        self.pin.take();
        fs::remove_dir(&self.path)
    }

    fn close(mut self) -> io::Result<()> {
        self.cleanup()
    }
}

impl Drop for PinnedTree {
    fn drop(&mut self) {
        if self.pin.is_some() {
            let _ = self.cleanup();
        }
    }
}

fn catch_up(drive: char) -> io::Result<Value> {
    use windows_sys::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    let base = std::path::PathBuf::from(format!(r"{drive}:\loomward-lab\scale"));
    let pins = [
        pinned(base.parent().unwrap(), FILE_SHARE_READ)
            .map_err(|e| io::Error::new(e.kind(), format!("{drive} lab ancestor pin: {e}")))?,
        pinned(&base, FILE_SHARE_READ)
            .map_err(|e| io::Error::new(e.kind(), format!("{drive} scale pin: {e}")))?,
    ];
    let mut available = 0;
    if unsafe { GetDiskFreeSpaceExW(wide(&base).as_ptr(), &mut available, null_mut(), null_mut()) }
        == 0
    {
        return Err(io::Error::last_os_error());
    }
    if available < 20_000_000_000 {
        return Err(invalid("lab has less than 20 GB free"));
    }
    let tree = PinnedTree::new(
        tempfile::Builder::new()
            .prefix("watch-usn-")
            .tempdir_in(&base)
            .map_err(|e| io::Error::new(e.kind(), format!("{drive} tempdir create: {e}")))?,
    )?;
    if tree.path().parent() != Some(base.as_path()) {
        return Err(invalid("tempdir escaped pinned lab parent"));
    }
    fs::write(
        tree.path().join(".loomward-lab-marker"),
        loomward_lab::MARKER,
    )?;
    let root_pin = tree.pin.as_ref().unwrap();
    let mut info = unsafe { zeroed::<BY_HANDLE_FILE_INFORMATION>() };
    if unsafe { GetFileInformationByHandle(root_pin.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let parent_id = ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64;
    let before = query(root_pin.as_raw_handle())
        .map_err(|e| io::Error::new(e.kind(), format!("{drive} initial query: {e}")))?;
    let sibling = PinnedTree::new(
        tempfile::Builder::new()
            .prefix("watch-usn-control-")
            .tempdir_in(&base)?,
    )?;
    if sibling.path().parent() != Some(base.as_path()) {
        return Err(invalid("control escaped pinned lab parent"));
    }
    fs::write(
        sibling.path().join(".loomward-lab-marker"),
        loomward_lab::MARKER,
    )?;
    for i in 0..10 {
        File::create(sibling.path().join(format!("control{i:02}")))?;
    }
    let mut input = READ_USN_JOURNAL_DATA_V1 {
        StartUsn: before.NextUsn,
        ReasonMask: u32::MAX,
        ReturnOnlyOnClose: 0,
        Timeout: 0,
        BytesToWaitFor: 0,
        UsnJournalID: before.UsnJournalID,
        MinMajorVersion: 2,
        MaxMajorVersion: 3,
    };
    let generation_start = Instant::now();
    for i in 0..10_000 {
        File::create(tree.path().join(format!("f{i:06}")))
            .map_err(|e| io::Error::new(e.kind(), format!("{drive} create {i}: {e}")))?;
    }
    for i in 0..10_000 {
        fs::rename(
            tree.path().join(format!("f{i:06}")),
            tree.path().join(format!("r{i:06}")),
        )
        .map_err(|e| io::Error::new(e.kind(), format!("{drive} rename {i}: {e}")))?;
    }
    for i in 0..10_000 {
        fs::remove_file(tree.path().join(format!("r{i:06}")))
            .map_err(|e| io::Error::new(e.kind(), format!("{drive} remove {i}: {e}")))?;
    }
    let generation_ms = generation_start.elapsed().as_secs_f64() * 1000.0;
    let after = query(root_pin.as_raw_handle())?;
    if after.UsnJournalID != before.UsnJournalID
        || before.NextUsn < after.FirstUsn
        || before.NextUsn < after.LowestValidUsn
    {
        return Err(invalid("journal gap during lab generation"));
    }
    // Reopen: the saved cursor is independent of the original handle, without pretending to reboot.
    let reopened = directory(tree.path(), FILE_SHARE_READ | FILE_SHARE_WRITE)?;
    let mut runs = Vec::new();
    for (label, mask, close_only) in [
        ("all_reasons", u32::MAX, 0),
        ("rename_reasons", 0x3000, 0),
        ("close_only", 0x80000000, 1),
    ] {
        input.StartUsn = before.NextUsn;
        input.ReasonMask = mask;
        input.ReturnOnlyOnClose = close_only;
        let start = Instant::now();
        let cpu_start = cpu(false)?;
        let mut lab_records = 0u64;
        let mut other_records = 0u64;
        let mut named = 0;
        let mut bytes = 0u64;
        let mut calls = 0;
        let mut reasons = BTreeMap::new();
        let mut versions = BTreeMap::new();
        let mut ids = std::collections::BTreeSet::new();
        while input.StartUsn < after.NextUsn && calls < 10_000 {
            let output = read_usn(reopened.as_raw_handle(), &input)?;
            calls += 1;
            bytes += output.len() as u64;
            let next = u64_at(&output, 0)? as i64;
            if next <= input.StartUsn {
                return Err(invalid("USN cursor failed to progress"));
            }
            for (id, parent, reason, name_length, version) in usn_records(&output)? {
                if parent != parent_id {
                    other_records += 1;
                    continue;
                }
                lab_records += 1;
                ids.insert(id);
                named += u64::from(name_length != 0);
                *reasons.entry(format!("0x{reason:08x}")).or_insert(0u64) += 1;
                *versions.entry(version.to_string()).or_insert(0u64) += 1;
            }
            input.StartUsn = next;
        }
        let seconds = start.elapsed().as_secs_f64();
        if input.StartUsn < after.NextUsn {
            return Err(invalid("USN call budget exhausted"));
        }
        runs.push(json!({"filter": label,"mask": mask,"return_only_on_close": close_only,"lab_records":lab_records,"outside_lab_records_count_only":other_records,"lab_unique_file_ids":ids.len(),"lab_records_with_names":named,"lab_reasons":reasons,"lab_versions":versions,"calls":calls,"bytes":bytes,"seconds":seconds,"records_per_s":lab_records as f64/seconds,"cpu_s":cpu(false)?-cpu_start,"cursor_reached_target":true}));
    }
    input.ReasonMask = u32::MAX;
    input.ReturnOnlyOnClose = 0;
    input.StartUsn = after.NextUsn;
    input.UsnJournalID ^= 1;
    let wrong_id = read_usn(reopened.as_raw_handle(), &input)
        .err()
        .and_then(|e| e.raw_os_error());
    input.UsnJournalID = after.UsnJournalID;
    input.StartUsn = after.FirstUsn - 1;
    let stale_cursor = read_usn(reopened.as_raw_handle(), &input)
        .err()
        .and_then(|e| e.raw_os_error());
    input.StartUsn = after.NextUsn;
    input.MinMajorVersion = 99;
    input.MaxMajorVersion = 99;
    let unsupported_version = read_usn(reopened.as_raw_handle(), &input)
        .err()
        .and_then(|e| e.raw_os_error());
    drop(reopened);
    tree.close()?;
    sibling.close()?;
    drop(pins);
    Ok(
        json!({"volume":drive.to_string(),"lab":"marked watch-usn-* owned tempdir below loomward-lab/scale","files":10000,"mutation_operations":30000,"generation_ms":generation_ms,"same_journal_after_changes":true,"handle_reopen_catchup":true,"catch_up_span_usn":after.NextUsn - before.NextUsn,"runs":runs,"errors":{"wrong_journal_id":wrong_id,"below_first_usn":stale_cursor,"unsupported_version":unsupported_version},"cleanup":"removed_owned_tempdir"}),
    )
}

fn watch_case(kib: usize, operations: usize, stalled: bool, bulk: bool) -> io::Result<Value> {
    let tree = tempfile::Builder::new()
        .prefix("loomward-watch-")
        .tempdir()?;
    fs::write(
        tree.path().join(".loomward-lab-marker"),
        loomward_lab::MARKER,
    )?;
    let sub = tree.path().join("before");
    fs::create_dir(&sub)?;
    if bulk {
        for i in 0..10_000 {
            File::create(sub.join(format!("f{i:06}")))?;
        }
    }
    let dir = OpenOptions::new()
        .access_mode(FILE_LIST_DIRECTORY)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED)
        .open(tree.path())?;
    let done = Arc::new(AtomicBool::new(false));
    let finished = done.clone();
    let (ready_tx, ready_rx) = mpsc::channel();
    let epoch = Instant::now();
    let process_cpu = cpu(false)?;
    let watcher = thread::spawn(move || -> io::Result<Value> {
        let cpu_start = cpu(true)?;
        let event = Handle(unsafe { CreateEventW(null(), 0, 0, null()) });
        if event.0.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut buffer = vec![0u32; kib * 256];
        let mut ov: OVERLAPPED = unsafe { zeroed() };
        ov.hEvent = event.0;
        let mut observed = BTreeMap::new();
        let mut counts = [0u64; 5];
        let mut completions = 0;
        let mut zeros = 0;
        let mut enum_errors = 0;
        let mut bytes_total = 0u64;
        let mut first_overflow_ms = None;
        let mut max_batch = 0;
        let mut duplicate_hints = 0;
        let mut first = true;
        loop {
            let pending = PendingRead::start(&dir, &mut buffer, &mut ov)?;
            let error = pending.error;
            if first {
                ready_tx.send(()).map_err(|_| invalid("generator gone"))?;
                first = false;
                if stalled {
                    while !finished.load(Ordering::Acquire) {
                        thread::sleep(Duration::from_millis(1));
                    }
                }
            }
            let mut returned = 0;
            let error = if error == Some(ERROR_NOTIFY_ENUM_DIR) {
                error
            } else {
                loop {
                    match unsafe { WaitForSingleObject(event.0, 300) } {
                        WAIT_OBJECT_0 => break,
                        WAIT_TIMEOUT if finished.load(Ordering::Acquire) => {
                            drop(pending);
                            return Ok(
                                json!({"actions": counts, "completions": completions, "zero_byte_completions": zeros, "error_notify_enum_dir": enum_errors, "first_overflow_ms": first_overflow_ms, "bytes": bytes_total, "max_batch_records": max_batch, "duplicate_hints": duplicate_hints, "watcher_cpu_s": cpu(true)?-cpu_start, "observed": observed}),
                            );
                        }
                        WAIT_TIMEOUT => continue,
                        _ => return Err(io::Error::last_os_error()),
                    }
                }
                let ok = unsafe {
                    GetOverlappedResult(dir.as_raw_handle(), pending.ov, &mut returned, 0)
                };
                (ok == 0).then(|| unsafe { GetLastError() })
            };
            completions += 1;
            let now = epoch.elapsed().as_secs_f64() * 1000.0;
            if overflow(error, returned) {
                if error.is_some() {
                    enum_errors += 1;
                } else {
                    zeros += 1;
                }
                first_overflow_ms.get_or_insert(now);
            } else if let Some(code) = error {
                return Err(io::Error::from_raw_os_error(code as i32));
            } else {
                bytes_total += returned as u64;
                // DWORD storage keeps both the kernel buffer and decoded byte slice aligned.
                let raw = unsafe {
                    std::slice::from_raw_parts(
                        pending.buffer.as_ptr().cast::<u8>(),
                        returned as usize,
                    )
                };
                let events = decode(raw)?;
                max_batch = max_batch.max(events.len());
                for (action, name) in events {
                    counts[action as usize - 1] += 1;
                    if observed.insert(format!("{action}:{name}"), now).is_some() {
                        duplicate_hints += 1;
                    }
                }
            }
            drop(pending);
            ov = unsafe { zeroed() };
            ov.hEvent = event.0;
        }
    });
    let watcher = Watcher {
        done: done.clone(),
        thread: Some(watcher),
    };
    ready_rx
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| invalid("watcher did not arm"))?;
    let start = Instant::now();
    let mut sent = BTreeMap::new();
    let generation = (|| -> io::Result<()> {
        if bulk {
            let now = epoch.elapsed().as_secs_f64() * 1000.0;
            sent.insert("4:before".to_string(), now);
            sent.insert("5:after".to_string(), now);
            fs::rename(&sub, tree.path().join("after"))?;
        } else {
            let files = operations / 3;
            let creates = files + operations % 3;
            for i in 0..creates {
                sent.insert(
                    format!("1:before\\f{i:06}"),
                    epoch.elapsed().as_secs_f64() * 1000.0,
                );
                File::create(sub.join(format!("f{i:06}")))?;
            }
            for i in 0..files {
                let now = epoch.elapsed().as_secs_f64() * 1000.0;
                sent.insert(format!("4:before\\f{i:06}"), now);
                sent.insert(format!("5:before\\r{i:06}"), now);
                fs::rename(sub.join(format!("f{i:06}")), sub.join(format!("r{i:06}")))?;
            }
            for i in 0..files {
                sent.insert(
                    format!("2:before\\r{i:06}"),
                    epoch.elapsed().as_secs_f64() * 1000.0,
                );
                fs::remove_file(sub.join(format!("r{i:06}")))?;
            }
        }
        Ok(())
    })();
    let generate_ms = start.elapsed().as_secs_f64() * 1000.0;
    let mut result = watcher.join()?;
    generation?;
    let wall_s = epoch.elapsed().as_secs_f64();
    let observed = result.as_object_mut().unwrap().remove("observed").unwrap();
    let mut latencies: Vec<f64> = sent
        .iter()
        .filter_map(|(key, time)| {
            observed
                .get(key)?
                .as_f64()
                .map(|arrival| (arrival - time).max(0.0))
        })
        .collect();
    latencies.sort_by(f64::total_cmp);
    let percentile = |p: usize| {
        latencies
            .get((latencies.len().saturating_sub(1) * p) / 100)
            .copied()
    };
    result["buffer_kib"] = json!(kib);
    result["stalled_until_burst_end"] = json!(stalled);
    result["case"] = json!(if bulk {
        "directory_rename_10000_children"
    } else {
        "create_rename_delete"
    });
    result["mutation_operations"] = json!(if bulk { 1 } else { operations });
    result["expected_name_records"] = json!(sent.len());
    result["matched_name_records"] = json!(latencies.len());
    result["latency_ms"] =
        json!({"p50": percentile(50), "p95": percentile(95), "max": latencies.last()});
    result["generation_ms"] = json!(generate_ms);
    result["wall_s_including_300ms_drain"] = json!(wall_s);
    result["process_cpu_s"] = json!(cpu(false)? - process_cpu);
    result["final_child_count"] =
        json!(fs::read_dir(tree.path().join(if bulk { "after" } else { "before" }))?.count());
    tree.close()?;
    result["cleanup"] = json!("removed_owned_tempdir");
    Ok(result)
}

pub fn run() -> io::Result<()> {
    non_elevated()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["--usn"] {
        println!("{}", serde_json::to_string_pretty(&usn())?);
        return Ok(());
    }
    if args.len() != 2 || !["--run", "--catch-up"].contains(&args[0].as_str()) {
        return Err(invalid(
            "use --usn, --run OUTPUT.json, or --catch-up OUTPUT.json",
        ));
    }
    if args[0] == "--catch-up" {
        let mut rows = Vec::new();
        for drive in ['G', 'E'] {
            rows.push(catch_up(drive)?);
        }
        let mut report: Value = serde_json::from_slice(&fs::read(&args[1])?)?;
        report["catch_up"] = json!(rows);
        fs::write(&args[1], serde_json::to_vec_pretty(&report)?)?;
        return Ok(());
    }
    let mut report = json!({"schema_version": 1, "elevated": false, "lab": "owned OS tempdirs; no real paths or names retained", "watch": []});
    for kib in [4, 64, 1024] {
        for operations in [1000, 10_000, 100_000] {
            for stalled in [false, true] {
                let row = watch_case(kib, operations, stalled, false)?;
                eprintln!("buffer={kib} KiB operations={operations} stalled={stalled}: matched={} zeros={} enum_errors={}", row["matched_name_records"], row["zero_byte_completions"],row["error_notify_enum_dir"]);
                report["watch"].as_array_mut().unwrap().push(row);
                fs::write(&args[1], serde_json::to_vec_pretty(&report)?)?;
            }
        }
        report["watch"]
            .as_array_mut()
            .unwrap()
            .push(watch_case(kib, 1, false, true)?);
        fs::write(&args[1], serde_json::to_vec_pretty(&report)?)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_arming_cancels_and_drains_the_pending_read() {
        let tree = tempfile::tempdir().unwrap();
        let dir = OpenOptions::new()
            .access_mode(FILE_LIST_DIRECTORY)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED)
            .open(tree.path())
            .unwrap();
        let event = Handle(unsafe { CreateEventW(null(), 0, 0, null()) });
        assert!(!event.0.is_null());
        let mut ov: OVERLAPPED = unsafe { zeroed() };
        ov.hEvent = event.0;
        let mut buffer = vec![0u32; 1024];
        let (ready_tx, ready_rx) = mpsc::channel::<()>();
        drop(ready_rx);
        let result = (|| -> io::Result<()> {
            let pending = PendingRead::start(&dir, &mut buffer, &mut ov)?;
            assert!(pending.error.is_none() || pending.error == Some(ERROR_IO_PENDING));
            ready_tx.send(()).map_err(|_| invalid("generator gone"))?;
            Ok(())
        })();
        assert!(result.is_err());
        let mut returned = 0;
        assert_eq!(
            unsafe { GetOverlappedResult(dir.as_raw_handle(), &ov, &mut returned, 0) },
            0
        );
        assert_eq!(
            unsafe { GetLastError() },
            windows_sys::Win32::Foundation::ERROR_OPERATION_ABORTED
        );
    }

    #[test]
    fn arming_timeout_stops_and_joins_before_tree_cleanup() {
        let tree = tempfile::tempdir().unwrap();
        let path = tree.path().to_owned();
        let done = Arc::new(AtomicBool::new(false));
        let finished = done.clone();
        let exited = Arc::new(AtomicBool::new(false));
        let child_exited = exited.clone();
        let (ready_tx, ready_rx) = mpsc::channel::<()>();
        let (started_tx, started_rx) = mpsc::channel();
        let watcher = Watcher {
            done,
            thread: Some(thread::spawn(move || {
                started_tx.send(()).unwrap();
                while !finished.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_millis(1));
                }
                assert!(path.is_dir());
                assert!(ready_tx.send(()).is_err());
                child_exited.store(true, Ordering::Release);
                Ok(json!({}))
            })),
        };
        started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(
            ready_rx.recv_timeout(Duration::ZERO),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        drop(ready_rx);
        drop(watcher);
        assert!(exited.load(Ordering::Acquire));
        tree.close().unwrap();
    }

    #[test]
    fn pinned_tree_cleanup_keeps_the_root_fixed_and_removes_contents() {
        let parent = tempfile::tempdir().unwrap();
        for explicit_close in [true, false] {
            let tree = PinnedTree::new(tempfile::tempdir_in(parent.path()).unwrap()).unwrap();
            let path = tree.path().to_owned();
            fs::create_dir(path.join("child")).unwrap();
            fs::write(path.join("child/file"), b"synthetic").unwrap();
            fs::write(path.join("marker"), loomward_lab::MARKER).unwrap();
            let error = fs::rename(&path, parent.path().join("replaced")).unwrap_err();
            assert_eq!(
                error.raw_os_error(),
                Some(windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION as i32)
            );
            if explicit_close {
                tree.close().unwrap();
            } else {
                drop(tree);
            }
            assert!(!path.exists());
        }
    }

    #[test]
    fn both_overflow_forms_dirty_scope() {
        assert!(overflow(None, 0));
        assert!(overflow(Some(ERROR_NOTIFY_ENUM_DIR), 0));
        assert!(overflow(Some(ERROR_NOTIFY_ENUM_DIR), 128));
        assert!(!overflow(None, 128));
        assert!(!overflow(Some(5), 0));
    }
    #[test]
    fn notification_decoder_checks_boundaries() {
        let mut record = Vec::new();
        for value in [0u32, 1, 2] {
            record.extend(value.to_le_bytes());
        }
        record.extend(65u16.to_le_bytes());
        assert_eq!(decode(&record).unwrap(), vec![(1, "A".to_string())]);
        assert!(decode(&record[..13]).is_err());
        record[8] = 3;
        assert!(decode(&record).is_err());
        record[8] = 2;
        record[0] = 4;
        assert!(decode(&record).is_err());
    }
    #[test]
    fn usn_decoder_refuses_truncated_and_unsupported_records() {
        let mut output = vec![0u8; 72];
        output[8..12].copy_from_slice(&64u32.to_le_bytes());
        output[12] = 2;
        output[16..24].copy_from_slice(&123u64.to_le_bytes());
        output[24..32].copy_from_slice(&456u64.to_le_bytes());
        output[48..52].copy_from_slice(&0x3000u32.to_le_bytes());
        assert_eq!(
            usn_records(&output).unwrap(),
            vec![(123, 456, 0x3000, 0, 2)]
        );
        assert!(usn_records(&output[..71]).is_err());
        output[12] = 4;
        assert!(usn_records(&output).is_err());
        output[12] = 2;
        output[64..66].copy_from_slice(&2u16.to_le_bytes());
        output[66..68].copy_from_slice(&72u16.to_le_bytes());
        assert!(usn_records(&output).is_err());
    }

    #[test]
    fn usn_names_start_after_the_version_header_on_utf16_boundaries() {
        for (version, header, name_offset) in [(2u16, 60u16, 56usize), (3, 76, 72)] {
            let mut output = vec![0u8; 88];
            output[8..12].copy_from_slice(&80u32.to_le_bytes());
            output[12..14].copy_from_slice(&version.to_le_bytes());
            output[8 + name_offset..10 + name_offset].copy_from_slice(&2u16.to_le_bytes());
            for offset in [0, header - 2, header + 1] {
                output[10 + name_offset..12 + name_offset].copy_from_slice(&offset.to_le_bytes());
                assert!(
                    usn_records(&output).is_err(),
                    "version {version}, offset {offset}"
                );
            }
            output[10 + name_offset..12 + name_offset].copy_from_slice(&header.to_le_bytes());
            assert_eq!(usn_records(&output).unwrap()[0].3, 2);
            output[8 + name_offset..10 + name_offset].fill(0);
            output[10 + name_offset..12 + name_offset].fill(0);
            assert_eq!(usn_records(&output).unwrap()[0].3, 0);
        }
    }
}
