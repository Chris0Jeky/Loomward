use super::*;
use crate::{
    enumerate::{self, FileIdObs, SourceError},
    identity::{self, ObservedIdentity},
    win::Handle,
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io,
    mem::ManuallyDrop,
    os::windows::{ffi::OsStrExt, io::AsRawHandle},
    path::Component,
    ptr,
};
use windows_sys::Win32::{
    Foundation::*,
    Storage::FileSystem::*,
    System::{Threading::*, IO::*},
};

const BLOCK: usize = 64 * 1024;
const SHARE: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;

pub(super) fn non_elevated() -> Result<(), Reason> {
    crate::win::non_elevated().map_err(|e| {
        if e.kind() == io::ErrorKind::PermissionDenied {
            Reason::Elevated
        } else {
            error(e)
        }
    })
}

pub(super) struct Reader<F> {
    pins: Vec<File>,
    hook: F,
}
struct Opened {
    file: File,
    pins: Vec<File>,
}
fn error(error: io::Error) -> Reason {
    match error.raw_os_error() {
        Some(32 | 33) => Reason::SharingViolation,
        Some(5) => Reason::AccessDenied,
        Some(2 | 3) => Reason::ChangedDuringRead,
        _ => Reason::Io,
    }
}
fn source(error: SourceError) -> Reason {
    match error {
        SourceError::Refused | SourceError::IdentityChanged => Reason::ChangedDuringRead,
        SourceError::AccessDenied => Reason::AccessDenied,
        // NtCreateFile returns NTSTATUS rather than Win32 codes.
        SourceError::Io(code) if code as u32 == 0xc0000043 => Reason::SharingViolation,
        SourceError::Io(code) if matches!(code as u32, 0xc0000034 | 0xc000003a) => {
            Reason::ChangedDuringRead
        }
        _ => Reason::Io,
    }
}
fn observe(file: &File) -> Result<ObservedIdentity, Reason> {
    let borrowed = ManuallyDrop::new(Handle(file.as_raw_handle()));
    identity::from_handle(&borrowed).map_err(error)
}
fn signature(current: &ObservedIdentity, expected: &ObservedIdentity) -> bool {
    current.object == expected.object
        && current.size == expected.size
        && current.last_write_time == expected.last_write_time
        && current.change_time == expected.change_time
        && current.attributes == expected.attributes
        && current.link_count == expected.link_count
        && current.ancestors == expected.ancestors
}
impl<F: FnMut(Event, usize)> Reader<F> {
    pub(super) fn new(root: &Path, expected: &OpenedIdentity, hook: F) -> Result<Self, Reason> {
        non_elevated()?;
        let pins = enumerate::root_handles(
            root,
            FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
        )
        .map_err(source)?;
        let current = observe(pins.last().unwrap())?;
        if expected.id != Some(FileIdObs::Id128(current.object.file_id))
            || expected.volume_serial.map(|s| s.to_string()).as_ref()
                != Some(&current.object.volume_serial)
        {
            return Err(Reason::ChangedDuringRead);
        }
        Ok(Self { pins, hook })
    }
    fn open(&self, entry: &Entry, content: bool, meter: &Meter) -> Result<Opened, Reason> {
        meter.check()?;
        self.open_relative(&entry.relative, Some(entry), content, meter)
    }
    fn open_relative(
        &self,
        relative: &Path,
        expected: Option<&Entry>,
        content: bool,
        meter: &Meter,
    ) -> Result<Opened, Reason> {
        let components: Vec<_> = relative.components().collect();
        if components.is_empty()
            || components.iter().any(|c| match c {
                Component::Normal(name) => {
                    name.encode_wide().any(|c| matches!(c, 0 | 58))
                        || name.encode_wide().count() > 32767
                }
                _ => true,
            })
        {
            return Err(Reason::InvalidPath);
        }
        let mut ancestors = self
            .pins
            .iter()
            .map(observe)
            .map(|r| r.map(|i| i.object))
            .collect::<Result<Vec<_>, _>>()?;
        let mut pins = Vec::new();
        for component in &components[..components.len() - 1] {
            meter.check()?;
            let Component::Normal(name) = component else {
                unreachable!()
            };
            let parent = pins.last().unwrap_or_else(|| self.pins.last().unwrap());
            let dir = enumerate::open_relative_mode(
                parent,
                &name.encode_wide().collect::<Vec<_>>(),
                FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                false,
            )
            .map_err(source)?;
            let current = observe(&dir)?;
            if enumerate::refused_attributes(current.attributes)
                || current.attributes & FILE_ATTRIBUTE_DIRECTORY == 0
            {
                return Err(Reason::ChangedDuringRead);
            }
            ancestors.push(current.object);
            pins.push(dir);
        }
        if expected.is_some_and(|entry| ancestors != entry.expected.ancestors) {
            return Err(Reason::ChangedDuringRead);
        }
        let Component::Normal(name) = components.last().unwrap() else {
            unreachable!()
        };
        let name: Vec<_> = name.encode_wide().collect();
        let parent = pins.last().unwrap_or_else(|| self.pins.last().unwrap());
        // Metadata first: no content handle is opened for a refused class.
        let metadata = enumerate::open_relative_mode(
            parent,
            &name,
            FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            SHARE,
            false,
        )
        .map_err(source)?;
        if let Some(entry) = expected {
            self.verify(&metadata, &ancestors, entry)?;
        } else {
            attributes(observe(&metadata)?.attributes)?;
        }
        let file = if content {
            enumerate::open_relative_mode(
                parent,
                &name,
                FILE_READ_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                FILE_SHARE_READ | FILE_SHARE_DELETE,
                true,
            )
            .map_err(source)?
        } else {
            metadata
        };
        if let Some(entry) = expected {
            self.verify(&file, &ancestors, entry)?;
        }
        Ok(Opened { file, pins })
    }
    fn verify(
        &self,
        file: &File,
        ancestors: &[crate::identity::ObjectKey],
        entry: &Entry,
    ) -> Result<(), Reason> {
        if unsafe { GetFileType(file.as_raw_handle()) } != FILE_TYPE_DISK {
            return Err(Reason::SpecialFile);
        }
        let mut current = observe(file)?;
        attributes(current.attributes)?;
        current.ancestors = ancestors.to_vec();
        if !signature(&current, &entry.expected) {
            return Err(Reason::ChangedDuringRead);
        }
        Ok(())
    }
    fn finish(&self, opened: &Opened, entry: &Entry, meter: &Meter) -> Result<(), Reason> {
        let ancestors = self
            .pins
            .iter()
            .chain(&opened.pins)
            .map(observe)
            .map(|r| r.map(|i| i.object))
            .collect::<Result<Vec<_>, _>>()?;
        self.verify(&opened.file, &ancestors, entry)?;
        // Reopen the name through pinned parents: a replaced name is not the original handle.
        self.open(entry, false, meter)?;
        meter.check()
    }
    fn read(
        &mut self,
        index: usize,
        file: &File,
        offset: u64,
        buffer: &mut [u8],
        meter: &mut Meter,
    ) -> Result<(), Reason> {
        meter.reserve(buffer.len() as u64)?;
        let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if event.is_null() {
            return Err(error(io::Error::last_os_error()));
        }
        let event = Handle(event);
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = event.0;
        overlapped.Anonymous.Anonymous.Offset = offset as u32;
        overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;
        let ok = unsafe {
            ReadFile(
                file.as_raw_handle(),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                ptr::null_mut(),
                &mut overlapped,
            )
        };
        if ok == 0 && unsafe { GetLastError() } != ERROR_IO_PENDING {
            return Err(error(io::Error::last_os_error()));
        }
        if ok == 0 {
            (self.hook)(Event::PendingRead, index);
        }
        let stopped = loop {
            if let Err(reason) = meter.check() {
                break Some(reason);
            }
            let remaining = meter.deadline.saturating_duration_since(Instant::now());
            let millis = remaining.as_millis().min(10) as u32;
            match unsafe { WaitForSingleObject(event.0, millis) } {
                WAIT_OBJECT_0 => break None,
                WAIT_TIMEOUT => {}
                _ => break Some(Reason::Io),
            }
        };
        let mut count = 0;
        if let Some(reason) = stopped {
            // Drain cancelled I/O before dropping its OVERLAPPED, event or buffer.
            unsafe {
                CancelIoEx(file.as_raw_handle(), &overlapped);
                GetOverlappedResult(file.as_raw_handle(), &overlapped, &mut count, 1);
            }
            meter.consumed(count as usize);
            return Err(reason);
        }
        let result =
            unsafe { GetOverlappedResult(file.as_raw_handle(), &overlapped, &mut count, 0) };
        meter.consumed(count as usize);
        if result == 0 {
            return Err(error(io::Error::last_os_error()));
        }
        (self.hook)(Event::Read, index);
        if count as usize != buffer.len() {
            return Err(Reason::ChangedDuringRead);
        }
        meter.check()
    }
}
impl<F: FnMut(Event, usize)> ContentReader for Reader<F> {
    fn validate(&mut self, _: usize, entry: &Entry, meter: &Meter) -> Result<(), Reason> {
        attributes(entry.expected.attributes)?;
        self.open(entry, false, meter)?;
        meter.check()
    }
    fn hash(
        &mut self,
        index: usize,
        entry: &Entry,
        partial: bool,
        meter: &mut Meter,
    ) -> Result<[u8; 32], Reason> {
        (self.hook)(if partial { Event::Partial } else { Event::Full }, index);
        let opened = self.open(entry, true, meter)?;
        let size = entry
            .expected
            .size
            .parse::<u64>()
            .map_err(|_| Reason::Unsupported)?;
        let ranges = if partial {
            let head = size.min(BLOCK as u64);
            vec![
                (0, head),
                (size.max(2 * head) - head, (size - head).min(head)),
            ]
        } else {
            vec![(0, size)]
        };
        let mut digest = Sha256::new();
        let mut buffer = vec![0; BLOCK];
        for (mut offset, mut remaining) in ranges {
            while remaining != 0 {
                let count = remaining.min(BLOCK as u64) as usize;
                self.read(index, &opened.file, offset, &mut buffer[..count], meter)?;
                digest.update(&buffer[..count]);
                offset += count as u64;
                remaining -= count as u64;
            }
        }
        self.finish(&opened, entry, meter)?;
        Ok(digest.finalize().into())
    }
    fn equal(
        &mut self,
        left: usize,
        right: usize,
        entries: &[Entry],
        meter: &mut Meter,
    ) -> Result<bool, Skip> {
        (self.hook)(Event::Compare, right);
        let failed_left = |reason| Skip {
            entry: left,
            reason,
        };
        let failed_right = |reason| Skip {
            entry: right,
            reason,
        };
        let a = self
            .open(&entries[left], true, meter)
            .map_err(failed_left)?;
        let b = self
            .open(&entries[right], true, meter)
            .map_err(failed_right)?;
        let size = entries[left]
            .expected
            .size
            .parse::<u64>()
            .map_err(|_| failed_left(Reason::Unsupported))?;
        let mut offset = 0;
        let mut buffers = [vec![0; BLOCK], vec![0; BLOCK]];
        let mut equal = true;
        while offset < size {
            let count = (size - offset).min(BLOCK as u64) as usize;
            meter.reserve(2 * count as u64).map_err(failed_right)?;
            self.read(left, &a.file, offset, &mut buffers[0][..count], meter)
                .map_err(failed_left)?;
            self.read(right, &b.file, offset, &mut buffers[1][..count], meter)
                .map_err(failed_right)?;
            if buffers[0][..count] != buffers[1][..count] {
                equal = false;
                break;
            }
            offset += count as u64;
        }
        self.finish(&a, &entries[left], meter)
            .map_err(failed_left)?;
        self.finish(&b, &entries[right], meter)
            .map_err(failed_right)?;
        Ok(equal)
    }
}

pub(super) fn observe_candidate(
    root: &Path,
    expected: &OpenedIdentity,
    relative: &Path,
    budget: Budget,
    cancel: Arc<AtomicBool>,
) -> Result<ObservedIdentity, Reason> {
    let reader = Reader::new(root, expected, |_, _| {})?;
    let mut meter = Meter::new(budget);
    meter.cancel = Some(cancel);
    meter.check()?;
    let opened = reader.open_relative(relative, None, false, &meter)?;
    let mut observed = observe(&opened.file)?;
    observed.ancestors = reader
        .pins
        .iter()
        .chain(&opened.pins)
        .map(observe)
        .map(|r| r.map(|o| o.object))
        .collect::<Result<Vec<_>, _>>()?;
    meter.check()?;
    Ok(observed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::System::Pipes::CreateNamedPipeW;

    #[test]
    fn pending_content_read_is_cancelled_and_drained() {
        use std::io::Write;
        let name = format!(
            r"\\.\pipe\loomward-duplicate-cancel-fixture-{}",
            std::process::id()
        );
        let wide: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
        let handle = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                PIPE_ACCESS_INBOUND | FILE_FLAG_OVERLAPPED,
                0,
                1,
                0,
                64,
                0,
                ptr::null(),
            )
        };
        assert_ne!(handle, INVALID_HANDLE_VALUE);
        let file = unsafe { File::from_raw_handle(handle) };
        let mut writer = File::options().write(true).open(name).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut pending = false;
        let mut reader = Reader {
            pins: vec![],
            hook: |event, _| {
                if event == Event::PendingRead {
                    pending = true;
                    cancel.store(true, Ordering::Release);
                }
            },
        };
        let mut meter = Meter::new(Budget {
            bytes: 100,
            time: Duration::from_millis(100),
        });
        meter.cancel = Some(cancel.clone());
        assert_eq!(
            reader.read(0, &file, 0, &mut [0; 4], &mut meter),
            Err(Reason::Cancelled)
        );
        assert!(pending);
        assert_eq!(meter.read, 0);
        // Cancellation drains the old OVERLAPPED; the same handle remains usable.
        writer.write_all(b"same").unwrap();
        let mut reader = Reader {
            pins: vec![],
            hook: |_, _| {},
        };
        let mut buffer = [0; 4];
        reader
            .read(0, &file, 0, &mut buffer, &mut Meter::new(Budget::test()))
            .unwrap();
        assert_eq!(&buffer, b"same");
    }

    #[test]
    fn named_pipe_handle_is_a_visible_special_file_refusal() {
        if crate::enumerate::running_elevated() {
            assert_eq!(
                crate::win::non_elevated().unwrap_err().kind(),
                io::ErrorKind::PermissionDenied
            );
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let entry = Entry {
            relative: "synthetic-pipe".into(),
            expected: identity::observe(temp.path()).unwrap(),
        };
        let name: Vec<_> = format!(
            r"\\.\pipe\loomward-duplicate-fixture-{}",
            std::process::id()
        )
        .encode_utf16()
        .chain(Some(0))
        .collect();
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_INBOUND,
                0,
                1,
                0,
                0,
                0,
                ptr::null(),
            )
        };
        assert_ne!(handle, INVALID_HANDLE_VALUE);
        let file = unsafe { File::from_raw_handle(handle) };
        let reader = Reader {
            pins: vec![],
            hook: |_, _| {},
        };
        assert_eq!(reader.verify(&file, &[], &entry), Err(Reason::SpecialFile));
    }
}
