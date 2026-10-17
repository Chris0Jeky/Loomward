//! Root response assembly, shared by listing and grant/change notices.
use super::{catalog_error, db, encode, scan_view, ts_ns, Inner};
use loomward_catalog::{NodeKey, Reader};
use loomward_protocol::ErrorBody;
use serde_json::{json, Value};

impl Inner {
    pub(super) fn root_json(
        &self,
        r: &db::RootRow,
        reader: Option<&mut Reader>,
    ) -> Result<Value, ErrorBody> {
        let (state, mut coverage) = scan_view(r);
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
                "last_job_id": null,
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
    use std::path::Path;
    use std::time::Duration;

    fn fixture() -> (
        tempfile::TempDir,
        Service,
        rusqlite::Connection,
        db::RootRow,
    ) {
        let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-roots");
        std::fs::create_dir_all(&target).unwrap();
        let tmp = tempfile::tempdir_in(target.canonicalize().unwrap()).unwrap();
        let catalog = Catalog::open(tmp.path(), "synthetic").unwrap();
        let WriteReply::Root {
            grant_id,
            root_id,
            dir_id,
        } = catalog
            .writer()
            .call(WriteCommand::RegisterRoot(RootObservation {
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
        let result = service.inner.root_json(&row, Some(&mut reader)).unwrap();
        assert_eq!(result["scan"]["coverage"], "complete");
        assert_eq!(result["totals"]["complete"], true);
    }

    #[test]
    fn repair_between_root_row_and_totals_cannot_leave_complete_coverage() {
        let (_tmp, service, connection, row) = fixture();
        dirty(&connection, &row);
        let mut reader = service.inner.catalog.reader().unwrap();
        let result = service.inner.root_json(&row, Some(&mut reader)).unwrap();
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
        let result = service.inner.root_json(&row, Some(&mut reader)).unwrap();
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
        let result = service.inner.root_json(&revoked, None).unwrap();
        assert_eq!(result["grant_state"], "revoked");
        assert_eq!(result["scan"]["state"], "stale");
        assert_eq!(result["scan"]["coverage"], "stale");
        assert!(result["totals"].is_null());
    }
}
