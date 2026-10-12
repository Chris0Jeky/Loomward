//! Public service regression for #361. Synthetic catalogue only, no filesystem scan.
use loomward_catalog::{Catalog, DirListing, RootObservation, WriteCommand, WriteReply};
use loomward_protocol::{CallContext, DatasetClass, ResponseEnvelope, ViewService};
use loomward_service::{Config, Service};
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

fn fixture() -> (tempfile::TempDir, Service, rusqlite::Connection, i64) {
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
    (tmp, service, connection, root_id)
}

fn listed_root(service: &Service) -> Value {
    let request = serde_json::from_value(json!({
        "protocol": "loomward/3",
        "request_id": "root-coverage",
        "command": "roots.list",
        "payload": {}
    }))
    .unwrap();
    match service.call(request, &CallContext::http()) {
        ResponseEnvelope::Ok(response) => {
            let roots = &response.result["roots"];
            assert_eq!(roots.as_array().unwrap().len(), 1);
            roots[0].clone()
        }
        ResponseEnvelope::Err(error) => panic!("roots.list failed: {:?}", error.error),
    }
}

#[test]
fn completed_dirty_root_has_unknown_totals_and_partial_coverage() {
    let (_tmp, service, connection, root_id) = fixture();
    let before = listed_root(&service);
    assert_eq!(before["scan"]["state"], "complete");
    assert_eq!(before["scan"]["coverage"], "complete");
    assert!(!before["totals"].is_null(), "known empty root is not unknown");

    // Dirty only the aggregate witness. The completed traversal and sub_complete flag stay set.
    assert_eq!(
        connection
            .execute(
                "UPDATE dir SET dirty_rev=agg_valid_rev+1 WHERE root_id=?1 AND parent_id IS NULL",
                [root_id],
            )
            .unwrap(),
        1
    );
    let dirty = listed_root(&service);
    assert_eq!(dirty["grant_state"], "active");
    assert_eq!(dirty["scan"]["state"], "complete");
    assert!(dirty["totals"].is_null());
    assert_eq!(dirty["scan"]["coverage"], "partial");

    // Restore the unchanged aggregate's witness, without fabricating a new scan or totals.
    connection
        .execute(
            "UPDATE dir SET dirty_rev=agg_valid_rev WHERE root_id=?1 AND parent_id IS NULL",
            [root_id],
        )
        .unwrap();
    let current = listed_root(&service);
    assert_eq!(current["scan"]["coverage"], "complete");
    assert_eq!(current["totals"], before["totals"]);
}
