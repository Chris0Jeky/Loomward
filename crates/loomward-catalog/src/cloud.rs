use crate::{query, Error, NodeKey, Reader, Result, EXCLUDED_FLAGS};

/// Residency evidence from Windows directory enumeration, never provider health or permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloudState {
    /// Offline/recall-marked: not fully local; some bytes may still be allocated locally.
    CloudOnly,
    /// No recall/offline evidence; a pin preference can identify a hydrated reparse entry.
    Local,
    /// Pinned preference and no recall/offline evidence; not a provider completion guarantee.
    Pinned,
    Unknown,
}

/// Classify already-listed attributes without opening a path. Missing attributes stay unknown.
/// Zero is the existing catalogue sentinel for missing/portable attributes. UNPINNED alone is
/// an eviction preference, not evidence of eviction. Provider identity/health are always unknown.
pub fn cloud_state(attributes: Option<u32>) -> CloudState {
    let Some(attrs) = attributes.filter(|attrs| cfg!(windows) && *attrs != 0) else {
        return CloudState::Unknown;
    };
    if attrs & (0x00400000 | 0x00040000 | 0x00001000) != 0 {
        CloudState::CloudOnly
    } else if attrs & 0x00180000 == 0x00180000 {
        CloudState::Unknown
    } else if attrs & 0x00080000 != 0 {
        CloudState::Pinned
    } else if attrs & (0x400 | 0x200) != 0 && attrs & 0x00100000 == 0 {
        CloudState::Unknown
    } else {
        CloudState::Local
    }
}

/// Recorded file-entry bytes (hard links count once per name), not reclaimable bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloudBytes {
    pub files: u64,
    pub logical_bytes: u64,
    /// Unknown if any contributing file lacks an allocation observation.
    pub allocated_bytes: Option<u64>,
}
impl Default for CloudBytes {
    fn default() -> Self {
        Self {
            files: 0,
            logical_bytes: 0,
            allocated_bytes: Some(0),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct CloudSummary {
    pub cloud_only: CloudBytes,
    pub local: CloudBytes,
    pub pinned: CloudBytes,
    pub unknown: CloudBytes,
    /// These are observations, not proof of complete/current provider contents.
    pub partial: bool,
}

const CLOUD_SUMMARY_SQL: &str = "WITH RECURSIVE walk(id,listing_state,coverage,flags) AS (
                    SELECT id,listing_state,
                      coalesce(listing_outcome,CASE listing_state WHEN 'unlisted' THEN 'unscanned' WHEN 'incomplete' THEN 'partial' ELSE listing_state END),flags
                    FROM dir WHERE id=?1
                    UNION ALL
                    SELECT d.id,d.listing_state,
                      coalesce(d.listing_outcome,CASE d.listing_state WHEN 'unlisted' THEN 'unscanned' WHEN 'incomplete' THEN 'partial' ELSE d.listing_state END),d.flags
                    FROM dir d JOIN walk w ON d.parent_id=w.id
                    WHERE d.root_id=?2 AND d.listing_state!='absent_pending'
                      AND w.listing_state NOT IN ('excluded','denied')
                      AND w.coverage NOT IN ('excluded','denied') AND w.flags & ?3=0
                ) SELECT f.attrs,f.logical,f.allocated FROM file f JOIN walk w ON f.dir_id=w.id
                  WHERE w.listing_state NOT IN ('excluded','denied')
                    AND w.coverage NOT IN ('excluded','denied') AND w.flags & ?3=0";

impl Reader {
    /// Suggested VM-step budget for cloud summaries: about 1.18M files in a flat subtree.
    /// Measured with `cloud_query_vm_steps_per_file`; directory traversal costs extra.
    pub const DEFAULT_CLOUD_SUMMARY_STEPS: u64 = 32_000_000;

    /// Catalogue-only totals below an active directory; never opens the recorded paths.
    /// No file-flag filter: includes `cloud_placeholder`, `reparse_not_followed`,
    /// `excluded_state_dir`, sensitive-name and protected-context files when their directory
    /// is included. Hydrated reparse placeholders without recall/offline or pin/unpin evidence
    /// land in Unknown. Hidden/absent and excluded/denied or exclusion-flagged directory
    /// descendants are not counted. Pass `Self::DEFAULT_CLOUD_SUMMARY_STEPS` for the suggested
    /// work budget. Exhaustion returns ResourceBudget, deadlines return Cancelled, and other
    /// interrupts remain SQL errors; no truncated total is returned as complete.
    pub fn cloud_summary(&mut self, directory: i64, work_budget: u64) -> Result<CloudSummary> {
        let deadline = self.deadline;
        if deadline.is_some_and(|at| std::time::Instant::now() >= at) {
            return Err(Error::Cancelled);
        }
        self.read(|conn| {
            let guard = query::WorkGuard::new(conn, work_budget, deadline)?;
            let result = (|| {
                let root = query::get(conn, NodeKey::Dir(directory))?;
                let mut report = CloudSummary {
                    partial: root.coverage() != "complete"
                        || !root.valid
                        || query::live(conn, root.grant)?,
                    ..CloudSummary::default()
                };
                let mut stmt = conn.prepare(CLOUD_SUMMARY_SQL)?;
                let mut rows =
                    stmt.query(rusqlite::params![directory, root.grant, EXCLUDED_FLAGS])?;
                while let Some(row) = rows.next()? {
                    let attrs: u32 = row.get(0)?;
                    let logical = u64::try_from(row.get::<_, i64>(1)?)
                        .map_err(|_| Error::Invalid("negative logical bytes"))?;
                    let allocated = row
                        .get::<_, Option<i64>>(2)?
                        .map(u64::try_from)
                        .transpose()
                        .map_err(|_| Error::Invalid("negative allocated bytes"))?;
                    let bucket = match cloud_state(Some(attrs)) {
                        CloudState::CloudOnly => &mut report.cloud_only,
                        CloudState::Local => &mut report.local,
                        CloudState::Pinned => &mut report.pinned,
                        CloudState::Unknown => &mut report.unknown,
                    };
                    bucket.files = add(bucket.files, 1)?;
                    bucket.logical_bytes = add(bucket.logical_bytes, logical)?;
                    bucket.allocated_bytes = match (bucket.allocated_bytes, allocated) {
                        (Some(a), Some(b)) => Some(add(a, b)?),
                        _ => None,
                    };
                }
                Ok(report)
            })();
            match result {
                Err(e) if query::is_budget_interrupt(&e) && guard.hit() => {
                    Err(Error::ResourceBudget)
                }
                Err(e)
                    if query::is_budget_interrupt(&e)
                        && deadline.is_some_and(|at| std::time::Instant::now() >= at) =>
                {
                    Err(Error::Cancelled)
                }
                other => other,
            }
        })
    }
}

fn add(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b)
        .ok_or(Error::Invalid("cloud byte overflow"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_query_vm_steps_per_file() {
        use crate::{Catalog, RootObservation, WriteCommand, WriteReply};
        let tmp = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(tmp.path(), "synthetic").unwrap();
        let WriteReply::Root {
            grant_id, dir_id, ..
        } = catalog
            .writer()
            .call(WriteCommand::RegisterRoot(RootObservation {
                volume_guid: None,
                volume_key: "fixture".into(),
                display_name: "Synthetic".into(),
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
        let db = rusqlite::Connection::open(tmp.path().join("catalog.db")).unwrap();
        let reader = catalog.reader().unwrap();
        for files in [1_000, 10_000, 100_000] {
            db.execute("DELETE FROM file", []).unwrap();
            db.execute(
                "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<?2)
                 INSERT INTO file(dir_id,name,logical,allocated,attrs,flags,born_run,seen_run)
                 SELECT ?1,'synthetic-'||x,1,0,4194304,8,0,0 FROM n",
                rusqlite::params![dir_id, files],
            )
            .unwrap();
            let mut stmt = reader.conn.prepare(CLOUD_SUMMARY_SQL).unwrap();
            let mut rows = stmt
                .query(rusqlite::params![dir_id, grant_id, EXCLUDED_FLAGS])
                .unwrap();
            let mut seen = 0;
            while rows.next().unwrap().is_some() {
                seen += 1;
            }
            drop(rows);
            assert_eq!(seen, files);
            let steps = stmt.get_status(rusqlite::StatementStatus::VmStep);
            eprintln!(
                "cloud query: {files} files, {steps} VM steps, {:.4} steps/file",
                steps as f64 / files as f64
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn listed_attribute_residency_patterns() {
        for attrs in [0x00400000, 0x00040000, 0x1000] {
            for preference in [0, 0x80000, 0x100000, 0x180000] {
                assert_eq!(cloud_state(Some(attrs | preference)), CloudState::CloudOnly);
            }
        }
        for attrs in [0x80, 0x20, 0x100000, 0x100020, 0x100420] {
            assert_eq!(cloud_state(Some(attrs)), CloudState::Local);
        }
        assert_eq!(cloud_state(Some(0x80020)), CloudState::Pinned);
        for attrs in [0, 0x180000, 0x400, 0x200] {
            assert_eq!(cloud_state(Some(attrs)), CloudState::Unknown);
        }
        assert_eq!(cloud_state(None), CloudState::Unknown);
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_attributes_are_unknown() {
        for attrs in [None, Some(0), Some(0x80), Some(0x80000), Some(0x400000)] {
            assert_eq!(cloud_state(attrs), CloudState::Unknown);
        }
    }
}
