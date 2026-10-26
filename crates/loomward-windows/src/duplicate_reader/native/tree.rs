//! C1 observations through the q-10 reader and validated directory source.
use super::*;
use crate::enumerate::{DirSource, Flow, ListOutcome, NativeSource};
use loomward_operations::tree::{Kind, MetadataClass as Class, Node, Observation, Tree};
use std::{
    collections::BTreeMap,
    mem::ManuallyDrop,
    os::windows::{ffi::OsStringExt, fs::OpenOptionsExt, io::FromRawHandle},
};
use windows_sys::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{
            FileAlternateNameInformation, FileEaInformation, FileIdBothDirectoryInformation,
            NtCreateFile, NtQueryDirectoryFile, NtQueryInformationFile, FILE_OPEN,
            FILE_OPEN_NO_RECALL, FILE_OPEN_REPARSE_POINT,
        },
    },
    Win32::Security::*,
};

/// Most entries one tree read may list (the root, every listed directory and every listed file).
const MAX_TREE_ENTRIES: usize = 200_000;

#[cfg(test)]
thread_local! {
    static ACL_OPEN_ACCESSES: std::cell::RefCell<Option<Vec<u32>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn record_acl_open(access: u32) {
    ACL_OPEN_ACCESSES.with_borrow_mut(|accesses| {
        if let Some(accesses) = accesses {
            accesses.push(access);
        }
    });
}

#[cfg(test)]
fn record_tree_open_accesses(run: impl FnOnce()) -> Vec<u32> {
    ACL_OPEN_ACCESSES.with_borrow_mut(|accesses| *accesses = Some(Vec::new()));
    let mut accesses = enumerate::record_open_accesses(run);
    ACL_OPEN_ACCESSES.with_borrow_mut(|acl| accesses.extend(acl.take().unwrap()));
    accesses
}

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
        crate::noncached::sector_for_file,
    )
}

/// `read_tree` over an explicit listing source and sector-geometry query, so the tests can force
/// a listing class or a volume whose geometry cannot be read.
fn read_tree_from(
    source: &NativeSource,
    root: &Path,
    expected: &OpenedIdentity,
    budget: Budget,
    max_entries: usize,
    cancel: Arc<AtomicBool>,
    geometry: fn(&File) -> io::Result<usize>,
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
    // Some USB bridges and card readers do not answer the alignment query; that is not an I/O fault
    // of any file, so it gets its own reason rather than `io` (#903).
    reader.sector =
        Some(geometry(reader.pins.last().unwrap()).map_err(|_| Reason::GeometryUnavailable)?);
    let (dir, identity) = source.open_root(root).map_err(super::source)?;
    if enumerate::expected_identity_matches(identity, *expected).is_err()
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
    // The root's parent is outside the grant; never enumerate it for an alternate name.
    let root_short = short_name(dir.file(), || Err("metadata_unreadable"));
    // A failed parent read leaves legacy root ACEs unknown, rather than treating them as explicit.
    let root_parent_acl = root_parent_security(root, &observe(dir.file())?)
        .ok()
        .map(|acl| Arc::new(Ok(acl)));
    let mut pending = vec![(
        PathBuf::new(),
        dir,
        root_security,
        root_short,
        root_parent_acl,
    )];
    let mut directories = Vec::new();
    let mut reserved = 1;
    while let Some((path, dir, security, short, parent_acl)) = pending.pop() {
        if let Err(reason) = meter.check() {
            tree.nodes.insert(path, Err(reason.as_str()));
            for (path, _, _, _, _) in pending.drain(..) {
                tree.nodes.insert(path, Err(reason.as_str()));
            }
            break;
        }
        let before = observe(dir.file())?;
        let mut node = metadata(
            dir.file(),
            &before,
            security.as_ref().ok(),
            parent_acl.as_deref(),
            Kind::Directory,
            short,
        );
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
        let mut short_names = None;
        let short_name_limit = children.len();
        let parent_acl = parent_security(dir.file()).ok().map(|a| Arc::new(Ok(a)));
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
                    let short = short_name(child.file(), || {
                        listed_short_name(
                            &dir,
                            child.file(),
                            &name,
                            &mut short_names,
                            &meter,
                            short_name_limit,
                        )
                    });
                    pending.push((relative.clone(), child, security, short, parent_acl.clone()));
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
                let short = short_name(&opened.file, || {
                    listed_short_name(
                        &dir,
                        &opened.file,
                        &name,
                        &mut short_names,
                        &meter,
                        short_name_limit,
                    )
                });
                let mut node = metadata(
                    &opened.file,
                    &observed,
                    security.as_ref().ok(),
                    parent_acl.as_deref(),
                    Kind::File,
                    short,
                );
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

fn metadata(
    file: &File,
    observed: &ObservedIdentity,
    security: Option<&File>,
    parent_acl: Option<&Observation<SecurityDescriptor>>,
    kind: Kind,
    short: Observation<Vec<u8>>,
) -> Node {
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
        nt_information(file, FileEaInformation, 8)
            .map_err(InformationError::reason)
            .and_then(|v| {
                v.get(..4)
                    .map(|b| if b == [0; 4] { vec![] } else { b.to_vec() })
                    .ok_or("malformed_metadata")
            }),
    );
    values.insert(Class::ShortName, short);
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
            let result = read_security_for_parent(security, kind, parent_acl);
            values.insert(
                Class::Owner,
                result.as_ref().map(|v| v.owner.clone()).map_err(|e| *e),
            );
            values.insert(
                Class::SecurityDescriptor,
                result.and_then(|v| explicit_aces(&v, kind, parent_acl)),
            );
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

#[derive(Debug)]
enum InformationError {
    Status(i32),
    Malformed,
}
impl InformationError {
    fn reason(self) -> &'static str {
        match self {
            Self::Status(_) => "metadata_unreadable",
            Self::Malformed => "malformed_metadata",
        }
    }
}

fn short_name(
    file: &File,
    fallback: impl FnOnce() -> Observation<Vec<u8>>,
) -> Observation<Vec<u8>> {
    short_name_information(
        nt_information(file, FileAlternateNameInformation, 1024),
        fallback,
    )
}

fn short_name_information(
    information: Result<Vec<u8>, InformationError>,
    fallback: impl FnOnce() -> Observation<Vec<u8>>,
) -> Observation<Vec<u8>> {
    let value = match information {
        Err(InformationError::Status(code)) if code == 0xc00000bbu32 as i32 => return fallback(),
        result => result.map_err(InformationError::reason)?,
    };
    let header = value.get(..4).ok_or("malformed_metadata")?;
    let len = u32::from_le_bytes(header.try_into().unwrap()) as usize;
    if len % 2 != 0 {
        return Err("malformed_metadata");
    }
    value
        .get(4..4 + len)
        .map(Vec::from)
        .ok_or("malformed_metadata")
}

type ShortNames = BTreeMap<Vec<u16>, (u64, Vec<u8>)>;

fn listed_short_name(
    parent: &enumerate::NativeDir,
    file: &File,
    name: &[u16],
    cache: &mut Option<Observation<ShortNames>>,
    meter: &Meter,
    max_entries: usize,
) -> Observation<Vec<u8>> {
    // A 64-bit directory ID cannot establish identity on ReFS (N3).
    if parent.filesystem().eq_ignore_ascii_case("ReFS") {
        return Err("metadata_unreadable");
    }
    let names =
        cache.get_or_insert_with(|| directory_short_names(parent.file(), meter, max_entries));
    let (listed_id, short) = names
        .as_ref()
        .map_err(|e| *e)?
        .get(name)
        .ok_or("metadata_unreadable")?;
    if *listed_id == 0 || legacy_index(file) != Some(*listed_id) {
        return Err("metadata_unreadable");
    }
    Ok(short.clone())
}

fn directory_short_names(
    parent: &File,
    meter: &Meter,
    max_entries: usize,
) -> Observation<ShortNames> {
    // A used directory handle retains its first search filter. Restart once and select exact
    // native names from a bounded listing, cached per parent rather than rescanned per child.
    let mut buffer = vec![0u64; 2048];
    let mut names = BTreeMap::new();
    let mut first = true;
    let mut entries = 0;
    while names.len() < max_entries {
        meter.check().map_err(|_| "metadata_unreadable")?;
        let mut status: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
        let result = unsafe {
            NtQueryDirectoryFile(
                parent.as_raw_handle(),
                ptr::null_mut(),
                None,
                ptr::null(),
                &mut status,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 8) as u32,
                FileIdBothDirectoryInformation,
                // Do not read ahead past this request's remaining child budget.
                true,
                ptr::null(),
                first,
            )
        };
        if result == 0x80000006u32 as i32 {
            // STATUS_NO_MORE_FILES
            return Ok(names);
        }
        if result != 0 || status.Information == 0 || status.Information > buffer.len() * 8 {
            return Err("metadata_unreadable");
        }
        first = false;
        let bytes =
            unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), status.Information) };
        let mut offset = 0;
        loop {
            meter.check().map_err(|_| "metadata_unreadable")?;
            entries += 1;
            if entries > max_entries.saturating_add(2) {
                // Allow the listing's dot entries.
                return Err("metadata_unreadable");
            }
            // FILE_ID_BOTH_DIR_INFORMATION: fixed header 104 bytes, ShortName at 70, ID at 96.
            let header = bytes
                .get(offset..offset + 104)
                .ok_or("metadata_unreadable")?;
            let next = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
            let length = u32::from_le_bytes(header[60..64].try_into().unwrap()) as usize;
            let short_length = header[68] as usize;
            if length == 0
                || length % 2 != 0
                || short_length > 24
                || short_length % 2 != 0
                || (next != 0 && (next % 8 != 0 || next < 104 + length))
            {
                return Err("metadata_unreadable");
            }
            let name = bytes
                .get(offset + 104..offset + 104 + length)
                .ok_or("metadata_unreadable")?
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect::<Vec<_>>();
            let id = u64::from_le_bytes(header[96..104].try_into().unwrap());
            let short = header[70..70 + short_length].to_vec();
            if name != [46] && name != [46, 46] && names.insert(name, (id, short)).is_some() {
                return Err("metadata_unreadable");
            }
            if next == 0 {
                break;
            }
            offset = offset.checked_add(next).ok_or("metadata_unreadable")?;
        }
    }
    Ok(names)
}

fn nt_information(file: &File, class: i32, bytes: usize) -> Result<Vec<u8>, InformationError> {
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
        return Err(InformationError::Status(result));
    }
    let length = status.Information;
    if length > bytes {
        return Err(InformationError::Malformed);
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

struct SecurityDescriptor {
    owner: Vec<u8>,
    group: Vec<u8>,
    dacl: Option<Vec<Vec<u8>>>,
    control: u16,
}

fn root_parent_security(
    root: &Path,
    expected: &ObservedIdentity,
) -> Observation<SecurityDescriptor> {
    #[cfg(test)]
    if acl_tests::FAIL_ROOT_PARENT_READ.get() {
        return Err("parent_security_descriptor_unreadable");
    }
    // Metadata-only read outside the grant, bound back to the observed root before using its ACL.
    let parent = root
        .parent()
        .ok_or("parent_security_descriptor_unreadable")?;
    let access = READ_CONTROL | FILE_READ_ATTRIBUTES;
    #[cfg(test)]
    record_acl_open(access);
    let file = File::options()
        .access_mode(access)
        .share_mode(SHARE)
        .custom_flags(
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_OPEN_NO_RECALL,
        )
        .open(parent)
        .map_err(|_| "parent_security_descriptor_unreadable")?;
    let borrowed = ManuallyDrop::new(Handle(file.as_raw_handle()));
    let info = crate::win::info::<FILE_ATTRIBUTE_TAG_INFO>(&borrowed, FileAttributeTagInfo)
        .map_err(|_| "parent_security_descriptor_unreadable")?;
    if info.FileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
        != FILE_ATTRIBUTE_DIRECTORY
    {
        return Err("parent_security_descriptor_unreadable");
    }
    let name: Vec<_> = root
        .file_name()
        .ok_or("parent_security_descriptor_unreadable")?
        .encode_wide()
        .collect();
    let child = enumerate::open_relative_mode(
        &file,
        &name,
        READ_CONTROL | FILE_READ_ATTRIBUTES,
        SHARE,
        true,
    )
    .map_err(|_| "parent_security_descriptor_unreadable")?;
    let observed = observe(&child).map_err(|_| "parent_security_descriptor_unreadable")?;
    if observed.object.file_id != expected.object.file_id
        || observed.object.volume_serial != expected.object.volume_serial
        || observed.object.quality != expected.object.quality
    {
        return Err("parent_security_descriptor_unreadable");
    }
    read_security(&file)
}

// Reopen the pinned directory itself: no parent-name lookup and only READ_CONTROL access.
fn parent_security(parent: &File) -> Observation<SecurityDescriptor> {
    let access = READ_CONTROL;
    #[cfg(test)]
    record_acl_open(access);
    let mut name = UNICODE_STRING::default();
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle(),
        ObjectName: &mut name,
        ..Default::default()
    };
    let mut handle = ptr::null_mut();
    let mut status = IO_STATUS_BLOCK::default();
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            access,
            &attributes,
            &mut status,
            ptr::null(),
            0,
            SHARE,
            FILE_OPEN,
            FILE_OPEN_REPARSE_POINT | FILE_OPEN_NO_RECALL,
            ptr::null(),
            0,
        )
    };
    if result < 0 {
        return Err("parent_security_descriptor_unreadable");
    }
    read_security(&unsafe { File::from_raw_handle(handle) })
}

// Callers without a granted parent retain the old fail-closed classification.
#[cfg(any(test, feature = "copy-primitives"))]
pub(crate) fn security_metadata(file: &File) -> Observation<(Vec<u8>, Vec<u8>, bool)> {
    let security = read_security(file)?;
    let explicit = explicit_aces(&security, Kind::File, Some(&Err("parent_not_supplied")))?;
    Ok((
        explicit,
        security.owner,
        security.control & SE_DACL_PROTECTED != 0,
    ))
}

fn read_security(file: &File) -> Observation<SecurityDescriptor> {
    read_security_for_parent(file, Kind::File, None)
}

fn read_security_for_parent(
    file: &File,
    kind: Kind,
    parent: Option<&Observation<SecurityDescriptor>>,
) -> Observation<SecurityDescriptor> {
    let needs_group = parent
        .and_then(|p| p.as_ref().ok())
        .and_then(|p| p.dacl.as_ref())
        .is_some_and(|aces| {
            aces.iter().any(|ace| {
                matches!(ace[0], 0 | 1 | 2 | 3 | 9 | 10 | 13 | 14)
                    && u32::from(ace[1])
                        & if kind == Kind::File {
                            OBJECT_INHERIT_ACE
                        } else {
                            CONTAINER_INHERIT_ACE
                        }
                        != 0
                    && ace.get(8..20) == Some(&[1, 1, 0, 0, 0, 0, 0, 3, 1, 0, 0, 0])
            })
        });
    let mut needed = 0;
    let flags = OWNER_SECURITY_INFORMATION
        | DACL_SECURITY_INFORMATION
        | if needs_group {
            GROUP_SECURITY_INFORMATION
        } else {
            0
        };
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
    decode_security(descriptor, needs_group)
}

fn decode_security(
    descriptor: *mut std::ffi::c_void,
    needs_group: bool,
) -> Observation<SecurityDescriptor> {
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
    let mut aces = Vec::new();
    if present != 0 && !dacl.is_null() {
        for index in 0..unsafe { (*dacl).AceCount } {
            let mut ace = ptr::null_mut();
            if unsafe { GetAce(dacl, index as u32, &mut ace) } == 0 {
                return Err("acl_unreadable");
            }
            let header = unsafe { &*ace.cast::<ACE_HEADER>() };
            if usize::from(header.AceSize) < size_of::<ACE_HEADER>() {
                return Err("acl_unreadable");
            }
            aces.push(
                unsafe {
                    std::slice::from_raw_parts(ace.cast::<u8>(), usize::from(header.AceSize))
                }
                .to_vec(),
            );
        }
    }
    let group = if needs_group
        && control & (SE_DACL_AUTO_INHERITED | SE_DACL_PROTECTED) == 0
        && aces
            .iter()
            .any(|ace| u32::from(ace[1]) & INHERITED_ACE == 0)
    {
        let mut group = ptr::null_mut();
        if unsafe { GetSecurityDescriptorGroup(descriptor, &mut group, &mut defaulted) } == 0
            || group.is_null()
        {
            return Err("security_descriptor_unknown");
        }
        unsafe { std::slice::from_raw_parts(group.cast::<u8>(), GetLengthSid(group) as usize) }
            .to_vec()
    } else {
        vec![]
    };
    Ok(SecurityDescriptor {
        owner,
        group,
        dacl: (present != 0 && !dacl.is_null()).then_some(aces),
        control,
    })
}

fn explicit_aces(
    security: &SecurityDescriptor,
    kind: Kind,
    parent: Option<&Observation<SecurityDescriptor>>,
) -> Observation<Vec<u8>> {
    let Some(aces) = &security.dacl else {
        return Ok(vec![0]);
    };
    let legacy = security.control & (SE_DACL_AUTO_INHERITED | SE_DACL_PROTECTED) == 0;
    if legacy
        && parent.is_none()
        && aces
            .iter()
            .any(|ace| u32::from(ace[1]) & INHERITED_ACE == 0)
    {
        // No readable parent observation: legacy inheritance cannot be established.
        return Err("security_descriptor_unknown");
    }
    let mut expected = if legacy {
        parent
            .and_then(|p| p.as_ref().ok())
            .and_then(|p| p.dacl.as_ref())
            .map(|aces| propagated_aces(aces, kind, &security.owner, &security.group))
            .unwrap_or_default()
    } else {
        vec![]
    };
    let mut explicit = Vec::new();
    // Flagged ACEs consume their matches first, so an extra unflagged duplicate stays explicit.
    for inherited in [true, false] {
        for ace in aces
            .iter()
            .filter(|ace| (u32::from(ace[1]) & INHERITED_ACE != 0) == inherited)
        {
            if let Some(index) = expected
                .iter()
                .position(|candidate| same_ace(ace, candidate))
            {
                expected.swap_remove(index);
            } else if !inherited {
                explicit.push(ace[0]);
            }
        }
    }
    Ok(explicit)
}

fn same_ace(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left[0] == right[0]
        && (left[1] ^ right[1]) & !(INHERITED_ACE as u8) == 0
        && left[2..] == right[2..]
}

fn propagated_aces(aces: &[Vec<u8>], kind: Kind, owner: &[u8], group: &[u8]) -> Vec<Vec<u8>> {
    let mut result = Vec::new();
    for ace in aces {
        // Object-specific/unknown ACE layouts need more inheritance context; leave them explicit.
        if !matches!(ace[0], 0 | 1 | 2 | 3 | 9 | 10 | 13 | 14) || ace.len() < 16 {
            continue;
        }
        let sid_len = 8 + usize::from(ace[9]) * 4;
        let Some(sid) = ace.get(8..8 + sid_len) else {
            continue;
        };
        if sid[0] != 1 {
            continue;
        }
        let creator = if sid == [1, 1, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0] {
            Some(owner)
        } else if sid == [1, 1, 0, 0, 0, 0, 0, 3, 1, 0, 0, 0] {
            Some(group)
        } else {
            None
        };
        let flags = u32::from(ace[1]);
        let propagation = OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE;
        let cleared =
            flags & !(propagation | NO_PROPAGATE_INHERIT_ACE | INHERIT_ONLY_ACE | INHERITED_ACE);
        let mask = u32::from_le_bytes(ace[4..8].try_into().unwrap());
        if kind == Kind::File {
            if flags & OBJECT_INHERIT_ACE != 0 {
                result.push(effective_ace(ace, cleared as u8, mask, creator, sid_len));
            }
        } else if flags & CONTAINER_INHERIT_ACE != 0 {
            let inherits = flags & NO_PROPAGATE_INHERIT_ACE == 0;
            if inherits
                && (mask & (GENERIC_READ | GENERIC_WRITE | GENERIC_EXECUTE | GENERIC_ALL) != 0
                    || creator.is_some())
            {
                result.push(effective_ace(ace, cleared as u8, mask, creator, sid_len));
                let mut inherit_only = ace.clone();
                inherit_only[1] = (cleared | (flags & propagation) | INHERIT_ONLY_ACE) as u8;
                result.push(inherit_only);
            } else {
                let flags = cleared | if inherits { flags & propagation } else { 0 };
                result.push(effective_ace(ace, flags as u8, mask, creator, sid_len));
            }
        } else if flags & OBJECT_INHERIT_ACE != 0 && flags & NO_PROPAGATE_INHERIT_ACE == 0 {
            let mut inherit_only = ace.clone();
            inherit_only[1] = (cleared | OBJECT_INHERIT_ACE | INHERIT_ONLY_ACE) as u8;
            result.push(inherit_only);
        }
    }
    result
}

fn effective_ace(
    ace: &[u8],
    flags: u8,
    mut mask: u32,
    creator: Option<&[u8]>,
    sid_len: usize,
) -> Vec<u8> {
    let mapping = GENERIC_MAPPING {
        GenericRead: FILE_GENERIC_READ,
        GenericWrite: FILE_GENERIC_WRITE,
        GenericExecute: FILE_GENERIC_EXECUTE,
        GenericAll: FILE_ALL_ACCESS,
    };
    unsafe {
        MapGenericMask(&mut mask, &mapping);
    }
    let mut effective = ace.to_vec();
    effective[1] = flags;
    effective[4..8].copy_from_slice(&mask.to_le_bytes());
    if let Some(sid) = creator {
        effective.splice(8..8 + sid_len, sid.iter().copied());
        let length = effective.len() as u16;
        effective[2..4].copy_from_slice(&length.to_le_bytes());
    }
    effective
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
        let digest = reader
            .hash_noncached(0, &file, size, meter)
            .map_err(Reason::as_str)?;
        if !signature(&before, &observe(&file).map_err(Reason::as_str)?) {
            return Err("changed_during_read");
        }
        result.extend_from_slice(&(stream.len() as u32).to_le_bytes());
        for unit in stream {
            result.extend_from_slice(&unit.to_le_bytes());
        }
        result.extend_from_slice(&size.to_le_bytes());
        result.extend_from_slice(&digest);
    }
    reader
        .finish(opened, entry, meter)
        .map_err(Reason::as_str)?;
    Ok(result)
}

#[cfg(test)]
#[path = "tree_acl_tests.rs"]
mod acl_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enumerate::Strategy;
    use loomward_operations::tree::{compare, DifferenceKind, Field};

    #[test]
    fn short_name_fallback_is_only_for_the_unsupported_class() {
        assert_eq!(
            short_name_information(Err(InformationError::Status(0xc00000bbu32 as i32)), || Ok(
                vec![1, 0]
            )),
            Ok(vec![1, 0])
        );
        for code in [0xc0000022u32, 0xc0000003, 0xc000000d, 0xc000003a] {
            assert_eq!(
                short_name_information(Err(InformationError::Status(code as i32)), || panic!(
                    "unexpected fallback"
                )),
                Err("metadata_unreadable")
            );
        }
        assert_eq!(
            short_name_information(Ok(vec![0; 4]), || panic!("unexpected fallback")),
            Ok(vec![])
        );
        for value in [vec![], vec![1, 0, 0, 0, 1], vec![2, 0, 0, 0]] {
            assert_eq!(
                short_name_information(Ok(value), || panic!("unexpected fallback")),
                Err("malformed_metadata")
            );
        }
        assert_eq!(
            short_name_information(Err(InformationError::Status(0xc00000bbu32 as i32)), || Err(
                "metadata_unreadable"
            )),
            Err("metadata_unreadable")
        );
    }

    #[test]
    fn native_short_name_fallback_respects_the_requested_entry_budget() {
        if crate::enumerate::skip_elevated_test() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        for index in 0..8 {
            std::fs::write(temp.path().join(format!("fixture{index}")), b"synthetic").unwrap();
        }
        let (parent, expected) = NativeSource::default().open_root(temp.path()).unwrap();
        let budget = Budget {
            bytes: 1024,
            time: Duration::from_secs(30),
        };
        let max_entries = 3;
        let tree = read_tree(
            temp.path(),
            &expected,
            budget,
            max_entries,
            Default::default(),
        )
        .unwrap()
        .0;
        assert_eq!(tree.nodes.len(), max_entries);
        assert_eq!(
            tree.nodes[Path::new("")].as_ref().unwrap().children,
            Err("entry_budget")
        );
        let meter = Meter::new(budget);
        let names = directory_short_names(parent.file(), &meter, max_entries - 1).unwrap();
        assert_eq!(names.len(), max_entries - 1);
        assert!(!names.contains_key(&".".encode_utf16().collect::<Vec<_>>()));
        assert!(!names.contains_key(&"..".encode_utf16().collect::<Vec<_>>()));
        let mut cache = Some(Ok(names));
        for index in 0..8 {
            let name: Vec<_> = format!("fixture{index}").encode_utf16().collect();
            let file = enumerate::open_relative_mode(
                parent.file(),
                &name,
                FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                SHARE,
                false,
            )
            .unwrap();
            let listed = cache
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .contains_key(&name);
            let result =
                short_name_information(Err(InformationError::Status(0xc00000bbu32 as i32)), || {
                    listed_short_name(&parent, &file, &name, &mut cache, &meter, max_entries - 1)
                });
            if listed {
                assert_eq!(result, Ok(vec![]));
            } else {
                assert_eq!(result, Err("metadata_unreadable"));
            }
        }
        assert_eq!(cache.unwrap().unwrap().len(), max_entries - 1);
    }

    #[test]
    fn native_listing_replacement_cannot_supply_the_open_entries_short_name() {
        if crate::enumerate::skip_elevated_test() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("fixture"), b"original").unwrap();
        let (parent, _) = NativeSource::default().open_root(temp.path()).unwrap();
        let name: Vec<_> = "fixture".encode_utf16().collect();
        let file = enumerate::open_relative_mode(
            parent.file(),
            &name,
            FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            SHARE,
            false,
        )
        .unwrap();
        std::fs::rename(temp.path().join("fixture"), temp.path().join("retained")).unwrap();
        std::fs::write(temp.path().join("fixture"), b"replacement").unwrap();
        assert_eq!(
            listed_short_name(
                &parent,
                &file,
                &name,
                &mut None,
                &Meter::new(Budget {
                    bytes: 1024,
                    time: Duration::from_secs(30)
                }),
                2,
            ),
            Err("metadata_unreadable")
        );
    }

    #[test]
    fn native_83_file_and_directory_have_no_short_name() {
        if crate::enumerate::skip_elevated_test() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("fixture"), b"synthetic").unwrap();
        std::fs::create_dir(root.join("folder")).unwrap();
        let tree = snapshot(&root);
        for path in [Path::new("folder"), Path::new("fixture")] {
            assert_eq!(
                tree.nodes[path].as_ref().unwrap().metadata[&Class::ShortName],
                Ok(vec![]),
                "{path:?}"
            );
        }
    }

    #[test]
    fn native_listing_short_names_match_real_alternate_names_without_opens() {
        if crate::enumerate::skip_elevated_test() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let name = "synthetic long fixture.txt";
        std::fs::write(temp.path().join(name), b"synthetic").unwrap();
        std::fs::write(temp.path().join("other"), b"other").unwrap();
        let (parent, _) = NativeSource::default().open_root(temp.path()).unwrap();
        let wide: Vec<_> = name.encode_utf16().collect();
        let file = enumerate::open_relative_mode(
            parent.file(),
            &wide,
            FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            SHARE,
            false,
        )
        .unwrap();
        let other = enumerate::open_relative_mode(
            parent.file(),
            &"other".encode_utf16().collect::<Vec<_>>(),
            FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            SHARE,
            false,
        )
        .unwrap();
        let path: Vec<_> = temp
            .path()
            .join(name)
            .as_os_str()
            .encode_wide()
            .chain([0])
            .collect();
        let mut short = vec![0u16; path.len() + 1];
        let count =
            unsafe { GetShortPathNameW(path.as_ptr(), short.as_mut_ptr(), short.len() as u32) };
        assert!(count > 0 && (count as usize) < short.len());
        let short_path = PathBuf::from(std::ffi::OsString::from_wide(&short[..count as usize]));
        let alias: Vec<_> = short_path
            .file_name()
            .unwrap()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect();
        if alias
            == name
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>()
        {
            // 8.3 name creation is disabled on this volume: there is no alias to read back.
            eprintln!("skipped: the temp volume creates no 8.3 aliases");
            return;
        }
        let mut cache = None;
        let meter = Meter::new(Budget {
            bytes: 1024,
            time: Duration::from_secs(30),
        });
        let accesses = enumerate::record_open_accesses(|| {
            assert_eq!(
                listed_short_name(&parent, &file, &wide, &mut cache, &meter, 2),
                Ok(alias.clone())
            );
            assert_eq!(
                listed_short_name(&parent, &other, &wide, &mut cache, &meter, 2),
                Err("metadata_unreadable")
            );
            assert_eq!(
                listed_short_name(
                    &parent,
                    &file,
                    &"missing".encode_utf16().collect::<Vec<_>>(),
                    &mut cache,
                    &meter,
                    2,
                ),
                Err("metadata_unreadable")
            );
        });
        assert!(accesses.is_empty(), "{accesses:?}");
        assert_eq!(
            snapshot(temp.path()).nodes[Path::new(name)]
                .as_ref()
                .unwrap()
                .metadata[&Class::ShortName],
            Ok(alias)
        );
    }

    pub(super) fn snapshot(path: &Path) -> Tree {
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
        if crate::enumerate::skip_elevated_test() {
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
    fn tree_content_opens_are_noncached_deny_write_and_no_recall() {
        use windows_sys::Wdk::Storage::FileSystem::{
            FILE_NO_INTERMEDIATE_BUFFERING, FILE_OPEN_NO_RECALL, FILE_OPEN_REPARSE_POINT,
        };
        if crate::enumerate::skip_elevated_test() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let data = vec![0x5a; 1024 * 1024 + 17];
        std::fs::write(temp.path().join("fixture"), &data).unwrap();
        std::fs::write(temp.path().join("fixture:note"), b"stream").unwrap();
        let (_, expected) = NativeSource::default().open_root(temp.path()).unwrap();
        let options = enumerate::record_open_options(|| {
            let (tree, bytes) = read_tree(
                temp.path(),
                &expected,
                Budget {
                    bytes: data.len() as u64 + 6,
                    time: Duration::from_secs(30),
                },
                100,
                Default::default(),
            )
            .unwrap();
            assert_eq!(bytes, data.len() as u64 + 6);
            let node = tree.nodes[Path::new("fixture")].as_ref().unwrap();
            assert_eq!(node.content, Ok(Sha256::digest(&data).into()));
            assert!(node.metadata[&Class::NamedStreams].is_ok(), "{tree:?}");
        });
        let content: Vec<_> = options
            .into_iter()
            .filter(|(access, share, _)| {
                access & FILE_READ_DATA != 0 && *share == FILE_SHARE_READ | FILE_SHARE_DELETE
            })
            .collect();
        assert_eq!(content.len(), 2, "default and named streams: {content:?}");
        for (access, share, options) in content {
            assert_eq!(access, FILE_READ_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE);
            assert_eq!(share, FILE_SHARE_READ | FILE_SHARE_DELETE);
            // NtCreateFile's equivalent of FILE_FLAG_NO_BUFFERING.
            let required =
                FILE_NO_INTERMEDIATE_BUFFERING | FILE_OPEN_NO_RECALL | FILE_OPEN_REPARSE_POINT;
            assert_eq!(options & required, required);
        }
    }

    #[test]
    fn native_control_and_named_stream_content_are_read_only() {
        if crate::enumerate::skip_elevated_test() {
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
                    &stream.encode_utf16().collect::<Vec<_>>(),
                )
                .unwrap_err(),
                enumerate::SourceError::Refused
            );
        }
        let mut first = Tree::default();
        let accesses = record_tree_open_accesses(|| {
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
        let accesses = record_tree_open_accesses(|| {
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
        if crate::enumerate::skip_elevated_test() {
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

    /// #903: a bridge that cannot report sector geometry gets its own reason, not `io`.
    #[test]
    fn unreadable_sector_geometry_is_its_own_reason() {
        if crate::enumerate::skip_elevated_test() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("f"), b"bytes").unwrap();
        let source = NativeSource::default();
        let (_, expected) = source.open_root(temp.path()).unwrap();
        let result = read_tree_from(
            &source,
            temp.path(),
            &expected,
            budget(),
            100,
            Default::default(),
            |_| Err(io::Error::from_raw_os_error(1)),
        );
        assert_eq!(result.unwrap_err(), Reason::GeometryUnavailable);
        assert_eq!(
            Reason::GeometryUnavailable.as_str(),
            "sector_geometry_unavailable"
        );
    }

    fn pattern(size: usize, seed: u8) -> Vec<u8> {
        (0..size)
            .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
            .collect()
    }

    /// #903: the sector boundaries of the non-cached tree path against real files: empty, one byte
    /// under, on and over a 512-byte sector, and the same around 4 KiB. Equal content hashes equal
    /// to the plain SHA-256; a one-byte change in the last (partial) sector is a content difference.
    #[test]
    fn tree_content_sector_boundary_sizes_hash_exactly_and_differ_on_the_last_byte() {
        if crate::enumerate::skip_elevated_test() {
            return;
        }
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let sizes = [0usize, 511, 512, 4095, 4096, 4097];
        for size in sizes {
            let data = pattern(size, 7);
            std::fs::write(a.path().join(format!("same-{size}")), &data).unwrap();
            std::fs::write(b.path().join(format!("same-{size}")), &data).unwrap();
            let mut changed = data.clone();
            if let Some(last) = changed.last_mut() {
                *last ^= 1;
            }
            std::fs::write(a.path().join(format!("last-{size}")), &data).unwrap();
            std::fs::write(b.path().join(format!("last-{size}")), &changed).unwrap();
        }
        let (left, right) = (snapshot(a.path()), snapshot(b.path()));
        for (tree, other) in [(&left, false), (&right, true)] {
            for size in sizes {
                let mut data = pattern(size, 7);
                let node = tree.nodes[Path::new(&format!("same-{size}"))]
                    .as_ref()
                    .unwrap();
                assert_eq!(
                    node.content,
                    Ok(Sha256::digest(&data).into()),
                    "same-{size}"
                );
                if other && size > 0 {
                    *data.last_mut().unwrap() ^= 1;
                }
                let node = tree.nodes[Path::new(&format!("last-{size}"))]
                    .as_ref()
                    .unwrap();
                assert_eq!(
                    node.content,
                    Ok(Sha256::digest(&data).into()),
                    "last-{size}"
                );
            }
        }
        let report = compare(&left, &right);
        let content: Vec<_> = report
            .differences
            .iter()
            .filter(|d| matches!(d.kind, DifferenceKind::Content))
            .map(|d| d.path.clone())
            .collect();
        // The empty file cannot differ in content.
        let expected: Vec<PathBuf> = sizes
            .iter()
            .filter(|size| **size > 0)
            .map(|size| format!("last-{size}").into())
            .collect();
        let mut content = content;
        content.sort();
        let mut expected = expected;
        expected.sort();
        assert_eq!(content, expected);
        assert!(report.unknowns.is_empty(), "{:?}", report.unknowns);
    }

    /// #903: the overlapped `read_count` callback `sector_hash` drives on this path returns the
    /// short tail of a file as a count below the request, and a read at the sector past the end
    /// returns only the remainder. A mid-file short read cannot be produced by a real file on
    /// demand; its continuation is pinned by `sector_hash_continues_short_reads...` over the same
    /// generic loop.
    #[test]
    fn noncached_read_count_returns_the_short_tail() {
        if crate::enumerate::skip_elevated_test() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let (_, expected) = NativeSource::default().open_root(temp.path()).unwrap();
        let mut reader = Reader::new(temp.path(), &expected, |_, _| {}).unwrap();
        let sector = crate::noncached::sector_for_file(reader.pins.last().unwrap()).unwrap();
        reader.sector = Some(sector);
        std::fs::write(temp.path().join("f"), pattern(sector + 1, 3)).unwrap();
        let mut meter = Meter::new(budget());
        let opened = reader
            .open_relative(Path::new("f"), None, true, &meter)
            .unwrap();
        let mut buffer = crate::noncached::AlignedBuffer::new(sector);
        let request = 2 * sector;
        let got = reader
            .read_count(
                0,
                &opened.file,
                0,
                &mut buffer.bytes()[..request],
                &mut meter,
                request,
            )
            .unwrap();
        assert_eq!(got, sector + 1, "short of the {request}-byte request");
        let got = reader
            .read_count(
                0,
                &opened.file,
                sector as u64,
                &mut buffer.bytes()[..sector],
                &mut meter,
                sector,
            )
            .unwrap();
        assert_eq!(got, 1);
    }

    /// Find lists no file ID and Both a 64-bit one; neither may read as `changed_during_read`.
    #[test]
    fn listing_classes_without_a_128_bit_id_still_read_every_file() {
        if crate::enumerate::skip_elevated_test() {
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
                crate::noncached::sector_for_file,
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
        if crate::enumerate::skip_elevated_test() {
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
            crate::noncached::sector_for_file,
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
        if crate::enumerate::skip_elevated_test() {
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
                crate::noncached::sector_for_file,
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
        if crate::enumerate::skip_elevated_test() {
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
                crate::noncached::sector_for_file,
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
