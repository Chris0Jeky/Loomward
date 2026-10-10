//! `--grant-root` refusal rules (docs/41 section 12, ADR-V3-08, ADR-V3-15).
//!
//! Browser mode has no folder picker, so roots come only from the command line. A synthetic
//! session accepts only lab roots registered in the synthetic `state.db` (`lab_root`, written by
//! `loomward-lab generate --register`). That registry does not exist yet, so a synthetic session
//! refuses every `--grant-root` (fail closed) until the service lane can check it. A personal
//! session accepts an existing owner folder that is not a volume root, not a reparse point or
//! cloud placeholder, and does not overlap the state directory or another root.

use loomward_protocol::DatasetClass;
use std::path::{Path, PathBuf};

/// Validates the roots and returns them canonicalised. The error names the rule, never the path
/// (private paths stay out of logs).
pub fn validate_roots(
    dataset: DatasetClass,
    roots: &[PathBuf],
    state_dir: Option<&Path>,
) -> Result<Vec<PathBuf>, String> {
    if dataset == DatasetClass::Synthetic && !roots.is_empty() {
        return Err(
            "synthetic_session_requires_lab_root: a synthetic session accepts only \
            registered lab roots, and no lab-root registry exists yet"
                .into(),
        );
    }
    let state = state_dir
        .map(|d| {
            d.canonicalize()
                .map_err(|_| "--state-dir must name an existing directory".to_string())
        })
        .transpose()?;
    let mut out: Vec<PathBuf> = Vec::new();
    for (i, root) in roots.iter().enumerate() {
        let n = i + 1;
        let meta = std::fs::symlink_metadata(root)
            .map_err(|_| format!("grant root #{n} does not exist"))?;
        if !meta.is_dir() || meta.file_type().is_symlink() || unsafe_attributes(&meta) {
            return Err(format!(
                "grant root #{n} must be a plain directory, not a link, junction or cloud placeholder"
            ));
        }
        let canonical = root
            .canonicalize()
            .map_err(|_| format!("grant root #{n} cannot be resolved"))?;
        if canonical.parent().is_none() {
            return Err(format!(
                "grant root #{n} is a volume root; volumes are granted only through the native picker"
            ));
        }
        let overlaps = |a: &Path, b: &Path| a.starts_with(b) || b.starts_with(a);
        if state.as_deref().is_some_and(|s| overlaps(&canonical, s)) {
            return Err(format!("grant root #{n} overlaps the state directory"));
        }
        if out.iter().any(|r| overlaps(&canonical, r)) {
            return Err(format!("grant root #{n} overlaps another grant root"));
        }
        out.push(canonical);
    }
    Ok(out)
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

    fn tmp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lw-http-grants-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn synthetic_refuses_every_root_until_the_lab_registry_exists() {
        let a = tmp("syn");
        let err =
            validate_roots(DatasetClass::Synthetic, std::slice::from_ref(&a), None).unwrap_err();
        assert!(err.starts_with("synthetic_session_requires_lab_root"));
        assert_eq!(
            validate_roots(DatasetClass::Synthetic, &[], None),
            Ok(vec![])
        );
        let _ = std::fs::remove_dir_all(a);
    }

    #[test]
    fn personal_refusals() {
        let base = tmp("personal");
        let (a, b, state) = (base.join("a"), base.join("a").join("b"), base.join("state"));
        for d in [&a, &b, &state] {
            std::fs::create_dir_all(d).unwrap();
        }
        let p = DatasetClass::Personal;
        assert_eq!(
            validate_roots(p, std::slice::from_ref(&a), Some(&state))
                .unwrap()
                .len(),
            1
        );
        assert!(validate_roots(p, &[a.clone(), b.clone()], None)
            .unwrap_err()
            .contains("another grant root"));
        assert!(
            validate_roots(p, std::slice::from_ref(&state), Some(&state))
                .unwrap_err()
                .contains("state directory")
        );
        assert!(validate_roots(p, std::slice::from_ref(&base), Some(&state))
            .unwrap_err()
            .contains("state directory"));
        assert!(validate_roots(p, &[base.join("missing")], None)
            .unwrap_err()
            .contains("does not exist"));
        let root = PathBuf::from(std::path::Component::RootDir.as_os_str());
        let volume = std::env::temp_dir()
            .ancestors()
            .last()
            .map(Path::to_path_buf)
            .unwrap_or(root);
        assert!(validate_roots(p, &[volume], None)
            .unwrap_err()
            .contains("volume root"));
        let file = base.join("f.txt");
        std::fs::write(&file, b"x").unwrap();
        assert!(validate_roots(p, &[file], None)
            .unwrap_err()
            .contains("plain directory"));
        let _ = std::fs::remove_dir_all(base);
    }
}
