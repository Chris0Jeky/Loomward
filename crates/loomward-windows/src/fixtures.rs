//! Opt-in disposable laboratory only. Not a production filesystem executor.
#[cfg(windows)]
use crate::identity::observe;
use crate::identity::ObjectKey;
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::{collections::HashSet, fs, io::Write, path::Component};
use std::{
    io,
    path::{Path, PathBuf},
};

pub const LAB_ROOT: &str = r"G:\loomward-lab\fixtures";
#[cfg(windows)]
const MARKER: &str = ".loomward-fixtures.json";
#[cfg(windows)]
const VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    File,
    Directory,
    DirectoryLink,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub relative: PathBuf,
    pub kind: Kind,
    pub identity: Option<ObjectKey>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skip {
    pub case: String,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    version: u32,
    root_identity: ObjectKey,
    pub entries: Vec<Entry>,
    pub skips: Vec<Skip>,
    pub stale_identity: Option<ObjectKey>,
}

pub fn create(root: &Path) -> io::Result<Manifest> {
    create_in(root, Path::new(LAB_ROOT))
}
pub fn destroy(root: &Path) -> io::Result<()> {
    destroy_in(root, Path::new(LAB_ROOT))
}
#[cfg(windows)]
fn refuse(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

#[cfg(not(windows))]
pub(crate) fn create_in(_: &Path, _: &Path) -> io::Result<Manifest> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Windows fixtures unavailable on this platform",
    ))
}
#[cfg(not(windows))]
pub(crate) fn destroy_in(_: &Path, _: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Windows fixtures unavailable on this platform",
    ))
}

#[cfg(windows)]
fn scoped(root: &Path, base: &Path) -> io::Result<PathBuf> {
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    if !root.is_absolute()
        || !base.is_absolute()
        || root.components().any(|c| {
            !matches!(
                c,
                Component::Prefix(_) | Component::RootDir | Component::Normal(_)
            )
        })
    {
        return Err(refuse(
            "fixture root must be an absolute path without traversal",
        ));
    }
    let relative = root
        .strip_prefix(base)
        .map_err(|_| refuse("fixture root is outside the lab"))?;
    if relative.as_os_str().is_empty() {
        return Err(refuse(
            "the lab base itself is not a disposable fixture root",
        ));
    }
    for part in relative.components() {
        let name = part
            .as_os_str()
            .to_str()
            .ok_or_else(|| refuse("invalid fixture root encoding"))?;
        if name.ends_with(['.', ' ']) || name.contains([':', '<', '>', '"', '|', '?', '*']) {
            return Err(refuse("ambiguous Win32 fixture root component"));
        }
    }
    // Walk before creating anything, including ancestors of the declared lab.
    for ancestor in root.ancestors().filter(|p| !p.as_os_str().is_empty()) {
        match fs::symlink_metadata(ancestor) {
            Ok(_) if observe(ancestor)?.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 => {
                return Err(refuse("reparse ancestor refused"))
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(root.to_path_buf())
}

#[cfg(windows)]
pub(crate) fn pin(root: &Path) -> io::Result<Vec<crate::win::Handle>> {
    use windows_sys::Win32::Storage::FileSystem::*;
    let mut ancestors: Vec<_> = root
        .ancestors()
        .filter(|p| !p.as_os_str().is_empty())
        .collect();
    ancestors.reverse();
    let mut guards = Vec::new();
    for path in ancestors {
        match crate::win::open(
            path,
            FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
        ) {
            Ok(handle) => {
                let tag: FILE_ATTRIBUTE_TAG_INFO = crate::win::info(&handle, FileAttributeTagInfo)?;
                if tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                    return Err(refuse("reparse ancestor refused while pinning lab"));
                }
                guards.push(handle);
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => break,
            Err(e) => return Err(e),
        }
    }
    Ok(guards)
}

#[cfg(windows)]
fn load(root: &Path) -> io::Result<Manifest> {
    let path = root.join(MARKER);
    let marker_identity = observe(&path)?;
    if marker_identity.object.reparse_tag != 0 || marker_identity.link_count != 1 {
        return Err(refuse("marker cannot be a reparse point or hard link"));
    }
    if fs::metadata(&path)?.len() > 1024 * 1024 {
        return Err(refuse("oversized fixture marker"));
    }
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(path)?).map_err(|_| refuse("invalid fixture marker"))?;
    if manifest.version != VERSION || manifest.root_identity != observe(root)?.object {
        return Err(refuse("fixture root identity does not match marker"));
    }
    let mut names = HashSet::new();
    for entry in &manifest.entries {
        if entry.relative.as_os_str().is_empty()
            || entry
                .relative
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
            || entry.relative == Path::new(MARKER)
            || !names.insert(entry.relative.clone())
        {
            return Err(refuse("unsafe or duplicate manifest entry"));
        }
        // ADS is allowed only for the lab's one fixed stream; never device namespace syntax.
        let text = entry.relative.to_string_lossy();
        if text.contains(':') && text != "streams.txt:loomward" {
            return Err(refuse("unrecognised stream syntax"));
        }
    }
    Ok(manifest)
}

#[cfg(windows)]
fn preflight(root: &Path, manifest: &Manifest) -> io::Result<()> {
    let paths: HashSet<_> = manifest
        .entries
        .iter()
        .map(|entry| root.join(&entry.relative))
        .collect();
    check_streams(root, &paths)?;
    check_streams(&root.join(MARKER), &paths)?;
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for child in fs::read_dir(dir)? {
            let child = child?;
            if child.path() == root.join(MARKER) {
                continue;
            }
            if !paths.contains(&child.path()) {
                return Err(refuse(
                    "unlisted content: cleanup refused before deleting anything",
                ));
            }
            let observation = observe(&child.path())?;
            if observation.object.reparse_tag == 0 {
                check_streams(&child.path(), &paths)?;
            }
            if child.file_type()?.is_dir() && observation.object.reparse_tag == 0 {
                pending.push(child.path());
            }
        }
    }
    for entry in &manifest.entries {
        let path = root.join(&entry.relative);
        match observe(&path) {
            Ok(current) if Some(&current.object) == entry.identity.as_ref() => {
                let directory = current.attributes
                    & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY
                    != 0;
                if directory != matches!(entry.kind, Kind::Directory | Kind::DirectoryLink)
                    || (current.object.reparse_tag != 0 && entry.kind == Kind::Directory)
                {
                    return Err(refuse("fixture entry type changed"));
                }
                for parent in path.ancestors().skip(1).take_while(|p| *p != root) {
                    if observe(parent)?.object.reparse_tag != 0 {
                        return Err(refuse("fixture entry has a reparse ancestor"));
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            _ => {
                return Err(refuse(
                    "fixture entry replaced or creation interrupted; retain for inspection",
                ))
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
fn check_streams(path: &Path, listed: &HashSet<PathBuf>) -> io::Result<()> {
    use windows_sys::Win32::{Foundation::*, Storage::FileSystem::*};
    struct Search(HANDLE);
    impl Drop for Search {
        fn drop(&mut self) {
            unsafe {
                FindClose(self.0);
            }
        }
    }
    let text = crate::win::wide(path)?;
    let mut stream = WIN32_FIND_STREAM_DATA::default();
    let search = Search(unsafe {
        FindFirstStreamW(
            text.as_ptr(),
            FindStreamInfoStandard,
            (&mut stream as *mut WIN32_FIND_STREAM_DATA).cast(),
            0,
        )
    });
    if search.0 == INVALID_HANDLE_VALUE {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_HANDLE_EOF as i32) {
            return Ok(());
        }
        return Err(error);
    }
    loop {
        let name = String::from_utf16(
            &stream.cStreamName[..stream
                .cStreamName
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(stream.cStreamName.len())],
        )
        .map_err(|_| refuse("unrepresentable stream name: cleanup refused"))?;
        if name != "::$DATA" {
            let suffix = name
                .strip_suffix(":$DATA")
                .ok_or_else(|| refuse("unknown stream type"))?;
            let mut stream_path = path.as_os_str().to_os_string();
            stream_path.push(suffix);
            if !listed.contains(Path::new(&stream_path)) {
                return Err(refuse(
                    "unlisted alternate stream: cleanup refused before deleting anything",
                ));
            }
        }
        if unsafe {
            FindNextStreamW(
                search.0,
                (&mut stream as *mut WIN32_FIND_STREAM_DATA).cast(),
            )
        } == 0
        {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_HANDLE_EOF as i32) {
                return Ok(());
            }
            return Err(error);
        }
    }
}

#[cfg(windows)]
fn save(root: &Path, manifest: &Manifest) -> io::Result<()> {
    // A partial marker fails closed; interrupted creation never licenses recursive cleanup.
    fs::write(root.join(MARKER), serde_json::to_vec_pretty(manifest)?)
}

#[cfg(windows)]
fn add(
    root: &Path,
    manifest: &mut Manifest,
    relative: &Path,
    kind: Kind,
    action: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<()> {
    let path = root.join(relative);
    if fs::symlink_metadata(&path).is_ok() {
        return Err(refuse("fixture entry already exists"));
    }
    manifest.entries.push(Entry {
        relative: relative.into(),
        kind,
        identity: None,
    });
    save(root, manifest)?;
    let result = action(&path);
    if let Ok(current) = observe(&path) {
        manifest.entries.last_mut().unwrap().identity = Some(current.object);
    }
    save(root, manifest)?;
    result
}

#[cfg(windows)]
fn optional(
    root: &Path,
    manifest: &mut Manifest,
    case: &str,
    action: impl FnOnce(&mut Manifest) -> io::Result<()>,
) -> io::Result<()> {
    if let Err(error) = action(manifest) {
        eprintln!("SKIP {case}: {error}");
        manifest.skips.push(Skip {
            case: case.into(),
            reason: error.to_string(),
        });
        save(root, manifest)?;
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn create_in(root: &Path, base: &Path) -> io::Result<Manifest> {
    let root = scoped(root, base)?;
    let _guards = pin(&root)?;
    if root.exists() && fs::read_dir(&root)?.next().is_some() {
        let root = fs::canonicalize(&root)?;
        let manifest =
            load(&root).map_err(|_| refuse("nonempty root without a valid lab marker"))?;
        preflight(&root, &manifest)?;
        return Ok(manifest);
    }
    fs::create_dir_all(&root)?;
    let _created_guards = pin(&root)?;
    let root = fs::canonicalize(&root)?;
    let mut manifest = Manifest {
        version: VERSION,
        root_identity: observe(&root)?.object,
        entries: Vec::new(),
        skips: Vec::new(),
        stale_identity: None,
    };
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join(MARKER))?
        .write_all(&serde_json::to_vec_pretty(&manifest)?)?;
    for name in [
        "ordinary.txt",
        "equal.txt",
        "streams.txt",
        "sparse.bin",
        "compressed.txt",
        "attributes.txt",
        "locked.txt",
        "recreated.txt",
        "日本語-🧵.txt",
        "Case.txt",
    ] {
        add(&root, &mut manifest, Path::new(name), Kind::File, |p| {
            fs::write(p, b"loomward synthetic fixture\n")
        })?;
    }
    add(
        &root,
        &mut manifest,
        Path::new("hard-link.txt"),
        Kind::File,
        |p| fs::hard_link(root.join("ordinary.txt"), p),
    )?;
    add(
        &root,
        &mut manifest,
        Path::new("target"),
        Kind::Directory,
        |p| fs::create_dir(p),
    )?;
    add(
        &root,
        &mut manifest,
        Path::new("target/inside.txt"),
        Kind::File,
        |p| fs::write(p, b"synthetic target"),
    )?;
    optional(&root, &mut manifest, "case-variant-collision", |m| {
        add(&root, m, Path::new("case.txt"), Kind::File, |p| {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(p)
                .map(|_| ())
        })
    })?;
    optional(&root, &mut manifest, "file-symlink", |m| {
        add(&root, m, Path::new("file-symlink"), Kind::File, |p| {
            std::os::windows::fs::symlink_file(root.join("ordinary.txt"), p)
        })
    })?;
    optional(&root, &mut manifest, "directory-symlink", |m| {
        add(
            &root,
            m,
            Path::new("directory-symlink"),
            Kind::DirectoryLink,
            |p| std::os::windows::fs::symlink_dir(root.join("target"), p),
        )
    })?;
    optional(&root, &mut manifest, "directory-junction", |m| {
        add(&root, m, Path::new("junction"), Kind::DirectoryLink, |p| {
            junction(p, &root.join("target"))
        })
    })?;
    optional(&root, &mut manifest, "alternate-data-stream", |m| {
        add(
            &root,
            m,
            Path::new("streams.txt:loomward"),
            Kind::File,
            |p| fs::write(p, b"synthetic alternate stream"),
        )
    })?;
    optional(&root, &mut manifest, "sparse-file", |_| {
        control(
            &root.join("sparse.bin"),
            windows_sys::Win32::System::Ioctl::FSCTL_SET_SPARSE,
            &[],
        )?;
        fs::OpenOptions::new()
            .write(true)
            .open(root.join("sparse.bin"))?
            .set_len(8 * 1024 * 1024)
    })?;
    optional(&root, &mut manifest, "ntfs-compression", |_| {
        control(
            &root.join("compressed.txt"),
            windows_sys::Win32::System::Ioctl::FSCTL_SET_COMPRESSION,
            &2u16.to_le_bytes(),
        )
    })?;
    set_attributes(
        &root.join("attributes.txt"),
        windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_READONLY
            | windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_HIDDEN
            | windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_SYSTEM,
    )?;
    let mut relative = PathBuf::new();
    for index in 0..3 {
        relative.push(format!("long-{index}-{}", "x".repeat(90)));
        add(&root, &mut manifest, &relative, Kind::Directory, |p| {
            fs::create_dir(p)
        })?;
    }
    relative.push("long-file.txt");
    add(&root, &mut manifest, &relative, Kind::File, |p| {
        fs::write(p, b"extended length fixture")
    })?;
    let stale = root.join("recreated.txt");
    manifest.stale_identity = Some(observe(&stale)?.object);
    fs::remove_file(&stale)?;
    fs::write(&stale, b"recreated synthetic fixture")?;
    manifest
        .entries
        .iter_mut()
        .find(|e| e.relative == Path::new("recreated.txt"))
        .unwrap()
        .identity = Some(observe(&stale)?.object);
    for (case, reason) in [
        (
            "cloud-placeholder",
            "requires an explicitly configured cloud provider; not fabricated",
        ),
        (
            "removable-media",
            "requires physical media/disconnect; not fabricated",
        ),
        (
            "deny-share-lock",
            "in-test only; CLI cannot retain a lock after exit",
        ),
    ] {
        manifest.skips.push(Skip {
            case: case.into(),
            reason: reason.into(),
        });
    }
    save(&root, &manifest)?;
    Ok(manifest)
}

#[cfg(windows)]
pub(crate) fn destroy_in(root: &Path, base: &Path) -> io::Result<()> {
    let root = scoped(root, base)?;
    let mut guards = pin(&root)?;
    let root = fs::canonicalize(root)?;
    let manifest = load(&root)?;
    preflight(&root, &manifest)?;
    let mut directories = Vec::new();
    for entry in &manifest.entries {
        if entry.kind == Kind::Directory {
            let path = root.join(&entry.relative);
            if path.exists() {
                directories.push((path.clone(), pin(&path)?));
            }
        }
    }
    preflight(&root, &manifest)?;
    // Reverse creation order removes streams/links before files and children before parents.
    for entry in manifest.entries.iter().rev() {
        let path = root.join(&entry.relative);
        match observe(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Ok(current) if Some(&current.object) == entry.identity.as_ref() => {}
            _ => return Err(refuse("fixture changed after cleanup preflight")),
        }
        if entry.kind == Kind::File {
            if entry.relative == Path::new("attributes.txt") {
                set_attributes(
                    &path,
                    windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL,
                )?;
            }
            fs::remove_file(path)?;
        } else {
            directories.retain(|(directory, _)| *directory != path);
            fs::remove_dir(path)?;
        }
    }
    fs::remove_file(root.join(MARKER))?;
    drop(directories);
    guards.pop(); // Release the root only; its parent stays pinned through the final unlink.
    fs::remove_dir(root)
}

#[cfg(windows)]
fn set_attributes(path: &Path, attributes: u32) -> io::Result<()> {
    let text = crate::win::wide(path)?;
    if unsafe {
        windows_sys::Win32::Storage::FileSystem::SetFileAttributesW(text.as_ptr(), attributes)
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn control(path: &Path, code: u32, input: &[u8]) -> io::Result<()> {
    use windows_sys::Win32::{
        Foundation::{GENERIC_READ, GENERIC_WRITE},
        Storage::FileSystem::*,
        System::IO::DeviceIoControl,
    };
    let handle = crate::win::open(
        path,
        GENERIC_READ | GENERIC_WRITE,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
    )?;
    let mut returned = 0;
    if unsafe {
        DeviceIoControl(
            handle.0,
            code,
            input.as_ptr().cast(),
            input.len() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
pub(crate) fn junction(path: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    fs::create_dir(path)?;
    let target = fs::canonicalize(target)?;
    let target = target.as_os_str().to_string_lossy();
    let normal = target.strip_prefix(r"\\?\").unwrap_or(&target);
    let substitute: Vec<_> = std::ffi::OsStr::new(&format!(r"\??\{normal}"))
        .encode_wide()
        .collect();
    let print: Vec<_> = std::ffi::OsStr::new(normal).encode_wide().collect();
    let mut buffer = Vec::new();
    buffer.extend_from_slice(&0xA0000003u32.to_le_bytes());
    buffer
        .extend_from_slice(&((8 + (substitute.len() + print.len() + 2) * 2) as u16).to_le_bytes());
    buffer.extend_from_slice(&0u16.to_le_bytes());
    for value in [
        0u16,
        (substitute.len() * 2) as u16,
        ((substitute.len() + 1) * 2) as u16,
        (print.len() * 2) as u16,
    ] {
        buffer.extend_from_slice(&value.to_le_bytes());
    }
    for value in substitute
        .iter()
        .chain(std::iter::once(&0))
        .chain(print.iter())
        .chain(std::iter::once(&0))
    {
        buffer.extend_from_slice(&value.to_le_bytes());
    }
    control(
        path,
        windows_sys::Win32::System::Ioctl::FSCTL_SET_REPARSE_POINT,
        &buffer,
    )
}
