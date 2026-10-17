//! The lab-root registry (ADR-V3-15, docs/41 section 12): the only way a directory becomes
//! grantable in a synthetic session. Registration creates the directory itself (refusing one that
//! already holds anything), records its native identity in the synthetic `state.db`, and a later
//! grant must present a root with exactly that identity. A path alone never qualifies.

use crate::{db, now_ns, paths};
use loomward_catalog::Catalog;
use loomward_protocol::RootGrantResultRefusal as Refusal;
use std::path::Path;

pub(crate) fn valid_digest(digest: &str) -> bool {
    loomward_protocol::Digest::new(digest).is_ok()
}

pub(crate) fn log_digest(digest: &str) -> Result<(), Refusal> {
    if !valid_digest(digest) {
        return Err(Refusal::IdentityUnavailable);
    }
    eprintln!("Loomward lab grant: registered manifest digest {digest} (not reverified)");
    Ok(())
}

/// `(volume_key, root_file_id)` of a directory, observed from an open handle. The 128-bit ID is
/// kept whole; a legacy 64-bit ID retains its actual width.
pub fn identity(root: &Path) -> Result<(String, Vec<u8>), Refusal> {
    let observed =
        loomward_windows::identity::observe(root).map_err(|_| Refusal::IdentityUnavailable)?;
    let object = observed.object;
    identity_parts(object, false)
}

fn identity_parts(
    object: loomward_windows::identity::ObjectKey,
    require_128: bool,
) -> Result<(String, Vec<u8>), Refusal> {
    let id = match object.quality {
        loomward_windows::identity::IdentityQuality::FileId128 => object.file_id.to_vec(),
        loomward_windows::identity::IdentityQuality::Legacy64 { .. } => {
            if require_128 {
                return Err(Refusal::IdentityUnavailable);
            }
            object.file_id[..8].to_vec()
        }
    };
    Ok((format!("vsn_{}", object.volume_serial), id))
}

/// Creates `root` (which must not exist, or be an empty directory) and registers it as a lab root
/// in the synthetic store under `state_dir`. Returns the registry row id. `loomward-lab generate
/// --register` is the intended caller; it fills the directory afterwards.
pub fn register(
    state_dir: &Path,
    root: &Path,
    seed: &str,
    tier: &str,
    manifest_digest: &str,
) -> Result<i64, String> {
    if !valid_digest(manifest_digest) {
        return Err("invalid lab manifest digest".into());
    }
    let state_dir = paths::prospective_dir(state_dir)?;
    let root = paths::check_new_root(root, &state_dir)?;
    Catalog::validate_stores(&state_dir, "synthetic").map_err(|e| format!("state store: {e}"))?;
    let ancestor = root
        .ancestors()
        .find(|p| p.is_dir())
        .ok_or("lab parent unavailable")?;
    let (_ancestor_pins, observed) = loomward_windows::identity::pin_directory(ancestor)
        .map_err(|_| "cannot pin the lab parent")?;
    identity_parts(observed.object, true)
        .map_err(|r| format!("lab root refused: {}", crate::refusal_name(r)))?;
    if root.exists() {
        paths::check_root(&root, Some(&state_dir), &[], false)
            .map_err(|r| format!("lab root refused: {}", crate::refusal_name(r)))?;
    }
    std::fs::create_dir_all(&root).map_err(|_| "cannot create the lab root")?;
    let (_root_pins, observed) =
        loomward_windows::identity::pin_directory(&root).map_err(|_| "cannot pin the lab root")?;
    let root = paths::check_root(&root, Some(&state_dir), &[], false)
        .map_err(|r| format!("lab root refused: {}", crate::refusal_name(r)))?;
    if std::fs::read_dir(&root)
        .map_err(|_| "the lab root cannot be read")?
        .next()
        .is_some()
    {
        return Err("a lab root must be a new or empty directory".into());
    }
    #[cfg(test)]
    AFTER_EMPTY.with(|hook| {
        if let Some(hook) = hook.take() {
            hook();
        }
    });
    let (volume_key, file_id) = identity_parts(observed.object, true)
        .map_err(|r| format!("lab root refused: {}", crate::refusal_name(r)))?;
    // Opening the catalogue creates or migrates the store and refuses a personal one.
    let _catalog =
        Catalog::open(&state_dir, "synthetic").map_err(|e| format!("state store: {e}"))?;
    let conn = db::open(&state_dir).map_err(|e| format!("state store: {e}"))?;
    db::insert_lab_root(
        &conn,
        &volume_key,
        &file_id,
        manifest_digest,
        seed,
        tier,
        now_ns(),
    )
    .map_err(|e| format!("lab registry: {e}"))
}

#[cfg(test)]
thread_local! {
    static AFTER_EMPTY: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = Default::default();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn registration_pins_empty_root_against_personal_replacement() {
        let temp = crate::tests::temp_root();
        let root = temp.path().join("lab");
        let personal = temp.path().join("personal-fixture");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&personal).unwrap();
        std::fs::write(personal.join("synthetic-data.bin"), b"fixture").unwrap();
        let personal_id = identity(&personal).unwrap();
        let replaced = std::rc::Rc::new(std::cell::Cell::new(false));
        let did_replace = replaced.clone();
        let target = root.clone();
        AFTER_EMPTY.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                if std::fs::rename(&target, target.with_extension("old")).is_ok() {
                    std::fs::rename(&personal, &target).unwrap();
                    did_replace.set(true);
                }
            }))
        });
        let state = temp.path().join("state");
        let result = register(
            &state,
            &root,
            "42",
            "S",
            &format!("sha256:{}", "00".repeat(32)),
        );
        if result.is_ok() {
            let conn = db::open(&state).unwrap();
            let registered: (String, Vec<u8>) = conn
                .query_row(
                    "SELECT volume_key,root_file_id FROM st.lab_root",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_ne!(
                registered, personal_id,
                "personal identity registered as synthetic"
            );
        }
        assert!(
            !replaced.get(),
            "empty root was replaceable during registration"
        );
        assert!(
            result.is_ok(),
            "ordinary empty root must register: {result:?}"
        );
    }
    #[test]
    fn review_lab_digests_are_canonical() {
        assert!(valid_digest(&format!("sha256:{}", "0a".repeat(32))));
        for digest in ["sha256:00".to_owned(), format!("sha256:{}", "A".repeat(64))] {
            assert!(!valid_digest(&digest), "{digest}");
            assert_eq!(log_digest(&digest), Err(Refusal::IdentityUnavailable));
        }
    }

    #[test]
    fn hardening_lab_identity_refuses_legacy_width() {
        let mut object = loomward_windows::identity::ObjectKey {
            volume_serial: "1".into(),
            file_id: [7; 16],
            quality: loomward_windows::identity::IdentityQuality::FileId128,
            creation_time: "0".into(),
            reparse_tag: 0,
        };
        assert_eq!(identity_parts(object.clone(), true).unwrap().1.len(), 16);
        object.quality = loomward_windows::identity::IdentityQuality::Legacy64 {
            reason: "synthetic fallback".into(),
        };
        assert_eq!(
            identity_parts(object, true),
            Err(Refusal::IdentityUnavailable)
        );
    }
}
