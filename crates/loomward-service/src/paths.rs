//! Path rules for a grant root (docs/41 section 12, ADR-V3-08, ADR-V3-15), shared by every way a
//! root arrives: `--grant-root` on `loomward-serve` and the desktop folder picker (#167). A root is
//! an existing plain directory (not a link, junction or cloud placeholder), on a local volume
//! (never a UNC or device path, #146), that does not overlap the state directory or another root.
//! Volume roots come only through the owner's native picker. Path rules are necessary, not
//! sufficient: provenance (lab registry, dataset class) is checked against the root's native
//! identity by the service.

use loomward_protocol::RootGrantResultRefusal as Refusal;
use std::path::{Path, PathBuf};

/// Resolve a local destination before creating it; reject unsafe existing ancestors.
pub fn prospective_dir(path: &Path) -> Result<PathBuf, String> {
    if network_path(path) {
        return Err("network_or_removable_unsupported".into());
    }
    let absolute = std::path::absolute(path).map_err(|_| "cannot resolve directory")?;
    let mut ancestor = absolute.as_path();
    let mut missing = Vec::new();
    loop {
        match std::fs::symlink_metadata(ancestor) {
            Ok(meta) => {
                if meta.file_type().is_symlink() || unsafe_attributes(&meta) {
                    return Err("reparse_or_placeholder".into());
                }
                if !meta.is_dir() {
                    return Err("not_a_directory".into());
                }
                // Check every existing ancestor before canonicalisation can follow it.
                for parent in ancestor.ancestors().skip(1) {
                    let meta = std::fs::symlink_metadata(parent)
                        .map_err(|_| "cannot inspect directory")?;
                    if meta.file_type().is_symlink() || unsafe_attributes(&meta) {
                        return Err("reparse_or_placeholder".into());
                    }
                }
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                missing.push(ancestor.file_name().ok_or("cannot resolve directory")?);
                ancestor = ancestor.parent().ok_or("cannot resolve directory")?;
            }
            Err(_) => return Err("cannot inspect directory".into()),
        }
    }
    let mut resolved = ancestor
        .canonicalize()
        .map_err(|_| "cannot resolve directory")?;
    if network_path(&resolved) {
        return Err("network_or_removable_unsupported".into());
    }
    for part in missing.iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}

/// Shared CLI/picker fence. The explicit native volume-root exception remains.
pub fn validate_profile_root(root: &Path, profile: &Path) -> Result<(), String> {
    let folded = |p: &Path| p.to_string_lossy().replace('/', "\\").to_lowercase();
    let (r, p) = (folded(root), folded(profile));
    if root.parent().is_some() && (r == p || p.starts_with(&(r.clone() + "\\"))) {
        return Err("whole profiles are not eligible".into());
    }
    let profiles = profile.parent().ok_or("profile unavailable")?;
    if root
        .parent()
        .is_some_and(|parent| folded(parent) == folded(profiles))
    {
        return Err("whole profiles are not eligible".into());
    }
    for component in root.components() {
        if [
            ".ssh",
            ".aws",
            ".azure",
            ".gnupg",
            ".kube",
            ".codex",
            ".claude",
            "appdata",
            ".mozilla",
            ".config",
            "user data",
            "browser profiles",
            "login data",
            "keychains",
        ]
        .contains(
            &component
                .as_os_str()
                .to_string_lossy()
                .to_lowercase()
                .as_str(),
        ) {
            return Err("credential and browser stores are not eligible".into());
        }
    }
    Ok(())
}

fn check_profile(root: &Path) -> Result<(), Refusal> {
    let profile = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
    let profile = profile
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/unavailable/profile"));
    let profile = profile.canonicalize().unwrap_or(profile);
    validate_profile_root(root, &profile).map_err(|_| Refusal::ProtectedProfileOrStore)
}

pub(crate) fn check_new_root(root: &Path, state: &Path) -> Result<PathBuf, String> {
    let root = prospective_dir(root)?;
    if root.parent().is_none() {
        return Err("volume_root".into());
    }
    check_profile(&root).map_err(crate::refusal_name)?;
    let ancestor = root
        .ancestors()
        .find(|p| p.is_dir())
        .ok_or("root parent unavailable")?;
    if loomward_windows::volumes::is_removable(ancestor).map_err(|_| "identity_unavailable")? {
        return Err("network_or_removable_unsupported".into());
    }
    if root.starts_with(state) || state.starts_with(&root) {
        return Err("lab root overlaps the state directory".into());
    }
    Ok(root)
}

/// Static files must be disjoint from all active grants and the state directory.
pub fn validate_static_dir(
    dir: Option<&Path>,
    roots: &[PathBuf],
    state: Option<&Path>,
) -> Result<Option<PathBuf>, String> {
    let Some(dir) = dir else { return Ok(None) };
    let dir = dir.canonicalize().map_err(|_| "cannot resolve --static")?;
    if !dir.is_dir() {
        return Err("--static must name a directory".into());
    }
    for protected in roots.iter().map(PathBuf::as_path).chain(state) {
        let overlaps = match protected.canonicalize() {
            Ok(protected) => dir.starts_with(&protected) || protected.starts_with(&dir),
            // An offline, renamed or deleted grant cannot be resolved. Compare its stored path as
            // text so the server still starts, and still refuse any textual overlap.
            Err(_) => lexical_overlap(&dir, protected),
        };
        if overlaps {
            return Err("--static overlaps a grant root or the state directory".into());
        }
    }
    Ok(Some(dir))
}

/// Case-insensitive, separator-normalised prefix overlap of two absolute paths, either way round.
/// The verbatim drive prefix (`\\?\`) is ignored, so a canonical path matches its stored form.
fn lexical_overlap(a: &Path, b: &Path) -> bool {
    fn norm(p: &Path) -> String {
        let s = p.to_string_lossy().replace('/', "\\").to_lowercase();
        let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
        s.trim_end_matches('\\').to_string() + "\\"
    }
    let (a, b) = (norm(a), norm(b));
    a.starts_with(&b) || b.starts_with(&a)
}

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
    check_profile(&canonical)?;
    if loomward_windows::volumes::is_removable(&canonical)
        .map_err(|_| Refusal::IdentityUnavailable)?
    {
        return Err(Refusal::NetworkOrRemovableUnsupported);
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
    let state = state_dir.map(prospective_dir).transpose()?;
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

/// UNC (`\\server\share`, `\\?\UNC\...`), device (`\\.\`, `\??\`) and every other verbatim
/// namespace (`\\?\GLOBALROOT\...` shadow copies and raw devices, `\\?\Volume{..}`): no silent
/// network or device I/O (invariant 2, #146, #185). Only the verbatim drive form `\\?\C:\` is local.
fn network_path(path: &Path) -> bool {
    let text = path.to_string_lossy().replace('/', "\\");
    if let Some(rest) = text.strip_prefix("\\\\?\\") {
        let b = rest.as_bytes();
        let drive = b.len() >= 2
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && (b.len() == 2 || b[2] == b'\\');
        return !drive;
    }
    text.starts_with("\\\\") || text.starts_with("\\??\\")
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
    fn hardening_profile_and_store_fence_is_shared_with_cli_roots() {
        let tmp = tempfile::tempdir_in(".").unwrap();
        let profile = tmp.path().join("profiles").join("fixture");
        let allowed = profile.join("Documents").join("lab");
        std::fs::create_dir_all(&allowed).unwrap();
        let profile = profile.canonicalize().unwrap();
        assert!(validate_profile_root(&allowed.canonicalize().unwrap(), &profile).is_ok());
        for relative in [
            ".ssh",
            ".aws",
            ".azure",
            ".gnupg",
            ".kube",
            ".codex",
            ".claude",
            "AppData",
            ".mozilla",
            "User Data",
        ] {
            let store = profile.join(relative).join("nested");
            std::fs::create_dir_all(&store).unwrap();
            assert_eq!(
                check_root(&store, None, &[], false),
                Err(Refusal::ProtectedProfileOrStore)
            );
            assert!(validate_profile_root(&store.canonicalize().unwrap(), &profile).is_err());
        }
        assert!(validate_profile_root(&profile, &profile).is_err());
        assert!(validate_profile_root(profile.parent().unwrap(), &profile).is_err());
        let other = profile.parent().unwrap().join("other");
        std::fs::create_dir_all(&other).unwrap();
        assert!(validate_profile_root(&other, &profile).is_err());
        assert!(check_root(&allowed, None, &[], false).is_ok());
    }

    #[test]
    fn hardening_prospective_paths_reject_network_before_any_inspection() {
        for path in [
            r"\\?\GLOBALROOT\Device\fixture",
            r"\\server\share\lab",
            r"\\?\UNC\server\share\lab",
        ] {
            assert_eq!(
                prospective_dir(Path::new(path)).unwrap_err(),
                "network_or_removable_unsupported"
            );
        }
        let tmp = tempfile::tempdir_in(".").unwrap();
        let state = tmp.path().join("missing").join("state");
        let canonical = prospective_dir(&state).unwrap();
        assert_eq!(
            canonical,
            tmp.path()
                .canonicalize()
                .unwrap()
                .join("missing")
                .join("state")
        );
        assert!(!state.exists());
    }

    #[test]
    fn refusals_by_rule() {
        let tmp = tempfile::tempdir_in(".").unwrap();
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
        assert!(rule(&[PathBuf::from(r"\\server\share")], Some(&state))
            .ends_with("network_or_removable_unsupported"));
        // A refusal names the rule and the position, never the path.
        assert!(!rule(&[a.clone(), b.clone()], None).contains(&*base.to_string_lossy()));
    }

    /// #224: an unresolvable stored grant compares as text instead of failing the start.
    #[test]
    fn an_offline_grant_is_compared_lexically() {
        let tmp = tempfile::tempdir_in(".").unwrap();
        let dist = tmp.path().join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        let canonical = dist.canonicalize().unwrap();
        let elsewhere = tmp.path().join("gone-elsewhere");
        assert_eq!(
            validate_static_dir(Some(&dist), &[elsewhere], None),
            Ok(Some(canonical.clone()))
        );
        // Stored grant paths are canonical (long names), as check_root records them.
        assert!(validate_static_dir(Some(&dist), &[canonical.join("gone")], None).is_err());
        assert!(lexical_overlap(&canonical, &canonical.join("gone")));
        assert!(lexical_overlap(
            &canonical.join("x"),
            &PathBuf::from(canonical.to_string_lossy().to_uppercase())
        ));
        assert!(!lexical_overlap(
            &canonical,
            &canonical.with_file_name("dist-other")
        ));
    }

    /// #185 item 4: only the verbatim drive form is local; every other verbatim or device
    /// namespace (shadow copies, raw devices, volume GUIDs, pipes, NT `\??\`) is refused.
    #[test]
    fn only_the_verbatim_drive_form_is_local() {
        for device in [
            r"\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy1\x",
            r"\\?\globalroot\Device\HarddiskVolume1\x",
            "//?/GLOBALROOT/Device/HarddiskVolume1/x",
            r"\\?\Volume{00000000-0000-0000-0000-000000000000}\x",
            r"\\?\pipe\x",
            r"\\?\C",
            r"\\?\C:x",
            r"\\?\",
            r"\??\C:\x",
        ] {
            assert_eq!(
                check_root(Path::new(device), None, &[], true),
                Err(Refusal::NetworkOrRemovableUnsupported),
                "{device}"
            );
        }
        for local in [r"\\?\C:\x", r"\\?\c:", r"C:\x", "x"] {
            assert!(!network_path(Path::new(local)), "{local}");
        }
    }
}
