//! The lab-root registry (ADR-V3-15, docs/41 section 12): the only way a directory becomes
//! grantable in a synthetic session. Registration creates the directory itself (refusing one that
//! already holds anything), records its native identity in the synthetic `state.db`, and a later
//! grant must present a root with exactly that identity. A path alone never qualifies.

use crate::{db, now_ns, paths};
use loomward_catalog::Catalog;
use loomward_protocol::RootGrantResultRefusal as Refusal;
use std::path::Path;

/// `(volume_key, root_file_id)` of a directory, observed from an open handle. The 128-bit ID is
/// kept whole; a legacy 64-bit ID is kept at its own width (never padded to "128").
pub fn identity(root: &Path) -> Result<(String, Vec<u8>), Refusal> {
    let observed =
        loomward_windows::identity::observe(root).map_err(|_| Refusal::IdentityUnavailable)?;
    let object = observed.object;
    let id = match object.quality {
        loomward_windows::identity::IdentityQuality::FileId128 => object.file_id.to_vec(),
        loomward_windows::identity::IdentityQuality::Legacy64 { .. } => {
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
    if root.exists()
        && std::fs::read_dir(root)
            .map_err(|_| "the lab root cannot be read")?
            .next()
            .is_some()
    {
        return Err("a lab root must be a new or empty directory".into());
    }
    std::fs::create_dir_all(root).map_err(|_| "cannot create the lab root")?;
    std::fs::create_dir_all(state_dir).map_err(|_| "cannot create the state directory")?;
    let state = state_dir
        .canonicalize()
        .map_err(|_| "cannot resolve the state directory")?;
    let root = paths::check_root(root, Some(&state), &[], false)
        .map_err(|r| format!("lab root refused: {}", crate::refusal_name(r)))?;
    let (volume_key, file_id) =
        identity(&root).map_err(|r| format!("lab root refused: {}", crate::refusal_name(r)))?;
    // Opening the catalogue creates or migrates the store and refuses a personal one.
    let _catalog = Catalog::open(&state, "synthetic").map_err(|e| format!("state store: {e}"))?;
    let conn = db::open(&state).map_err(|e| format!("state store: {e}"))?;
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
