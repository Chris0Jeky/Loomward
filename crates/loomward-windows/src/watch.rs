//! Recursive, read-only change hints. Notifications never establish absence or grant access.
use crate::enumerate::SourceError;

/// A name hint or loss of notification coverage. Names retain exact UTF-16.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// FILE_ACTION_* and a root-relative name; both rename names are independent hints.
    Name { action: u32, name: Vec<u16> },
    /// Overflow, malformed records or a failed watcher requires a full granted-root relist.
    RootDirty,
}

/// Both Windows overflow completion forms discard history, even after earlier name hints.
pub fn overflow(error: Option<u32>, bytes: usize) -> bool {
    error == Some(1022) || (error.is_none() && bytes == 0)
}

/// Validate a FILE_NOTIFY_INFORMATION chain before exposing any names.
pub fn decode(bytes: &[u8]) -> Result<Vec<Change>, SourceError> {
    let mut result = Vec::new();
    let mut at = 0;
    loop {
        let header = bytes.get(at..at + 12).ok_or(SourceError::Refused)?;
        let word = |i| u32::from_le_bytes(header[i..i + 4].try_into().unwrap());
        let next = word(0) as usize;
        let action = word(4);
        let len = word(8) as usize;
        if !(1..=5).contains(&action) || len == 0 || len % 2 != 0 {
            return Err(SourceError::Refused);
        }
        let end = at.checked_add(12 + len).ok_or(SourceError::Refused)?;
        let raw = bytes.get(at + 12..end).ok_or(SourceError::Refused)?;
        let name: Vec<u16> = raw
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        // A notification is a relative hint, never an input path to a native open.
        if name.split(|c| *c == 92).any(|part| {
            part.is_empty()
                || part == [46]
                || part == [46, 46]
                || part.iter().any(|c| matches!(*c, 0 | 47 | 58))
        }) {
            return Err(SourceError::Refused);
        }
        result.push(Change::Name { action, name });
        if next == 0 {
            return Ok(result);
        }
        if next % 4 != 0 || next < 12 + len || next >= bytes.len() - at {
            return Err(SourceError::Refused);
        }
        at += next;
    }
}

/// 100 ns ticks of slack below `since`: a real change just after it can carry a timestamp that is one
/// system-clock tick (about 16 ms) older.
#[cfg(windows)]
const SINCE_SLACK: i64 = 1_000_000;

/// Drop FILE_ACTION_MODIFIED hints that only report an access-time update (#717).
///
/// NTFS delivers a last-access change under the last-write filter bit, so reading or listing a directory
/// raises MODIFIED for it, and a scan then dirtied its own root. An access-time update never changes
/// ChangeTime, so a MODIFIED target whose ChangeTime precedes `since` (FILETIME, the start of a scan that
/// listed everything before it) was only read. Every other hint, and any target that cannot be pinned
/// below `root` without following a reparse point, is kept; at most 64 distinct names are checked.
#[cfg(windows)]
pub fn without_access_only(
    root: &std::path::Path,
    changes: Vec<Change>,
    since: i64,
) -> Vec<Change> {
    use std::collections::BTreeMap;
    let Ok(root) = crate::enumerate::open_readonly_directory(root) else {
        return changes;
    };
    let since = since.saturating_sub(SINCE_SLACK);
    let mut seen: BTreeMap<Vec<u16>, bool> = BTreeMap::new();
    let mut quiet = |name: &[u16]| -> bool {
        if let Some(known) = seen.get(name) {
            return *known;
        }
        let known = seen.len() < 64
            && (|| -> std::io::Result<bool> {
                let mut walk: Option<std::fs::File> = None;
                for part in name.split(|c| *c == 92) {
                    let parent = walk.as_ref().unwrap_or(&root);
                    walk = Some(crate::enumerate::open_metadata_child(parent, part)?);
                }
                let observed = crate::identity::from_file(walk.as_ref().unwrap())?;
                Ok(observed.change_time.parse::<i64>().is_ok_and(|t| t < since))
            })()
            .unwrap_or(false);
        seen.insert(name.to_vec(), known);
        known
    };
    changes
        .into_iter()
        .filter(|c| !matches!(c, Change::Name { action: 3, name } if quiet(name)))
        .collect()
}

#[cfg(windows)]
mod native {
    use super::*;
    use crate::enumerate::{expected_identity_matches, open_watch_root, OpenedIdentity};
    use std::{
        fs::File,
        mem::zeroed,
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
        path::Path,
        ptr::{null, null_mut},
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc, Arc, Mutex,
        },
        thread::{self, JoinHandle},
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{
        Foundation::*,
        Storage::FileSystem::*,
        System::{
            Threading::*,
            IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
        },
    };

    const BUFFER_WORDS: usize = 64 * 1024 / 4;
    const FILTER: u32 = FILE_NOTIFY_CHANGE_FILE_NAME
        | FILE_NOTIFY_CHANGE_DIR_NAME
        | FILE_NOTIFY_CHANGE_SIZE
        | FILE_NOTIFY_CHANGE_LAST_WRITE
        | FILE_NOTIFY_CHANGE_ATTRIBUTES
        | FILE_NOTIFY_CHANGE_SECURITY;

    #[derive(Default)]
    struct Checkpoint {
        held: Option<mpsc::SyncSender<()>>,
    }
    impl Checkpoint {
        fn request(
            &mut self,
            reply: mpsc::SyncSender<()>,
            completed: impl FnOnce() -> Result<bool, SourceError>,
        ) -> Result<bool, SourceError> {
            if completed()? {
                self.held = Some(reply);
                Ok(true)
            } else {
                let _ = reply.send(());
                Ok(false)
            }
        }
        fn deliver(&mut self, hints: Vec<Change>, callback: &impl Fn(Vec<Change>)) {
            callback(hints);
            if let Some(reply) = self.held.take() {
                let _ = reply.send(());
            }
        }
        fn poll(
            &mut self,
            requests: &mpsc::Receiver<mpsc::SyncSender<()>>,
            completed: impl FnOnce() -> Result<bool, SourceError>,
            validate: impl FnOnce() -> Result<(), SourceError>,
        ) -> Result<bool, SourceError> {
            // Take the request even when every overlapped read completes immediately.
            if let Ok(reply) = requests.try_recv() {
                validate()?;
                self.request(reply, completed)
            } else {
                completed()
            }
        }
    }

    fn validate_path(path: &Path, expected: OpenedIdentity) -> Result<(), SourceError> {
        let (_, _, identity) = open_watch_root(path)?;
        expected_identity_matches(identity, expected)
    }

    struct PendingRead<'a> {
        dir: &'a File,
        buffer: &'a mut [u32],
        ov: &'a mut OVERLAPPED,
        pending: bool,
        error: Option<u32>,
    }
    impl<'a> PendingRead<'a> {
        fn start(
            dir: &'a File,
            buffer: &'a mut [u32],
            ov: &'a mut OVERLAPPED,
        ) -> Result<Self, SourceError> {
            let mut read = Self {
                dir,
                buffer,
                ov,
                pending: false,
                error: None,
            };
            let ok = unsafe {
                ReadDirectoryChangesW(
                    dir.as_raw_handle(),
                    read.buffer.as_mut_ptr().cast(),
                    (read.buffer.len() * 4) as u32,
                    1,
                    FILTER,
                    null_mut(),
                    read.ov,
                    None,
                )
            };
            read.error = (ok == 0).then(|| unsafe { GetLastError() });
            read.pending = ok != 0 || read.error == Some(ERROR_IO_PENDING);
            if read
                .error
                .is_some_and(|e| e != ERROR_IO_PENDING && e != ERROR_NOTIFY_ENUM_DIR)
            {
                return Err(SourceError::Io(read.error.unwrap() as i32));
            }
            Ok(read)
        }
        fn finish(&mut self) -> (Option<u32>, usize) {
            if self.error == Some(ERROR_NOTIFY_ENUM_DIR) {
                return (self.error, 0);
            }
            let mut returned = 0;
            let ok =
                unsafe { GetOverlappedResult(self.dir.as_raw_handle(), self.ov, &mut returned, 0) };
            let error = (ok == 0).then(|| unsafe { GetLastError() });
            if error != Some(ERROR_IO_INCOMPLETE) {
                self.pending = false;
            }
            (error, returned as usize)
        }
    }
    impl Drop for PendingRead<'_> {
        fn drop(&mut self) {
            if self.pending {
                let mut returned = 0;
                // CancelIoEx is only a request; keep the buffer, OVERLAPPED and handle alive until completion.
                unsafe {
                    CancelIoEx(self.dir.as_raw_handle(), self.ov);
                    GetOverlappedResult(self.dir.as_raw_handle(), self.ov, &mut returned, 1);
                }
            }
        }
    }

    /// One 64 KiB recursive watcher. Start returns only after its first I/O request is armed.
    /// The callback must only enqueue/coalesce hints: it must not enumerate or write a database.
    pub struct Watcher {
        done: Arc<AtomicBool>,
        failed: Arc<AtomicBool>,
        thread: Mutex<Option<JoinHandle<()>>>,
        checkpoints: mpsc::SyncSender<mpsc::SyncSender<()>>,
    }
    impl Watcher {
        /// Open a non-elevated root with component-wise no-recall, reparse-refusing pins.
        /// A supplied identity is compared with the opened handle before any watch is armed.
        pub fn start(
            path: &Path,
            expected: Option<OpenedIdentity>,
            callback: impl Fn(Vec<Change>) + Send + 'static,
        ) -> Result<Self, SourceError> {
            let (pins, dir, identity) = open_watch_root(path)?;
            if let Some(expected) = expected {
                expected_identity_matches(identity, expected)?;
            }
            let done = Arc::new(AtomicBool::new(false));
            let stopped = done.clone();
            let failed = Arc::new(AtomicBool::new(false));
            let failure = failed.clone();
            let path = path.to_owned();
            let (ready_tx, ready_rx) = mpsc::sync_channel(1);
            let (checkpoints, checkpoint_rx) = mpsc::sync_channel::<mpsc::SyncSender<()>>(1);
            let thread = thread::spawn(move || {
                let _pins = pins;
                let outcome = (|| -> Result<(), SourceError> {
                    let raw = unsafe { CreateEventW(null(), 0, 0, null()) };
                    if raw.is_null() {
                        return Err(SourceError::Io(unsafe { GetLastError() } as i32));
                    }
                    let event = unsafe { OwnedHandle::from_raw_handle(raw) };
                    let mut buffer = vec![0u32; BUFFER_WORDS];
                    let mut previous = None;
                    let mut checkpoint = Checkpoint::default();
                    let mut checked = Instant::now();
                    let mut first = true;
                    loop {
                        if stopped.load(Ordering::Acquire) {
                            return Ok(());
                        }
                        let mut ov: OVERLAPPED = unsafe { zeroed() };
                        ov.hEvent = event.as_raw_handle();
                        let mut pending = PendingRead::start(&dir, &mut buffer, &mut ov)?;
                        if first {
                            ready_tx.send(Ok(())).map_err(|_| SourceError::Refused)?;
                            first = false;
                        }
                        // Rearm before decoding/delivering the previous completion.
                        if let Some(hints) = previous.take() {
                            checkpoint.deliver(hints, &callback);
                        }
                        let (error, returned) = loop {
                            if stopped.load(Ordering::Acquire) {
                                return Ok(());
                            }
                            // A handle can survive a root/ancestor rename without a notification.
                            if checked.elapsed() >= Duration::from_millis(250) {
                                validate_path(&path, identity)?;
                                checked = Instant::now();
                            }
                            if checkpoint.poll(
                                &checkpoint_rx,
                                || {
                                    if pending.error == Some(ERROR_NOTIFY_ENUM_DIR) {
                                        return Ok(true);
                                    }
                                    match unsafe { WaitForSingleObject(event.as_raw_handle(), 0) } {
                                        WAIT_OBJECT_0 => Ok(true),
                                        WAIT_TIMEOUT => Ok(false),
                                        _ => Err(SourceError::Io(unsafe { GetLastError() } as i32)),
                                    }
                                },
                                || validate_path(&path, identity),
                            )? {
                                break pending.finish();
                            }
                            match unsafe { WaitForSingleObject(event.as_raw_handle(), 10) } {
                                WAIT_OBJECT_0 => break pending.finish(),
                                WAIT_TIMEOUT => (),
                                _ => return Err(SourceError::Io(unsafe { GetLastError() } as i32)),
                            }
                        };
                        previous = Some(if overflow(error, returned) {
                            vec![Change::RootDirty]
                        } else if let Some(code) = error {
                            return Err(SourceError::Io(code as i32));
                        } else if returned > pending.buffer.len() * 4 {
                            vec![Change::RootDirty]
                        } else {
                            let raw = unsafe {
                                std::slice::from_raw_parts(
                                    pending.buffer.as_ptr().cast::<u8>(),
                                    returned,
                                )
                            };
                            decode(raw).unwrap_or_else(|_| vec![Change::RootDirty])
                        });
                    }
                })();
                if let Err(error) = outcome {
                    failure.store(true, Ordering::Release);
                    let _ = ready_tx.try_send(Err(error));
                    callback(vec![Change::RootDirty]);
                }
            });
            let watcher = Self {
                done,
                failed,
                thread: Mutex::new(Some(thread)),
                checkpoints,
            };
            match ready_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(Ok(())) => Ok(watcher),
                Ok(Err(error)) => Err(error),
                Err(_) => Err(SourceError::Refused),
            }
        }
        /// Drain already-completed hints before taking a reconciliation epoch.
        pub fn checkpoint(&self) -> Result<(), SourceError> {
            let (tx, rx) = mpsc::sync_channel(1);
            self.checkpoints
                .try_send(tx)
                .map_err(|_| SourceError::Refused)?;
            rx.recv_timeout(Duration::from_secs(2))
                .map_err(|_| SourceError::Refused)?;
            if self.failed() {
                return Err(SourceError::Refused);
            }
            Ok(())
        }
        /// Native I/O or the granted path lost coverage; the next scan must rebuild this watch.
        pub fn failed(&self) -> bool {
            self.failed.load(Ordering::Acquire)
        }
        /// Stop and join, cancelling and draining pending I/O before releasing root pins.
        pub fn stop(&self) {
            self.done.store(true, Ordering::Release);
            if let Some(thread) = self.thread.lock().unwrap().take() {
                let _ = thread.join();
            }
        }
    }
    impl Drop for Watcher {
        fn drop(&mut self) {
            self.stop();
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn continuous_completions_do_not_starve_checkpoint() {
            let mut checkpoint = Checkpoint::default();
            let (requests, receiver) = mpsc::sync_channel(1);
            for ready in [true, true, true, true, false] {
                let (reply, ack) = mpsc::sync_channel(1);
                requests.send(reply).unwrap();
                let validated = std::cell::Cell::new(false);
                assert_eq!(
                    checkpoint
                        .poll(
                            &receiver,
                            || Ok(ready),
                            || {
                                validated.set(true);
                                Ok(())
                            },
                        )
                        .unwrap(),
                    ready
                );
                assert!(
                    validated.get(),
                    "a ready completion must not bypass the request"
                );
                if ready {
                    assert!(matches!(ack.try_recv(), Err(mpsc::TryRecvError::Empty)));
                    let delivered = std::cell::Cell::new(false);
                    checkpoint.deliver(vec![Change::RootDirty], &|hints| {
                        assert_eq!(hints, vec![Change::RootDirty]);
                        assert!(
                            ack.try_recv().is_err(),
                            "hints must precede acknowledgement"
                        );
                        delivered.set(true);
                    });
                    assert!(delivered.get());
                }
                ack.try_recv().unwrap();
            }
        }
        #[test]
        fn completion_between_poll_and_request_is_delivered_before_ack() {
            let mut checkpoint = Checkpoint::default();
            let (tx, rx) = mpsc::sync_channel(1);
            let completion = std::cell::Cell::new(false);
            assert!(!completion.get()); // the first zero wait timed out
            completion.set(true); // completion arrives just before taking the request
            assert!(checkpoint.request(tx, || Ok(completion.get())).unwrap());
            assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
            let delivered = std::cell::Cell::new(false);
            let callback = |hints| {
                assert_eq!(hints, vec![Change::RootDirty]);
                assert!(rx.try_recv().is_err());
                delivered.set(true);
            };
            checkpoint.deliver(vec![Change::RootDirty], &callback);
            rx.try_recv().unwrap();
            assert!(delivered.get());
            let (tx, rx) = mpsc::sync_channel(1);
            assert!(!checkpoint.request(tx, || Ok(false)).unwrap());
            rx.try_recv().unwrap();
        }

        #[test]
        fn armed_watch_allows_root_delete_and_fails_closed() {
            let tree = fixture();
            let root = tree.path().join("root");
            std::fs::create_dir(&root).unwrap();
            if elevated_refusal(&root) {
                return;
            }
            let (tx, rx) = mpsc::channel();
            let watcher = Watcher::start(&root, None, move |batch| {
                let _ = tx.send(batch);
            })
            .unwrap();
            std::fs::remove_dir(&root).unwrap();
            assert!(rx
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .contains(&Change::RootDirty));
            assert!(watcher.failed());
            watcher.stop();
        }

        #[test]
        fn filter_covers_attributes_and_security() {
            assert_ne!(FILTER & FILE_NOTIFY_CHANGE_ATTRIBUTES, 0);
            assert_ne!(FILTER & FILE_NOTIFY_CHANGE_SECURITY, 0);
        }
        fn fixture() -> tempfile::TempDir {
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap()
        }
        fn elevated_refusal(path: &Path) -> bool {
            if !crate::enumerate::running_elevated() {
                return false;
            }
            assert!(Watcher::start(path, None, |_| ()).is_err());
            true
        }
        #[test]
        fn native_fixture_latency_and_continuous_drain() {
            use std::time::Instant;
            let tree = fixture();
            if elevated_refusal(tree.path()) {
                return;
            }
            let (tx, rx) = mpsc::channel();
            let watcher = Watcher::start(tree.path(), None, move |batch| {
                tx.send((Instant::now(), batch)).unwrap();
            })
            .unwrap();
            let mut latency = Vec::new();
            let mut overflows = 0;
            for i in 0..50 {
                let name = format!("latency-{i}");
                let expected: Vec<u16> = name.encode_utf16().collect();
                let started = Instant::now();
                File::create(tree.path().join(name)).unwrap();
                loop {
                    let (observed, batch) = rx.recv_timeout(Duration::from_secs(2)).unwrap();
                    overflows += batch.iter().filter(|c| **c == Change::RootDirty).count();
                    if batch
                        .iter()
                        .any(|c| matches!(c, Change::Name { action: 1, name } if *name == expected))
                    {
                        latency.push(observed.duration_since(started).as_secs_f64() * 1000.0);
                        break;
                    }
                }
            }
            watcher.checkpoint().unwrap();
            watcher.stop();
            assert_eq!(overflows, 0);
            latency.sort_by(f64::total_cmp);
            eprintln!("64 KiB native fixture: 50 creates, overflow={overflows}, latency p50/p95/max ms={:.3}/{:.3}/{:.3}",
                latency[24], latency[47], latency[49]);
        }
        #[test]
        fn stalled_native_consumer_reports_real_overflow() {
            use std::time::Instant;
            let tree = fixture();
            if elevated_refusal(tree.path()) {
                return;
            }
            let (first_tx, first_rx) = mpsc::sync_channel(1);
            let (release_tx, release_rx) = mpsc::sync_channel(1);
            let (tx, rx) = mpsc::channel();
            let first = AtomicBool::new(true);
            let watcher = Watcher::start(tree.path(), None, move |batch| {
                if first.swap(false, Ordering::AcqRel) {
                    first_tx.send(()).unwrap();
                    if release_rx.recv().is_err() {
                        return;
                    }
                }
                tx.send(batch).unwrap();
            })
            .unwrap();
            // Drop the release sender before joining the watcher even if generation panics.
            let release = release_tx;
            File::create(tree.path().join("prime")).unwrap();
            first_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            let started = Instant::now();
            for i in 0..10_000 {
                File::create(tree.path().join(format!("burst-{i}"))).unwrap();
            }
            let generated_ms = started.elapsed().as_secs_f64() * 1000.0;
            release.send(()).unwrap();
            let mut name_hints = 0;
            loop {
                let batch = rx.recv_timeout(Duration::from_secs(2)).unwrap();
                name_hints += batch
                    .iter()
                    .filter(|c| matches!(c, Change::Name { .. }))
                    .count();
                if batch.contains(&Change::RootDirty) {
                    break;
                }
            }
            watcher.checkpoint().unwrap();
            watcher.stop();
            eprintln!("64 KiB stalled native fixture (shared-host): 10000 creates, generation ms={generated_ms:.3}, root invalidated, name hints before overflow={name_hints}, detect since burst start ms={:.3}",
                started.elapsed().as_secs_f64() * 1000.0);
        }
        #[test]
        fn pending_read_early_exit_drains_before_storage_is_reused() {
            let tree =
                tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
            if crate::enumerate::running_elevated() {
                assert!(open_watch_root(tree.path()).is_err());
                return;
            }
            let (_pins, dir, _) = open_watch_root(tree.path()).unwrap();
            let raw = unsafe { CreateEventW(null(), 1, 0, null()) };
            assert!(!raw.is_null());
            let event = unsafe { OwnedHandle::from_raw_handle(raw) };
            let mut ov: OVERLAPPED = unsafe { zeroed() };
            ov.hEvent = event.as_raw_handle();
            let mut buffer = vec![0u32; BUFFER_WORDS];
            let pending = PendingRead::start(&dir, &mut buffer, &mut ov).unwrap();
            assert!(pending.pending);
            drop(pending);
            assert_eq!(
                unsafe { WaitForSingleObject(event.as_raw_handle(), 0) },
                WAIT_OBJECT_0
            );
            let mut returned = 0;
            assert_eq!(
                unsafe { GetOverlappedResult(dir.as_raw_handle(), &ov, &mut returned, 0) },
                0
            );
            assert_eq!(unsafe { GetLastError() }, ERROR_OPERATION_ABORTED);
            buffer.fill(0); // storage is safe to reuse only after the cancelled completion.
        }
    }
}
#[cfg(windows)]
pub use native::Watcher;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn both_overflows_discard_history() {
        assert!(overflow(Some(1022), 123));
        assert!(overflow(None, 0));
        assert!(!overflow(Some(5), 0));
        assert!(!overflow(None, 12));
    }
    #[test]
    fn exact_utf16_and_checked_notification_chain() {
        let mut bytes = Vec::new();
        for word in [0u32, 4, 2] {
            bytes.extend(word.to_le_bytes());
        }
        bytes.extend(0xd800u16.to_le_bytes());
        assert_eq!(
            decode(&bytes).unwrap(),
            vec![Change::Name {
                action: 4,
                name: vec![0xd800]
            }]
        );
        for at in [0, 4, 8] {
            let mut bad = bytes.clone();
            bad[at..at + 4].copy_from_slice(&1u32.to_le_bytes());
            if at == 4 {
                bad[at..at + 4].copy_from_slice(&6u32.to_le_bytes());
            }
            assert!(decode(&bad).is_err());
        }
        bytes[12..14].copy_from_slice(&92u16.to_le_bytes());
        assert!(decode(&bytes).is_err());
    }
}

#[cfg(all(test, windows))]
mod access_only_tests {
    use super::*;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn filetime_now() -> i64 {
        (SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            / 100) as i64
            + 116_444_736_000_000_000
    }
    fn modified(name: &str) -> Change {
        Change::Name {
            action: 3,
            name: name.encode_utf16().collect(),
        }
    }
    fn fixture() -> tempfile::TempDir {
        tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap()
    }

    #[test]
    fn modified_hints_not_changed_since_the_scan_began_are_access_updates() {
        if crate::enumerate::running_elevated() {
            return;
        }
        let tree = fixture();
        std::fs::create_dir(tree.path().join("sub")).unwrap();
        let file = tree.path().join("sub").join("a.bin");
        std::fs::write(&file, b"x").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let since = filetime_now();
        let kept = Change::Name {
            action: 1,
            name: "sub".encode_utf16().collect(),
        };
        let hints = vec![
            modified("sub"),
            modified(r"sub\a.bin"),
            kept.clone(),
            modified("missing"),
            Change::RootDirty,
        ];
        // Nothing changed after `since`: both MODIFIED targets were only read. A created, vanished or
        // overflow hint is never an access update.
        assert_eq!(
            without_access_only(tree.path(), hints.clone(), since),
            vec![kept.clone(), modified("missing"), Change::RootDirty]
        );
        // A real attribute change after `since` moves ChangeTime but not LastWriteTime.
        let mut permissions = std::fs::metadata(&file).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&file, permissions).unwrap();
        let kept_after = without_access_only(tree.path(), hints.clone(), since);
        assert!(kept_after.contains(&modified(r"sub\a.bin")));
        assert!(!kept_after.contains(&modified("sub")));
        // A scan that began before the fixture existed makes every hint real.
        assert_eq!(
            without_access_only(tree.path(), hints.clone(), since - 600_000_000),
            hints
        );
    }

    #[test]
    fn listing_a_directory_raises_only_hints_the_filter_drops() {
        if crate::enumerate::running_elevated() {
            return;
        }
        let tree = fixture();
        let sub = tree.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("a.bin"), b"x").unwrap();
        std::thread::sleep(Duration::from_millis(1500)); // creation hints and their lazy flush
        let (tx, rx) = std::sync::mpsc::channel();
        let watcher = Watcher::start(tree.path(), None, move |batch| {
            let _ = tx.send(batch);
        })
        .unwrap();
        let since = filetime_now();
        std::thread::sleep(Duration::from_millis(100));
        let _ = std::fs::read_dir(&sub).unwrap().count();
        let _ = std::fs::read(sub.join("a.bin")).unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        watcher.checkpoint().unwrap();
        watcher.stop();
        let hints: Vec<Change> = rx.try_iter().flatten().collect();
        // Hosts with last-access updates disabled raise nothing; where enabled, NTFS reports
        // the directory as MODIFIED (#717) and the filter must drop it.
        eprintln!("hints raised by reads: {hints:?}");
        assert!(without_access_only(tree.path(), hints, since).is_empty());
    }
}
