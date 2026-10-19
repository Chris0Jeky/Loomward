//! Native copy primitives (docs/47 lane C2) and the lab executor that wires them (lane C5, `exec`).
//! These primitives confer no operation grant. The only consumer is `loomward-service`, which runs
//! the executor for synthetic sessions on registered lab roots; personal sessions stay refused
//! (AGENTS.md invariant 1, gate C8 closed). Never expose them through MCP or a general command.

use crate::{
    duplicate_reader, enumerate,
    identity::{self, ObjectKey},
    win,
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Read, Write},
    mem::{offset_of, size_of, ManuallyDrop},
    os::windows::io::{AsRawHandle, FromRawHandle},
    path::Path,
    ptr,
    sync::Arc,
};
use windows_sys::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{
            FileEaInformation, NtCreateFile, NtQueryInformationFile, FILE_CREATE,
            FILE_EA_INFORMATION, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_NO_RECALL,
            FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
        },
    },
    Win32::{Foundation::*, Storage::FileSystem::*, System::IO::IO_STATUS_BLOCK},
};

const SHARE: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
const HOLD: u32 = FILE_SHARE_READ | FILE_SHARE_DELETE;
const BLOCK: usize = 1024 * 1024;
const PRESERVED: u32 = FILE_ATTRIBUTE_READONLY
    | FILE_ATTRIBUTE_HIDDEN
    | FILE_ATTRIBUTE_ARCHIVE
    | FILE_ATTRIBUTE_NOT_CONTENT_INDEXED;

/// A refused metadata class, retained inside `io::Error` for typed classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedAttribute {
    pub attributes: u32,
}
impl std::fmt::Display for UnsupportedAttribute {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("unsupported_attribute")
    }
}
impl std::error::Error for UnsupportedAttribute {}

fn supported_attributes(attributes: u32) -> io::Result<()> {
    let attributes = attributes
        & (FILE_ATTRIBUTE_SYSTEM
            | FILE_ATTRIBUTE_TEMPORARY
            | FILE_ATTRIBUTE_COMPRESSED
            | FILE_ATTRIBUTE_SPARSE_FILE
            | FILE_ATTRIBUTE_ENCRYPTED
            | FILE_ATTRIBUTE_INTEGRITY_STREAM);
    if attributes != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            UnsupportedAttribute { attributes },
        ));
    }
    Ok(())
}

fn inherited_acl(file: &File) -> io::Result<()> {
    let (explicit, _, protected) =
        duplicate_reader::security_metadata(file).map_err(io::Error::other)?;
    if protected || !explicit.is_empty() {
        return Err(refused("destination ACL is not inherited"));
    }
    Ok(())
}

fn refused(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn source_error(error: enumerate::SourceError) -> io::Error {
    match error {
        enumerate::SourceError::Io(code) if code < 0 => nt_error(code),
        enumerate::SourceError::Io(code) => io::Error::from_raw_os_error(code),
        enumerate::SourceError::AccessDenied => io::Error::from(io::ErrorKind::PermissionDenied),
        _ => refused("identity or path refused"),
    }
}
fn nt_error(code: i32) -> io::Error {
    io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(code) } as i32)
}
fn borrowed(file: &File) -> ManuallyDrop<win::Handle> {
    ManuallyDrop::new(win::Handle(file.as_raw_handle()))
}
fn info<T: Default>(file: &File, class: FILE_INFO_BY_HANDLE_CLASS) -> io::Result<T> {
    win::info(&borrowed(file), class)
}
fn set<T>(file: &File, class: FILE_INFO_BY_HANDLE_CLASS, value: &T) -> io::Result<()> {
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            class,
            ptr::from_ref(value).cast(),
            size_of::<T>() as u32,
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn component(name: &str) -> io::Result<Vec<u16>> {
    let wide: Vec<_> = name.encode_utf16().collect();
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if wide.is_empty()
        || wide.len() > 255
        || name.ends_with(['.', ' '])
        || wide
            .iter()
            .any(|c| *c < 32 || matches!(*c, 34 | 42 | 47 | 58 | 60 | 62 | 63 | 92 | 124))
        || matches!(
            stem.as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "CONIN$"
                | "CONOUT$"
                | "COM¹"
                | "COM²"
                | "COM³"
                | "LPT¹"
                | "LPT²"
                | "LPT³"
        )
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    {
        return Err(refused("name_unrepresentable"));
    }
    Ok(wide)
}
fn same(file: &File, expected: &ObjectKey) -> io::Result<()> {
    if identity::from_file(file)?.object != *expected {
        return Err(refused("identity mismatch"));
    }
    Ok(())
}
fn directory(file: &File) -> io::Result<()> {
    let current = identity::from_file(file)?;
    if enumerate::refused_attributes(current.attributes)
        || current.attributes & FILE_ATTRIBUTE_DIRECTORY == 0
    {
        return Err(refused("unsupported directory"));
    }
    supported_attributes(current.attributes)
}

// FILE_DIRECTORY_FILE and FILE_OPEN_NO_RECALL are incompatible; validate directory
// type on the returned handle instead, as enumerate's pinned opens do.
fn relative(
    parent: &File,
    name: &[u16],
    access: u32,
    share: u32,
    create: bool,
    options: u32,
) -> io::Result<File> {
    win::non_elevated()?;
    let mut name = name.to_vec();
    let mut unicode = UNICODE_STRING {
        Length: (name.len() * 2) as u16,
        MaximumLength: (name.len() * 2) as u16,
        Buffer: name.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle(),
        ObjectName: &mut unicode,
        Attributes: 0,
        SecurityDescriptor: ptr::null_mut(),
        SecurityQualityOfService: ptr::null_mut(),
    };
    let mut status = IO_STATUS_BLOCK::default();
    let mut handle = ptr::null_mut();
    let code = unsafe {
        NtCreateFile(
            &mut handle,
            access | SYNCHRONIZE,
            &attributes,
            &mut status,
            ptr::null(),
            FILE_ATTRIBUTE_NORMAL,
            share,
            if create { FILE_CREATE } else { FILE_OPEN },
            FILE_OPEN_REPARSE_POINT
                | if options & windows_sys::Wdk::Storage::FileSystem::FILE_DIRECTORY_FILE == 0 {
                    FILE_OPEN_NO_RECALL
                } else {
                    0
                }
                | FILE_SYNCHRONOUS_IO_NONALERT
                | options,
            ptr::null(),
            0,
        )
    };
    if code < 0 {
        Err(nt_error(code))
    } else {
        Ok(unsafe { File::from_raw_handle(handle) })
    }
}

/// Verified, pinned destination parent. No path is used for child effects.
pub struct Destination {
    pins: Vec<File>,
    volume: File,
    sector: usize,
}
impl Destination {
    pub fn open(path: &Path, expected: &ObjectKey) -> io::Result<Self> {
        let pins = enumerate::root_handles(
            path,
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
        )
        .map_err(source_error)?;
        let parent = pins.last().unwrap();
        same(parent, expected)?;
        let mut fs = [0u16; 32];
        if unsafe {
            GetVolumeInformationByHandleW(
                parent.as_raw_handle(),
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                fs.as_mut_ptr(),
                fs.len() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let fs =
            String::from_utf16_lossy(&fs[..fs.iter().position(|c| *c == 0).unwrap_or(fs.len())]);
        if !matches!(fs.as_str(), "NTFS" | "ReFS") {
            return Err(refused("identity_unverified"));
        }
        // This host rejects directory hints for OpenFileById (ERROR_NOT_SUPPORTED).
        // A zero-access volume handle supplies ID lookup and geometry, never raw disk I/O.
        // The hint is the volume the pinned parent lives on, not the one the path's drive letter names: a
        // mounted folder, junction or subst alias reaches another volume through a different letter.
        match path.components().next() {
            Some(std::path::Component::Prefix(prefix))
                if matches!(
                    prefix.kind(),
                    std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)
                ) => {}
            _ => return Err(refused("local drive required")),
        }
        let volume_path = enumerate::volume_device(parent).map_err(source_error)?;
        let handle = unsafe {
            CreateFileW(
                volume_path.as_ptr(),
                0,
                SHARE,
                ptr::null(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let volume = unsafe { File::from_raw_handle(handle) };
        let sector = physical_sector(&volume)?;
        Ok(Self {
            pins,
            volume,
            sector,
        })
    }
    fn parent(&self) -> &File {
        self.pins.last().unwrap()
    }
    /// Identities of the volume root down to the destination parent, from the pinned handles.
    pub fn ancestry(&self) -> io::Result<Vec<ObjectKey>> {
        self.pins
            .iter()
            .map(|p| identity::from_file(p).map(|o| o.object))
            .collect()
    }
    /// The destination folder's OWN case-sensitivity flag (`FileCaseSensitiveInfo`), never the
    /// volume-wide search flag: per-directory case sensitivity is what decides a collision there.
    pub fn case_sensitive(&self) -> io::Result<bool> {
        const CASE_SENSITIVE_DIR: u32 = 1;
        match info::<FILE_CASE_SENSITIVE_INFO>(self.parent(), FileCaseSensitiveInfo) {
            Ok(flags) => Ok(flags.Flags & CASE_SENSITIVE_DIR != 0),
            // A file system without per-directory case sensitivity cannot have the flag set.
            Err(e) if matches!(e.raw_os_error(), Some(1 | 50 | 87)) => Ok(false),
            Err(e) => Err(e),
        }
    }
    /// The destination parent lies under a cloud sync root (writing there would upload, invariant 2).
    pub fn cloud_synced(&self) -> io::Result<bool> {
        use windows_sys::Win32::Storage::CloudFilters::{
            CfGetSyncRootInfoByHandle, CF_SYNC_ROOT_BASIC_INFO, CF_SYNC_ROOT_INFO_BASIC,
        };
        let mut info = CF_SYNC_ROOT_BASIC_INFO::default();
        let mut returned = 0;
        let code = unsafe {
            CfGetSyncRootInfoByHandle(
                self.parent().as_raw_handle(),
                CF_SYNC_ROOT_INFO_BASIC,
                ptr::from_mut(&mut info).cast(),
                size_of::<CF_SYNC_ROOT_BASIC_INFO>() as u32,
                &mut returned,
            )
        };
        // HRESULT_FROM_WIN32(ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT): the documented negative answer.
        const NOT_UNDER_SYNC_ROOT: i32 = 0x8007_0186_u32 as i32;
        match code {
            0 => Ok(true),
            NOT_UNDER_SYNC_ROOT => Ok(false),
            other => Err(io::Error::other(format!(
                "sync root query failed: {other:#x}"
            ))),
        }
    }
    /// Open a journaled object by its file ID on the pinned destination volume, accepting only the
    /// journaled creation times given (original or planned). Never by name.
    fn by_journaled_id(
        &self,
        file_id: [u8; 16],
        creations: &[i64],
        is_directory: bool,
    ) -> io::Result<(File, ObjectKey)> {
        let volume_serial = identity::from_file(self.parent())?.object.volume_serial;
        let mut last = refused("no journaled creation time");
        for creation in creations {
            let key = ObjectKey {
                volume_serial: volume_serial.clone(),
                file_id,
                quality: identity::IdentityQuality::FileId128,
                creation_time: creation.to_string(),
                reparse_tag: 0,
            };
            match by_id(
                &self.volume,
                &key,
                FILE_READ_ATTRIBUTES,
                SHARE,
                if is_directory {
                    FILE_FLAG_BACKUP_SEMANTICS
                } else {
                    0
                },
            ) {
                Ok(file) => return Ok((file, key)),
                Err(e) => last = e,
            }
        }
        Err(last)
    }
    /// Create one new staging folder. Never adopt an existing name.
    pub fn stage(&self, name: &str) -> io::Result<Staging<'_>> {
        let name = component(name)?;
        supported_attributes(identity::from_file(self.parent())?.attributes)?;
        let file = relative(
            self.parent(),
            &name,
            FILE_READ_ATTRIBUTES,
            SHARE,
            true,
            windows_sys::Wdk::Storage::FileSystem::FILE_DIRECTORY_FILE,
        )?;
        directory(&file)?;
        let recorded = identity::from_file(&file)?.object;
        Ok(Staging {
            destination: self,
            file,
            name,
            recorded,
            published: false,
        })
    }
}

/// Only a newly created staging tree can mint write handles.
pub struct Staging<'a> {
    destination: &'a Destination,
    file: File,
    name: Vec<u16>,
    recorded: ObjectKey,
    published: bool,
}
impl Staging<'_> {
    pub fn identity(&self) -> &ObjectKey {
        &self.recorded
    }
    /// Set the staging root's final metadata last. Journal its planned creation
    /// time before this call; creation-time preservation changes the identity record.
    pub fn set_metadata(&mut self, metadata: BasicMetadata) -> io::Result<()> {
        let _guard = self.guard()?;
        let file = relative(
            self.destination.parent(),
            &self.name,
            GENERIC_WRITE | FILE_READ_ATTRIBUTES,
            SHARE,
            false,
            0,
        )?;
        same(&file, &self.recorded)?;
        metadata.apply(&file)?;
        self.recorded = identity::from_file(&file)?.object;
        Ok(())
    }
    // LIST_DIRECTORY makes the deny-delete share effective; attributes alone do not.
    fn guard(&self) -> io::Result<File> {
        self.pin(true)
    }
    /// The current name (staging, or the final name once published) bound to the recorded identity.
    /// Reads (verification, the post-publish re-hash) may pin a published folder; writes never do.
    fn pin(&self, write: bool) -> io::Result<File> {
        if write && self.published {
            return Err(refused("published staging is never cleaned or written"));
        }
        same(&self.file, &self.recorded)?;
        let guard = relative(
            self.destination.parent(),
            &self.name,
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            false,
            0,
        )?;
        directory(&guard)?;
        same(&guard, &self.recorded)?;
        Ok(guard)
    }
    fn parents(&self, path: &[(&str, ObjectKey)]) -> io::Result<Vec<File>> {
        self.pins(path, true)
    }
    fn pins(&self, path: &[(&str, ObjectKey)], write: bool) -> io::Result<Vec<File>> {
        let mut pins = vec![self.pin(write)?];
        for (name, expected) in path {
            let file = relative(
                pins.last().unwrap(),
                &component(name)?,
                FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                false,
                0,
            )?;
            directory(&file)?;
            same(&file, expected)?;
            pins.push(file);
        }
        Ok(pins)
    }
    pub fn create_directory(
        &self,
        path: &[(&str, ObjectKey)],
        name: &str,
    ) -> io::Result<ObjectKey> {
        let pins = self.parents(path)?;
        let file = relative(
            pins.last().unwrap(),
            &component(name)?,
            FILE_READ_ATTRIBUTES,
            SHARE,
            true,
            windows_sys::Wdk::Storage::FileSystem::FILE_DIRECTORY_FILE,
        )?;
        directory(&file)?;
        Ok(identity::from_file(&file)?.object)
    }
    pub fn create_file(&self, path: &[(&str, ObjectKey)], name: &str) -> io::Result<StagedFile> {
        let pins = self.parents(path)?;
        let file = relative(
            pins.last().unwrap(),
            &component(name)?,
            GENERIC_WRITE | FILE_READ_ATTRIBUTES | READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            true,
            FILE_NON_DIRECTORY_FILE,
        )?;
        supported_attributes(identity::from_file(&file)?.attributes)?;
        Ok(StagedFile {
            file,
            _pins: Arc::new(pins),
        })
    }
    /// Reopen by recorded ID, check its current name binding, hash under a deny-write hold.
    pub fn verify(
        &self,
        path: &[(&str, ObjectKey)],
        name: &str,
        recorded: &ObjectKey,
    ) -> io::Result<VerifiedFile> {
        let pins = self.pins(path, false)?;
        let file = by_id(
            &self.destination.volume,
            recorded,
            GENERIC_READ,
            HOLD,
            FILE_FLAG_NO_BUFFERING,
        )?;
        let named = relative(
            pins.last().unwrap(),
            &component(name)?,
            FILE_READ_ATTRIBUTES,
            SHARE,
            false,
            FILE_NON_DIRECTORY_FILE,
        )?;
        same(&named, recorded)?;
        let (digest, size) = noncached_hash(&file, self.destination.sector)?;
        Ok(VerifiedFile {
            file,
            digest,
            size,
            sector: self.destination.sector,
        })
    }
    /// Immediate entry names under identity-pinned staging or published parents; never follows links.
    pub fn entries(&self, path: &[(&str, ObjectKey)]) -> io::Result<Vec<String>> {
        self.entries_with_limit(path, loomward_operations::MAX_ITEMS as usize)
    }

    fn entries_with_limit(
        &self,
        path: &[(&str, ObjectKey)],
        limit: usize,
    ) -> io::Result<Vec<String>> {
        let pins = self.pins(path, false)?;
        let folder = pins.last().unwrap();
        let mut buffer = vec![0u64; 8192];
        let mut names = Vec::new();
        let mut first = true;
        loop {
            buffer.fill(0);
            if unsafe {
                GetFileInformationByHandleEx(
                    folder.as_raw_handle(),
                    if first {
                        FileIdExtdDirectoryRestartInfo
                    } else {
                        FileIdExtdDirectoryInfo
                    },
                    buffer.as_mut_ptr().cast(),
                    (buffer.len() * 8) as u32,
                )
            } == 0
            {
                let error = io::Error::last_os_error();
                return if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                    Ok(names)
                } else {
                    Err(error)
                };
            }
            let bytes = unsafe {
                std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), buffer.len() * 8)
            };
            enumerate::decode_buffer(bytes, enumerate::Strategy::Extended, &mut |entry| {
                let name = String::from_utf16(entry.name)
                    .ok()
                    .filter(|n| component(n).is_ok());
                if let Some(name) = name.filter(|_| names.len() < limit) {
                    names.push(name);
                    enumerate::Flow::Continue
                } else {
                    enumerate::Flow::Stop
                }
            })
            .map_err(|_| refused("incomplete staging listing"))?;
            first = false;
        }
    }
    /// Caller must durably record publish intent and check the grant epoch first.
    /// This one-shot syscall never replaces an entry or retries a refused rename.
    pub fn publish(&mut self, final_name: &str) -> io::Result<()> {
        if self.published {
            return Err(refused("already published"));
        }
        let name = component(final_name)?;
        // Case-only differences name the same entry on a case-insensitive folder (#506 LOW).
        if final_name.to_lowercase() == String::from_utf16_lossy(&self.name).to_lowercase() {
            return Err(refused("final name equals the staging name"));
        }
        let file = relative(
            self.destination.parent(),
            &self.name,
            DELETE | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            false,
            0,
        )?;
        same(&file, &self.recorded)?;
        let bytes = size_of::<FILE_RENAME_INFO>()
            .max(offset_of!(FILE_RENAME_INFO, FileName) + (name.len() + 1) * 2);
        let mut storage = vec![0u64; bytes.div_ceil(8)];
        let rename = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        unsafe {
            (*rename).Anonymous.ReplaceIfExists = false;
            (*rename).RootDirectory = self.destination.parent().as_raw_handle();
            (*rename).FileNameLength = (name.len() * 2) as u32;
            ptr::copy_nonoverlapping(
                name.as_ptr(),
                ptr::addr_of_mut!((*rename).FileName).cast::<u16>(),
                name.len(),
            );
        }
        if unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FileRenameInfo,
                rename.cast(),
                bytes as u32,
            )
        } == 0
        {
            let error = io::Error::last_os_error();
            if !matches!(error.raw_os_error(), Some(87 | 123)) {
                return Err(error);
            }
            #[cfg(test)]
            eprintln!(
                "publish: relative Win32 rename returned {error}; using native pinned-parent rename"
            );
            // The Win32 wrapper rejects a relative name on this host, with 123 or (seen in the C5
            // service lab, cause not isolated) 87; keep the
            // same pinned-parent buffer and no-replace semantics in the native call.
            let mut status = IO_STATUS_BLOCK::default();
            let code = unsafe {
                windows_sys::Wdk::Storage::FileSystem::NtSetInformationFile(
                    file.as_raw_handle(),
                    &mut status,
                    rename.cast(),
                    bytes as u32,
                    windows_sys::Wdk::Storage::FileSystem::FileRenameInformation,
                )
            };
            if code < 0 {
                return Err(nt_error(code));
            }
        }
        self.published = true;
        self.name = name;
        Ok(())
    }
    /// Recovery (#506): attach to a journaled staging folder ONLY by its journaled file ID, accepting
    /// the original or the journaled planned creation time. The journaled token name must still
    /// resolve to that same object; a name alone never attaches anything.
    pub fn attach<'d>(
        destination: &'d Destination,
        token: &str,
        file_id: [u8; 16],
        creations: &[i64],
    ) -> io::Result<Staging<'d>> {
        Self::attach_as(destination, token, file_id, creations, false)
    }
    /// After a crash between the rename and `committed`: attach to the published folder by the same
    /// rule, read-only. Writes, cleanup and a second publish stay refused.
    pub fn attach_published<'d>(
        destination: &'d Destination,
        final_name: &str,
        file_id: [u8; 16],
        creations: &[i64],
    ) -> io::Result<Staging<'d>> {
        Self::attach_as(destination, final_name, file_id, creations, true)
    }
    fn attach_as<'d>(
        destination: &'d Destination,
        name: &str,
        file_id: [u8; 16],
        creations: &[i64],
        published: bool,
    ) -> io::Result<Staging<'d>> {
        let name = component(name)?;
        let (_, recorded) = destination.by_journaled_id(file_id, creations, true)?;
        let file = relative(
            destination.parent(),
            &name,
            FILE_READ_ATTRIBUTES,
            SHARE,
            false,
            0,
        )?;
        same(&file, &recorded)?;
        directory(&file)?;
        Ok(Staging {
            destination,
            file,
            name,
            recorded,
            published,
        })
    }
    /// The current identity of a journaled entry: opened by its journaled ID (original or planned
    /// creation time), then confirmed by its journaled name under the pinned staging path.
    pub fn resolve(
        &self,
        path: &[(&str, ObjectKey)],
        name: &str,
        file_id: [u8; 16],
        creations: &[i64],
        is_directory: bool,
    ) -> io::Result<ObjectKey> {
        let pins = self.pins(path, false)?;
        let (_, recorded) = self
            .destination
            .by_journaled_id(file_id, creations, is_directory)?;
        let named = relative(
            pins.last().unwrap(),
            &component(name)?,
            FILE_READ_ATTRIBUTES,
            SHARE,
            false,
            if is_directory {
                0
            } else {
                FILE_NON_DIRECTORY_FILE
            },
        )?;
        same(&named, &recorded)?;
        if (identity::from_file(&named)?.attributes & FILE_ATTRIBUTE_DIRECTORY != 0) != is_directory
        {
            return Err(refused("journaled entry changed kind"));
        }
        Ok(recorded)
    }
    /// Preserved metadata and the inherited-only ACL contract, through the bound name.
    pub fn metadata(
        &self,
        path: &[(&str, ObjectKey)],
        name: &str,
        recorded: &ObjectKey,
    ) -> io::Result<BasicMetadata> {
        let pins = self.pins(path, false)?;
        let named = relative(
            pins.last().unwrap(),
            &component(name)?,
            FILE_READ_ATTRIBUTES | READ_CONTROL,
            SHARE,
            false,
            0,
        )?;
        same(&named, recorded)?;
        inherited_acl(&named)?;
        BasicMetadata::read(&named)
    }
    /// The staging (or published) folder's own preserved metadata.
    pub fn own_metadata(&self) -> io::Result<BasicMetadata> {
        let named = relative(
            self.destination.parent(),
            &self.name,
            FILE_READ_ATTRIBUTES | READ_CONTROL,
            SHARE,
            false,
            0,
        )?;
        same(&named, &self.recorded)?;
        inherited_acl(&named)?;
        BasicMetadata::read(&named)
    }
    /// A staged or published folder carries no named stream and no extended attributes. Section 6
    /// refuses both on folders at preview, so any found was added later (a hold denies neither).
    pub fn folder_plain(
        &self,
        path: &[(&str, ObjectKey)],
        name: &str,
        recorded: &ObjectKey,
    ) -> io::Result<bool> {
        let pins = self.pins(path, false)?;
        let named = relative(
            pins.last().unwrap(),
            &component(name)?,
            FILE_READ_ATTRIBUTES | FILE_READ_EA,
            SHARE,
            false,
            0,
        )?;
        same(&named, recorded)?;
        plain(&named)
    }
    /// `folder_plain` for the staging (or published) folder itself.
    pub fn own_plain(&self) -> io::Result<bool> {
        same(&self.file, &self.recorded)?;
        let named = relative(
            self.destination.parent(),
            &self.name,
            FILE_READ_ATTRIBUTES | FILE_READ_EA,
            SHARE,
            false,
            0,
        )?;
        same(&named, &self.recorded)?;
        plain(&named)
    }
    /// Caller must first withdraw any journaled publish intent. No adoption by name.
    /// The by-ID pin below shares delete, so it excludes nothing. Identity, link count and disposition are
    /// checked, and the removal made, on the name-opened handle, which shares READ only: no other handle
    /// can open the name for delete or write while it is held.
    pub fn cleanup_file(
        &self,
        path: &[(&str, ObjectKey)],
        name: &str,
        recorded: &ObjectKey,
    ) -> io::Result<()> {
        self.cleanup_entry(path, name, recorded, false)
    }
    pub fn cleanup_directory(
        &self,
        path: &[(&str, ObjectKey)],
        name: &str,
        recorded: &ObjectKey,
    ) -> io::Result<()> {
        self.cleanup_entry(path, name, recorded, true)
    }
    fn cleanup_entry(
        &self,
        path: &[(&str, ObjectKey)],
        name: &str,
        recorded: &ObjectKey,
        is_directory: bool,
    ) -> io::Result<()> {
        let pins = self.parents(path)?;
        let _identity_pin = by_id(
            &self.destination.volume,
            recorded,
            FILE_READ_ATTRIBUTES,
            SHARE,
            if is_directory {
                FILE_FLAG_BACKUP_SEMANTICS
            } else {
                0
            },
        )?;
        let named = relative(
            pins.last().unwrap(),
            &component(name)?,
            DELETE | FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES,
            FILE_SHARE_READ,
            false,
            if is_directory {
                0
            } else {
                FILE_NON_DIRECTORY_FILE
            },
        )?;
        same(&named, recorded)?;
        let current = identity::from_file(&named)?;
        if (!is_directory && current.link_count != 1)
            || (current.attributes & FILE_ATTRIBUTE_DIRECTORY != 0) != is_directory
        {
            return Err(refused(
                "cleanup requires one link to a staged regular file",
            ));
        }
        // POSIX unlink needs a name-opened handle on this NTFS host; the by-ID
        // pin alone returns ERROR_INVALID_PARAMETER. Both handles must match.
        remove(&named)
    }
    /// Remove only this original staging name, and only when the OS finds it empty.
    /// Pending publish intent must already have been withdrawn by the journal owner.
    pub fn cleanup(self) -> io::Result<()> {
        if self.published {
            return Err(refused("published staging is never cleaned"));
        }
        let file = relative(
            self.destination.parent(),
            &self.name,
            DELETE | FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES,
            FILE_SHARE_READ,
            false,
            0,
        )?;
        same(&file, &self.recorded)?;
        directory(&file)?;
        remove(&file)
    }
    /// Set child directory metadata bottom-up after writing and verifying its children.
    /// Journal the expected new creation time before calling this operation.
    pub fn set_directory_metadata(
        &self,
        path: &[(&str, ObjectKey)],
        name: &str,
        recorded: &ObjectKey,
        metadata: BasicMetadata,
    ) -> io::Result<ObjectKey> {
        let pins = self.parents(path)?;
        let file = relative(
            pins.last().unwrap(),
            &component(name)?,
            GENERIC_WRITE | FILE_READ_ATTRIBUTES,
            SHARE,
            false,
            0,
        )?;
        same(&file, recorded)?;
        directory(&file)?;
        metadata.apply(&file)?;
        Ok(identity::from_file(&file)?.object)
    }
}

fn disposition(file: &File, flags: u32) -> io::Result<()> {
    set(
        file,
        FileDispositionInfoEx,
        &FILE_DISPOSITION_INFO_EX { Flags: flags },
    )
}
fn remove(file: &File) -> io::Result<()> {
    let flags = FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS;
    match disposition(
        file,
        flags | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    ) {
        Ok(()) => Ok(()),
        Err(error) if matches!(error.raw_os_error(), Some(1 | 50 | 87)) => {
            let mut basic: FILE_BASIC_INFO = info(file, FileBasicInfo)?;
            basic.FileAttributes &= !FILE_ATTRIBUTE_READONLY;
            if basic.FileAttributes == 0 {
                basic.FileAttributes = FILE_ATTRIBUTE_NORMAL;
            }
            set(file, FileBasicInfo, &basic)?;
            disposition(file, flags)
        }
        Err(error) => Err(error),
    }
}

fn by_id(
    parent: &File,
    recorded: &ObjectKey,
    access: u32,
    share: u32,
    flags: u32,
) -> io::Result<File> {
    win::non_elevated()?;
    let descriptor = FILE_ID_DESCRIPTOR {
        dwSize: size_of::<FILE_ID_DESCRIPTOR>() as u32,
        Type: ExtendedFileIdType,
        Anonymous: FILE_ID_DESCRIPTOR_0 {
            ExtendedFileId: FILE_ID_128 {
                Identifier: recorded.file_id,
            },
        },
    };
    let mut handle = unsafe {
        OpenFileById(
            parent.as_raw_handle(),
            &descriptor,
            access | SYNCHRONIZE,
            share,
            ptr::null(),
            FILE_FLAG_OPEN_NO_RECALL | FILE_FLAG_OPEN_REPARSE_POINT | flags,
        )
    };
    if handle == INVALID_HANDLE_VALUE
        && io::Error::last_os_error().raw_os_error() == Some(50)
        && recorded.file_id[8..] == [0; 8]
    {
        let descriptor = FILE_ID_DESCRIPTOR {
            dwSize: size_of::<FILE_ID_DESCRIPTOR>() as u32,
            Type: FileIdType,
            Anonymous: FILE_ID_DESCRIPTOR_0 {
                FileId: i64::from_le_bytes(recorded.file_id[..8].try_into().unwrap()),
            },
        };
        handle = unsafe {
            OpenFileById(
                parent.as_raw_handle(),
                &descriptor,
                access | SYNCHRONIZE,
                share,
                ptr::null(),
                FILE_FLAG_OPEN_NO_RECALL | FILE_FLAG_OPEN_REPARSE_POINT | flags,
            )
        };
    }
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(handle) };
    same(&file, recorded)?;
    let current = identity::from_file(&file)?;
    if enumerate::refused_attributes(current.attributes) {
        return Err(refused("unsupported staged attributes"));
    }
    supported_attributes(current.attributes)?;
    Ok(file)
}

/// Fields preserved by v1; access/change times are deliberately not promised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BasicMetadata {
    pub creation_time: i64,
    pub last_write_time: i64,
    pub attributes: u32,
}
impl BasicMetadata {
    fn read(file: &File) -> io::Result<Self> {
        let basic: FILE_BASIC_INFO = info(file, FileBasicInfo)?;
        supported_attributes(basic.FileAttributes)?;
        Ok(Self {
            creation_time: basic.CreationTime,
            last_write_time: basic.LastWriteTime,
            attributes: basic.FileAttributes & PRESERVED,
        })
    }
    fn apply(self, file: &File) -> io::Result<()> {
        if self.attributes & !PRESERVED != 0 || self.creation_time <= 0 || self.last_write_time <= 0
        {
            return Err(refused("unsupported metadata"));
        }
        set(
            file,
            FileBasicInfo,
            &FILE_BASIC_INFO {
                CreationTime: self.creation_time,
                LastWriteTime: self.last_write_time,
                FileAttributes: if self.attributes == 0 {
                    FILE_ATTRIBUTE_NORMAL
                } else {
                    self.attributes
                },
                ..Default::default()
            },
        )?;
        file.sync_all()
    }
}
/// A create-new handle, never an existing destination file.
pub struct StagedFile {
    file: File,
    // Keep the staging name and every parent immovable until writes and metadata finish.
    _pins: Arc<Vec<File>>,
}
impl StagedFile {
    /// Journal this identity BEFORE preallocation, streams, metadata or writes.
    pub fn identity(&self) -> io::Result<ObjectKey> {
        Ok(identity::from_file(&self.file)?.object)
    }
    /// Journal the planned creation time before changing it; re-query `identity` after success.
    /// After a crash, reopen by journaled file ID with the original or journaled planned creation
    /// time, never by name.
    pub fn set_metadata(&self, metadata: BasicMetadata) -> io::Result<()> {
        metadata.apply(&self.file)
    }
    /// Create a named $DATA stream on this newly created file, never replace one.
    pub fn create_stream(&self, name: &str) -> io::Result<StagedFile> {
        let name = stream_name(name)?;
        Ok(StagedFile {
            file: relative(
                &self.file,
                &name,
                GENERIC_WRITE | FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                true,
                FILE_NON_DIRECTORY_FILE,
            )?,
            _pins: self._pins.clone(),
        })
    }
    /// Preallocate, write exactly the declared length in 1 MiB chunks, then flush once.
    /// On failure retain staging for identity-bound cleanup, never publish it.
    pub fn write_and_flush(&self, reader: &mut impl Read, size: u64) -> io::Result<[u8; 32]> {
        if info::<FILE_STANDARD_INFO>(&self.file, FileStandardInfo)?.EndOfFile != 0 {
            return Err(refused("staged writes require an empty file"));
        }
        let allocation = i64::try_from(size).map_err(|_| refused("file too large"))?;
        set(
            &self.file,
            FileAllocationInfo,
            &FILE_ALLOCATION_INFO {
                AllocationSize: allocation,
            },
        )?;
        let mut buffer = vec![0; BLOCK];
        let mut digest = Sha256::new();
        let mut remaining = size;
        let mut writer = &self.file;
        while remaining != 0 {
            let count = remaining.min(BLOCK as u64) as usize;
            reader.read_exact(&mut buffer[..count])?;
            writer.write_all(&buffer[..count])?;
            digest.update(&buffer[..count]);
            remaining -= count as u64;
        }
        self.file.sync_all()?;
        Ok(digest.finalize().into())
    }
}

/// Keep this handle alive through publish and the committed journal record.
pub struct VerifiedFile {
    file: File,
    pub digest: [u8; 32],
    pub size: u64,
    sector: usize,
}
impl VerifiedFile {
    pub fn metadata(&self) -> io::Result<BasicMetadata> {
        BasicMetadata::read(&self.file)
    }
    /// Named `$DATA` streams on the held destination object, as `:<name>:$DATA` with sizes (#506).
    pub fn streams(&self) -> io::Result<Vec<(String, u64)>> {
        streams(&self.file)
    }
    /// Extended-attribute bytes on the held object. A deny-write hold does not deny FILE_WRITE_EA,
    /// so this is re-read before `committed` (section 6: EAs are refused, never copied).
    pub fn ea_size(&self) -> io::Result<u32> {
        ea_size(&self.file)
    }
    pub fn verify_stream(&self, name: &str) -> io::Result<VerifiedFile> {
        let file = relative(
            &self.file,
            &stream_name(name)?,
            GENERIC_READ,
            HOLD,
            false,
            FILE_NON_DIRECTORY_FILE
                | windows_sys::Wdk::Storage::FileSystem::FILE_NO_INTERMEDIATE_BUFFERING,
        )?;
        supported_attributes(identity::from_file(&file)?.attributes)?;
        let (digest, size) = noncached_hash(&file, self.sector)?;
        Ok(VerifiedFile {
            file,
            digest,
            size,
            sector: self.sector,
        })
    }
}
fn stream_name(name: &str) -> io::Result<Vec<u16>> {
    component(name)?;
    Ok(format!(":{name}:$DATA").encode_utf16().collect())
}
fn physical_sector(volume: &File) -> io::Result<usize> {
    use windows_sys::Win32::System::{Ioctl::*, IO::DeviceIoControl};
    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageAccessAlignmentProperty,
        QueryType: PropertyStandardQuery,
        ..Default::default()
    };
    let mut alignment = STORAGE_ACCESS_ALIGNMENT_DESCRIPTOR::default();
    let mut returned = 0;
    if unsafe {
        DeviceIoControl(
            volume.as_raw_handle(),
            IOCTL_STORAGE_QUERY_PROPERTY,
            ptr::from_ref(&query).cast(),
            size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            ptr::from_mut(&mut alignment).cast(),
            size_of::<STORAGE_ACCESS_ALIGNMENT_DESCRIPTOR>() as u32,
            &mut returned,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if returned < size_of::<STORAGE_ACCESS_ALIGNMENT_DESCRIPTOR>() as u32 {
        return Err(refused("truncated sector geometry"));
    }
    sector_alignment(
        alignment.BytesPerLogicalSector,
        alignment.BytesPerPhysicalSector,
    )
}
fn sector_alignment(logical: u32, physical: u32) -> io::Result<usize> {
    if !logical.is_power_of_two()
        || !physical.is_power_of_two()
        || physical < logical
        || physical as usize > BLOCK
    {
        return Err(refused("unknown sector alignment"));
    }
    Ok(physical as usize)
}
fn noncached_hash(file: &File, sector: usize) -> io::Result<([u8; 32], u64)> {
    let size = info::<FILE_STANDARD_INFO>(file, FileStandardInfo)?.EndOfFile;
    if size < 0 {
        return Err(refused("negative size"));
    }
    let size = size as u64;
    let mut storage = vec![0u8; BLOCK + sector];
    let offset = storage.as_ptr().align_offset(sector);
    let buffer = &mut storage[offset..offset + BLOCK];
    let mut file = file;
    let mut remaining = size;
    let mut digest = Sha256::new();
    while remaining != 0 {
        let logical = remaining.min(BLOCK as u64) as usize;
        let request = logical.div_ceil(sector) * sector;
        let count = file.read(&mut buffer[..request])?;
        if count != logical {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "noncached read size changed",
            ));
        }
        digest.update(&buffer[..count]);
        remaining -= count as u64;
    }
    Ok((digest.finalize().into(), size))
}

/// Deny-write, no-recall source reader reusing the duplicate reader's signatures,
/// ancestor pins, bounded reads and final reopen-by-name check unchanged.
pub struct Source {
    source: duplicate_reader::CopySource,
    offset: u64,
    size: u64,
    streams: Vec<(String, File, u64, u64)>,
}
impl Source {
    pub fn open(
        root: &Path,
        expected_root: &enumerate::OpenedIdentity,
        entry: duplicate_reader::Entry,
        budget: duplicate_reader::Budget,
    ) -> Result<Self, duplicate_reader::Reason> {
        let size = entry
            .expected
            .size
            .parse()
            .map_err(|_| duplicate_reader::Reason::Unsupported)?;
        let source = duplicate_reader::CopySource::open(root, expected_root, entry, budget)?;
        let mut held = Vec::new();
        for (name, size) in
            streams(source.file()).map_err(|_| duplicate_reader::Reason::Unsupported)?
        {
            let file = relative(
                source.file(),
                &name.encode_utf16().collect::<Vec<_>>(),
                FILE_READ_DATA | FILE_READ_ATTRIBUTES,
                HOLD,
                false,
                FILE_NON_DIRECTORY_FILE,
            )
            .map_err(|error| match error.raw_os_error() {
                Some(32 | 33) => duplicate_reader::Reason::SharingViolation,
                Some(5) => duplicate_reader::Reason::AccessDenied,
                _ => duplicate_reader::Reason::Unsupported,
            })?;
            held.push((name, file, size, 0));
        }
        source.finish()?;
        Ok(Self {
            source,
            offset: 0,
            size,
            streams: held,
        })
    }
    pub fn streams(&self) -> Vec<(String, u64)> {
        self.streams
            .iter()
            .map(|(name, _, size, _)| (name.clone(), *size))
            .collect()
    }
    /// Stream names are exactly the enumerated `:<name>:$DATA` values.
    pub fn stream_reader(&mut self, name: &str) -> io::Result<NamedSource<'_>> {
        let index = self
            .streams
            .iter()
            .position(|(n, _, _, _)| n == name)
            .ok_or_else(|| refused("unknown source stream"))?;
        Ok(NamedSource {
            source: self,
            index,
        })
    }
    pub fn finish(&self) -> Result<(), duplicate_reader::Reason> {
        self.source.finish()?;
        if streams(self.source.file()).map_err(|_| duplicate_reader::Reason::Unsupported)?
            != self.streams()
        {
            return Err(duplicate_reader::Reason::ChangedDuringRead);
        }
        Ok(())
    }
}
pub struct NamedSource<'a> {
    source: &'a mut Source,
    index: usize,
}
impl Read for NamedSource<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let (_, file, size, offset) = &mut self.source.streams[self.index];
        let count = (*size - *offset).min(buffer.len() as u64) as usize;
        if count == 0 {
            return Ok(0);
        }
        self.source
            .source
            .read_stream(file, *offset, &mut buffer[..count])
            .map_err(|e| io::Error::other(format!("source stream read: {e:?}")))?;
        *offset += count as u64;
        Ok(count)
    }
}
impl Read for Source {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = (self.size - self.offset).min(buffer.len() as u64) as usize;
        if count == 0 {
            return Ok(0);
        }
        self.source
            .read(self.offset, &mut buffer[..count])
            .map_err(|e| io::Error::other(format!("source read: {e:?}")))?;
        self.offset += count as u64;
        Ok(count)
    }
}
fn ea_size(file: &File) -> io::Result<u32> {
    let mut ea = FILE_EA_INFORMATION::default();
    let mut status = IO_STATUS_BLOCK::default();
    let code = unsafe {
        NtQueryInformationFile(
            file.as_raw_handle(),
            &mut status,
            ptr::from_mut(&mut ea).cast(),
            size_of::<FILE_EA_INFORMATION>() as u32,
            FileEaInformation,
        )
    };
    if code < 0 {
        return Err(nt_error(code));
    }
    Ok(ea.EaSize)
}
fn plain(file: &File) -> io::Result<bool> {
    let streams = match streams(file) {
        Ok(s) => s,
        // FileStreamInfo answers ERROR_HANDLE_EOF for an object with no stream (a plain folder).
        Err(e) if e.raw_os_error() == Some(38) => Vec::new(),
        Err(e) => return Err(e),
    };
    Ok(streams.is_empty() && ea_size(file)? == 0)
}
pub(crate) fn supported_file(file: &File) -> io::Result<()> {
    let current = identity::from_file(file)?;
    supported_attributes(current.attributes)?;
    if ea_size(file)? != 0 {
        return Err(refused("extended_attributes"));
    }
    streams(file)?;
    Ok(())
}

/// What a copy preview needs beyond `identity::observe`: point-in-time, read-only, no content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// The probed object; the caller binds it to its own observation of the same name.
    pub object: ObjectKey,
    pub ea_size: u32,
    /// Named `$DATA` streams, bare names (no colons) with sizes.
    pub streams: Vec<(String, u64)>,
    /// The object carries its own (non-inherited) ACEs; reported, never copied.
    pub explicit_aces: bool,
}

/// Observe a preview candidate without following a reparse point or recalling a placeholder.
pub fn probe(path: &Path) -> io::Result<Probe> {
    let handle = win::open(
        path,
        FILE_READ_ATTRIBUTES | FILE_READ_EA | READ_CONTROL,
        SHARE,
    )?;
    let file = ManuallyDrop::new(unsafe { File::from_raw_handle(handle.0) });
    let object = identity::from_file(&file)?.object;
    let ea_bytes = ea_size(&file)?;
    let streams = match streams(&file) {
        Ok(s) => s,
        // FileStreamInfo answers ERROR_HANDLE_EOF for an object with no stream (a folder).
        Err(e) if e.raw_os_error() == Some(38) => Vec::new(),
        Err(e) => return Err(e),
    }
    .into_iter()
    .map(|(n, size)| {
        (
            n.trim_start_matches(':')
                .trim_end_matches(":$DATA")
                .to_string(),
            size,
        )
    })
    .collect();
    let explicit_aces = !duplicate_reader::security_metadata(&file)
        .map_err(|e| io::Error::other(e.to_string()))?
        .0
        .is_empty();
    drop(handle);
    Ok(Probe {
        object,
        ea_size: ea_bytes,
        streams,
        explicit_aces,
    })
}

/// Named $DATA streams only; default data is represented by the file itself.
/// The bounded native buffer accommodates 64 maximum-length component names.
fn streams(file: &File) -> io::Result<Vec<(String, u64)>> {
    let mut buffer = vec![0u64; 64 * 1024 / 8];
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileStreamInfo,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 8) as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let bytes = buffer.len() * 8;
    let mut offset = 0;
    let mut result = Vec::new();
    loop {
        let header = offset_of!(FILE_STREAM_INFO, StreamName);
        if offset + size_of::<FILE_STREAM_INFO>() > bytes {
            return Err(refused("invalid stream record"));
        }
        let entry = unsafe {
            ptr::read_unaligned(
                buffer
                    .as_ptr()
                    .cast::<u8>()
                    .add(offset)
                    .cast::<FILE_STREAM_INFO>(),
            )
        };
        let len = entry.StreamNameLength as usize;
        if len % 2 != 0 || offset + header + len > bytes || entry.StreamSize < 0 {
            return Err(refused("invalid stream record"));
        }
        let name = unsafe {
            std::slice::from_raw_parts(
                buffer
                    .as_ptr()
                    .cast::<u8>()
                    .add(offset + header)
                    .cast::<u16>(),
                len / 2,
            )
        };
        let name = String::from_utf16(name).map_err(|_| refused("unrepresentable stream name"))?;
        if name != "::$DATA" {
            if !name.starts_with(':') || !name.ends_with(":$DATA") {
                return Err(refused("unsupported stream type"));
            }
            result.push((name, entry.StreamSize as u64));
            if result.len() > 64 {
                return Err(refused("too_many_streams"));
            }
        }
        if entry.NextEntryOffset == 0 {
            break;
        }
        if (entry.NextEntryOffset as usize) < header + len || entry.NextEntryOffset % 8 != 0 {
            return Err(refused("invalid stream offset"));
        }
        offset += entry.NextEntryOffset as usize;
    }
    Ok(result)
}

#[cfg(test)]
mod tests;

pub mod exec;
