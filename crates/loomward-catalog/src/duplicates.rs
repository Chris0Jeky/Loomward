use crate::{query::WorkGuard, *};
use rusqlite::params;

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
            let _guard = WorkGuard::new(conn, 2_000_000, deadline)?;
            // visible_dir supplies the grant/tombstone rules; excluded ancestors also shield old rows.
            let mut stmt = conn.prepare("WITH RECURSIVE shielded(id) AS (
                SELECT id FROM dir WHERE root_id=?1 AND (listing_state='excluded' OR flags & ?4 != 0)
                UNION SELECT d.id FROM dir d JOIN shielded s ON d.parent_id=s.id),
                eligible AS (SELECT f.*, d.root_id FROM file f JOIN dir d ON d.id=f.dir_id
                JOIN visible_dir v ON v.id=d.id WHERE d.root_id=?1 AND d.id NOT IN (SELECT id FROM shielded)
                AND f.flags & ?4 = 0 AND f.logical>=?2 AND f.logical>0),
                sizes AS (SELECT logical FROM eligible GROUP BY logical HAVING count(*)>=2)
                SELECT f.id,f.logical,f.file_id,f.created_ft,f.modified_ft,f.changed_ft,f.attrs,g.volume_key
                FROM eligible f JOIN sizes s ON s.logical=f.logical JOIN root_grant g ON g.id=f.root_id
                ORDER BY f.logical,f.id LIMIT ?3")?;
            let rows = stmt.query_map(params![root, floor.max(1) as i64, cap as i64 + 1,
                EXCLUDED_FLAGS | 1 | (1 << 1) | (1 << 6) | (1 << 9)], |r| Ok((
                r.get::<_, i64>(0)?, DuplicateCandidate { relative: String::new(), logical: r.get::<_, i64>(1)? as u64,
                file_id: r.get(2)?, created: r.get(3)?, modified: r.get(4)?, changed: r.get(5)?, attributes: r.get(6)?, volume_key: r.get(7)? }
            )))?.collect::<rusqlite::Result<Vec<_>>>()?;
            let mut truncated = rows.len() > cap;
            let mut entries = Vec::new();
            let mut path_bytes = 0;
            for (id, mut candidate) in rows.into_iter().take(cap) {
                let path = crate::query::node_path(conn, NodeKey::File(id))?;
                if path.truncated { return Err(Error::ResourceBudget); }
                candidate.relative = path.ancestors.iter().skip(1).map(|p| p.name.as_str()).collect::<Vec<_>>().join("\\");
                if candidate.relative.chars().count() > 32768 { return Err(Error::ResourceBudget); }
                // Bound retained reports, including fifty reports in jobs.list, below transport limits.
                path_bytes += serde_json::to_string(&candidate.relative).expect("string JSON").len();
                if path_bytes > 64 * 1024 { truncated = true; break; }
                entries.push(candidate);
            }
            // A cap cutting off all but one member must not create a one-name candidate group.
            let mut sizes = std::collections::HashMap::new();
            for e in &entries { *sizes.entry(e.logical).or_insert(0) += 1; }
            entries.retain(|e| sizes[&e.logical] >= 2);
            Ok(DuplicateCandidates { entries, truncated, partial: !complete })
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
