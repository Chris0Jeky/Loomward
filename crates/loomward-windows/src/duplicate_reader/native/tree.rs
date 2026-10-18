//! C1 observations through the q-10 reader and validated directory source.
use super::*;
use crate::enumerate::{DirSource, Flow, ListOutcome, NativeSource};
use loomward_operations::tree::{Kind, MetadataClass as Class, Node, Observation, Tree};
use std::{collections::BTreeMap, mem::ManuallyDrop, os::windows::ffi::OsStringExt};
use windows_sys::{
    Wdk::Storage::FileSystem::{
        FileAlternateNameInformation, FileEaInformation, NtQueryInformationFile,
    },
    Win32::Security::*,
};

/// Most entries one tree read may list (the root, every listed directory and every listed file).
const MAX_TREE_ENTRIES: usize = 200_000;

/// One entry budget: the root and every listed child, files and directories alike, count once.
/// A request above `MAX_TREE_ENTRIES` is refused as `entry_budget` rather than clamped.
pub(crate) fn read_tree(
    root: &Path,
    expected: &OpenedIdentity,
    budget: Budget,
    max_entries: usize,
    cancel: Arc<AtomicBool>,
) -> Result<(Tree, u64), Reason> {
    read_tree_from(
        &NativeSource::default(),
        root,
        expected,
        budget,
        max_entries,
        cancel,
    )
}

/// `read_tree` over an explicit listing source, so the tests can force a listing class.
fn read_tree_from(
    source: &NativeSource,
    root: &Path,
    expected: &OpenedIdentity,
    budget: Budget,
    max_entries: usize,
    cancel: Arc<AtomicBool>,
) -> Result<(Tree, u64), Reason> {
    let mut meter = Meter::new(budget);
    meter.cancel = Some(cancel);
    let mut tree = Tree::default();
    if let Err(reason) = meter.check() {
        tree.nodes.insert(PathBuf::new(), Err(reason.as_str()));
        return Ok((tree, 0));
    }
    if max_entries == 0 || max_entries > MAX_TREE_ENTRIES {
        tree.nodes.insert(PathBuf::new(), Err("entry_budget"));
        return Ok((tree, 0));
    }
    let mut reader = Reader::new(root, expected, |_, _| {})?;
    let (dir, identity) = source.open_root(root).map_err(super::source)?;
    if identity.id != expected.id
        || identity.volume_serial != expected.volume_serial
        || !matches!(
            identity.basis,
            enumerate::IdBasis::Listed | enumerate::IdBasis::PostOpen
        )
    {
        return Err(Reason::IdentityUnverified);
    }
    let root_security = enumerate::root_handles(
        root,
        READ_CONTROL | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        SHARE,
    )
    .map(|mut files| files.pop().unwrap());
    let mut pending = vec![(PathBuf::new(), dir, root_security)];
    let mut directories = Vec::new();
    let mut reserved = 1;
    while let Some((path, dir, security)) = pending.pop() {
        if let Err(reason) = meter.check() {
            tree.nodes.insert(path, Err(reason.as_str()));
            for (path, _, _) in pending.drain(..) {
                tree.nodes.insert(path, Err(reason.as_str()));
            }
            break;
        }
        let before = observe(dir.file())?;
        let mut node = metadata(dir.file(), &before, security.as_ref().ok(), Kind::Directory);
        let mut children = Vec::new();
        let outcome = source.list(&dir, &mut |raw| {
            if meter.check().is_err() || reserved >= max_entries {
                return Flow::Stop;
            }
            reserved += 1;
            children.push((
                raw.name.to_vec(),
                raw.file_id,
                raw.attributes,
                raw.end_of_file,
                raw.creation,
                raw.last_write,
                raw.change,
            ));
            Flow::Continue
        });
        node.children =
            match outcome {
                ListOutcome::Complete => Ok(()),
                _ => Err(meter.check().err().map(Reason::as_str).unwrap_or(
                    if reserved >= max_entries {
                        "entry_budget"
                    } else {
                        "incomplete_listing"
                    },
                )),
            };
        tree.nodes.insert(path.clone(), Ok(node));
        for (name, id, attributes, size, creation, write, change) in children {
            let relative = path.join(std::ffi::OsString::from_wide(&name));
            let result = (|| {
                meter.check()?;
                if enumerate::refused_attributes(attributes) {
                    return Err(Reason::ReparsePoint);
                }
                let security = enumerate::open_relative_mode(
                    dir.file(),
                    &name,
                    READ_CONTROL | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                    SHARE,
                    false,
                );
                if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                    let raw = enumerate::RawEntry {
                        name: &name,
                        file_id: id,
                        attributes,
                        reparse_tag: None,
                        end_of_file: size,
                        allocation_size: None,
                        creation,
                        last_write: write,
                        change,
                        last_access: None,
                    };
                    let (child, _) = source.open_child(&dir, &raw).map_err(super::source)?;
                    pending.push((relative.clone(), child, security));
                    return Ok(None);
                }
                let opened = reader.open_relative(&relative, None, false, &meter)?;
                let observed = observe(&opened.file)?;
                // The listed ID binds at its own width, as enumerate::opened does: 128-bit against
                // the observed 128-bit ID; 64-bit against the 64-bit query on this handle, never
                // a truncation; no listed ID (Find) falls back to the post-open identity.
                match id {
                    Some(enumerate::FileIdObs::Id128(listed))
                        if listed != observed.object.file_id =>
                    {
                        return Err(Reason::ChangedDuringRead);
                    }
                    Some(enumerate::FileIdObs::Id64(listed)) => {
                        // Non-unique on ReFS (N3): such an ID cannot establish identity.
                        if source.is_refs(&dir) {
                            return Err(Reason::IdentityUnverified);
                        }
                        match legacy_index(&opened.file) {
                            Some(index) if index == listed => {}
                            Some(_) => return Err(Reason::ChangedDuringRead),
                            None => return Err(Reason::IdentityUnverified),
                        }
                    }
                    _ => {}
                }
                if observed.size.parse::<u64>().ok() != Some(size)
                    || observed.attributes != attributes
                    || creation.is_some_and(|t| observed.object.creation_time != t.to_string())
                    || write.is_some_and(|t| observed.last_write_time != t.to_string())
                    || change.is_some_and(|t| observed.change_time != t.to_string())
                {
                    return Err(Reason::ChangedDuringRead);
                }
                let mut node =
                    metadata(&opened.file, &observed, security.as_ref().ok(), Kind::File);
                let mut expected = observed;
                expected.ancestors = reader
                    .pins
                    .iter()
                    .chain(&opened.pins)
                    .map(observe)
                    .map(|o| o.map(|o| o.object))
                    .collect::<Result<_, _>>()?;
                let entry = Entry {
                    relative: relative.clone(),
                    expected,
                };
                node.content = reader
                    .hash(0, &entry, false, &mut meter)
                    .map_err(Reason::as_str);
                node.metadata.insert(
                    Class::NamedStreams,
                    stream_hashes(&mut reader, &opened, &entry, &mut meter),
                );
                reader.finish(&opened, &entry, &meter)?;
                Ok(Some(node))
            })();
            match result {
                Ok(Some(node)) => {
                    tree.nodes.insert(relative, Ok(node));
                }
                Err(reason) => {
                    tree.nodes.insert(relative, Err(reason.as_str()));
                }
                Ok(None) => {}
            }
        }
        directories.push((path, dir, before));
    }
    // A later edit must not turn an earlier listing into evidence of absence.
    for (path, dir, before) in directories {
        if !observe(dir.file()).is_ok_and(|after| signature(&before, &after)) {
            tree.nodes.insert(path, Err("changed_during_read"));
        }
    }
    Ok((tree, meter.read))
}

/// The 64-bit index of an open handle, the query a 64-bit listing is compared with.
fn legacy_index(file: &File) -> Option<u64> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    (unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } != 0)
        .then_some((u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow))
}

fn metadata(file: &File, observed: &ObservedIdentity, security: Option<&File>, kind: Kind) -> Node {
    let mut values: BTreeMap<_, _> = Class::ALL
        .into_iter()
        .map(|class| (class, Err("unsupported_metadata")))
        .collect();
    let mut set = |class, value: Vec<u8>| {
        values.insert(class, Ok(value));
    };
    set(
        Class::CreationTime,
        observed.object.creation_time.as_bytes().into(),
    );
    set(
        Class::LastWriteTime,
        observed.last_write_time.as_bytes().into(),
    );
    set(
        Class::LastAccessTime,
        observed.last_access_time.as_bytes().into(),
    );
    set(Class::ChangeTime, observed.change_time.as_bytes().into());
    set(Class::HardLinks, observed.link_count.to_le_bytes().into());
    let preserved = observed.attributes
        & (FILE_ATTRIBUTE_READONLY
            | FILE_ATTRIBUTE_HIDDEN
            | FILE_ATTRIBUTE_ARCHIVE
            | FILE_ATTRIBUTE_NOT_CONTENT_INDEXED);
    set(Class::PreservedAttributes, preserved.to_le_bytes().into());
    let refused = observed.attributes
        & (FILE_ATTRIBUTE_SYSTEM
            | FILE_ATTRIBUTE_TEMPORARY
            | FILE_ATTRIBUTE_COMPRESSED
            | FILE_ATTRIBUTE_SPARSE_FILE
            | FILE_ATTRIBUTE_ENCRYPTED
            | FILE_ATTRIBUTE_INTEGRITY_STREAM);
    set(
        Class::UnsupportedAttributes,
        if refused == 0 {
            vec![]
        } else {
            refused.to_le_bytes().into()
        },
    );
    set(
        Class::ReparsePoint,
        if observed.object.reparse_tag == 0
            && observed.attributes & FILE_ATTRIBUTE_REPARSE_POINT == 0
        {
            vec![]
        } else {
            observed.object.reparse_tag.to_le_bytes().into()
        },
    );
    values.insert(
        Class::ExtendedAttributes,
        nt_information(file, FileEaInformation, 8).and_then(|v| {
            v.get(..4)
                .map(|b| if b == [0; 4] { vec![] } else { b.to_vec() })
                .ok_or("malformed_metadata")
        }),
    );
    values.insert(
        Class::ShortName,
        nt_information(file, FileAlternateNameInformation, 1024).and_then(|v| {
            if v.len() < 4 {
                return Err("malformed_metadata");
            }
            let len = u32::from_le_bytes(v[..4].try_into().unwrap()) as usize;
            if len % 2 != 0 {
                return Err("malformed_metadata");
            }
            v.get(4..4 + len).map(Vec::from).ok_or("malformed_metadata")
        }),
    );
    values.insert(
        Class::CaseSensitivity,
        if kind == Kind::Directory {
            let borrowed = ManuallyDrop::new(Handle(file.as_raw_handle()));
            crate::win::info::<FILE_CASE_SENSITIVE_INFO>(&borrowed, FileCaseSensitiveInfo)
                .map(|i| i.Flags.to_le_bytes().into())
                .map_err(|_| "case_sensitivity_unknown")
        } else {
            Ok(vec![])
        },
    );
    values.insert(Class::ObjectId, object_id(file));
    if let Some(security) = security {
        if observe(security).is_ok_and(|s| signature(&s, observed)) {
            let result = security_metadata(security);
            values.insert(
                Class::SecurityDescriptor,
                result.as_ref().map(|v| v.0.clone()).map_err(|e| *e),
            );
            values.insert(Class::Owner, result.map(|v| v.1));
        }
    }
    if kind == Kind::Directory {
        values.insert(
            Class::NamedStreams,
            streams(file).and_then(|s| {
                if s.is_empty() {
                    Ok(vec![])
                } else {
                    Err("directory_streams_unsupported")
                }
            }),
        );
    }
    Node {
        kind,
        size: observed.size.parse().map_err(|_| "size_unknown"),
        content: Err("content_not_read"),
        metadata: values,
        children: Ok(()),
    }
}

fn nt_information(file: &File, class: i32, bytes: usize) -> Observation<Vec<u8>> {
    let mut buffer = vec![0u64; bytes.div_ceil(8)];
    let mut status: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
    let result = unsafe {
        NtQueryInformationFile(
            file.as_raw_handle(),
            &mut status,
            buffer.as_mut_ptr().cast(),
            bytes as u32,
            class,
        )
    };
    if result == 0xc0000034u32 as i32 && class == FileAlternateNameInformation {
        return Ok(vec![0; 4]);
    }
    if result < 0 {
        return Err("metadata_unreadable");
    }
    let length = status.Information;
    if length > bytes {
        return Err("malformed_metadata");
    }
    Ok(unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), length) }.to_vec())
}

fn object_id(file: &File) -> Observation<Vec<u8>> {
    use windows_sys::Win32::System::Ioctl::{FILE_OBJECTID_BUFFER, FSCTL_GET_OBJECT_ID};
    let mut data = FILE_OBJECTID_BUFFER::default();
    let mut returned = 0;
    let result = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_GET_OBJECT_ID,
            ptr::null(),
            0,
            (&mut data as *mut FILE_OBJECTID_BUFFER).cast(),
            std::mem::size_of_val(&data) as u32,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if result != 0 {
        Ok(data.ObjectId.into())
    } else if unsafe { GetLastError() } == ERROR_FILE_NOT_FOUND {
        Ok(vec![])
    } else {
        Err("object_id_unknown")
    }
}

pub(crate) fn security_metadata(file: &File) -> Observation<(Vec<u8>, Vec<u8>, bool)> {
    let mut needed = 0;
    let flags = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    unsafe {
        GetKernelObjectSecurity(file.as_raw_handle(), flags, ptr::null_mut(), 0, &mut needed);
    }
    if needed == 0 || needed > 64 * 1024 {
        return Err("security_descriptor_unknown");
    }
    let mut buffer = vec![0u32; (needed as usize).div_ceil(4)];
    let descriptor = buffer.as_mut_ptr().cast();
    if unsafe {
        GetKernelObjectSecurity(file.as_raw_handle(), flags, descriptor, needed, &mut needed)
    } == 0
    {
        return Err("security_descriptor_unreadable");
    }
    let mut owner = ptr::null_mut();
    let mut defaulted = 0;
    let mut dacl = ptr::null_mut();
    let mut present = 0;
    let mut control = 0;
    let mut revision = 0;
    if unsafe { GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted) } == 0
        || owner.is_null()
        || unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted) }
            == 0
        || unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0
    {
        return Err("security_descriptor_unreadable");
    }
    let owner =
        unsafe { std::slice::from_raw_parts(owner.cast::<u8>(), GetLengthSid(owner) as usize) }
            .to_vec();
    // A null or absent DACL does not establish inherited destination permissions.
    let mut explicit = if present == 0 || dacl.is_null() {
        vec![0]
    } else {
        vec![]
    };
    if !dacl.is_null() {
        for index in 0..unsafe { (*dacl).AceCount } {
            let mut ace = ptr::null_mut();
            if unsafe { GetAce(dacl, index as u32, &mut ace) } == 0 {
                return Err("acl_unreadable");
            }
            let header = unsafe { &*ace.cast::<ACE_HEADER>() };
            if u32::from(header.AceFlags) & INHERITED_ACE == 0 {
                explicit.push(header.AceType);
            }
        }
    }
    Ok((explicit, owner, control & SE_DACL_PROTECTED != 0))
}

fn streams(file: &File) -> Observation<BTreeMap<Vec<u16>, u64>> {
    let mut buffer = vec![0u64; 8192];
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileStreamInfo,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 8) as u32,
        )
    } == 0
    {
        // FileStreamInfo documents ERROR_HANDLE_EOF as an empty stream list.
        if unsafe { GetLastError() } == ERROR_HANDLE_EOF {
            return Ok(BTreeMap::new());
        }
        return Err("streams_unreadable");
    }
    let bytes =
        unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), buffer.len() * 8) };
    let mut found = BTreeMap::new();
    let mut offset = 0;
    loop {
        let header = bytes.get(offset..offset + 24).ok_or("malformed_streams")?;
        let next = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
        let length = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
        let size = i64::from_le_bytes(header[8..16].try_into().unwrap());
        if length % 2 != 0 || size < 0 {
            return Err("malformed_streams");
        }
        if length == 0 && next == 0 {
            break;
        }
        let name: Vec<_> = bytes
            .get(offset + 24..offset + 24 + length)
            .ok_or("malformed_streams")?
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        if name != "::$DATA".encode_utf16().collect::<Vec<_>>() {
            if found.insert(name, size as u64).is_some() {
                return Err("duplicate_stream");
            }
            if found.len() > 64 {
                return Err("too_many_streams");
            }
        }
        if next == 0 {
            break;
        }
        if next < 24 + length || next % 8 != 0 {
            return Err("malformed_streams");
        }
        offset = offset.checked_add(next).ok_or("malformed_streams")?;
    }
    Ok(found)
}

fn stream_hashes(
    reader: &mut Reader<impl FnMut(Event, usize)>,
    opened: &Opened,
    entry: &Entry,
    meter: &mut Meter,
) -> Observation<Vec<u8>> {
    let mut result = Vec::new();
    let parent = opened
        .pins
        .last()
        .unwrap_or_else(|| reader.pins.last().unwrap())
        .try_clone()
        .map_err(|_| "io")?;
    let name: Vec<_> = entry
        .relative
        .file_name()
        .ok_or("invalid_path")?
        .encode_wide()
        .collect();
    for (stream, size) in streams(&opened.file)? {
        meter.check().map_err(Reason::as_str)?;
        let file = enumerate::open_data_stream(&parent, &name, &stream)
            .map_err(|e| super::source(e).as_str())?;
        let before = observe(&file).map_err(Reason::as_str)?;
        if before.object != entry.expected.object || before.size.parse::<u64>().ok() != Some(size) {
            return Err("changed_during_read");
        }
        let mut digest = Sha256::new();
        let mut buffer = vec![0; BLOCK];
        let mut offset = 0;
        while offset < size {
            let count = (size - offset).min(BLOCK as u64) as usize;
            reader
                .read(0, &file, offset, &mut buffer[..count], meter)
                .map_err(Reason::as_str)?;
            digest.update(&buffer[..count]);
            offset += count as u64;
        }
        if !signature(&before, &observe(&file).map_err(Reason::as_str)?) {
            return Err("changed_during_read");
        }
        result.extend_from_slice(&(stream.len() as u32).to_le_bytes());
        for unit in stream {
            result.extend_from_slice(&unit.to_le_bytes());
        }
        result.extend_from_slice(&size.to_le_bytes());
        result.extend_from_slice(&digest.finalize());
    }
    reader
        .finish(opened, entry, meter)
        .map_err(Reason::as_str)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enumerate::Strategy;
    use loomward_operations::tree::{compare, DifferenceKind, Field};
    fn snapshot(path: &Path) -> Tree {
        let source = NativeSource::default();
        let (_, expected) = source.open_root(path).unwrap();
        read_tree(
            path,
            &expected,
            Budget {
                bytes: 1024 * 1024,
                time: Duration::from_secs(30),
            },
            100,
            Default::default(),
        )
        .unwrap()
        .0
    }
    #[test]
    fn native_lab_trees_find_planted_shape_size_content_and_metadata() {
        if crate::enumerate::running_elevated() {
            return;
        }
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        for root in [a.path(), b.path()] {
            std::fs::create_dir(root.join("folder")).unwrap();
            for name in ["same", "size", "content", "attributes"] {
                std::fs::write(root.join(name), b"aaaa").unwrap();
            }
        }
        std::fs::write(a.path().join("missing"), b"a").unwrap();
        std::fs::write(b.path().join("extra"), b"a").unwrap();
        std::fs::write(a.path().join("shape"), b"a").unwrap();
        std::fs::create_dir(b.path().join("shape")).unwrap();
        std::fs::create_dir(a.path().join("empty-missing")).unwrap();
        std::fs::write(a.path().join("old-name"), b"same").unwrap();
        std::fs::write(b.path().join("new-name"), b"same").unwrap();
        std::fs::write(b.path().join("size"), b"longer").unwrap();
        std::fs::write(b.path().join("content"), b"bbbb").unwrap();
        let mut permissions = std::fs::metadata(b.path().join("attributes"))
            .unwrap()
            .permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(b.path().join("attributes"), permissions).unwrap();
        let report = compare(&snapshot(a.path()), &snapshot(b.path()));
        let found: Vec<_> = report
            .differences
            .iter()
            .filter(|d| {
                !matches!(
                    d.kind,
                    DifferenceKind::Metadata(Class::CreationTime | Class::LastWriteTime)
                )
            })
            .map(|d| (d.path.clone(), d.kind.clone()))
            .collect();
        assert_eq!(
            found,
            vec![
                (
                    "attributes".into(),
                    DifferenceKind::Metadata(Class::PreservedAttributes)
                ),
                ("content".into(), DifferenceKind::Content),
                ("empty-missing".into(), DifferenceKind::Missing),
                ("extra".into(), DifferenceKind::Extra),
                ("missing".into(), DifferenceKind::Missing),
                ("new-name".into(), DifferenceKind::Extra),
                ("old-name".into(), DifferenceKind::Missing),
                ("shape".into(), DifferenceKind::Kind),
                ("size".into(), DifferenceKind::Size),
            ]
        );
        assert!(!report
            .unknowns
            .iter()
            .any(|u| matches!(u.field, Field::Entry | Field::Content | Field::Size)));
    }

    #[test]
    fn native_control_and_named_stream_content_are_read_only() {
        if crate::enumerate::running_elevated() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("fixture");
        std::fs::write(&path, b"aaaa").unwrap();
        let (dir, _) = NativeSource::default().open_root(temp.path()).unwrap();
        for (name, stream) in [
            ("..", ":note:$DATA"),
            ("fixture", "::$DATA"),
            ("fixture", ":../note:$DATA"),
            ("fixture", ":note:other:$DATA"),
            ("fixture", ":note:$INDEX_ALLOCATION"),
        ] {
            assert_eq!(
                enumerate::open_data_stream(
                    dir.file(),
                    &name.encode_utf16().collect::<Vec<_>>(),
                    &stream.encode_utf16().collect::<Vec<_>>()
                )
                .unwrap_err(),
                enumerate::SourceError::Refused
            );
        }
        let mut first = Tree::default();
        let accesses = crate::enumerate::record_open_accesses(|| {
            first = snapshot(temp.path());
        });
        let writes = FILE_WRITE_DATA
            | FILE_APPEND_DATA
            | FILE_WRITE_EA
            | FILE_WRITE_ATTRIBUTES
            | FILE_DELETE_CHILD
            | DELETE
            | WRITE_DAC
            | WRITE_OWNER
            | GENERIC_WRITE
            | GENERIC_ALL;
        assert!(!accesses.is_empty());
        assert!(
            accesses.iter().all(|access| access & writes == 0),
            "{accesses:?}"
        );
        let second = snapshot(temp.path());
        let report = compare(&first, &second);
        assert!(report.equal(), "{report:?}");
        std::fs::write(path.with_file_name("fixture:note"), b"stream-a").unwrap();
        let accesses = crate::enumerate::record_open_accesses(|| {
            first = snapshot(temp.path());
        });
        assert!(
            accesses.iter().all(|access| access & writes == 0),
            "{accesses:?}"
        );
        assert!(
            first.nodes[Path::new("fixture")].as_ref().unwrap().metadata[&Class::NamedStreams]
                .is_ok(),
            "{first:?}"
        );
        std::fs::write(path.with_file_name("fixture:note"), b"stream-b").unwrap();
        let second = snapshot(temp.path());
        let report = compare(&first, &second);
        assert!(
            report
                .differences
                .iter()
                .any(|d| d.path == Path::new("fixture")
                    && d.kind == DifferenceKind::Metadata(Class::NamedStreams)),
            "{report:?}"
        );
        assert_eq!(std::fs::read(path).unwrap(), b"aaaa");
    }

    #[test]
    fn native_budget_stops_and_writer_refusal_stay_unknown() {
        if crate::enumerate::running_elevated() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("fixture"), b"same").unwrap();
        let expected = NativeSource::default().open_root(temp.path()).unwrap().1;
        let read = |bytes, count, cancel| {
            read_tree(
                temp.path(),
                &expected,
                Budget {
                    bytes,
                    time: Duration::from_secs(30),
                },
                count,
                cancel,
            )
            .unwrap()
            .0
        };
        for tree in [
            read(0, 100, Default::default()),
            read(100, 1, Default::default()),
            read(100, 100, Arc::new(AtomicBool::new(true))),
        ] {
            let report = compare(&tree, &tree);
            assert!(!report.equal());
            assert!(!report.unknowns.is_empty());
            assert!(report.differences.is_empty());
        }
        let writer = File::options()
            .write(true)
            .open(temp.path().join("fixture"))
            .unwrap();
        let tree = read(100, 100, Default::default());
        let report = compare(&tree, &tree);
        assert!(!report.equal());
        assert!(
            report
                .unknowns
                .iter()
                .any(|u| u.reason == "sharing_violation"),
            "{report:?}"
        );
        drop(writer);
    }

    fn budget() -> Budget {
        Budget {
            bytes: 1024 * 1024,
            time: Duration::from_secs(30),
        }
    }

    /// Find lists no file ID and Both a 64-bit one; neither may read as `changed_during_read`.
    #[test]
    fn listing_classes_without_a_128_bit_id_still_read_every_file() {
        if crate::enumerate::running_elevated() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("sub")).unwrap();
        std::fs::write(temp.path().join("sub").join("inner"), b"same bytes").unwrap();
        std::fs::write(temp.path().join("outer"), b"other bytes").unwrap();
        let run = |source: &NativeSource| {
            let (_, expected) = source.open_root(temp.path()).unwrap();
            read_tree_from(
                source,
                temp.path(),
                &expected,
                budget(),
                100,
                Default::default(),
            )
            .unwrap()
            .0
        };
        let extended = run(&NativeSource::default());
        for strategy in [Strategy::Find, Strategy::Both] {
            let forced = run(&NativeSource::with_strategy(strategy));
            for file in ["outer", "sub/inner"] {
                let key = PathBuf::from(file);
                let node = forced.nodes[&key].as_ref().unwrap_or_else(|reason| {
                    panic!("{strategy:?} {file}: {reason}");
                });
                assert_eq!(
                    node.content,
                    extended.nodes[&key].as_ref().unwrap().content,
                    "{strategy:?} {file}"
                );
            }
        }
    }

    /// A 64-bit listing on ReFS is non-unique (N3): it binds nothing, so the honest answer is an
    /// unknown `identity_unverified`, never `changed_during_read` and never equal.
    #[test]
    fn both_listing_on_refs_is_identity_unverified_not_changed() {
        if crate::enumerate::running_elevated() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("outer"), b"other bytes").unwrap();
        let source = NativeSource::with_strategy(Strategy::Both).with_filesystem("ReFS");
        let (_, expected) = source.open_root(temp.path()).unwrap();
        let tree = read_tree_from(
            &source,
            temp.path(),
            &expected,
            budget(),
            100,
            Default::default(),
        )
        .unwrap()
        .0;
        assert_eq!(
            tree.nodes[Path::new("outer")].as_ref().err(),
            Some(&"identity_unverified")
        );
    }

    /// One budget counts the root, every listed directory and every listed file. With four entries
    /// the directory `sub` cannot finish; with five the whole tree lists.
    #[test]
    fn entry_budget_counts_directories_and_files_alike() {
        if crate::enumerate::running_elevated() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("sub")).unwrap();
        std::fs::write(temp.path().join("sub").join("a"), b"a").unwrap();
        std::fs::write(temp.path().join("sub").join("b"), b"b").unwrap();
        std::fs::write(temp.path().join("top"), b"t").unwrap();
        let source = NativeSource::default();
        let (_, expected) = source.open_root(temp.path()).unwrap();
        let run = |max| {
            read_tree_from(
                &source,
                temp.path(),
                &expected,
                budget(),
                max,
                Default::default(),
            )
            .unwrap()
            .0
        };
        let whole = run(5);
        assert_eq!(
            whole.nodes[Path::new("sub")].as_ref().unwrap().children,
            Ok(())
        );
        let short = run(4);
        assert_eq!(
            short.nodes[Path::new("sub")].as_ref().unwrap().children,
            Err("entry_budget")
        );
    }

    /// A ceiling is a refusal, not a silent clamp: 200,000 is honoured, one more is refused.
    #[test]
    fn entry_budget_above_the_ceiling_is_refused_not_clamped() {
        if crate::enumerate::running_elevated() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("only"), b"x").unwrap();
        let source = NativeSource::default();
        let (_, expected) = source.open_root(temp.path()).unwrap();
        let run = |max| {
            read_tree_from(
                &source,
                temp.path(),
                &expected,
                budget(),
                max,
                Default::default(),
            )
            .unwrap()
            .0
        };
        assert_eq!(
            run(200_000).nodes[Path::new("")].as_ref().unwrap().children,
            Ok(())
        );
        assert_eq!(
            run(200_001).nodes[Path::new("")].as_ref().err(),
            Some(&"entry_budget")
        );
    }
}
