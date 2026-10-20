//! #738 service-lifetime regressions. These use the real service, grants, SQLite journal and
//! finished worker handles, but never open a native copy source or destination. No lab/elevation
//! early return: the worker exercises the metadata failure that a failed executor leaves behind.
use super::*;
use crate::{CallContext, Config, Service};
use loomward_operations::{
    store::fault::{Kind as FaultKind, Stage},
    Destination, Entry, FenceCause, Identity, Manifest, ManifestItem, ObjectId, OpState, Record,
    Refusal, Replay, Store,
};
use loomward_windows::{
    effects::exec,
    identity::{IdentityQuality, ObjectKey, ObservedIdentity},
};
use std::{
    sync::{atomic::Ordering, mpsc, Weak},
    time::{Duration, Instant},
};

const OP: u64 = 1;
const WIRE: &str = "cp_terminal738";

struct Fixture {
    service: Service,
    path: PathBuf,
    root: String,
    _dir: tempfile::TempDir,
}

impl Fixture {
    fn publishing() -> Self {
        let dir = crate::tests::temp_root();
        let service = Service::open(Config {
            state_dir: dir.path().to_path_buf(),
            dataset: DatasetClass::Synthetic,
            allow_personal: true,
            allow_identity_unverified: false,
            grant_roots: vec![],
        })
        .unwrap();
        let (root, grant) = crate::tests::fixture(&service, 0);
        let identity = Identity {
            volume: 7,
            file_id: [2; 16],
        };
        let key = ObjectKey {
            volume_serial: "7".into(),
            file_id: identity.file_id,
            quality: IdentityQuality::FileId128,
            creation_time: "2".into(),
            reparse_tag: 0,
        };
        let manifest = Manifest {
            destination: Destination {
                grant: grant as u64,
                parent: identity,
                folder_name: "Loomward copy".into(),
            },
            root: None,
            items: vec![ManifestItem {
                name: "fixture.bin".into(),
                kind: loomward_operations::tree::Kind::File,
                identity,
                size: 1,
                creation_ft: 2,
                last_write_ft: 2,
                change_ft: 2,
                attributes: 0,
                link_count: 1,
                streams: vec![],
            }],
        };
        let digest = manifest.digest();
        let plan = exec::Plan {
            operation_id: WIRE.into(),
            source_root: dir.path().join("source-must-not-exist"),
            source_root_id: [1; 16],
            source_root_volume: 7,
            source_root_guid: None,
            source_root_attributes: 0,
            destination: dir.path().join("destination-must-not-exist"),
            destination_key: key.clone(),
            destination_root: (7, [1; 16]),
            destination_root_guid: None,
            grants: vec![grant],
            manifest,
            sources: vec![(
                PathBuf::from("fixture.bin"),
                ObservedIdentity {
                    object: key,
                    link_count: 1,
                    attributes: 0,
                    size: "1".into(),
                    allocation: "1".into(),
                    last_access_time: "2".into(),
                    last_write_time: "2".into(),
                    change_time: "2".into(),
                    ancestors: vec![],
                },
            )],
        };
        let path = loomward_catalog::sqlite_dir(dir.path()).join("state.db");
        // Seed only metadata, as in the portable journal fixtures. No approval or effect is run.
        let row = Replay::new(OP).row_for(&Entry {
            epoch: 1,
            record: Record::Started { digest, items: 1 },
        });
        let raw = rusqlite::Connection::open(&path).unwrap();
        raw.execute(
            "INSERT INTO op_journal(op_id,seq,body,chain) VALUES(1,1,?1,?2)",
            rusqlite::params![row.body, row.chain],
        )
        .unwrap();
        raw.execute(
            "INSERT INTO op_journal_head(op_id,seq,chain) VALUES(1,1,?1)",
            [&row.chain],
        )
        .unwrap();
        let mut store = service.inner.copies.store.as_ref().unwrap().lock().unwrap();
        let id = |n| ObjectId {
            file_id: [n; 16],
            creation_ft: i64::from(n),
        };
        for record in [
            Record::StagingIntent {
                token: "lw-staging-738".into(),
            },
            Record::StagingCreated { id: id(1) },
            Record::CreateBatch {
                items: vec![(0, "fixture.bin".into())],
            },
            Record::Identities {
                items: vec![(0, id(2))],
            },
            Record::Verified { items: vec![0] },
            Record::Transition {
                to: OpState::Blocked,
                code: Some(Refusal::SourceInUse),
            },
            Record::Transition {
                to: OpState::Copying,
                code: None,
            },
            Record::Transition {
                to: OpState::Staged,
                code: None,
            },
            Record::PublishIntent {
                final_name: "Loomward copy".into(),
            },
        ] {
            store.append(OP, 1, record).unwrap();
        }
        store
            .set_plan(OP, &serde_json::to_string(&plan).unwrap())
            .unwrap();
        drop(store);
        Fixture {
            root: service.inner.root_id(root),
            service,
            path,
            _dir: dir,
        }
    }

    fn store(&self) -> &Arc<Mutex<Store>> {
        self.service.inner.copies.store.as_ref().unwrap()
    }

    fn fault(&self, stage: Stage) {
        self.store()
            .lock()
            .unwrap()
            .set_fault(Some(Box::new(move |record, at| {
                ((matches!(record, Record::Fenced { .. }) && at == Stage::Row)
                    || (matches!(record, Record::RevokedAfterPublish { .. }) && at == stage))
                    .then_some(FaultKind::Full)
            })));
    }

    fn call(&self, command: &str, payload: Value) -> Result<Payload, ErrorBody> {
        static REQUEST: AtomicU64 = AtomicU64::new(0);
        let req: RequestEnvelope = serde_json::from_value(json!({
            "protocol": "loomward/3",
            "request_id": format!("r{}", REQUEST.fetch_add(1, Ordering::Relaxed)),
            "command": command,
            "payload": payload,
            "deadline_ms": 60_000,
        }))
        .unwrap();
        match self.service.call(req, &CallContext::http()) {
            ResponseEnvelope::Ok(ok) => {
                ok.validate_for(Command::from_name(command).unwrap())
                    .unwrap();
                Ok(ok.result)
            }
            ResponseEnvelope::Err(err) => Err(err.error),
        }
    }

    fn revoke(&self) -> Result<Payload, ErrorBody> {
        self.call(
            "roots.revoke",
            json!({"root_id": self.root, "purge_catalog": false}),
        )
    }

    fn status(&self) -> Payload {
        self.call("copy.status", json!({"operation_id": WIRE}))
            .unwrap()
    }

    fn committed(&self) -> Replay {
        Store::open(&self.path)
            .unwrap()
            .append(OP, 1, Record::Committed)
            .unwrap();
        self.store().lock().unwrap().replay(OP).unwrap().unwrap()
    }

    fn assert_receipt(&self, before: &Replay, cause: FenceCause) {
        let reopened = Store::open(&self.path).unwrap();
        let after = reopened.replay(OP).unwrap().unwrap();
        assert_eq!(after.state, OpState::Committed);
        assert_eq!(after.epoch, before.epoch);
        assert_eq!(after.digest, before.digest);
        assert_eq!(after.items, before.items);
        assert_eq!(after.staging, before.staging);
        assert_eq!(after.publish, before.publish);
        assert_eq!(after.cancelled, before.cancelled);
        assert_eq!(after.fenced, before.fenced);
        assert_eq!(after.seq, before.seq + 1);
        let annotations: Vec<_> = reopened
            .rows(OP)
            .unwrap()
            .into_iter()
            .filter_map(
                |row| match serde_json::from_str::<Entry>(&row.body).unwrap().record {
                    Record::RevokedAfterPublish { cause } => Some(cause),
                    _ => None,
                },
            )
            .collect();
        assert_eq!(annotations, vec![cause]);
        let status = self.status();
        assert_eq!(status["state"], "committed");
        assert_eq!(status["items_verified"], 1);
        assert_eq!(status["refusal"]["code"], "source_in_use");
        assert!(!self._dir.path().join("source-must-not-exist").exists());
        assert!(!self._dir.path().join("destination-must-not-exist").exists());
    }

    fn admission_error(&self) -> ErrorBody {
        self.call(
            "copy.start",
            json!({
                "operation_id": "cp_new738",
                "digest": format!("sha256:{}", "00".repeat(32)),
            }),
        )
        .unwrap_err()
    }

    fn finished_pending_worker(&self, cause: FenceCause) -> (Replay, Weak<exec::Control>) {
        let control = Arc::new(exec::Control::new(false));
        assert!(exec::fence(self.store(), OP, &control, cause).is_err());
        let before = self.committed();
        let weak = Arc::downgrade(&control);
        let worker_control = control.clone();
        let worker_store = self.store().clone();
        let (completed, done) = mpsc::channel();
        let (returned, result) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            // The journal fault stays active through this return AND service reaping. A one-shot
            // fault could heal in finish() and would not exercise the ownership regression.
            returned
                .send(exec::fence(&worker_store, OP, &worker_control, cause).is_err())
                .unwrap();
            let _ = completed.send(());
        });
        assert!(result.recv_timeout(Duration::from_secs(5)).unwrap());
        let until = Instant::now() + Duration::from_secs(5);
        while !thread.is_finished() {
            assert!(Instant::now() < until, "metadata worker did not finish");
            std::thread::yield_now();
        }
        self.service.inner.copies.running.lock().unwrap().insert(
            OP,
            Run {
                control,
                thread,
                done,
            },
        );
        self.service
            .inner
            .copies
            .ops
            .lock()
            .unwrap()
            .insert(WIRE.into(), OP);
        (before, weak)
    }
}

fn pending(error: ErrorBody) {
    assert!(error.retryable, "{error:?}");
    assert_eq!(
        error.detail.as_ref().unwrap()["reason"],
        "copy_fence_pending"
    );
}

#[test]
fn finished_worker_metadata_survives_status_reaping_and_retries_once() {
    for stage in [Stage::Row, Stage::Head, Stage::Ack] {
        let fixture = Fixture::publishing();
        fixture.fault(stage);
        let (before, control) = fixture.finished_pending_worker(FenceCause::EmergencyStop);
        assert_eq!(fixture.status()["state"], "committed");
        assert!(fixture
            .service
            .inner
            .copies
            .running
            .lock()
            .unwrap()
            .is_empty());
        assert!(fixture.service.inner.copies.ops.lock().unwrap().is_empty());
        assert!(
            control.strong_count() > 0,
            "{stage:?}: status lost the pending cause"
        );
        let error = fixture.admission_error();
        assert_eq!(error.code, ErrorCode::Busy);
        pending(error);
        fixture.store().lock().unwrap().set_fault(None);
        fixture.revoke().unwrap();
        fixture.assert_receipt(&before, FenceCause::EmergencyStop);
        assert_eq!(
            control.strong_count(),
            0,
            "successful retry must retire the control"
        );
        assert_eq!(fixture.admission_error().code, ErrorCode::NotFound);
        fixture.revoke().unwrap();
        fixture.assert_receipt(&before, FenceCause::EmergencyStop);
    }
}

#[test]
fn end_of_revoke_reaping_keeps_a_finished_workers_retryable_annotation() {
    for stage in [Stage::Row, Stage::Head] {
        let fixture = Fixture::publishing();
        fixture.fault(stage);
        let (before, control) = fixture.finished_pending_worker(FenceCause::GrantRevoked);
        pending(fixture.revoke().unwrap_err());
        assert!(fixture
            .service
            .inner
            .copies
            .running
            .lock()
            .unwrap()
            .is_empty());
        assert!(
            control.strong_count() > 0,
            "{stage:?}: the error response lost its retry"
        );
        assert_eq!(fixture.status()["state"], "committed");
        fixture.store().lock().unwrap().set_fault(None);
        fixture.revoke().unwrap();
        fixture.assert_receipt(&before, FenceCause::GrantRevoked);
        assert_eq!(control.strong_count(), 0);
    }
}

#[test]
fn cancellation_join_keeps_a_failed_terminal_annotation_for_retry() {
    for stage in [Stage::Row, Stage::Head] {
        let fixture = Fixture::publishing();
        fixture.fault(stage);
        let (before, control) = fixture.finished_pending_worker(FenceCause::EmergencyStop);
        pending(
            fixture
                .call("copy.cancel", json!({"operation_id": WIRE}))
                .unwrap_err(),
        );
        assert!(fixture
            .service
            .inner
            .copies
            .running
            .lock()
            .unwrap()
            .is_empty());
        assert!(
            control.strong_count() > 0,
            "{stage:?}: joining discarded the pending cause"
        );
        assert_eq!(fixture.status()["state"], "committed");
        fixture.store().lock().unwrap().set_fault(None);
        fixture
            .call("copy.cancel", json!({"operation_id": WIRE}))
            .unwrap();
        fixture.assert_receipt(&before, FenceCause::EmergencyStop);
        assert_eq!(control.strong_count(), 0);
    }
}

#[test]
fn a_no_worker_fence_retains_its_preterminal_cause_across_foreign_commit() {
    for stage in [Stage::Row, Stage::Head, Stage::Ack] {
        let fixture = Fixture::publishing();
        fixture.fault(stage);
        pending(fixture.revoke().unwrap_err());
        assert!(fixture
            .service
            .inner
            .copies
            .running
            .lock()
            .unwrap()
            .is_empty());
        let before = fixture.committed();
        pending(fixture.revoke().unwrap_err());
        assert_eq!(fixture.status()["state"], "committed");
        pending(fixture.admission_error());
        fixture.store().lock().unwrap().set_fault(None);
        fixture.revoke().unwrap();
        fixture.assert_receipt(&before, FenceCause::GrantRevoked);
        assert_eq!(fixture.admission_error().code, ErrorCode::NotFound);
        fixture.revoke().unwrap();
        fixture.assert_receipt(&before, FenceCause::GrantRevoked);
    }
}
