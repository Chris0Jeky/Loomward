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
}
impl Reader {
    pub fn duplicate_candidates(
        &mut self,
        root: i64,
        floor: u64,
        cap: usize,
    ) -> Result<DuplicateCandidates> {
        if !(2..=200).contains(&cap) || floor > i64::MAX as u64 {
            return Err(Error::Invalid("duplicate candidate bounds"));
        }
        let deadline = self.deadline;
        if deadline.is_some_and(|at| std::time::Instant::now() >= at) {
            return Err(Error::Cancelled);
        }
        let result = self.read(|conn| {
            crate::query::revisions(conn, Some(root))?;
            let complete: bool = conn.query_row("SELECT sub_complete FROM dir WHERE root_id=?1 AND parent_id IS NULL", [root], |r|r.get(0))?;
            // Sized so a 130k-file root answers (about 0.4 s debug, #715); a larger root is refused (resource_budget), never truncated.
            let _guard = WorkGuard::new(conn, 32_000_000, deadline)?;
            // visible_dir supplies the grant/tombstone rules; excluded ancestors also shield old rows.
            let mut stmt = conn.prepare("WITH RECURSIVE shielded(id) AS (
                SELECT id FROM dir WHERE root_id=?1 AND (listing_state='excluded' OR flags & ?4 != 0)
                UNION SELECT d.id FROM dir d JOIN shielded s ON d.parent_id=s.id),
                eligible AS (SELECT f.*, d.root_id FROM file f JOIN dir d ON d.id=f.dir_id
                JOIN visible_dir v ON v.id=d.id WHERE d.root_id=?1 AND d.id NOT IN (SELECT id FROM shielded)
                AND f.flags & ?4 = 0 AND f.logical>=?2 AND f.logical>0),
                sizes AS (SELECT logical FROM eligible GROUP BY logical HAVING count(*)>=2)
                SELECT f.dir_id,f.logical,f.file_id,f.created_ft,f.modified_ft,f.changed_ft,f.attrs,g.volume_key,f.name
                FROM eligible f JOIN sizes s ON s.logical=f.logical JOIN root_grant g ON g.id=f.root_id
                ORDER BY f.logical,f.id LIMIT ?3")?;
            let rows = stmt.query_map(params![root, floor.max(1) as i64, cap as i64 + 1,
                EXCLUDED_FLAGS | 1 | (1 << 1) | (1 << 6) | (1 << 9) | crate::IDENTITY_UNVERIFIED], |r| Ok((
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
            Ok(DuplicateCandidates { entries, truncated, partial, skipped })
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
}
