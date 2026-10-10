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

#[cfg(windows)]
mod native {
    use super::*;
    use crate::enumerate::{open_watch_root, OpenedIdentity};
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
        time::Duration,
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
        | FILE_NOTIFY_CHANGE_LAST_WRITE;

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
            if expected
                .is_some_and(|e| e.id != identity.id || e.volume_serial != identity.volume_serial)
            {
                return Err(SourceError::Refused);
            }
            let done = Arc::new(AtomicBool::new(false));
            let stopped = done.clone();
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
                            callback(hints);
                        }
                        let (error, returned) = if pending.error == Some(ERROR_NOTIFY_ENUM_DIR) {
                            (pending.error, 0)
                        } else {
                            loop {
                                if stopped.load(Ordering::Acquire) {
                                    return Ok(());
                                }
                                match unsafe { WaitForSingleObject(event.as_raw_handle(), 0) } {
                                    WAIT_OBJECT_0 => break pending.finish(),
                                    WAIT_TIMEOUT => {
                                        // At this barrier all already-completed hints have been delivered.
                                        if let Ok(reply) = checkpoint_rx.try_recv() {
                                            let _ = reply.send(());
                                        }
                                        match unsafe {
                                            WaitForSingleObject(event.as_raw_handle(), 10)
                                        } {
                                            WAIT_OBJECT_0 => break pending.finish(),
                                            WAIT_TIMEOUT => (),
                                            _ => {
                                                return Err(SourceError::Io(
                                                    unsafe { GetLastError() } as i32,
                                                ))
                                            }
                                        }
                                    }
                                    _ => {
                                        return Err(SourceError::Io(
                                            unsafe { GetLastError() } as i32
                                        ))
                                    }
                                }
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
                    let _ = ready_tx.try_send(Err(error));
                    callback(vec![Change::RootDirty]);
                }
            });
            let watcher = Self {
                done,
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
                .map_err(|_| SourceError::Refused)
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
                    release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                }
                tx.send(batch).unwrap();
            })
            .unwrap();
            File::create(tree.path().join("prime")).unwrap();
            first_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            let started = Instant::now();
            for i in 0..10_000 {
                File::create(tree.path().join(format!("burst-{i}"))).unwrap();
            }
            release_tx.send(()).unwrap();
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
            eprintln!("64 KiB stalled native fixture: 10000 creates, root invalidated, name hints before overflow={name_hints}, detect since burst start ms={:.3}",
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
