//! Path rules for a grant root (docs/41 section 12, ADR-V3-08, ADR-V3-15), shared by every way a
//! root arrives: `--grant-root` on `loomward-serve` and the desktop folder picker (#167). A root is
//! an existing plain directory (not a link, junction or cloud placeholder), on a local volume
//! (never a UNC or device path, #146), that does not overlap the state directory or another root.
//! Volume roots come only through the owner's native picker. Path rules are necessary, not
//! sufficient: provenance (lab registry, dataset class) is checked against the root's native
//! identity by the service.

use loomward_protocol::RootGrantResultRefusal as Refusal;
use std::path::{Path, PathBuf};

/// Checks one root and returns it canonicalised.
pub fn check_root(
    root: &Path,
    state_dir: Option<&Path>,
    others: &[PathBuf],
    allow_volume_root: bool,
) -> Result<PathBuf, Refusal> {
    if network_path(root) {
        return Err(Refusal::NetworkOrRemovableUnsupported);
    }
    let meta = std::fs::symlink_metadata(root).map_err(|_| Refusal::NotADirectory)?;
    if meta.file_type().is_symlink() || unsafe_attributes(&meta) {
        return Err(Refusal::ReparseOrPlaceholder);
    }
    if !meta.is_dir() {
        return Err(Refusal::NotADirectory);
    }
    let canonical = root
        .canonicalize()
        .map_err(|_| Refusal::IdentityUnavailable)?;
    if network_path(&canonical) {
        return Err(Refusal::NetworkOrRemovableUnsupported);
    }
    if canonical.parent().is_none() && !allow_volume_root {
        return Err(Refusal::VolumeRoot);
    }
    if let Some(state) = state_dir {
        if canonical.starts_with(state) {
            return Err(Refusal::InsideStateDir);
        }
        if state.starts_with(&canonical) {
            return Err(Refusal::ContainsStateDir);
        }
    }
    if others
        .iter()
        .any(|r| canonical.starts_with(r) || r.starts_with(&canonical))
    {
        return Err(Refusal::AlreadyGranted);
    }
    Ok(canonical)
}

/// `--grant-root` list: every root canonicalised, or the first refusal named by rule and position,
/// never by path (private paths stay out of logs).
pub fn validate_roots(roots: &[PathBuf], state_dir: Option<&Path>) -> Result<Vec<PathBuf>, String> {
    let state = state_dir
        .map(|d| {
            d.canonicalize()
                .map_err(|_| "--state-dir must name an existing directory".to_string())
        })
        .transpose()?;
    let mut out: Vec<PathBuf> = Vec::new();
    for (i, root) in roots.iter().enumerate() {
        let canonical = check_root(root, state.as_deref(), &out, false).map_err(|r| {
            let rule = serde_json::to_value(r)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default();
            format!("grant root #{} refused: {rule}", i + 1)
        })?;
        out.push(canonical);
    }
    Ok(out)
}

/// UNC (`\\server\share`, `\\?\UNC\...`) and device (`\\.\`) paths: no silent network I/O
/// (invariant 2, #146). The verbatim local form `\\?\C:\` is local.
fn network_path(path: &Path) -> bool {
    let text = path
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_uppercase();
    text.starts_with("\\\\?\\UNC\\")
        || text.starts_with("\\\\.\\")
        || (text.starts_with("\\\\") && !text.starts_with("\\\\?\\"))
}

#[cfg(windows)]
fn unsafe_attributes(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const REPARSE_POINT: u32 = 0x400;
    const OFFLINE: u32 = 0x1000;
    const RECALL_ON_OPEN: u32 = 0x4_0000;
    const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
    meta.file_attributes() & (REPARSE_POINT | OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS) != 0
}

#[cfg(not(windows))]
fn unsafe_attributes(_: &std::fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusals_by_rule() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (a, b, state) = (base.join("a"), base.join("a").join("b"), base.join("state"));
        for d in [&a, &b, &state] {
            std::fs::create_dir_all(d).unwrap();
        }
        assert_eq!(
            validate_roots(std::slice::from_ref(&a), Some(&state))
                .unwrap()
                .len(),
            1
        );
        let rule =
            |roots: &[PathBuf], state: Option<&Path>| validate_roots(roots, state).unwrap_err();
        assert!(rule(&[a.clone(), b.clone()], None).ends_with("already_granted"));
        assert!(rule(std::slice::from_ref(&state), Some(&state)).ends_with("inside_state_dir"));
        assert!(rule(std::slice::from_ref(&base), Some(&state)).ends_with("contains_state_dir"));
        assert!(rule(&[base.join("missing")], None).ends_with("not_a_directory"));
        let file = base.join("f.txt");
        std::fs::write(&file, b"x").unwrap();
        assert!(rule(&[file], None).ends_with("not_a_directory"));
        let volume = base.ancestors().last().unwrap().to_path_buf();
        assert!(rule(std::slice::from_ref(&volume), None).ends_with("volume_root"));
        // The picker may grant a volume root; the CLI never does.
        assert!(check_root(&volume, None, &[], true).is_ok());
        for unc in [
            r"\\server\share\x",
            r"\\?\UNC\server\share\x",
            r"\\.\PhysicalDrive0",
            "//server/share",
        ] {
            assert_eq!(
                check_root(Path::new(unc), None, &[], true),
                Err(Refusal::NetworkOrRemovableUnsupported),
                "{unc}"
            );
        }
        assert!(rule(&[PathBuf::from(r"\\server\share")], None)
            .ends_with("network_or_removable_unsupported"));
        // A refusal names the rule and the position, never the path.
        assert!(!rule(&[a.clone(), b.clone()], None).contains(&*base.to_string_lossy()));
    }
}
