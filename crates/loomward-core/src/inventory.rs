//! Portable metadata scanner. No contents, recursion through links, or Windows USN acceleration.
//! This path-based reference is NOT a hardened handle-relative filesystem boundary.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

fn hex(value: &str) -> String {
    hex_bytes(value.as_bytes())
}
fn hex_bytes(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}
fn record_id_bytes(root_key: &str, rel: &std::ffi::OsStr) -> Vec<u8> {
    // Raw bytes on Unix, WTF-8 on Windows (UTF-8 for any valid name), so existing IDs stay stable
    // while distinct invalid names no longer collapse through to_string_lossy.
    let raw = rel.as_encoded_bytes();
    let mut out = Vec::with_capacity(root_key.len() + 1 + raw.len());
    out.extend_from_slice(root_key.as_bytes());
    out.push(0);
    out.extend(raw.iter().map(|&b| if b == b'\\' { b'/' } else { b }));
    out
}
fn unix_ns(t: std::io::Result<SystemTime>) -> Option<String> {
    t.ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|n| n.as_nanos().to_string())
}
fn excluded(meta: &Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // reparse point, offline, recall-on-open, recall-on-data-access
        meta.file_attributes() & (0x400 | 0x1000 | 0x40000 | 0x400000) != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}
fn allocation(meta: &Metadata) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        meta.blocks().checked_mul(512)
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        None
    }
}
fn link_count(meta: &Metadata) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(meta.nlink())
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        None
    }
}
fn sensitive(name: &str) -> bool {
    let n = name.to_lowercase();
    n == ".env"
        || n.starts_with(".env.")
        || [".pem", ".key", ".pfx", ".p12", ".kdbx"]
            .iter()
            .any(|x| n.ends_with(x))
        || n == "id_rsa"
        || n == "id_ed25519"
}
fn root_path(root: &Path) -> Result<PathBuf, String> {
    let absolute = if root.is_absolute() {
        root.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(root)
    };
    for p in absolute.ancestors() {
        let m = fs::symlink_metadata(p).map_err(|e| e.to_string())?;
        if excluded(&m) {
            return Err("linked/offline root or ancestor is excluded".into());
        }
    }
    let path = absolute.canonicalize().map_err(|e| e.to_string())?;
    if !path.is_dir() {
        return Err("scan root must be a directory".into());
    }
    Ok(path)
}

pub fn scan(root: &Path, max_entries: usize, max_depth: usize) -> Result<Value, String> {
    if !(1..=200_000).contains(&max_entries) || max_depth > 256 {
        return Err("invalid scan bounds".into());
    }
    let root = root_path(root)?;
    let start = Instant::now();
    let root_key = hex(&root.to_string_lossy())[..16].to_string();
    let mut stack = vec![(root.clone(), 0_usize)];
    let mut files = Vec::new();
    let mut examined = 0;
    let mut dirs = 0;
    let mut skipped = 0;
    let mut errors = 0;
    let mut limit_hit = false;
    let mut logical = 0_u64;
    let mut known = 0_u64;
    let mut unknown = 0;
    while let Some((dir, depth)) = stack.pop() {
        if excluded(&fs::symlink_metadata(&dir).map_err(|e| e.to_string())?) {
            skipped += 1;
            continue;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(x) => x,
            Err(_) => {
                errors += 1;
                continue;
            }
        };
        for entry in entries {
            if examined >= max_entries {
                limit_hit = true;
                break;
            }
            examined += 1;
            let entry = match entry {
                Ok(x) => x,
                Err(_) => {
                    errors += 1;
                    continue;
                }
            };
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let meta = match fs::symlink_metadata(&path) {
                Ok(x) => x,
                Err(_) => {
                    errors += 1;
                    continue;
                }
            };
            if excluded(&meta)
                || [".loomward", "$recycle.bin", "system volume information"]
                    .contains(&name.to_lowercase().as_str())
            {
                skipped += 1;
                continue;
            }
            if meta.is_dir() {
                dirs += 1;
                if depth >= max_depth {
                    skipped += 1;
                } else {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            if !meta.is_file() {
                skipped += 1;
                continue;
            }
            let rel_path = path
                .strip_prefix(&root)
                .map_err(|e| e.to_string())?
                .to_owned();
            let rel = rel_path.to_string_lossy().replace('\\', "/");
            let allocated = allocation(&meta);
            let nlink = link_count(&meta);
            let mut flags = Vec::new();
            if rel_path.as_os_str().to_str().is_none() {
                flags.push("lossy_path");
            }
            if sensitive(&name) {
                flags.push("sensitive");
            }
            if rel.split('/').any(|p| {
                [".git", ".hg", ".svn", "windows", "program files", "appdata"]
                    .contains(&p.to_lowercase().as_str())
            }) {
                flags.push("protected_context");
            }
            if nlink.is_some_and(|n| n > 1) {
                flags.push("hardlinked");
            }
            if allocated.is_some_and(|n| n < meta.len()) {
                flags.push("sparse_or_compressed");
            }
            logical = logical
                .checked_add(meta.len())
                .ok_or("logical byte overflow")?;
            if let Some(n) = allocated {
                known = known.checked_add(n).ok_or("allocation byte overflow")?;
            } else {
                unknown += 1;
            }
            let ext = path
                .extension()
                .map(|s| format!(".{}", s.to_string_lossy().to_lowercase()))
                .unwrap_or_default();
            files.push(json!({"id":hex_bytes(&record_id_bytes(&root_key, rel_path.as_os_str()))[..24],"name":name,"relative_path":rel,
                "extension":ext,"size_bytes":meta.len(),"allocated_bytes":allocated,"mtime_ns":unix_ns(meta.modified()),
                "ctime_ns":null,"nlink":nlink,"flags":flags,"identity_quality":"native_path_observation_not_execution_identity"}));
        }
        if limit_hit {
            break;
        }
    }
    files.sort_by(|a, b| {
        a["relative_path"]
            .as_str()
            .cmp(&b["relative_path"].as_str())
    });
    Ok(
        json!({"schema_version":1,"mode":"observed","runtime":"rust_source_foundation","root":root.to_string_lossy(),"root_key":root_key,
        "observed_at_unix_seconds":SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|d|d.as_secs()),
        "summary":{"file_count":files.len(),"directory_count":dirs,"logical_bytes":logical,"allocated_bytes_known_sum":known,"allocated_unknown_count":unknown,"elapsed_seconds":start.elapsed().as_secs_f64(),"allocation_note":"Per-entry values, not deduplicated physical usage."},
        "coverage":{"complete_under_policy":!limit_hit&&errors==0,"unqualified_complete":!limit_hit&&errors==0&&skipped==0,"limit_hit":limit_hit,"examined_entries":examined,"skipped_count":skipped,"error_count":errors,"max_entries":max_entries,"max_depth":max_depth},
        "files":files,"capabilities":{"file_mutation":false,"process_mutation":false,"content_hash":false},
        "identity_note":"IDs are observation keys, not stable native file IDs. Native mtime_ns is a decimal string. Do not pass this snapshot to the Python duplicate reader."}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn logical_count_is_correct() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("a.txt"), b"hello").unwrap();
        let s = scan(t.path(), 100, 64).unwrap();
        assert_eq!(s["summary"]["logical_bytes"], 5);
        assert_eq!(s["summary"]["file_count"], 1);
    }
    #[test]
    fn zero_budget_rejected() {
        let t = tempfile::tempdir().unwrap();
        assert!(scan(t.path(), 0, 64).is_err());
    }
    #[test]
    fn file_is_not_root() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a");
        fs::write(&p, b"a").unwrap();
        assert!(scan(&p, 10, 64).is_err());
    }
    #[test]
    fn no_implicit_content_reads() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join(".env"), b"SECRET=1").unwrap();
        let s = scan(t.path(), 10, 64).unwrap();
        assert!(s["files"][0]["flags"]
            .as_array()
            .unwrap()
            .contains(&json!("sensitive")));
        assert!(!s.to_string().contains("SECRET=1"));
    }
    #[cfg(unix)]
    #[test]
    fn symlink_is_not_followed() {
        use std::os::unix::fs::symlink;
        let t = tempfile::tempdir().unwrap();
        let o = tempfile::tempdir().unwrap();
        fs::write(o.path().join("secret"), b"secret").unwrap();
        symlink(o.path(), t.path().join("link")).unwrap();
        assert_eq!(scan(t.path(), 10, 64).unwrap()["summary"]["file_count"], 0);
    }
    #[cfg(target_os = "linux")] // APFS and NTFS refuse non-UTF-8 names
    #[test]
    fn distinct_non_utf8_names_have_distinct_ids() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join(OsStr::from_bytes(b"a_\xff")), b"x").unwrap();
        fs::write(t.path().join(OsStr::from_bytes(b"a_\xfe")), b"y").unwrap();
        let s = scan(t.path(), 100, 64).unwrap();
        assert_eq!(s["summary"]["file_count"], 2);
        let files = s["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
        assert_ne!(
            files[0]["id"].as_str().unwrap(),
            files[1]["id"].as_str().unwrap(),
            "distinct non-UTF-8 names must not collapse to the same record ID"
        );
        for f in files {
            let flags = f["flags"].as_array().unwrap();
            assert!(flags.contains(&json!("lossy_path")));
        }
    }
    #[test]
    fn valid_name_id_is_unchanged_and_stable() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("hello.txt"), b"hello").unwrap();
        let first = scan(t.path(), 100, 64).unwrap();
        let second = scan(t.path(), 100, 64).unwrap();
        let file = &first["files"][0];
        assert_eq!(file["id"], second["files"][0]["id"]);
        let flags = file["flags"].as_array().unwrap();
        assert!(!flags.contains(&json!("lossy_path")));
        // A valid name keeps the pre-fix ID on every platform.
        let rel = file["relative_path"].as_str().unwrap();
        let root_key = first["root_key"].as_str().unwrap();
        let expected = hex(&format!("{root_key}\0{rel}"));
        assert_eq!(&expected[..24], file["id"].as_str().unwrap());
    }
}
