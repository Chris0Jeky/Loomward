use std::{io, path::Path};
pub const MAX_FILE_BYTES: usize = 1024 * 1024;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Text {
    Unicode(String),
    Ansi(Vec<u8>),
}
impl Text {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Unicode(s) => Some(s),
            Self::Ansi(b) if b.is_ascii() => std::str::from_utf8(b).ok(),
            _ => None,
        }
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shortcut {
    pub flags: u32,
    pub file_attributes: u32,
    pub id_list: Option<Vec<u8>>,
    pub link_info: Option<LinkInfo>,
    pub name: Option<Text>,
    pub relative_path: Option<Text>,
    pub working_directory: Option<Text>,
    pub arguments: Option<Text>,
    pub icon_location: Option<Text>,
    pub environment_target: Option<Text>,
    pub known_folder: Option<KnownFolder>,
    pub tracker: Option<Tracker>,
    pub unknown_extra_blocks: Vec<u32>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkInfo {
    pub local_base_path: Option<Text>,
    pub suffix: Text,
    pub volume: Option<Volume>,
    pub network: Option<Network>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    pub drive_type: u32,
    pub serial: u32,
    pub label: Text,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Network {
    pub share: Text,
    pub device: Option<Text>,
    pub provider_type: Option<u32>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownFolder {
    pub id: [u8; 16],
    pub id_list_offset: u32,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tracker {
    pub machine_id: Vec<u8>,
    pub droid: [u8; 32],
    pub birth_droid: [u8; 32],
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError(pub &'static str);
/// Parse bounded MS-SHLLINK bytes. ANSI text remains bytes: the producer's code page is unknown.
pub fn parse(bytes: &[u8]) -> Result<Shortcut, ParseError> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(ParseError("file size exceeds cap"));
    }
    let mut r = Cursor { bytes, pos: 0 };
    let h = r.take(76)?;
    if u32_at(h, 0)? != 76 || h[4..20] != [1, 20, 2, 0, 0, 0, 0, 0, 192, 0, 0, 0, 0, 0, 0, 70] {
        return Err(ParseError("invalid shell link header"));
    }
    if h[66..76].iter().any(|b| *b != 0) {
        return Err(ParseError("nonzero header reserved fields"));
    }
    let mut s = Shortcut {
        flags: u32_at(h, 20)?,
        file_attributes: u32_at(h, 24)?,
        ..Shortcut::default()
    };
    if s.flags & 1 != 0 {
        let len = r.u16()? as usize;
        let id = r.take(len)?;
        validate_ids(id)?;
        s.id_list = Some(id.to_vec());
    }
    if s.flags & 2 != 0 {
        s.link_info = Some(link_info(r.block(28)?)?);
    }
    for (flag, field) in [
        (4, &mut s.name),
        (8, &mut s.relative_path),
        (16, &mut s.working_directory),
        (32, &mut s.arguments),
        (64, &mut s.icon_location),
    ] {
        if s.flags & flag != 0 {
            let len = r.u16()? as usize;
            if len > MAX_TEXT_UNITS {
                return Err(ParseError("string length exceeds cap"));
            }
            *field = Some(if s.flags & 128 != 0 {
                unicode(r.take(len * 2)?)?
            } else {
                Text::Ansi(r.take(len)?.to_vec())
            });
        }
    }
    let mut blocks = 0;
    loop {
        let size = u32_at(r.bytes, r.pos)?;
        if size < 4 {
            r.take(4)?;
            if r.pos != bytes.len() {
                return Err(ParseError("trailing data after terminal block"));
            }
            break;
        }
        blocks += 1;
        if blocks > 64 {
            return Err(ParseError("extra block count exceeds cap"));
        }
        let b = r.block(8)?;
        let signature = u32_at(b, 4)?;
        match signature {
            0xa0000001 => {
                if b.len() != 788 || s.environment_target.is_some() {
                    return Err(ParseError("invalid environment block"));
                }
                let ansi = terminated(&b[8..268], false)?;
                let wide = terminated(&b[268..788], true)?;
                s.environment_target = Some(if wide.as_str() == Some("") {
                    ansi
                } else {
                    wide
                });
            }
            0xa000000b => {
                if b.len() != 28 || s.known_folder.is_some() {
                    return Err(ParseError("invalid known folder block"));
                }
                let offset = u32_at(b, 24)?;
                if s.id_list
                    .as_ref()
                    .is_some_and(|id| offset as usize >= id.len())
                {
                    return Err(ParseError("known folder IDList offset outside list"));
                }
                s.known_folder = Some(KnownFolder {
                    id: b[8..24].try_into().unwrap(),
                    id_list_offset: offset,
                });
            }
            0xa0000003 => {
                if b.len() != 96
                    || u32_at(b, 8)? != 88
                    || u32_at(b, 12)? != 0
                    || s.tracker.is_some()
                {
                    return Err(ParseError("invalid tracker block"));
                }
                let end = b[16..32]
                    .iter()
                    .position(|c| *c == 0)
                    .ok_or(ParseError("unterminated machine ID"))?;
                s.tracker = Some(Tracker {
                    machine_id: b[16..16 + end].to_vec(),
                    droid: b[32..64].try_into().unwrap(),
                    birth_droid: b[64..96].try_into().unwrap(),
                });
            }
            _ => s.unknown_extra_blocks.push(signature),
        }
    }
    Ok(s)
}

const MAX_TEXT_UNITS: usize = 32768;
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}
impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], ParseError> {
        let end = self
            .pos
            .checked_add(count)
            .ok_or(ParseError("length overflow"))?;
        let slice = self
            .bytes
            .get(self.pos..end)
            .ok_or(ParseError("truncated structure"))?;
        self.pos = end;
        Ok(slice)
    }
    fn u16(&mut self) -> Result<u16, ParseError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn block(&mut self, min: usize) -> Result<&'a [u8], ParseError> {
        let size = u32_at(self.bytes, self.pos)? as usize;
        if size < min {
            return Err(ParseError("invalid block size"));
        }
        self.take(size)
    }
}
fn u32_at(b: &[u8], at: usize) -> Result<u32, ParseError> {
    let end = at.checked_add(4).ok_or(ParseError("offset overflow"))?;
    Ok(u32::from_le_bytes(
        b.get(at..end)
            .ok_or(ParseError("truncated integer"))?
            .try_into()
            .unwrap(),
    ))
}
fn unicode(b: &[u8]) -> Result<Text, ParseError> {
    if b.len() % 2 != 0 {
        return Err(ParseError("odd UTF-16 length"));
    }
    let units: Vec<_> = b
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16(&units)
        .map(Text::Unicode)
        .map_err(|_| ParseError("invalid UTF-16"))
}
fn terminated(b: &[u8], wide: bool) -> Result<Text, ParseError> {
    if wide {
        let len = b
            .chunks_exact(2)
            .take(MAX_TEXT_UNITS + 1)
            .position(|c| c == [0, 0])
            .ok_or(ParseError("unterminated or oversized Unicode string"))?;
        unicode(&b[..len * 2])
    } else {
        let len = b
            .iter()
            .take(MAX_TEXT_UNITS + 1)
            .position(|c| *c == 0)
            .ok_or(ParseError("unterminated or oversized ANSI string"))?;
        Ok(Text::Ansi(b[..len].to_vec()))
    }
}
fn offset(b: &[u8], at: usize, min: usize) -> Result<usize, ParseError> {
    let n = u32_at(b, at)? as usize;
    if n < min || n >= b.len() {
        return Err(ParseError("offset outside structure"));
    }
    Ok(n)
}
fn text_at(b: &[u8], at: usize, min: usize, wide: bool) -> Result<Text, ParseError> {
    terminated(&b[offset(b, at, min)?..], wide)
}
fn preferred_text(
    b: &[u8],
    ansi: usize,
    wide: Option<usize>,
    min: usize,
) -> Result<Text, ParseError> {
    let a = text_at(b, ansi, min, false)?;
    match wide {
        Some(at) if u32_at(b, at)? != 0 => text_at(b, at, min, true),
        _ => Ok(a),
    }
}
fn validate_ids(id: &[u8]) -> Result<(), ParseError> {
    let mut r = Cursor { bytes: id, pos: 0 };
    for _ in 0..4096 {
        let size = r.u16()? as usize;
        if size == 0 {
            return if r.pos == id.len() {
                Ok(())
            } else {
                Err(ParseError("trailing IDList data"))
            };
        }
        if size < 2 {
            return Err(ParseError("invalid ItemID size"));
        }
        r.take(size - 2)?;
    }
    Err(ParseError("ItemID count exceeds cap"))
}
fn link_info(b: &[u8]) -> Result<LinkInfo, ParseError> {
    let header = u32_at(b, 4)? as usize;
    if (header != 28 && header < 36) || header > b.len() {
        return Err(ParseError("invalid LinkInfo header size"));
    }
    let flags = u32_at(b, 8)?;
    if flags & !3 != 0 {
        return Err(ParseError("invalid LinkInfo flags"));
    }
    let wide = header >= 36;
    let suffix = preferred_text(b, 24, wide.then_some(32), header)?;
    let (volume, local_base_path) = if flags & 1 != 0 {
        let start = offset(b, 12, header)?;
        let mut r = Cursor {
            bytes: &b[start..],
            pos: 0,
        };
        let v = r.block(16)?;
        let label_at = u32_at(v, 12)?;
        let label = if label_at == 20 {
            text_at(v, 16, 20, true)?
        } else {
            text_at(v, 12, 16, false)?
        };
        let drive_type = u32_at(v, 4)?;
        if drive_type > 6 {
            return Err(ParseError("invalid volume drive type"));
        }
        (
            Some(Volume {
                drive_type,
                serial: u32_at(v, 8)?,
                label,
            }),
            Some(preferred_text(b, 16, wide.then_some(28), header)?),
        )
    } else {
        if u32_at(b, 12)? != 0 || u32_at(b, 16)? != 0 || (wide && u32_at(b, 28)? != 0) {
            return Err(ParseError("local offsets without local flag"));
        }
        (None, None)
    };
    let network = if flags & 2 != 0 {
        let start = offset(b, 20, header)?;
        let mut r = Cursor {
            bytes: &b[start..],
            pos: 0,
        };
        Some(network_info(r.block(20)?)?)
    } else {
        if u32_at(b, 20)? != 0 {
            return Err(ParseError("network offset without network flag"));
        }
        None
    };
    Ok(LinkInfo {
        local_base_path,
        suffix,
        volume,
        network,
    })
}
fn network_info(b: &[u8]) -> Result<Network, ParseError> {
    let flags = u32_at(b, 4)?;
    if flags & !3 != 0 {
        return Err(ParseError("invalid network flags"));
    }
    let wide = u32_at(b, 8)? > 20;
    let header = if wide { 28 } else { 20 };
    let share = preferred_text(b, 8, wide.then_some(20), header)?;
    let device = if flags & 1 != 0 {
        Some(preferred_text(b, 12, wide.then_some(24), header)?)
    } else {
        if u32_at(b, 12)? != 0 || (wide && u32_at(b, 24)? != 0) {
            return Err(ParseError("device offset without device flag"));
        }
        None
    };
    let provider_type = if flags & 2 != 0 {
        Some(u32_at(b, 16)?)
    } else {
        None
    };
    Ok(Network {
        share,
        device,
        provider_type,
    })
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Present,
    Missing,
    VolumeUnavailable,
    NetworkUnreachableUnknown,
    Ambiguous,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeState {
    Available,
    Unavailable,
    Network,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataState {
    Present,
    Missing,
    Unknown,
}
pub trait TargetProbe {
    fn volume(&self, drive: char, expected_serial: Option<u32>) -> VolumeState;
    fn metadata(&self, path: &str) -> MetadataState;
}
/// Point-in-time evidence only. No relative resolution, environment expansion or network access.
pub fn health(s: &Shortcut, probe: &impl TargetProbe) -> Health {
    if s.environment_target.is_some() || s.flags & (0x200 | 0x2000000) != 0 {
        return Health::Ambiguous;
    }
    // A shell ID, advertised installer or unsupported resolver block cannot establish a path.
    if s.flags & (0x100 | 0x1000) != 0 || !s.unknown_extra_blocks.is_empty() {
        return Health::Unknown;
    }
    let Some(info) = &s.link_info else {
        return if s.relative_path.is_some() || s.known_folder.is_some() {
            Health::Ambiguous
        } else {
            Health::Unknown
        };
    };
    if info.network.is_some() || info.volume.as_ref().is_some_and(|v| v.drive_type == 4) {
        return Health::NetworkUnreachableUnknown;
    }
    let (Some(base), Some(suffix)) = (
        info.local_base_path.as_ref().and_then(Text::as_str),
        info.suffix.as_str(),
    ) else {
        return Health::Unknown;
    };
    if base.contains('%') || suffix.contains('%') {
        return Health::Ambiguous;
    }
    if base.starts_with("\\\\") {
        return if base.starts_with("\\\\?\\") || base.starts_with("\\\\.\\") {
            Health::Unknown
        } else {
            Health::NetworkUnreachableUnknown
        };
    }
    if base.as_bytes().get(1) != Some(&b':') || base.as_bytes().get(2) != Some(&b'\\') {
        return Health::Ambiguous;
    }
    // MS-SHLLINK appends the common suffix; it is never interpreted as an independent root.
    let path = format!("{base}{suffix}");
    let Some(drive) = local_drive(&path) else {
        return Health::Unknown;
    };
    let serial = info.volume.as_ref().map(|v| v.serial);
    match probe.volume(drive, serial) {
        VolumeState::Unavailable => return Health::VolumeUnavailable,
        VolumeState::Network => return Health::NetworkUnreachableUnknown,
        VolumeState::Unknown => return Health::Unknown,
        VolumeState::Available => {}
    }
    match probe.metadata(&path) {
        MetadataState::Present => Health::Present,
        MetadataState::Unknown => Health::Unknown,
        MetadataState::Missing => match probe.volume(drive, serial) {
            VolumeState::Available => Health::Missing,
            VolumeState::Unavailable => Health::VolumeUnavailable,
            VolumeState::Network => Health::NetworkUnreachableUnknown,
            VolumeState::Unknown => Health::Unknown,
        },
    }
}

fn local_drive(path: &str) -> Option<char> {
    let b = path.as_bytes();
    if b.len() < 3
        || path.encode_utf16().count() > MAX_TEXT_UNITS
        || !b[0].is_ascii_alphabetic()
        || b[1] != b':'
        || b[2] != b'\\'
        || path.contains(['\0', '/', '%'])
        || path.chars().any(|c| c < ' ')
    {
        return None;
    }
    for part in path[3..].split('\\').filter(|s| !s.is_empty()) {
        if part == "."
            || part == ".."
            || part.ends_with(['.', ' '])
            || part.contains([':', '*', '?', '"', '<', '>', '|'])
        {
            return None;
        }
        let stem = part
            .split('.')
            .next()?
            .trim_end_matches(' ')
            .to_ascii_uppercase();
        if ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&stem.as_str())
            || (stem.chars().count() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.chars().nth(3), Some('1'..='9' | '¹' | '²' | '³')))
        {
            return None;
        }
    }
    Some((b[0] as char).to_ascii_uppercase())
}
pub struct WindowsProbe;
impl TargetProbe for WindowsProbe {
    fn volume(&self, drive: char, serial: Option<u32>) -> VolumeState {
        #[cfg(windows)]
        {
            native::volume(drive, serial)
        }
        #[cfg(not(windows))]
        {
            let _ = (drive, serial);
            VolumeState::Unknown
        }
    }
    fn metadata(&self, path: &str) -> MetadataState {
        #[cfg(windows)]
        {
            native::metadata(path)
        }
        #[cfg(not(windows))]
        {
            let _ = path;
            MetadataState::Unknown
        }
    }
}
/// Explicit caller-selected local file only; never follows a shell target or repairs a shortcut.
pub fn read(path: &Path) -> io::Result<Shortcut> {
    #[cfg(windows)]
    {
        native::read(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Windows guarded shortcut reader unavailable",
        ))
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use loomward_windows::enumerate::{DirSource, NativeDir, NativeSource};
    use std::{
        fs::{File, OpenOptions},
        io::Read,
        mem::{size_of, zeroed},
        os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
        ptr,
    };
    use windows_sys::Win32::{
        Storage::FileSystem::*,
        System::WindowsProgramming::{DRIVE_NO_ROOT_DIR, DRIVE_REMOTE, DRIVE_UNKNOWN},
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    fn invalid(reason: &'static str) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidInput, reason)
    }
    pub(super) fn volume(drive: char, expected: Option<u32>) -> VolumeState {
        if !drive.is_ascii_alphabetic() {
            return VolumeState::Unknown;
        }
        let root = wide(&format!("{drive}:\\"));
        match unsafe { GetDriveTypeW(root.as_ptr()) } {
            DRIVE_REMOTE => return VolumeState::Network,
            DRIVE_NO_ROOT_DIR => return VolumeState::Unavailable,
            DRIVE_UNKNOWN => return VolumeState::Unknown,
            _ => {}
        }
        let mut serial = 0;
        if unsafe {
            GetVolumeInformationW(
                root.as_ptr(),
                ptr::null_mut(),
                0,
                &mut serial,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                0,
            )
        } == 0
        {
            return match io::Error::last_os_error().raw_os_error() {
                Some(21 | 55 | 1167) => VolumeState::Unavailable,
                _ => VolumeState::Unknown,
            };
        }
        if expected.is_some_and(|s| s != serial) {
            VolumeState::Unavailable
        } else {
            VolumeState::Available
        }
    }

    /// Retain every ancestor and final metadata handle with writes/renames refused for this read.
    fn pins(path: &str) -> io::Result<(NativeDir, Vec<File>)> {
        let drive = local_drive(path)
            .ok_or_else(|| invalid("only ordinary absolute local paths supported"))?;
        if volume(drive, None) != VolumeState::Available {
            return Err(invalid("local volume unavailable or unknown"));
        }
        let root = format!("{drive}:\\");
        // Reuse the existing fail-closed token check; a failed elevation query is not permission.
        let (guard, _) = NativeSource::default()
            .open_root(Path::new(&root))
            .map_err(|_| invalid("non-elevated guarded access required"))?;
        let mut files = Vec::new();
        let mut current = root;
        let parts: Vec<_> = path[3..].split('\\').filter(|s| !s.is_empty()).collect();
        for i in 0..=parts.len() {
            if i > 0 {
                if !current.ends_with('\\') {
                    current.push('\\');
                }
                current.push_str(parts[i - 1]);
            }
            let file = OpenOptions::new()
                .access_mode(FILE_READ_ATTRIBUTES)
                .share_mode(FILE_SHARE_READ)
                .custom_flags(
                    FILE_FLAG_OPEN_NO_RECALL
                        | FILE_FLAG_OPEN_REPARSE_POINT
                        | FILE_FLAG_BACKUP_SEMANTICS,
                )
                .open(&current)?;
            let attrs = attributes(&file)?;
            if loomward_windows::enumerate::refused_attributes(attrs) {
                return Err(invalid("placeholder, offline or reparse path refused"));
            }
            if i < parts.len() && attrs & FILE_ATTRIBUTE_DIRECTORY == 0 {
                return Err(invalid("ancestor is not a directory"));
            }
            files.push(file);
        }
        Ok((guard, files))
    }
    fn attributes(file: &File) -> io::Result<u32> {
        if unsafe { GetFileType(file.as_raw_handle()) } != FILE_TYPE_DISK {
            return Err(invalid("non-disk file refused"));
        }
        let mut info: FILE_ATTRIBUTE_TAG_INFO = unsafe { zeroed() };
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
        Ok(info.FileAttributes)
    }
    pub(super) fn metadata(path: &str) -> MetadataState {
        match pins(path) {
            Ok(_) => MetadataState::Present,
            Err(e) if matches!(e.raw_os_error(), Some(2 | 3)) => MetadataState::Missing,
            Err(_) => MetadataState::Unknown,
        }
    }
    pub(super) fn read(path: &Path) -> io::Result<Shortcut> {
        let text = path
            .to_str()
            .ok_or_else(|| invalid("path is not Unicode"))?;
        let (_root, pins) = pins(text)?;
        let metadata = pins.last().unwrap();
        if attributes(metadata)? & FILE_ATTRIBUTE_DIRECTORY != 0 {
            return Err(invalid("shortcut is a directory"));
        }
        if metadata.metadata()?.len() > MAX_FILE_BYTES as u64 {
            return Err(invalid("file size exceeds cap"));
        }
        let file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_NO_RECALL | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        if loomward_windows::enumerate::refused_attributes(attributes(&file)?) {
            return Err(invalid("placeholder, offline or reparse file refused"));
        }
        let mut bytes = Vec::new();
        file.take((MAX_FILE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        parse(&bytes).map_err(|e| invalid(e.0))
    }
}
