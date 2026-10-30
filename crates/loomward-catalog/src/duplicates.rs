use crate::{query::WorkGuard, *};
use rusqlite::params;
use std::collections::{hash_map::Entry, HashMap};

/// Scan observations, not native handles or authority to read content.
#[derive(Debug, Clone)]
pub struct DuplicateCandidate {
    pub relative: String,
    pub volume_key: String,
    pub file_id: Option<Vec<u8>>,
    pub logical: u64,
    pub created: Option<i64>,
    pub modified: Option<i64>,
    pub changed: Option<i64>,
    pub attributes: u32,
}
#[derive(Debug, Clone, Default)]
pub struct DuplicateCandidates {
    /// Ordered by exact logical size; every size has at least two visible names.
    pub entries: Vec<DuplicateCandidate>,
    pub truncated: bool,
    pub partial: bool,
    /// Unrepresentable paths omitted before native I/O, with bounded refusal reasons.
    pub skipped: Vec<&'static str>,
    /// Eligible files sharing a size, before the cap; `None` when the count ran out of budget.
    pub space: Option<CandidateSpace>,
}
/// The candidate space: files (and their logical bytes) that share an exact size with another eligible file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateSpace {
    pub files: u64,
    pub bytes: u64,
}
/// Shared by candidate selection and the space count so they cannot drift apart: ?1 root, ?2 size
/// floor, ?3 excluded flags; `{cols}` is the file columns `eligible` carries (the count needs none
/// of the identity or name columns). visible_dir supplies the grant/tombstone rules; excluded
/// ancestors also shield old rows.
const ELIGIBLE_SIZES: &str = "WITH RECURSIVE shielded(id) AS (
    SELECT id FROM dir WHERE root_id=?1 AND (listing_state='excluded' OR flags & ?3 != 0)
    UNION SELECT d.id FROM dir d JOIN shielded s ON d.parent_id=s.id),
    eligible AS (SELECT {cols}, d.root_id FROM file f JOIN dir d ON d.id=f.dir_id
    JOIN visible_dir v ON v.id=d.id WHERE d.root_id=?1 AND d.id NOT IN (SELECT id FROM shielded)
    AND f.flags & ?3 = 0 AND f.logical>=?2 AND f.logical>0),
    sizes AS (SELECT logical FROM eligible GROUP BY logical HAVING count(*)>=2)";
const SPACE_WORK_BUDGET: u64 = 32_000_000;
/// Count the candidate space without reading content. A count interrupted by its work budget or the
/// deadline is `None`, unknown, never zero: the candidates already selected are unaffected. Any other
/// failure (corruption, busy, overflow) is a real error and surfaces, never a budget stop (invariant 4).
fn candidate_space(
    conn: &rusqlite::Connection,
    root: i64,
    floor: i64,
    flags: u32,
    deadline: Option<std::time::Instant>,
    budget: u64,
) -> Result<Option<CandidateSpace>> {
    let _guard = WorkGuard::new(conn, budget, deadline)?;
    match conn
        .query_row(
            &format!(
                "{}
                SELECT count(*), coalesce(sum(logical),0) FROM eligible WHERE logical IN (SELECT logical FROM sizes)",
                ELIGIBLE_SIZES.replace("{cols}", "f.logical, f.flags")
            ),
            params![root, floor, flags],
            |r| {
                Ok(CandidateSpace {
                    files: r.get::<_, i64>(0)? as u64,
                    bytes: r.get::<_, i64>(1)? as u64,
                })
            },
        ) {
        Ok(space) => Ok(Some(space)),
        Err(e) => {
            let e = Error::from(e);
            if crate::query::is_budget_interrupt(&e) {
                Ok(None)
            } else {
                Err(e)
            }
        }
    }
}
/// Completed candidate-space counts shared by every reader of one `Catalog` (the service opens a fresh
/// reader per request, so a per-reader cache would never hit). Keyed on (root, size floor); each entry
/// carries the (root generation, root subtree revision) it was counted at (#1035 item 1).
pub(crate) type SpaceCache =
    std::sync::Arc<std::sync::Mutex<HashMap<(i64, i64), (Stamp, CandidateSpace)>>>;
type Stamp = (Option<i64>, i64);
/// The count at the root's current revision: served from `cache` only when the stamp read in this
/// snapshot equals the stamp it was counted at; otherwise computed. Aggregates under repair and a
/// running scan are never cached or served, and an unknown count (`None`) is never stored.
fn space_at_revision(
    cache: Option<&SpaceCache>,
    conn: &rusqlite::Connection,
    root: i64,
    floor: i64,
    flags: u32,
    deadline: Option<std::time::Instant>,
    budget: u64,
) -> Result<Option<CandidateSpace>> {
    let Some(cache) = cache else {
        return candidate_space(conn, root, floor, flags, deadline, budget);
    };
    let (valid, rev): (bool, i64) = conn.query_row(
        "SELECT agg_valid_rev>=dirty_rev, subtree_rev FROM dir WHERE root_id=?1 AND parent_id IS NULL",
        [root],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let stamp = (crate::query::revisions(conn, Some(root))?[0].1, rev);
    let cacheable = valid && !crate::query::live(conn, root)?;
    if cacheable {
        if let Some((at, space)) = cache
            .lock()
            .ok()
            .and_then(|c| c.get(&(root, floor)).copied())
        {
            if at == stamp {
                return Ok(Some(space));
            }
        }
    }
    let space = candidate_space(conn, root, floor, flags, deadline, budget)?;
    if let (true, Some(space), Ok(mut cache)) = (cacheable, space, cache.lock()) {
        if cache.len() >= 64 {
            cache.clear();
        }
        cache.insert((root, floor), (stamp, space));
    }
    Ok(space)
}
impl Reader {
    pub fn duplicate_candidates(
        &mut self,
        root: i64,
        floor: u64,
        cap: usize,
    ) -> Result<DuplicateCandidates> {
        self.duplicate_candidates_with(root, floor, cap, SPACE_WORK_BUDGET)
    }
    /// `space_budget` is the count's own work budget; tests shrink it to interrupt only the count.
    fn duplicate_candidates_with(
        &mut self,
        root: i64,
        floor: u64,
        cap: usize,
        space_budget: u64,
    ) -> Result<DuplicateCandidates> {
        if !(2..=200).contains(&cap) || floor > i64::MAX as u64 {
            return Err(Error::Invalid("duplicate candidate bounds"));
        }
        let deadline = self.deadline;
        if deadline.is_some_and(|at| std::time::Instant::now() >= at) {
            return Err(Error::Cancelled);
        }
        let cache = self.space_cache.clone();
        let result = self.read(|conn| {
            crate::query::revisions(conn, Some(root))?;
            let complete: bool = conn.query_row("SELECT sub_complete FROM dir WHERE root_id=?1 AND parent_id IS NULL", [root], |r|r.get(0))?;
            // Sized so a 130k-file root answers (about 0.4 s debug, #715); a larger root is refused (resource_budget), never truncated.
            let _guard = WorkGuard::new(conn, 32_000_000, deadline)?;
            let flags = EXCLUDED_FLAGS | 1 | (1 << 1) | (1 << 6) | (1 << 9) | crate::IDENTITY_UNVERIFIED;
            let mut stmt = conn.prepare(&format!("{}
                SELECT f.dir_id,f.logical,f.file_id,f.created_ft,f.modified_ft,f.changed_ft,f.attrs,g.volume_key,f.name
                FROM eligible f JOIN sizes s ON s.logical=f.logical JOIN root_grant g ON g.id=f.root_id
                ORDER BY f.logical,f.id LIMIT ?4", ELIGIBLE_SIZES.replace("{cols}", "f.*")))?;
            let rows = stmt.query_map(params![root, floor.max(1) as i64, flags, cap as i64 + 1], |r| Ok((
                r.get::<_, i64>(0)?, r.get::<_, String>(8)?, DuplicateCandidate { relative: String::new(), logical: r.get::<_, i64>(1)? as u64,
                file_id: r.get(2)?, created: r.get(3)?, modified: r.get(4)?, changed: r.get(5)?, attributes: r.get(6)?, volume_key: r.get(7)? }
            )))?.collect::<rusqlite::Result<Vec<_>>>()?;
            // Unreadable (access denied), unrepresentable (lossy) or identity-unverified (#219) names
            // are unknowns, not policy exclusions: filtering one leaves the report partial (invariant 4).
            let unknown: bool = conn.query_row("WITH RECURSIVE shielded(id) AS (
                SELECT id FROM dir WHERE root_id=?1 AND (listing_state='excluded' OR flags & ?3 != 0)
                UNION SELECT d.id FROM dir d JOIN shielded s ON d.parent_id=s.id)
                SELECT EXISTS(SELECT 1 FROM file f JOIN dir d ON d.id=f.dir_id JOIN visible_dir v ON v.id=d.id
                WHERE d.root_id=?1 AND d.id NOT IN (SELECT id FROM shielded)
                AND f.flags & ?4 != 0 AND f.logical>=?2 AND f.logical>0)",
                params![root, floor.max(1) as i64, EXCLUDED_FLAGS | 1 | (1 << 1) | (1 << 6) | (1 << 9), (1 << 6) | (1 << 9) | crate::IDENTITY_UNVERIFIED],
                |r| r.get(0))?;
            let mut truncated = rows.len() > cap;
            let mut entries = Vec::new();
            let mut skipped = Vec::new();
            let mut path_bytes = 0;
            let mut paths = HashMap::new();
            for (dir, name, mut candidate) in rows.into_iter().take(cap) {
                let path = match paths.entry(dir) {
                    Entry::Occupied(entry) => entry.into_mut(),
                    Entry::Vacant(entry) => entry.insert(crate::query::node_path(conn, NodeKey::Dir(dir))?),
                };
                // The file itself uses one of node_path's 512 slots.
                if path.truncated || path.ancestors.len() >= MAX_DIRECTORY_CHAIN_LENGTH { skipped.push("path_depth"); continue; }
                candidate.relative = path.ancestors.iter().skip(1).map(|p| p.name.as_str()).chain(std::iter::once(name.as_str())).collect::<Vec<_>>().join("\\");
                if candidate.relative.chars().count() > 32768 { skipped.push("path_length"); continue; }
                // Bound retained reports, including fifty reports in jobs.list, below transport limits.
                path_bytes += serde_json::to_string(&candidate.relative).expect("string JSON").len();
                if path_bytes > 64 * 1024 { truncated = true; break; }
                entries.push(candidate);
            }
            // A cap cutting off all but one member must not create a one-name candidate group.
            let mut sizes = std::collections::HashMap::new();
            for e in &entries { *sizes.entry(e.logical).or_insert(0) += 1; }
            entries.retain(|e| sizes[&e.logical] >= 2);
            let partial = !complete || unknown || !skipped.is_empty();
            // A fresh budget of the same size: the count must not starve because selection used its own.
            drop(_guard);
            let space = space_at_revision(cache.as_ref(), conn, root, floor.max(1) as i64, flags, deadline, space_budget)?;
            Ok(DuplicateCandidates { entries, truncated, partial, skipped, space })
        });
        match result {
            Err(Error::Sql(rusqlite::Error::SqliteFailure(e, _)))
                if e.code == rusqlite::ErrorCode::OperationInterrupted =>
            {
                Err(
                    if deadline.is_some_and(|at| std::time::Instant::now() >= at) {
                        Error::Cancelled
                    } else {
                        Error::ResourceBudget
                    },
                )
            }
            result => result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[test]
    fn a_space_count_that_runs_out_of_work_budget_is_unknown_not_zero() {
        let temp = tempfile::tempdir().unwrap();
        let c = Catalog::open(temp.path(), "synthetic").unwrap();
        let WriteReply::Root {
            grant_id,
            dir_id,
            root_id,
        } = c
            .writer()
            .call(WriteCommand::RegisterRoot(RootObservation {
                volume_guid: None,
                volume_key: "fixture".into(),
                display_name: "Root".into(),
                display_path: "Synthetic root".into(),
                root_file_id: Some(vec![0; 16]),
                filesystem: Some("NTFS".into()),
                origin: "fixture".into(),
                granted_via: "fixture".into(),
                observed_at_ns: 0,
            }))
            .unwrap()
        else {
            panic!()
        };
        let WriteReply::Run(run) = c
            .writer()
            .call(WriteCommand::BeginRun {
                grant_id,
                mode: "full".into(),
                strategy: "fixture".into(),
                started_at_ns: 0,
            })
            .unwrap()
        else {
            panic!()
        };
        c.writer()
            .call(WriteCommand::DirListing(DirListing {
                run_id: run,
                dir_id,
                files: (0..400)
                    .map(|i| Observation::file(format!("file-{i:03}"), 10 + i % 7, Some(4096)))
                    .collect(),
                dirs: vec![],
                state: "complete".into(),
                skipped: 0,
                errors: 0,
            }))
            .unwrap();
        c.writer()
            .call(WriteCommand::EndRun {
                run_id: run,
                state: "completed".into(),
                finished_at_ns: 1,
            })
            .unwrap();
        let reader = c.reader().unwrap();
        let flags =
            EXCLUDED_FLAGS | 1 | (1 << 1) | (1 << 6) | (1 << 9) | crate::IDENTITY_UNVERIFIED;
        let counted = candidate_space(&reader.conn, root_id, 1, flags, None, SPACE_WORK_BUDGET)
            .unwrap()
            .expect("an ample budget counts every file");
        assert_eq!(counted.files, 400);
        // One progress tick of budget cannot finish the aggregate: unknown, and no error.
        assert_eq!(
            candidate_space(&reader.conn, root_id, 1, flags, None, 1).unwrap(),
            None
        );
    }

    /// A completed run listing `files` in the root directory.
    fn catalog_with(files: Vec<Observation>) -> (tempfile::TempDir, Catalog, i64) {
        let temp = tempfile::tempdir().unwrap();
        let c = Catalog::open(temp.path(), "synthetic").unwrap();
        let WriteReply::Root {
            grant_id,
            dir_id,
            root_id,
        } = c
            .writer()
            .call(WriteCommand::RegisterRoot(RootObservation {
                volume_guid: None,
                volume_key: "fixture".into(),
                display_name: "Root".into(),
                display_path: "Synthetic root".into(),
                root_file_id: Some(vec![0; 16]),
                filesystem: Some("NTFS".into()),
                origin: "fixture".into(),
                granted_via: "fixture".into(),
                observed_at_ns: 0,
            }))
            .unwrap()
        else {
            panic!()
        };
        let WriteReply::Run(run) = c
            .writer()
            .call(WriteCommand::BeginRun {
                grant_id,
                mode: "full".into(),
                strategy: "fixture".into(),
                started_at_ns: 0,
            })
            .unwrap()
        else {
            panic!()
        };
        c.writer()
            .call(WriteCommand::DirListing(DirListing {
                run_id: run,
                dir_id,
                files,
                dirs: vec![],
                state: "complete".into(),
                skipped: 0,
                errors: 0,
            }))
            .unwrap();
        c.writer()
            .call(WriteCommand::EndRun {
                run_id: run,
                state: "completed".into(),
                finished_at_ns: 1,
            })
            .unwrap();
        (temp, c, root_id)
    }

    fn pairs(n: u64) -> Vec<Observation> {
        (0..n)
            .map(|i| Observation::file(format!("file-{i:03}"), 10 + i / 2, Some(4096)))
            .collect()
    }

    #[test]
    fn a_count_failure_that_is_not_a_budget_stop_surfaces_as_an_error() {
        let (temp, c, root) = catalog_with(pairs(40));
        // Two same-size files whose logical sum overflows i64: only the count sums sizes, so
        // selection still succeeds and the count's SQL error must reach the caller.
        rusqlite::Connection::open(temp.path().join("catalog.db"))
            .unwrap()
            .execute(
                "UPDATE file SET logical=5000000000000000000 WHERE name IN ('file-000','file-001')",
                [],
            )
            .unwrap();
        let mut reader = c.reader().unwrap();
        let failed = reader.duplicate_candidates(root, 1, 200);
        assert!(
            matches!(&failed, Err(e @ Error::Sql(_)) if !crate::query::is_budget_interrupt(e)),
            "an overflow is not unknown: {failed:?}"
        );
        // A denied statement is likewise an error, not a stop.
        let flags =
            EXCLUDED_FLAGS | 1 | (1 << 1) | (1 << 6) | (1 << 9) | crate::IDENTITY_UNVERIFIED;
        reader
            .conn
            .authorizer(Some(|_: AuthContext<'_>| Authorization::Deny))
            .unwrap();
        let denied = candidate_space(&reader.conn, root, 1, flags, None, SPACE_WORK_BUDGET);
        assert!(
            matches!(&denied, Err(e) if !crate::query::is_budget_interrupt(e)),
            "{denied:?}"
        );
    }

    #[test]
    fn a_count_interrupted_inside_the_full_path_leaves_entries_intact_and_the_read_committed() {
        let (_temp, c, root) = catalog_with(pairs(40));
        let mut reader = c.reader().unwrap();
        // The revision cache would serve the second call; this test needs the count to run.
        reader.space_cache = None;
        let whole = reader.duplicate_candidates(root, 1, 200).unwrap();
        assert_eq!(whole.entries.len(), 40);
        assert_eq!(whole.space.map(|s| s.files), Some(40));
        // One progress tick of budget for the count alone: selection keeps its own.
        let cut = reader.duplicate_candidates_with(root, 1, 200, 1).unwrap();
        assert_eq!(cut.space, None, "an interrupted count is unknown, not zero");
        assert_eq!(
            cut.entries.iter().map(|e| &e.relative).collect::<Vec<_>>(),
            whole
                .entries
                .iter()
                .map(|e| &e.relative)
                .collect::<Vec<_>>()
        );
        assert!(!cut.truncated && !cut.partial);
        // Ok means the read transaction committed; the connection holds no open transaction and
        // no leftover progress handler, so the next full read counts again.
        assert!(reader.conn.is_autocommit());
        assert_eq!(
            reader
                .duplicate_candidates(root, 1, 200)
                .unwrap()
                .space
                .map(|s| s.files),
            Some(40)
        );
    }

    #[test]
    fn a_cached_count_is_served_until_a_write_changes_the_revision() {
        let (temp, c, root) = catalog_with(pairs(40));
        let mut reader = c.reader().unwrap();
        let counted = |r: &mut Reader| r.duplicate_candidates(root, 1, 200).unwrap().space;
        assert_eq!(counted(&mut reader).map(|s| s.files), Some(40));
        // A fresh reader of the same catalogue shares the cache. The row edit below bypasses the
        // writer, so no revision moves: a stale answer here proves the second call was a hit.
        let raw = rusqlite::Connection::open(temp.path().join("catalog.db")).unwrap();
        raw.execute("UPDATE file SET logical=100000+id", [])
            .unwrap();
        let mut other = c.reader().unwrap();
        assert_eq!(counted(&mut other).map(|s| s.files), Some(40));
        assert_eq!(other.space_cache.as_ref().unwrap().lock().unwrap().len(), 1);
        // The edit really changed the data: a reader without the cache counts none of them,
        // since the sizes no longer pair up.
        other.space_cache = None;
        assert_eq!(counted(&mut other).map(|s| s.files), Some(0));
        // A real write (a second run listing new files) bumps the root's revision and generation.
        let (dir_id, grant): (i64, i64) = raw
            .query_row(
                "SELECT d.id,d.root_id FROM dir d WHERE d.root_id=?1 AND d.parent_id IS NULL",
                [root],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let w = c.writer();
        let WriteReply::Run(run) = w
            .call(WriteCommand::BeginRun {
                grant_id: grant,
                mode: "full".into(),
                strategy: "fixture".into(),
                started_at_ns: 2,
            })
            .unwrap()
        else {
            panic!()
        };
        w.call(WriteCommand::DirListing(DirListing {
            run_id: run,
            dir_id,
            files: pairs(10),
            dirs: vec![],
            state: "complete".into(),
            skipped: 0,
            errors: 0,
        }))
        .unwrap();
        // A scan in flight is neither served from nor stored in the cache.
        let mut live = c.reader().unwrap();
        let during = counted(&mut live);
        assert_ne!(
            during.map(|s| s.files),
            Some(40),
            "live run served the old count"
        );
        w.call(WriteCommand::EndRun {
            run_id: run,
            state: "completed".into(),
            finished_at_ns: 3,
        })
        .unwrap();
        let after = counted(&mut c.reader().unwrap());
        assert_eq!(after.map(|s| s.files), Some(10));
        // Stored under the new revision, so a repeat is a hit and agrees.
        assert_eq!(counted(&mut c.reader().unwrap()), after);
    }

    #[test]
    fn candidates_reuse_parent_paths_and_match_node_path_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let c = Catalog::open(temp.path(), "synthetic").unwrap();
        let WriteReply::Root {
            grant_id,
            dir_id,
            root_id,
        } = c
            .writer()
            .call(WriteCommand::RegisterRoot(RootObservation {
                volume_guid: None,
                volume_key: "fixture".into(),
                display_name: "Root".into(),
                display_path: "Synthetic root".into(),
                root_file_id: Some(vec![0; 16]),
                filesystem: Some("NTFS".into()),
                origin: "fixture".into(),
                granted_via: "fixture".into(),
                observed_at_ns: 0,
            }))
            .unwrap()
        else {
            panic!()
        };
        let WriteReply::Run(run) = c
            .writer()
            .call(WriteCommand::BeginRun {
                grant_id,
                mode: "full".into(),
                strategy: "fixture".into(),
                started_at_ns: 0,
            })
            .unwrap()
        else {
            panic!()
        };
        c.writer()
            .call(WriteCommand::DirListing(DirListing {
                run_id: run,
                dir_id,
                files: (0..200)
                    .map(|i| Observation::file(format!("file-{i:03}"), 10, Some(4096)))
                    .collect(),
                dirs: vec![],
                state: "complete".into(),
                skipped: 0,
                errors: 0,
            }))
            .unwrap();
        c.writer()
            .call(WriteCommand::EndRun {
                run_id: run,
                state: "completed".into(),
                finished_at_ns: 1,
            })
            .unwrap();
        let mut reader = c.reader().unwrap();
        let ids: Vec<i64> = reader
            .conn
            .prepare("SELECT id FROM file ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let expected: Vec<String> = ids
            .into_iter()
            .map(|id| {
                reader
                    .path(NodeKey::File(id))
                    .unwrap()
                    .ancestors
                    .into_iter()
                    .skip(1)
                    .map(|p| p.name)
                    .collect::<Vec<_>>()
                    .join("\\")
            })
            .collect();
        let reads = Arc::new(AtomicUsize::new(0));
        let counter = reads.clone();
        reader
            .conn
            .authorizer(Some(move |ctx: AuthContext<'_>| {
                if matches!(
                    ctx.action,
                    AuthAction::Read {
                        table_name: "file",
                        column_name: "name"
                    }
                ) {
                    counter.fetch_add(1, Ordering::Relaxed);
                }
                Authorization::Allow
            }))
            .unwrap();
        let report = reader.duplicate_candidates(root_id, 1, 200).unwrap();
        assert_eq!(
            report
                .entries
                .iter()
                .map(|e| &e.relative)
                .collect::<Vec<_>>(),
            expected.iter().collect::<Vec<_>>()
        );
        assert!(!report.partial);
        assert!(!report.truncated);
        assert!(
            reads.load(Ordering::Relaxed) <= 4,
            "candidate file rows must not be reread for each path: {}",
            reads.load(Ordering::Relaxed)
        );
    }

    /// `n` synthetic files in 1000-file directories under one root; `size(k)` is file k's logical size.
    fn big_catalog(n: usize, size: impl Fn(usize) -> u64) -> (tempfile::TempDir, Catalog, i64) {
        const PER_DIR: usize = 1000;
        let temp = tempfile::tempdir().unwrap();
        let c = Catalog::open(temp.path(), "synthetic").unwrap();
        let w = c.writer();
        let WriteReply::Root {
            grant_id,
            dir_id,
            root_id,
        } = w
            .call(WriteCommand::RegisterRoot(RootObservation {
                volume_guid: None,
                volume_key: "fixture".into(),
                display_name: "Root".into(),
                display_path: "Synthetic root".into(),
                root_file_id: Some(vec![0; 16]),
                filesystem: Some("NTFS".into()),
                origin: "fixture".into(),
                granted_via: "fixture".into(),
                observed_at_ns: 0,
            }))
            .unwrap()
        else {
            panic!()
        };
        let WriteReply::Run(run) = w
            .call(WriteCommand::BeginRun {
                grant_id,
                mode: "full".into(),
                strategy: "fixture".into(),
                started_at_ns: 0,
            })
            .unwrap()
        else {
            panic!()
        };
        let observed = (0..n.div_ceil(PER_DIR))
            .map(|d| {
                let mut o = Observation::file(format!("d{d}"), 0, Some(0));
                o.attrs = 0x10;
                o.file_id = Some(vec![0xD1; 15].into_iter().chain([d as u8 + 1]).collect());
                o.created_ft = Some(1);
                o
            })
            .collect();
        w.call(WriteCommand::DirListing(DirListing {
            run_id: run,
            dir_id,
            files: vec![],
            dirs: observed,
            state: "complete".into(),
            skipped: 0,
            errors: 0,
        }))
        .unwrap();
        let db = rusqlite::Connection::open(temp.path().join("catalog.db")).unwrap();
        let ids: Vec<i64> = db
            .prepare("SELECT id FROM dir WHERE parent_id=?1 ORDER BY id")
            .unwrap()
            .query_map([dir_id], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<_, rusqlite::Error>>()
            .unwrap();
        for (d, child) in ids.into_iter().enumerate() {
            let files = (0..PER_DIR.min(n - d * PER_DIR))
                .map(|i| {
                    let k = d * PER_DIR + i;
                    let mut o = Observation::file(format!("f{i}.bin"), size(k), Some(4096));
                    let mut id = [0u8; 16];
                    id[..8].copy_from_slice(&(k as u64 + 1).to_le_bytes());
                    o.file_id = Some(id.to_vec());
                    o
                })
                .collect();
            w.call(WriteCommand::DirListing(DirListing {
                run_id: run,
                dir_id: child,
                files,
                dirs: vec![],
                state: "complete".into(),
                skipped: 0,
                errors: 0,
            }))
            .unwrap();
        }
        w.call(WriteCommand::EndRun {
            run_id: run,
            state: "completed".into(),
            finished_at_ns: 1,
        })
        .unwrap();
        (temp, c, root_id)
    }

    /// #1035 item 1, measurement (release build, `--ignored --nocapture`): what the second catalogue
    /// pass costs on a 130k-file synthetic root. Four timings per shape, interleaved over ten runs
    /// after one warm-up: the full call with the cache off, the call with the count cut off at its
    /// first progress tick (selection only), the count alone, and the full call served from the cache.
    ///
    /// Result (Windows 11, release, 2026-10-30, 10 runs after a warm-up, medians; two sessions agree
    /// within about 10%): all files sharing one size, full call 134-141 ms against 82-94 ms for
    /// selection alone, so the count adds 47-52 ms (50-64%). With 20,000 distinct sizes the full call
    /// is 734-743 ms against 153-162 ms, so the count adds 572-590 ms (353-385%). The second shape is
    /// over the 100 ms line, hence the per-revision cache; a cached call is back at selection cost
    /// (84 and 163 ms). The first call after a write still pays the count.
    #[test]
    #[ignore = "#1035 synthetic 130k-file timing; run with --release --ignored --nocapture"]
    fn measure_the_candidate_space_count_at_130k() {
        use std::time::{Duration, Instant};
        fn stats(label: &str, mut t: Vec<Duration>) -> Duration {
            t.sort();
            let median = t[t.len() / 2];
            println!(
                "  {label:<16} median {:>8.1} ms  min {:>8.1}  max {:>8.1}",
                median.as_secs_f64() * 1e3,
                t[0].as_secs_f64() * 1e3,
                t[t.len() - 1].as_secs_f64() * 1e3
            );
            median
        }
        let flags =
            EXCLUDED_FLAGS | 1 | (1 << 1) | (1 << 6) | (1 << 9) | crate::IDENTITY_UNVERIFIED;
        type Shape = (&'static str, fn(usize) -> u64);
        let shapes: [Shape; 2] = [
            ("one size (all 130k share it)", |_| 10),
            ("20,000 sizes (about 6.5 each)", |k| {
                10 + (k % 20_000) as u64
            }),
        ];
        for (name, size) in shapes {
            let (_t, c, root) = big_catalog(130_000, size);
            let mut reader = c.reader().unwrap();
            let (mut full, mut selection, mut count, mut cached) = (vec![], vec![], vec![], vec![]);
            let mut space = None;
            let shared = reader.space_cache.take();
            for run in 0..11 {
                reader.space_cache = None;
                let t = Instant::now();
                let with = reader.duplicate_candidates(root, 1, 200).unwrap();
                let a = t.elapsed();
                let t = Instant::now();
                let without = reader.duplicate_candidates_with(root, 1, 200, 1).unwrap();
                let b = t.elapsed();
                let t = Instant::now();
                let alone =
                    candidate_space(&reader.conn, root, 1, flags, None, SPACE_WORK_BUDGET).unwrap();
                let s = t.elapsed();
                reader.space_cache = shared.clone();
                let t = Instant::now();
                let hit = reader.duplicate_candidates(root, 1, 200).unwrap();
                let h = t.elapsed();
                assert!(hit.space.is_some() && hit.space == with.space);
                assert!(with.space.is_some() && without.space.is_none() && alone == with.space);
                assert_eq!(with.entries.len(), without.entries.len());
                space = with.space;
                if run > 0 {
                    full.push(a);
                    selection.push(b);
                    count.push(s);
                    cached.push(h);
                }
            }
            println!("130,000 files, {name}, space {space:?}");
            let f = stats("full call", full);
            let sel = stats("selection only", selection);
            let cnt = stats("count alone", count);
            stats("cached full call", cached);
            println!(
                "  count adds {:.1} ms = {:.0}% over selection only (count alone {:.1} ms)",
                (f.as_secs_f64() - sel.as_secs_f64()) * 1e3,
                (f.as_secs_f64() / sel.as_secs_f64() - 1.0) * 100.0,
                cnt.as_secs_f64() * 1e3
            );
        }
    }
}
