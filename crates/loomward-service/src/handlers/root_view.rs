//! Root response assembly, shared by listing and grant/change notices.
use super::{catalog_error, db, encode, scan_view, ts_ns, Inner, RootPresence};
use loomward_catalog::{NodeKey, Reader};
use loomward_protocol::{ErrorBody, RootId};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub(crate) type RootPresences = std::collections::HashMap<i64, RootPresence>;
pub(crate) type RootPresenceCache = std::collections::HashMap<i64, (Option<Instant>, RootPresence)>;
const PRESENCE_TTL: Duration = Duration::from_secs(3);

pub(crate) const OFFLINE_SPACE_NOTE: &str =
    "Offline. Last-seen capacity and free space are observations, not current availability.";

pub(crate) fn volume_online(
    volume: &db::VolumeRow,
    roots: &[db::RootRow],
    presence: &RootPresences,
) -> bool {
    let mut online = None;
    for root in roots
        .iter()
        .filter(|r| r.volume == Some(volume.id) && r.grant_state == "active")
    {
        match presence.get(&root.grant) {
            Some(RootPresence::Online) => return true,
            Some(_) => online = Some(false),
            None => {}
        }
    }
    online.unwrap_or(volume.online)
}

fn missing_path(code: i32) -> bool {
    matches!(code as u32, 2 | 3 | 0xC000_000F | 0xC000_0034 | 0xC000_003A)
}

impl Inner {
    pub(crate) fn remember_root_presence(&self, grant: i64, presence: RootPresence) {
        self.root_presence_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(grant, (Some(Instant::now()), presence));
    }

    /// Mutation notices use short-lived display hints, never synchronous device I/O.
    /// Expired or invalidated native observations make the retained scan stale.
    pub(crate) fn cached_root_presence(&self) -> RootPresences {
        let now = Instant::now();
        self.root_presence_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(&grant, &(at, presence))| {
                (
                    grant,
                    if at.is_some_and(|at| now.saturating_duration_since(at) < PRESENCE_TTL) {
                        presence
                    } else {
                        RootPresence::Offline
                    },
                )
            })
            .collect()
    }

    pub(crate) fn invalidate_root_presence(&self) {
        for (at, _) in self
            .root_presence_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values_mut()
        {
            *at = None;
        }
    }

    pub(crate) fn forget_root_presence(&self, grant: i64) {
        self.root_presence_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&grant);
    }

    /// Probe before taking the mutation lock; the results are display observations, never grants.
    pub(crate) fn observe_root_presence(&self) -> Result<RootPresences, ErrorBody> {
        let rows = db::roots(&self.db(), None).map_err(crate::sql_error)?;
        Ok(rows
            .iter()
            .filter(|r| r.grant_state == "active")
            .filter_map(|r| self.root_presence(r).map(|p| (r.grant, p)))
            .collect())
    }

    /// Metadata-only presence of a native grant. A different volume is never online for this root.
    /// Fixtures and legacy grants without a pinned volume GUID keep their existing path.
    pub(crate) fn root_presence(&self, r: &db::RootRow) -> Option<RootPresence> {
        #[cfg(test)]
        assert!(
            !self
                .hooks
                .forbid_presence_probe
                .load(std::sync::atomic::Ordering::SeqCst),
            "mutation notice attempted a synchronous presence probe"
        );
        // Admission and placement always probe afresh; the cache is only for mutation notices.
        let presence = self.probe_root_presence(r);
        if let Some(presence) = presence {
            self.remember_root_presence(r.grant, presence);
        }
        presence
    }

    fn probe_root_presence(&self, r: &db::RootRow) -> Option<RootPresence> {
        if !matches!(r.origin.as_str(), "owner_granted" | "lab_generated") {
            return None;
        }
        let (key, file, guid): (String, Vec<u8>, Option<String>) = self
            .db()
            .query_row(
                "SELECT volume_key,root_file_id,volume_guid FROM st.root_grant WHERE id=?1",
                [r.grant],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok()?;
        let guid = guid?;
        #[cfg(test)]
        if self
            .hooks
            .check_presence_unlocked
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            assert!(
                self.mutation.try_lock().is_ok(),
                "presence probe holds the mutation lock"
            );
        }
        match self.open_observation(std::path::Path::new(&r.display_path)) {
            Ok((_, identity)) => Some(match Self::identity_triple(&identity) {
                Err(_) => RootPresence::Offline,
                Ok(seen) => {
                    if seen.2 != guid {
                        // Another volume holds the path (a stick on a detached disk's letter): the root
                        // is away, not replaced. identity_changed is terminal and redacts learning, so it
                        // is kept for the same volume with a different folder (#869 deep review).
                        RootPresence::Offline
                    } else if seen == (key, file, guid) {
                        RootPresence::Online
                    } else {
                        RootPresence::IdentityChanged
                    }
                }
            }),
            Err(loomward_windows::enumerate::SourceError::Io(code)) if missing_path(code) => {
                // Only a matching volume on an accessible ancestor proves "folder gone". A missing
                // mount or a foreign volume's parent must keep the detached root offline.
                for ancestor in std::path::Path::new(&r.display_path).ancestors().skip(1) {
                    match self.open_observation(ancestor) {
                        Ok((_, identity)) => {
                            return Some(
                                if identity.volume_guid.map(|g| format!("{g:032x}")) == Some(guid) {
                                    RootPresence::RootMissing
                                } else {
                                    RootPresence::Offline
                                },
                            );
                        }
                        Err(loomward_windows::enumerate::SourceError::Io(code))
                            if missing_path(code) => {}
                        Err(_) => break,
                    }
                }
                Some(RootPresence::Offline)
            }
            // Device not ready or disconnected. Other errors prove no absence.
            Err(loomward_windows::enumerate::SourceError::Io(code))
                if matches!(
                    code as u32,
                    15 | 21 | 55 | 1167 | 0xC000_000E | 0xC000_009D | 0xC000_00A3
                ) =>
            {
                Some(RootPresence::Offline)
            }
            Err(loomward_windows::enumerate::SourceError::IdentityUnverified) => {
                Some(RootPresence::Offline)
            }
            Err(_) => None,
        }
    }

    pub(super) fn root_json(
        &self,
        r: &db::RootRow,
        reader: Option<&mut Reader>,
        presence: Option<RootPresence>,
    ) -> Result<Value, ErrorBody> {
        let (mut state, mut coverage) = scan_view(r);
        if r.grant_state == "active"
            && matches!(
                presence,
                Some(
                    RootPresence::Offline
                        | RootPresence::RootMissing
                        | RootPresence::IdentityChanged
                )
            )
        {
            state = "stale";
            coverage = "stale";
        }
        let active = r.grant_state == "active";
        let totals = match (reader, r.dir, active && r.state != "never_scanned") {
            (Some(reader), Some(dir), true) => match reader.inspect(NodeKey::Dir(dir)) {
                // Mid-scan or awaiting repair, the aggregates are unsettled: unknown, not an error.
                Err(loomward_catalog::Error::RepairRequired) => None,
                result => result.map_err(catalog_error)?.subtree,
            },
            _ => None,
        };
        // The aggregates were read later than the root row and may already need repair.
        if coverage == "complete" && !totals.as_ref().is_some_and(|s| s.complete) {
            coverage = "partial";
        }
        let totals = totals.map(|s| encode(&s)).transpose()?;
        // The engine names the newest scan it submitted for this root (not persisted across restarts).
        let last_job_id = RootId::new(self.root_id(r.root))
            .ok()
            .and_then(|root| self.engine.last_scan_job(&root));
        let display: String = r.display_path.chars().take(1024).collect();
        Ok(json!({
            "root_id": self.root_id(r.root),
            "display_path": {"text": display, "truncated": r.display_path.chars().count() > 1024},
            "origin": r.origin,
            "dataset_class": self.dataset,
            "volume_id": r.volume.map(|v| self.volume_id(v)),
            "granted_at": ts_ns(r.granted_at_ns),
            "granted_via": r.granted_via,
            "grant_state": r.grant_state,
            "scan": {
                "state": state,
                "last_job_id": last_job_id,
                "generation": r.generation.map(|g| g.to_string()),
                "finished_at": r.finished_at_ns.map(ts_ns),
                "coverage": coverage,
            },
            "totals": totals,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, Service};
    use loomward_catalog::{Catalog, DirListing, RootObservation, WriteCommand, WriteReply};
    use loomward_protocol::DatasetClass;
    use std::time::Duration;

    fn fixture() -> (
        tempfile::TempDir,
        Service,
        rusqlite::Connection,
        db::RootRow,
    ) {
        let tmp = tempfile::tempdir_in(crate::paths::test_roots_base()).unwrap();
        let catalog = Catalog::open(tmp.path(), "synthetic").unwrap();
        let WriteReply::Root {
            grant_id,
            root_id,
            dir_id,
        } = catalog
            .writer()
            .call(WriteCommand::RegisterRoot(RootObservation {
                volume_guid: None,
                volume_key: "fixture".into(),
                display_name: "Synthetic".into(),
                display_path: "Synthetic root".into(),
                root_file_id: Some(vec![1; 16]),
                filesystem: Some("NTFS".into()),
                origin: "fixture".into(),
                granted_via: "fixture".into(),
                observed_at_ns: 0,
            }))
            .unwrap()
        else {
            panic!("register root reply");
        };
        let WriteReply::Run(run_id) = catalog
            .writer()
            .call(WriteCommand::BeginRun {
                grant_id,
                mode: "full".into(),
                strategy: "fixture".into(),
                started_at_ns: 0,
            })
            .unwrap()
        else {
            panic!("begin run reply");
        };
        catalog
            .writer()
            .call(WriteCommand::DirListing(DirListing {
                run_id,
                dir_id,
                files: vec![],
                dirs: vec![],
                state: "complete".into(),
                skipped: 0,
                errors: 0,
            }))
            .unwrap();
        catalog
            .writer()
            .call(WriteCommand::EndRun {
                run_id,
                state: "completed".into(),
                finished_at_ns: 1,
            })
            .unwrap();
        drop(catalog);
        let service = Service::open(Config {
            state_dir: tmp.path().to_path_buf(),
            dataset: DatasetClass::Synthetic,
            allow_personal: false,
            allow_identity_unverified: false,
            grant_roots: vec![],
        })
        .unwrap();
        let connection =
            rusqlite::Connection::open(loomward_catalog::sqlite_dir(tmp.path()).join("catalog.db"))
                .unwrap();
        connection.busy_timeout(Duration::from_secs(5)).unwrap();
        // Capture exactly the first read used by root_list, before the controlled interleaving.
        let row = db::roots(&service.inner.db(), Some(root_id))
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(scan_view(&row), ("complete", "complete"));
        (tmp, service, connection, row)
    }

    fn dirty(connection: &rusqlite::Connection, row: &db::RootRow) {
        assert_eq!(
            connection
                .execute(
                    "UPDATE dir SET dirty_rev=agg_valid_rev+1 WHERE id=?1",
                    [row.dir.unwrap()],
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn clean_root_and_totals_agree_on_complete_coverage() {
        let (_tmp, service, _connection, row) = fixture();
        let mut reader = service.inner.catalog.reader().unwrap();
        let result = service
            .inner
            .root_json(&row, Some(&mut reader), None)
            .unwrap();
        assert_eq!(result["scan"]["coverage"], "complete");
        assert_eq!(result["totals"]["complete"], true);
    }

    /// #715: one root over the unique-totals work budget must not fail the listing; its unique
    /// totals are null (unknown) and the stored aggregates stay exact.
    #[test]
    fn over_budget_unique_totals_stay_unknown_without_failing_the_root() {
        let (_tmp, service, _connection, row) = fixture();
        let mut reader = service.inner.catalog.reader().unwrap();
        reader.unique_totals_steps = 1;
        let result = service
            .inner
            .root_json(&row, Some(&mut reader), None)
            .unwrap();
        let totals = &result["totals"];
        assert_eq!(totals["files"], 0);
        assert_eq!(totals["complete"], true);
        for field in [
            "unique_objects",
            "unique_allocated_bytes",
            "multi_link_entries",
        ] {
            assert!(totals[field].is_null(), "{field}: {totals}");
        }
        assert_eq!(result["scan"]["coverage"], "complete");
    }

    #[test]
    fn repair_between_root_row_and_totals_cannot_leave_complete_coverage() {
        let (_tmp, service, connection, row) = fixture();
        dirty(&connection, &row);
        let mut reader = service.inner.catalog.reader().unwrap();
        let result = service
            .inner
            .root_json(&row, Some(&mut reader), None)
            .unwrap();
        // The real reader repairs the inactive dirty root using rollup(clean=false).
        assert_eq!(result["totals"]["complete"], false);
        assert_eq!(result["scan"]["state"], "complete");
        assert_eq!(result["scan"]["coverage"], "partial");
    }

    #[test]
    fn repair_required_between_reads_keeps_totals_unknown_and_coverage_partial() {
        let (_tmp, service, connection, row) = fixture();
        let WriteReply::Run(run_id) = service
            .inner
            .catalog
            .writer()
            .call(WriteCommand::BeginRun {
                grant_id: row.grant,
                mode: "refresh".into(),
                strategy: "fixture".into(),
                started_at_ns: 2,
            })
            .unwrap()
        else {
            panic!("begin run reply");
        };
        dirty(&connection, &row);
        let mut reader = service.inner.catalog.reader().unwrap();
        // Active-run aggregates are not auto-repaired: exercise the actual error branch.
        assert!(matches!(
            reader.inspect(NodeKey::Dir(row.dir.unwrap())),
            Err(loomward_catalog::Error::RepairRequired)
        ));
        let result = service
            .inner
            .root_json(&row, Some(&mut reader), None)
            .unwrap();
        service
            .inner
            .catalog
            .writer()
            .call(WriteCommand::EndRun {
                run_id,
                state: "cancelled".into(),
                finished_at_ns: 3,
            })
            .unwrap();
        assert!(result["totals"].is_null());
        assert_eq!(result["scan"]["coverage"], "partial");
    }

    #[test]
    fn revoked_roots_keep_stale_coverage_without_totals() {
        let (_tmp, service, _connection, row) = fixture();
        service
            .inner
            .catalog
            .writer()
            .call(WriteCommand::RevokeGrant {
                grant_id: row.grant,
                revoked_at_ns: 2,
            })
            .unwrap();
        let revoked = db::roots(&service.inner.db(), Some(row.root))
            .unwrap()
            .pop()
            .unwrap();
        let result = service.inner.root_json(&revoked, None, None).unwrap();
        assert_eq!(result["grant_state"], "revoked");
        assert_eq!(result["scan"]["state"], "stale");
        assert_eq!(result["scan"]["coverage"], "stale");
        assert!(result["totals"].is_null());
    }
}
