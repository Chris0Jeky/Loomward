//! Deterministic synthetic plans and a checked Windows directory-buffer decoder.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    io,
    path::{Path, PathBuf},
};

pub const MARKER: &str = "Loomward disposable scale lab v1\n";
pub const MAX_FILES: u64 = 3_000_000;
pub const QUEUE_LIMIT: usize = 1024;
pub const MAX_DEPTH: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    Dev,
    Media,
    Mixed,
}
impl Profile {
    pub fn parse(s: &str) -> io::Result<Self> {
        match s {
            "dev" => Ok(Self::Dev),
            "media" => Ok(Self::Media),
            "mixed" => Ok(Self::Mixed),
            _ => Err(invalid("profile must be dev, media or mixed")),
        }
    }
    pub fn bucket_size(self) -> u64 {
        match self {
            Self::Dev => 64,
            Self::Media => 128,
            Self::Mixed => 256,
        }
    }
}

pub fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

/// Deliberately a direct child: no relative paths, namespaces, traversal or ADS.
pub fn root_name(input: &str) -> io::Result<&str> {
    let prefix = ["G:\\loomward-lab\\scale\\", "E:\\loomward-lab\\scale\\"]
        .into_iter()
        .find(|p| {
            input
                .get(..p.len())
                .is_some_and(|s| s.eq_ignore_ascii_case(p))
        })
        .ok_or_else(|| invalid("root must be a direct child of G: or E: loomward-lab\\scale"))?;
    if input.len() <= prefix.len()
        || !input
            .get(..prefix.len())
            .is_some_and(|s| s.eq_ignore_ascii_case(prefix))
    {
        return Err(invalid(
            "root must be a direct child of G: or E: loomward-lab\\scale",
        ));
    }
    let name = &input[prefix.len()..];
    if name.len() > 80
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        || name.eq_ignore_ascii_case("con")
        || name.eq_ignore_ascii_case("prn")
        || name.eq_ignore_ascii_case("aux")
        || name.eq_ignore_ascii_case("nul")
        || (1..=9).any(|n| {
            name.eq_ignore_ascii_case(&format!("com{n}"))
                || name.eq_ignore_ascii_case(&format!("lpt{n}"))
        })
    {
        return Err(invalid(
            "root name must be non-device ASCII letters, digits, - or _",
        ));
    }
    Ok(name)
}

pub fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e3779b97f4a7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}

pub fn bucket_path(bucket: u64, seed: u64, profile: Profile) -> PathBuf {
    let mut path = PathBuf::from(format!("workspace-{:04}", bucket / 64));
    path.push(match profile {
        Profile::Dev => "node_modules/パッケージ-café",
        Profile::Media => "media/写真-é",
        Profile::Mixed => "projects/資料-café",
    });
    // Forty components including the shard and leaf; this deliberately crosses MAX_PATH.
    let depth = if bucket % 64 == 0 {
        36
    } else {
        (mix(bucket ^ seed) % 5) as usize
    };
    for level in 0..depth {
        path.push(format!("source-level-{level:02}"));
    }
    path.push(format!("package-{bucket:06}"));
    path
}

pub fn file_spec(index: u64, seed: u64, profile: Profile) -> (String, u64) {
    let h = mix(index ^ seed);
    let (extension, size) = match profile {
        Profile::Dev => (["rs", "js", "json", "ts"][h as usize % 4], h % 16385),
        Profile::Media => (["mkv", "wav", "raw"][h as usize % 3], (1 + h % 16) << 30),
        Profile::Mixed => match h % 100 {
            0..=69 => ("txt", 1 + (h >> 8) % 16384),
            70..=89 => ("jpg", (1 + (h >> 8) % 16) << 20),
            90..=97 => ("zip", (1 + (h >> 8) % 32) << 24),
            _ => ("model", (1 + (h >> 8) % 8) << 30),
        },
    };
    let case = if index % 2 == 0 { "Sample" } else { "sample" };
    (format!("{case}-{index:09}-λ.{extension}"), size)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Totals {
    pub files: u64,
    /// Descendants only: data/ itself is not counted.
    pub directories: u64,
    pub logical_bytes: u64,
    pub skipped_reparse: u64,
}
impl Totals {
    pub fn add(&mut self, other: Self) {
        self.files += other.files;
        self.directories += other.directories;
        self.logical_bytes += other.logical_bytes;
        self.skipped_reparse += other.skipped_reparse;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossTotals {
    #[serde(flatten)]
    pub totals: Totals,
    pub denied_directories: u64,
}

pub fn disagreements(
    baseline_name: &str,
    baseline: CrossTotals,
    name: &str,
    actual: CrossTotals,
) -> Vec<String> {
    [
        ("files", baseline.totals.files, actual.totals.files),
        (
            "directories",
            baseline.totals.directories,
            actual.totals.directories,
        ),
        (
            "logical_bytes",
            baseline.totals.logical_bytes,
            actual.totals.logical_bytes,
        ),
        (
            "skipped_reparse",
            baseline.totals.skipped_reparse,
            actual.totals.skipped_reparse,
        ),
        (
            "denied_directories",
            baseline.denied_directories,
            actual.denied_directories,
        ),
    ]
    .into_iter()
    .filter(|(_, expected, observed)| expected != observed)
    .map(|(field, expected, observed)| {
        format!("{name}.{field}={observed} differs from {baseline_name}.{field}={expected}")
    })
    .collect()
}

fn is_reserved_device_component(component: &str) -> bool {
    let trimmed = component.trim_end_matches([' ', '.']);
    let stem = trimmed
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches([' ', '.']);
    if stem.is_empty() {
        return false;
    }
    let lower = stem.to_ascii_lowercase();
    if lower.len() == 4
        && (lower.starts_with("com") || lower.starts_with("lpt"))
        && matches!(lower.as_bytes()[3], b'1'..=b'9')
    {
        return true;
    }
    matches!(
        lower.as_str(),
        "con" | "prn" | "aux" | "nul" | "com¹" | "com²" | "com³" | "lpt¹" | "lpt²" | "lpt³"
    )
}

/// Real observation scopes are explicit local paths, never profile or credential stores.
pub fn real_root(input: &str) -> io::Result<()> {
    let bytes = input.as_bytes();
    if bytes.len() < 4 || !bytes[0].is_ascii_alphabetic() || &bytes[1..3] != b":\\" {
        return Err(invalid(
            "real root must be an absolute local path below a volume root",
        ));
    }
    let parts: Vec<_> = input[3..].split('\\').collect();
    if parts.iter().any(|p| {
        p.is_empty()
            || *p == "."
            || *p == ".."
            || p.ends_with(['.', ' '])
            || p.contains([':', '/', '*', '?', '\0'])
    }) || parts.iter().any(|p| {
        [
            "users",
            "appdata",
            "browser",
            "chrome",
            "chromium",
            "firefox",
            "edge",
            "credentials",
            "keys",
            ".ssh",
            ".aws",
            ".azure",
            ".gnupg",
        ]
        .contains(&p.to_ascii_lowercase().as_str())
    }) || parts.iter().any(|p| is_reserved_device_component(p))
    {
        return Err(invalid(
            "profile, credential, browser, or ambiguous real root refused",
        ));
    }
    Ok(())
}

/// Direct children of `root` kept until last during teardown: the lab marker
/// and the busy file. NTFS resolves names case-insensitively, so the match is
/// by file name rather than case-sensitive path equality.
pub fn is_keeper(path: &Path, root: &Path) -> bool {
    if path.parent() != Some(root) {
        return false;
    }
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.eq_ignore_ascii_case(".loomward-lab-marker") || name.eq_ignore_ascii_case(".busy")
        })
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    pub complete: bool,
    pub seed: u64,
    pub profile: Profile,
    pub expected: Totals,
    pub file_data_allocation_bytes: u64,
    pub generation_seconds: f64,
    pub volume_free_before: u64,
    pub volume_free_after: u64,
}

pub fn expected(files: u64, seed: u64, profile: Profile) -> io::Result<Totals> {
    if files == 0 || files > MAX_FILES {
        return Err(invalid("files must be in 1..=3000000"));
    }
    let mut directories = BTreeSet::new();
    for bucket in 0..files.div_ceil(profile.bucket_size()) {
        let path = bucket_path(bucket, seed, profile);
        for parent in path.ancestors().filter(|p| !p.as_os_str().is_empty()) {
            directories.insert(parent.to_path_buf());
        }
    }
    Ok(Totals {
        files,
        directories: directories.len() as u64,
        logical_bytes: (0..files).map(|i| file_spec(i, seed, profile).1).sum(),
        skipped_reparse: 0,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub struct DirectoryRecord {
    pub name: Vec<u16>,
    pub logical_bytes: u64,
    pub allocation_bytes: u64,
    pub attributes: u32,
    pub reparse_tag: u32,
    pub file_id: [u8; 16],
}

/// FILE_ID_EXTD_DIR_INFO's variable-length chain; reject malformed lengths/offsets.
pub fn decode_records(
    buffer: &[u8],
    mut emit: impl FnMut(DirectoryRecord) -> io::Result<()>,
) -> io::Result<()> {
    let mut pos = 0;
    loop {
        let row = buffer
            .get(pos..)
            .ok_or_else(|| invalid("directory offset out of range"))?;
        if row.len() < 88 {
            return Err(invalid("truncated directory header"));
        }
        let u32_at = |offset| u32::from_le_bytes(row[offset..offset + 4].try_into().unwrap());
        let u64_at = |offset| u64::from_le_bytes(row[offset..offset + 8].try_into().unwrap());
        let next = u32_at(0) as usize;
        let name_len = u32_at(60) as usize;
        let end = 88usize
            .checked_add(name_len)
            .ok_or_else(|| invalid("name overflow"))?;
        if name_len == 0
            || name_len % 2 != 0
            || end > row.len()
            || (next != 0 && (next % 8 != 0 || next < end || next >= row.len()))
            || u64_at(40) > i64::MAX as u64
            || u64_at(48) > i64::MAX as u64
        {
            return Err(invalid("invalid directory record"));
        }
        let name: Vec<u16> = row[88..end]
            .chunks_exact(2)
            .map(|s| u16::from_le_bytes([s[0], s[1]]))
            .collect();
        if name.iter().any(|&c| matches!(c, 0 | 47 | 92 | 58)) {
            return Err(invalid("invalid directory name"));
        }
        emit(DirectoryRecord {
            name,
            logical_bytes: u64_at(40),
            allocation_bytes: u64_at(48),
            attributes: u32_at(56),
            reparse_tag: u32_at(68),
            file_id: row[72..88].try_into().unwrap(),
        })?;
        if next == 0 {
            return Ok(());
        }
        pos += next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cross_check_compares_every_counter_and_names_differences() {
        let baseline = CrossTotals {
            totals: Totals {
                files: 7,
                directories: 3,
                logical_bytes: 123,
                skipped_reparse: 2,
            },
            denied_directories: 1,
        };
        assert!(disagreements("std-single", baseline, "handle", baseline).is_empty());
        let changed = CrossTotals {
            totals: Totals {
                files: 8,
                directories: 4,
                logical_bytes: 124,
                skipped_reparse: 3,
            },
            denied_directories: 2,
        };
        let differences = disagreements("std-single", baseline, "handle", changed);
        assert_eq!(differences.len(), 5);
        assert_eq!(
            differences[0],
            "handle.files=8 differs from std-single.files=7"
        );
        assert_eq!(
            differences[4],
            "handle.denied_directories=2 differs from std-single.denied_directories=1"
        );
        for field in 0..5 {
            let mut single = baseline;
            match field {
                0 => single.totals.files += 1,
                1 => single.totals.directories += 1,
                2 => single.totals.logical_bytes += 1,
                3 => single.totals.skipped_reparse += 1,
                _ => single.denied_directories += 1,
            }
            assert_eq!(
                disagreements("baseline", baseline, "candidate", single).len(),
                1
            );
        }
    }
    #[test]
    fn scope_rejects_escape_and_devices() {
        assert_eq!(
            root_name(r"G:\loomward-lab\scale\test-1").unwrap(),
            "test-1"
        );
        for path in [
            r"G:\loomward-lab\scale",
            r"G:\loomward-lab\scale-other\x",
            r"G:\loomward-lab\scale\..\x",
            r"G:\loomward-lab\scale\x:y",
            r"G:\loomward-lab\scale\NUL",
            r"\\?\G:\loomward-lab\scale\x",
        ] {
            assert!(root_name(path).is_err(), "{path}");
        }
        assert_eq!(
            root_name(r"E:\loomward-lab\scale\mixed-1000000").unwrap(),
            "mixed-1000000"
        );
    }
    #[test]
    fn real_scope_rejects_broad_and_protected_paths() {
        for path in [r"C:\fixtures\real-A", r"E:\fixtures\real-D"] {
            assert!(real_root(path).is_ok());
        }
        for path in [
            r"C:\",
            r"C:\Users\owner",
            r"E:\Browser",
            r"E:\keys",
            r"E:\games\..\keys",
            r"E:\games:stream",
            r"E:\games\",
            r"\\?\E:\games",
            r"E:\games.",
            r"E:\games\AppData",
        ] {
            assert!(real_root(path).is_err(), "{path}");
        }
    }
    #[test]
    fn real_root_refuses_reserved_device_names() {
        for path in [
            r"G:\lab\COM1",
            r"G:\lab\con",
            r"G:\lab\NUL",
            r"G:\lab\nul.txt",
            r"G:\lab\Aux\x",
            r"G:\lab\com1.tar.gz",
            r"G:\lab\COM9\deep",
            r"G:\lab\deep\LPT1",
            r"G:\lab\con.tar.gz",
            r"G:\lab\COM¹",
            r"G:\lab\lpt³.txt",
            r"G:\lab\COM1 ",
            r"G:\lab\aux.",
        ] {
            assert!(real_root(path).is_err(), "{path}");
        }
        for path in [
            r"G:\lab\COM10",
            r"G:\lab\COM0",
            r"G:\lab\console",
            r"G:\lab\nullable",
            r"G:\lab\com1x",
        ] {
            assert!(real_root(path).is_ok(), "{path}");
        }
    }
    #[test]
    fn deterministic_profiles_and_independent_small_oracle() {
        for profile in [Profile::Dev, Profile::Mixed, Profile::Media] {
            let totals = expected(3, 42, profile).unwrap();
            assert_eq!(totals.files, 3);
            assert_eq!(totals.directories, 40);
            assert_eq!(
                totals.logical_bytes,
                file_spec(0, 42, profile).1
                    + file_spec(1, 42, profile).1
                    + file_spec(2, 42, profile).1
            );
            assert_eq!(bucket_path(0, 42, profile), bucket_path(0, 42, profile));
            assert!(bucket_path(0, 42, profile).to_string_lossy().len() > 260);
            assert_ne!(file_spec(0, 42, profile), file_spec(0, 43, profile));
        }
        assert!(expected(0, 0, Profile::Dev).is_err());
        assert!(expected(MAX_FILES + 1, 0, Profile::Dev).is_err());
        assert!(Profile::parse("bogus").is_err());
    }
    #[test]
    fn buffer_decoder_accepts_unicode_and_rejects_corruption() {
        let mut buffer = vec![0u8; 96];
        buffer[40..48].copy_from_slice(&123u64.to_le_bytes());
        buffer[60..64].copy_from_slice(&2u32.to_le_bytes());
        buffer[88..90].copy_from_slice(&0x03bbu16.to_le_bytes());
        let mut count = 0;
        decode_records(&buffer, |r| {
            assert_eq!(r.name, [0x03bb]);
            assert_eq!(r.logical_bytes, 123);
            count += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(count, 1);
        for len in [0u32, 1, 1000] {
            buffer[60..64].copy_from_slice(&len.to_le_bytes());
            assert!(decode_records(&buffer, |_| Ok(())).is_err());
        }
        buffer[60..64].copy_from_slice(&2u32.to_le_bytes());
        buffer[0..4].copy_from_slice(&8u32.to_le_bytes());
        assert!(decode_records(&buffer, |_| Ok(())).is_err());
        buffer[0..4].copy_from_slice(&0u32.to_le_bytes());
        buffer[88..90].copy_from_slice(&92u16.to_le_bytes());
        assert!(decode_records(&buffer, |_| Ok(())).is_err());
    }

    #[test]
    fn buffer_decoder_follows_offsets_and_propagates_failure() {
        let mut buffer = vec![0u8; 192];
        buffer[0..4].copy_from_slice(&96u32.to_le_bytes());
        for pos in [0, 96] {
            buffer[pos + 60..pos + 64].copy_from_slice(&2u32.to_le_bytes());
            buffer[pos + 88..pos + 90].copy_from_slice(&65u16.to_le_bytes());
        }
        let mut count = 0;
        decode_records(&buffer, |_| {
            count += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(count, 2);
        assert!(decode_records(&buffer, |_| Err(invalid("consumer failed"))).is_err());
        buffer[96 + 48..96 + 56].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(decode_records(&buffer, |_| Ok(())).is_err());
    }

    fn dir_record(
        next: u32,
        name: &[u16],
        logical: u64,
        allocation: u64,
        attributes: u32,
        reparse_tag: u32,
        file_id: [u8; 16],
    ) -> Vec<u8> {
        let mut buf = vec![0u8; 88 + name.len() * 2];
        buf[0..4].copy_from_slice(&next.to_le_bytes());
        buf[40..48].copy_from_slice(&logical.to_le_bytes());
        buf[48..56].copy_from_slice(&allocation.to_le_bytes());
        buf[56..60].copy_from_slice(&attributes.to_le_bytes());
        buf[60..64].copy_from_slice(&(name.len() as u32 * 2).to_le_bytes());
        buf[68..72].copy_from_slice(&reparse_tag.to_le_bytes());
        buf[72..88].copy_from_slice(&file_id);
        for (i, unit) in name.iter().enumerate() {
            buf[88 + i * 2..90 + i * 2].copy_from_slice(&unit.to_le_bytes());
        }
        buf
    }

    fn chain_records(first: Vec<u8>, stride: usize, second: Vec<u8>) -> Vec<u8> {
        let mut buf = first;
        assert!(buf.len() <= stride);
        buf.resize(stride, 0);
        buf.extend(second);
        buf
    }

    #[test]
    fn decode_records_emits_two_record_chain_with_fields() {
        let first = dir_record(96, &[0x41], 10, 20, 0x10, 0, [1; 16]);
        let second = dir_record(0, &[0x48, 0x69], 30, 40, 0x20, 7, [2; 16]);
        let buffer = chain_records(first, 96, second);
        let mut seen = Vec::new();
        decode_records(&buffer, |r| {
            seen.push((
                r.name.clone(),
                r.logical_bytes,
                r.allocation_bytes,
                r.attributes,
                r.reparse_tag,
                r.file_id,
            ));
            Ok(())
        })
        .unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0], (vec![0x41], 10, 20, 0x10, 0, [1; 16]));
        assert_eq!(seen[1], (vec![0x48, 0x69], 30, 40, 0x20, 7, [2; 16]));
    }

    #[test]
    fn decode_records_rejects_bad_name_lengths() {
        let empty = dir_record(0, &[], 1, 1, 0, 0, [0; 16]);
        assert!(decode_records(&empty, |_| Ok(())).is_err());
        let mut odd = dir_record(0, &[0x41], 1, 1, 0, 0, [0; 16]);
        odd[60..64].copy_from_slice(&3u32.to_le_bytes());
        odd.push(0);
        assert!(decode_records(&odd, |_| Ok(())).is_err());
        let mut overrun = dir_record(0, &[0x41], 1, 1, 0, 0, [0; 16]);
        overrun[60..64].copy_from_slice(&1000u32.to_le_bytes());
        assert!(decode_records(&overrun, |_| Ok(())).is_err());
    }

    #[test]
    fn decode_records_rejects_bad_next_offsets() {
        let mut unaligned = dir_record(92, &[0x41], 1, 1, 0, 0, [0; 16]);
        unaligned.resize(104, 0);
        assert!(decode_records(&unaligned, |_| Ok(())).is_err());
        let mut overlap = dir_record(88, &[0x41], 1, 1, 0, 0, [0; 16]);
        overlap.resize(184, 0);
        assert!(decode_records(&overlap, |_| Ok(())).is_err());
        let past_end = dir_record(96, &[0x41], 1, 1, 0, 0, [0; 16]);
        assert!(decode_records(&past_end, |_| Ok(())).is_err());
    }

    #[test]
    fn decode_records_rejects_reserved_name_characters() {
        for bad in [0x0000u16, 0x002F, 0x005C, 0x003A] {
            let buffer = dir_record(0, &[bad], 1, 1, 0, 0, [0; 16]);
            assert!(decode_records(&buffer, |_| Ok(())).is_err());
        }
    }

    #[test]
    fn decode_records_rejects_sizes_above_i64_max() {
        let huge = i64::MAX as u64 + 1;
        let logical = dir_record(0, &[0x41], huge, 1, 0, 0, [0; 16]);
        assert!(decode_records(&logical, |_| Ok(())).is_err());
        let allocation = dir_record(0, &[0x41], 1, huge, 0, 0, [0; 16]);
        assert!(decode_records(&allocation, |_| Ok(())).is_err());
        let boundary = dir_record(0, &[0x41], i64::MAX as u64, i64::MAX as u64, 0, 0, [0; 16]);
        assert!(decode_records(&boundary, |_| Ok(())).is_ok());
    }

    #[test]
    fn decode_records_rejects_short_header() {
        for len in [0usize, 10, 87] {
            let buffer = vec![0u8; len];
            assert!(decode_records(&buffer, |_| Ok(())).is_err());
        }
    }

    #[cfg(windows)]
    #[test]
    fn decoder_offsets_match_windows_abi() {
        use std::mem::offset_of;
        use windows_sys::Win32::Storage::FileSystem::FILE_ID_EXTD_DIR_INFO;
        assert_eq!(offset_of!(FILE_ID_EXTD_DIR_INFO, FileName), 88);
        assert_eq!(offset_of!(FILE_ID_EXTD_DIR_INFO, EndOfFile), 40);
        assert_eq!(offset_of!(FILE_ID_EXTD_DIR_INFO, AllocationSize), 48);
        assert_eq!(offset_of!(FILE_ID_EXTD_DIR_INFO, ReparsePointTag), 68);
        assert_eq!(offset_of!(FILE_ID_EXTD_DIR_INFO, FileId), 72);
    }
}
