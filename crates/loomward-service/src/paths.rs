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
    if network_path(&absolute) {
        return Err("network_or_removable_unsupported".into());
    }
    let mut existing = None;
    // Inspect root first: even symlink_metadata follows links in intermediate components.
    for ancestor in absolute.ancestors().collect::<Vec<_>>().into_iter().rev() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(meta) => {
                if meta.file_type().is_symlink() || unsafe_attributes(&meta) {
                    return Err("reparse_or_placeholder".into());
                }
                if !meta.is_dir() {
                    return Err("not_a_directory".into());
                }
                existing = Some(ancestor);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) => return Err("cannot inspect directory".into()),
        }
    }
    let ancestor = existing.ok_or("cannot resolve directory")?;
    let mut resolved = ancestor
        .canonicalize()
        .map_err(|_| "cannot resolve directory")?;
    if network_path(&resolved) {
        return Err("network_or_removable_unsupported".into());
    }
    let missing = absolute
        .strip_prefix(ancestor)
        .map_err(|_| "cannot resolve directory")?;
    // An empty suffix would append a trailing separator to a fully existing path.
    if !missing.as_os_str().is_empty() {
        resolved.push(missing);
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
    if root.parent().and_then(Path::file_name).is_some_and(|name| {
        matches!(
            name.to_string_lossy().to_lowercase().as_str(),
            "users" | "profiles" | "documents and settings"
        )
    }) {
        return Err("whole profiles are not eligible".into());
    }
    for component in root.strip_prefix(profiles).unwrap_or(root).components() {
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
            ".docker",
            ".cargo",
            ".npm",
            ".terraform.d",
            ".gemini",
            ".m2",
            ".gradle",
            ".android",
            ".minikube",
            ".ollama",
            ".oci",
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

/// #286: the OS profiles directory itself, any folder directly inside it (every profile, however
/// its folder is named) and any root containing it are refused. Both paths are canonical. The
/// explicit native volume-root exception remains, as in `validate_profile_root`.
pub fn validate_profiles_dir(root: &Path, profiles: &Path) -> Result<(), String> {
    let folded = |p: &Path| {
        p.to_string_lossy()
            .replace('/', "\\")
            .to_lowercase()
            .trim_end_matches('\\')
            .to_string()
    };
    let (r, d) = (folded(root), folded(profiles));
    let inside = root.parent().is_some_and(|parent| folded(parent) == d);
    let contains = root.parent().is_some() && d.starts_with(&(r.clone() + "\\"));
    if r == d || inside || contains {
        return Err("whole profiles are not eligible".into());
    }
    Ok(())
}

/// The OS profile folder; on Windows never the environment. Elsewhere (the Linux CI leg only) `HOME`.
fn os_profile() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        windows_folder(loomward_shell::known_folders::Folder::Profile)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

/// The OS profiles directory (`FOLDERID_UserProfiles`); elsewhere the parent of `HOME`.
fn os_profiles_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        windows_folder(loomward_shell::known_folders::Folder::UserProfiles)
    }
    #[cfg(not(windows))]
    {
        os_profile().and_then(|p| p.parent().map(Path::to_path_buf))
    }
}

#[cfg(windows)]
fn windows_folder(folder: loomward_shell::known_folders::Folder) -> Option<PathBuf> {
    use loomward_shell::known_folders::{FolderProvider, Location, WindowsFolders};
    match WindowsFolders.location(folder) {
        Location::FileSystem { current, .. } => Some(current),
        _ => None,
    }
}

pub(crate) fn check_profile(root: &Path) -> Result<(), Refusal> {
    check_profile_with(root, os_profile(), os_profiles_dir())
}

/// Fails closed: an unresolvable profile or profiles directory refuses every root.
pub(crate) fn check_profile_with(
    root: &Path,
    profile: Option<PathBuf>,
    profiles: Option<PathBuf>,
) -> Result<(), Refusal> {
    let resolve = |p: Option<PathBuf>| {
        p.and_then(|p| p.canonicalize().ok())
            .ok_or(Refusal::ProtectedProfileOrStore)
    };
    let (profile, profiles) = (resolve(profile)?, resolve(profiles)?);
    validate_profiles_dir(root, &profiles)
        .and_then(|()| validate_profile_root(root, &profile))
        .map_err(|_| Refusal::ProtectedProfileOrStore)
}

/// Where tests create synthetic grant fixtures (#465): the workspace's `target/test-roots`, unless
/// the profile fence refuses it, as it rightly does for a checkout under `%LOCALAPPDATA%` (every
/// `%TEMP%` worktree). Then `loomward-test-roots` at the root of the same volume, which no store
/// rule names. The fixtures move; the fence never relaxes. Panics if neither place is eligible.
#[cfg(any(test, feature = "test-support"))]
pub fn test_roots_base() -> PathBuf {
    let eligible = |dir: PathBuf| {
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        check_profile(&dir).is_ok().then_some(dir)
    };
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-roots");
    eligible(target.clone())
        .or_else(|| {
            let volume = target
                .canonicalize()
                .unwrap()
                .ancestors()
                .last()?
                .to_owned();
            eligible(volume.join("loomward-test-roots"))
        })
        .expect("no fixture location outside protected profile stores (#465)")
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
    for (i, root) in roots.iter().enumerate() {
        if overlaps_stored(&dir, root) {
            return Err(format!("--static overlaps grant root #{}", i + 1));
        }
    }
    if state.is_some_and(|state| overlaps_stored(&dir, state)) {
        return Err("--static overlaps the state directory".into());
    }
    Ok(Some(dir))
}

/// Whether a canonical `candidate` overlaps a stored path (a grant or the state directory), either
/// way round. The one rule for `--static` and the folder picker: a stored path that resolves
/// compares canonically; one that does not (offline, renamed, deleted) compares as text, so it
/// can still refuse and never skips silently (#224).
pub(crate) fn overlaps_stored(candidate: &Path, stored: &Path) -> bool {
    match stored.canonicalize() {
        Ok(stored) => candidate.starts_with(&stored) || stored.starts_with(candidate),
        Err(_) => lexical_overlap(candidate, stored),
    }
}

/// Separator-normalised prefix overlap of two absolute paths, either way round, case-insensitive
/// only on Windows (the host filesystem's semantics).
/// The verbatim drive prefix (`\\?\`) is ignored, so a canonical path matches its stored form.
fn lexical_overlap(a: &Path, b: &Path) -> bool {
    fn norm(p: &Path) -> String {
        let s = p.to_string_lossy().replace('/', "\\");
        let s = if cfg!(windows) { s.to_lowercase() } else { s };
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
    check_root_with(root, state_dir, others, allow_volume_root, check_profile)
}

/// [`check_root`] with the profile fence supplied (the OS folders outside tests).
pub(crate) fn check_root_with(
    root: &Path,
    state_dir: Option<&Path>,
    others: &[PathBuf],
    allow_volume_root: bool,
    check_profile: impl Fn(&Path) -> Result<(), Refusal>,
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
pub(crate) fn network_path(path: &Path) -> bool {
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

/// Windows device names open a device, not a file, whatever the folder (`CON`, `COM1 .txt`).
pub(crate) fn reserved_device(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    let stem = name.split('.').next().unwrap_or("").trim_end_matches(' ');
    let stem = stem.to_ascii_uppercase();
    ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&stem.as_str())
        || ["COM", "LPT"].iter().any(|p| {
            stem.strip_prefix(p).is_some_and(|n| {
                (n.len() == 1 && n.as_bytes()[0].is_ascii_digit())
                    || matches!(n, "\u{00b9}" | "\u{00b2}" | "\u{00b3}")
            })
        })
}

#[cfg(windows)]
pub(crate) fn unsafe_attributes(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const REPARSE_POINT: u32 = 0x400;
    const OFFLINE: u32 = 0x1000;
    const RECALL_ON_OPEN: u32 = 0x4_0000;
    const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
    meta.file_attributes() & (REPARSE_POINT | OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS) != 0
}

#[cfg(not(windows))]
pub(crate) fn unsafe_attributes(_: &std::fs::Metadata) -> bool {
    false
}

/// Whether file attributes mark an online-only cloud file: offline, or recalled on open or data access.
#[cfg_attr(not(windows), allow(dead_code))] // only the Windows caller and the platform-independent test use it
pub(crate) fn placeholder_bits(attributes: u32) -> bool {
    const OFFLINE: u32 = 0x1000;
    const RECALL_ON_OPEN: u32 = 0x4_0000;
    const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
    attributes & (OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS) != 0
}

#[cfg(windows)]
pub(crate) fn cloud_placeholder(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    placeholder_bits(meta.file_attributes())
}

#[cfg(not(windows))]
pub(crate) fn cloud_placeholder(_: &std::fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn followup_os_profile_ignores_environment() {
        let mut passed = true;
        for redirected in [false, true] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "paths::tests::followup_profile_environment_child",
                ])
                .env("LOOMWARD_PROFILE_ENV_TEST", "1")
                .env_remove("HOME");
            if redirected {
                child.env("USERPROFILE", "C:/synthetic/redirected-profile");
            } else {
                child.env_remove("USERPROFILE");
            }
            // `--exact` matching nothing also exits 0: require the child to have run (#286 LOW).
            let output = child.output().unwrap();
            passed &= output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains("1 passed");
        }
        assert!(passed, "OS profile protection depended on the environment");
    }

    #[cfg(windows)]
    #[test]
    fn followup_profile_environment_child() {
        if std::env::var_os("LOOMWARD_PROFILE_ENV_TEST").is_none() {
            return;
        }
        use loomward_shell::known_folders::{Folder, FolderProvider, Location, WindowsFolders};
        let Location::FileSystem { current, .. } = WindowsFolders.location(Folder::Profile) else {
            panic!("profile unavailable");
        };
        assert_eq!(
            check_profile(&current.canonicalize().unwrap()),
            Err(Refusal::ProtectedProfileOrStore)
        );
        let parent = current.canonicalize().unwrap().parent().unwrap().to_owned();
        if parent.parent().is_some() {
            assert_eq!(
                check_profile(&parent),
                Err(Refusal::ProtectedProfileOrStore)
            );
        }
        let tmp = crate::tests::temp_root();
        assert!(check_profile(&tmp.path().canonicalize().unwrap()).is_ok());
    }

    #[test]
    fn followup_profiles_directory_is_protected_independently_of_profile() {
        let tmp = crate::tests::temp_root();
        let other = tmp.path().join("Users/other");
        std::fs::create_dir_all(&other).unwrap();
        assert!(validate_profile_root(&other, &tmp.path().join("redirected/fixture")).is_err());
        assert!(validate_profile_root(
            &other.join("Documents"),
            &tmp.path().join("redirected/fixture")
        )
        .is_ok());
    }

    /// #286 MEDIUM: the profiles directory comes from the OS, not from its folder's name. A
    /// sibling profile under a redirected, differently named profiles folder is refused; a
    /// non-profile sibling of a redirected profile stays eligible (#253).
    #[test]
    fn profiles_directory_is_resolved_from_the_os_not_by_name() {
        let tmp = crate::tests::temp_root();
        let base = tmp.path().canonicalize().unwrap();
        let (home, work, os) = (
            base.join("Home"),
            base.join("Work"),
            base.join("ProfileStore"),
        );
        for d in [
            home.join("me"),
            home.join("me.OLDPC").join("Documents"),
            work.join("me"),
            work.join("project"),
            os.join("other"),
        ] {
            std::fs::create_dir_all(d).unwrap();
        }
        let refused = Err(Refusal::ProtectedProfileOrStore);
        // Profiles directory `Home` (not a recognised name): its children are profiles.
        let with_home =
            |root: &Path| check_profile_with(root, Some(home.join("me")), Some(home.clone()));
        let sibling = home.join("me.OLDPC");
        // The name rule alone accepts the sibling profile: this is the gap #286 names.
        assert!(validate_profile_root(&sibling, &home.join("me")).is_ok());
        assert_eq!(with_home(&sibling), refused);
        assert_eq!(with_home(&home), refused);
        assert_eq!(
            with_home(&base),
            refused,
            "a root containing the profiles directory"
        );
        assert!(with_home(&sibling.join("Documents")).is_ok());
        assert!(with_home(&work.join("project")).is_ok());
        // #253: a redirected profile `Work\me`, OS profiles directory elsewhere.
        let redirected =
            |root: &Path| check_profile_with(root, Some(work.join("me")), Some(os.clone()));
        assert!(redirected(&work.join("project")).is_ok());
        assert_eq!(redirected(&work.join("me")), refused);
        assert_eq!(redirected(&os.join("other")), refused);
        // Unless `Work` is itself the OS profiles directory.
        assert_eq!(
            check_profile_with(
                &work.join("project"),
                Some(work.join("me")),
                Some(work.clone())
            ),
            refused
        );
        // Fail closed when either folder cannot be resolved.
        let project = work.join("project");
        assert_eq!(
            check_profile_with(&project, Some(work.join("me")), None),
            refused
        );
        assert_eq!(
            check_profile_with(&project, None, Some(os.clone())),
            refused
        );
        assert_eq!(
            check_profile_with(&project, Some(work.join("me")), Some(base.join("missing"))),
            refused
        );
    }

    #[test]
    fn hardening_profile_and_store_fence_is_shared_with_cli_roots() {
        let tmp = crate::tests::temp_root();
        let profile = tmp.path().join("profiles").join("fixture");
        let allowed = profile.join("Documents").join("lab");
        std::fs::create_dir_all(&allowed).unwrap();
        let profile = profile.canonicalize().unwrap();
        assert!(validate_profile_root(&allowed.canonicalize().unwrap(), &profile).is_ok());
        for relative in [
            ".m2",
            ".gradle",
            ".android",
            ".minikube",
            ".ollama",
            ".oci",
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
        if elevated() {
            assert_eq!(
                check_root(&allowed, None, &[], false),
                Err(Refusal::IdentityUnavailable)
            );
        } else {
            assert!(check_root(&allowed, None, &[], false).is_ok());
        }
    }

    /// Hosted CI runs elevated, and an elevated host refuses every root (the volume open).
    fn elevated() -> bool {
        #[cfg(windows)]
        {
            loomward_windows::enumerate::running_elevated()
        }
        #[cfg(not(windows))]
        {
            false
        }
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
        let tmp = crate::tests::temp_root();
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
        // A fully existing directory resolves exactly to its canonical form, with no trailing separator.
        assert_eq!(
            prospective_dir(tmp.path()).unwrap().as_os_str(),
            tmp.path().canonicalize().unwrap().as_os_str()
        );
    }

    #[test]
    fn refusals_by_rule() {
        let tmp = crate::tests::temp_root();
        let base = tmp.path().canonicalize().unwrap();
        let (a, b, state) = (base.join("a"), base.join("a").join("b"), base.join("state"));
        for d in [&a, &b, &state] {
            std::fs::create_dir_all(d).unwrap();
        }
        if elevated() {
            let refused = validate_roots(std::slice::from_ref(&a), Some(&state)).unwrap_err();
            assert!(refused.ends_with("identity_unavailable"), "{refused}");
            return;
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
        let tmp = crate::tests::temp_root();
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
        // Case folds only where the filesystem does (Windows).
        assert_eq!(
            lexical_overlap(
                &canonical.join("x"),
                &PathBuf::from(canonical.to_string_lossy().to_uppercase())
            ),
            cfg!(windows)
        );
        assert!(!lexical_overlap(
            &canonical,
            &canonical.with_file_name("dist-other")
        ));
    }

    /// #224: a refused `--static` names the stored grant's position, never its path.
    #[test]
    fn a_static_overlap_names_the_grant_position_not_the_path() {
        let tmp = crate::tests::temp_root();
        let dist = tmp.path().join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        let canonical = dist.canonicalize().unwrap();
        let gone = canonical.join("gone-grant");
        let state = tmp.path().join("state-dir");
        std::fs::create_dir_all(&state).unwrap();
        let elsewhere = canonical.with_file_name("elsewhere-gone");
        let named = |roots: &[PathBuf], state: Option<&Path>| {
            validate_static_dir(Some(&dist), roots, state).unwrap_err()
        };
        let second = named(&[elsewhere.clone(), gone.clone()], None);
        assert!(second.contains("#2"), "{second}");
        assert!(!second.contains(&*tmp.path().to_string_lossy()), "{second}");
        // A resolvable grant is numbered the same way.
        let live = named(&[elsewhere, canonical.clone()], None);
        assert!(live.contains("#2"), "{live}");
        let state_error = named(&[], Some(&canonical.join("state-dir")));
        assert!(state_error.contains("state directory"), "{state_error}");
        assert!(!state_error.contains("grant"), "{state_error}");
    }

    /// #224: the picker and `--static` share one rule for a stored grant that cannot resolve.
    #[test]
    fn a_stored_grant_overlap_falls_back_to_text_when_unresolvable() {
        let tmp = crate::tests::temp_root();
        let base = tmp.path().canonicalize().unwrap();
        let parent = base.join("ws");
        std::fs::create_dir_all(&parent).unwrap();
        let gone = parent.join("gone");
        assert!(overlaps_stored(&parent, &gone));
        assert_eq!(
            overlaps_stored(
                &parent,
                &PathBuf::from(gone.to_string_lossy().to_uppercase())
            ),
            cfg!(windows)
        );
        assert!(!overlaps_stored(&parent, &base.join("other-gone")));
        // A resolvable stored grant still compares by canonical path.
        assert!(overlaps_stored(&parent.join("."), &parent));
    }

    /// Case-sensitive hosts keep case in the lexical fallback (Codex P2 on #304).
    #[cfg(not(windows))]
    #[test]
    fn lexical_overlap_keeps_case_on_case_sensitive_hosts() {
        assert!(!lexical_overlap(
            Path::new("/data/foo"),
            Path::new("/data/Foo/gone")
        ));
        assert!(lexical_overlap(
            Path::new("/data/Foo"),
            Path::new("/data/Foo/gone")
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
