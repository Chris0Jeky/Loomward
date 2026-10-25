//! docs/47 lane C5 (LW-040 #46): copy-and-verify served for SYNTHETIC sessions on registered lab roots.
//! Personal sessions never reach this module: `Service::call` answers them `copy_effect_gated` before
//! dispatch (AGENTS.md invariant 1; the C8 gate stays closed until the G5 power-cut and G9 restore runs).
//! A synthetic session's grants exist only for registered lab roots (ADR-V3-15), and the session root
//! gate is re-run here for every source and the destination. MCP has no copy command.
// The executor is Windows-only; other targets build the module but never serve a copy.
#![cfg_attr(not(windows), allow(dead_code))]
use crate::{catalog_error, db, fail, now_ns, ts_ns, with_reason, Inner, Out};
use loomward_catalog::NodeKey;
use loomward_protocol::*;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64},
        Arc, Mutex,
    },
};

/// docs/47 Q1, OPEN (agent-hq#461): may cancel and abandon delete Loomward's own unpublished staging?
/// `false` (default, option b): they stop and LEAVE the staging folder, reported in `staging_folder`.
/// `true`: identity-bound removal of Loomward's own journaled staging only. The owner's answer is this
/// one-line flip.
pub(crate) const LOOMWARD_COPY_STAGING_REMOVAL: bool = false;

pub(crate) struct Pending {
    sources: Vec<String>,
    destination: String,
    preview: loomward_operations::Preview,
    approval: Option<loomward_operations::Approval>,
    /// Unix ms after which the entry is evicted (#569): the preview's TTL, extended to the approval's
    /// own expiry once approved. A lapsed preview cannot be approved; `copy.start` also checks the
    /// approval's expiry after re-observing everything.
    pub(crate) expires_ms: i64,
}

#[cfg(all(test, windows))]
impl Pending {
    pub(crate) fn expire_approval(&mut self) {
        let approval =
            loomward_operations::approve(&self.preview, self.preview.manifest.digest(), 0, 0)
                .unwrap();
        self.expires_ms = approval.expires_at_ms();
        self.approval = Some(approval);
    }
}

/// How long an unapproved preview is kept: as long as an approval lives.
const PREVIEW_TTL_MS: i64 = loomward_operations::manifest::APPROVAL_TTL_MS;

pub(crate) struct Copies {
    /// The journal in `state.db`; `None` for personal sessions and off Windows.
    #[cfg(windows)]
    pub(crate) store: Option<Arc<Mutex<loomward_operations::Store>>>,
    pub(crate) pending: Mutex<HashMap<String, Pending>>,
    /// Wire ID to journal operation for active workers only; history stays in the store.
    pub(crate) ops: Mutex<HashMap<String, u64>>,
    #[cfg(windows)]
    pub(crate) running: Mutex<HashMap<u64, Run>>,
    /// Pending fence metadata outlives worker handles, including a failed no-worker revocation.
    /// One Control per operation; unresolved entries stop new admission instead of being evicted.
    #[cfg(windows)]
    pending_fences: Mutex<HashMap<u64, Arc<loomward_windows::effects::exec::Control>>>,
    /// The Q1 setting for this service (tests flip it; production reads the constant).
    pub(crate) removal: AtomicBool,
    /// How long `Drop` waits, in total, for cancelled workers (#569). A worker stuck in a blocking OS
    /// call is detached after that; its journal is left to recovery.
    pub(crate) shutdown_wait_ms: AtomicU64,
    /// Test seams: `copy.start` calls the gate at "prepared" (tree walked, nothing registered) and
    /// "spawned" (worker running, not yet in `running`); new runs get the hook on their control.
    #[cfg(all(test, windows))]
    pub(crate) gate: Mutex<Option<Gate>>,
    #[cfg(all(test, windows))]
    pub(crate) hook: Mutex<Option<RunHook>>,
    #[cfg(all(test, windows))]
    pub(crate) prepare_probe: Mutex<Option<PrepareProbe>>,
}

/// Synthetic GUID observations and the request captured before native preparation; test-only.
#[cfg(all(test, windows))]
pub(crate) struct PrepareProbe {
    pub(crate) source_guid: u128,
    pub(crate) destination_guid: u128,
    pub(crate) received: Arc<Mutex<Vec<Option<u128>>>>,
}

/// The run hook's types, for the copy tests (the primitives stay referenced from this module only).
#[cfg(all(test, windows))]
pub(crate) use loomward_windows::effects::exec::{Flow, Point};
#[cfg(all(test, windows))]
pub(crate) type Gate = Arc<dyn Fn(&str) + Send + Sync>;
#[cfg(all(test, windows))]
pub(crate) type RunHook = Arc<
    dyn Fn(
            loomward_windows::effects::exec::Point,
            &loomward_windows::effects::exec::Control,
        ) -> loomward_windows::effects::exec::Flow
        + Send
        + Sync,
>;

#[cfg(windows)]
pub(crate) struct Run {
    control: Arc<loomward_windows::effects::exec::Control>,
    thread: std::thread::JoinHandle<()>,
    done: std::sync::mpsc::Receiver<()>,
}

#[cfg(windows)]
impl Run {
    fn join(self) -> Result<(), Self> {
        if matches!(
            self.done.recv_timeout(std::time::Duration::from_secs(30)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ) {
            return Err(self);
        }
        let _ = self.thread.join();
        Ok(())
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn digest_text(d: [u8; 32]) -> String {
    format!("sha256:{}", hex(&d))
}

fn parse_digest(d: &str) -> Result<[u8; 32], ErrorBody> {
    let bad = || fail(ErrorCode::InvalidRequest, "malformed digest");
    let hexed = d.strip_prefix("sha256:").ok_or_else(bad)?;
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(hexed.get(2 * i..2 * i + 2).ok_or_else(bad)?, 16)
            .map_err(|_| bad())?;
    }
    Ok(out)
}

#[cfg(all(test, windows))]
mod pending_fence_tests {
    //! #738 service-lifetime regressions. These use the real service, grants, SQLite journal and
    //! finished worker handles, but never open a native copy source or destination. No lab/elevation
    //! early return: the worker exercises the metadata failure that a failed executor leaves behind.
    use super::*;
    use crate::{CallContext, Config, Service};
    use loomward_operations::{
        store::fault::{Kind as FaultKind, Stage},
        Destination, Entry, FenceCause, Identity, Manifest, ManifestItem, ObjectId, OpState,
        Record, Refusal, Replay, Store,
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

    fn finished_worker_after_first_replay_error(fixture: &Fixture) -> Weak<exec::Control> {
        let control = Arc::new(
            exec::Control::new(false).with_hook(|_, _| panic!("read-error retry ran an effect")),
        );
        let weak = Arc::downgrade(&control);
        let worker_control = control.clone();
        let worker_store = fixture.store().clone();
        let (completed, done) = mpsc::channel();
        let (returned, result) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            returned
                .send(
                    exec::fence(
                        &worker_store,
                        OP,
                        &worker_control,
                        FenceCause::EmergencyStop,
                    )
                    .is_err(),
                )
                .unwrap();
            let _ = completed.send(());
        });
        assert!(result.recv_timeout(Duration::from_secs(5)).unwrap());
        let until = Instant::now() + Duration::from_secs(5);
        while !thread.is_finished() {
            assert!(Instant::now() < until, "read-error worker did not finish");
            std::thread::yield_now();
        }
        fixture.service.inner.copies.running.lock().unwrap().insert(
            OP,
            Run {
                control,
                thread,
                done,
            },
        );
        fixture
            .service
            .inner
            .copies
            .ops
            .lock()
            .unwrap()
            .insert(WIRE.into(), OP);
        weak
    }

    #[test]
    fn a_first_replay_error_survives_service_reaping_until_its_provenance_is_resolved() {
        for foreign_commits_first in [false, true] {
            let fixture = Fixture::publishing();
            let raw = rusqlite::Connection::open(&fixture.path).unwrap();
            raw.execute_batch(
                "ALTER TABLE op_journal_head RENAME TO pending_fence_unavailable_head",
            )
            .unwrap();
            let control = finished_worker_after_first_replay_error(&fixture);
            // Keep the read failure active through the real status/reaper path. The status error is
            // expected, but dropping the finished worker must retain the original unknown request.
            assert!(fixture
                .call("copy.status", json!({"operation_id": WIRE}))
                .is_err());
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
                "reaping lost the unread request"
            );
            let admission = fixture.admission_error();
            assert_eq!(admission.code, ErrorCode::Busy);
            pending(admission);
            raw.execute_batch(
                "ALTER TABLE pending_fence_unavailable_head RENAME TO op_journal_head",
            )
            .unwrap();
            if foreign_commits_first {
                fixture.committed();
                let before = fixture.store().lock().unwrap().rows(OP).unwrap();
                for _ in 0..2 {
                    pending(fixture.revoke().unwrap_err());
                    let status = fixture.status();
                    assert_eq!(status["state"], "committed");
                    assert_eq!(status["items_verified"], 1);
                    assert_eq!(status["refusal"]["code"], "source_in_use");
                    assert!(control.strong_count() > 0);
                    assert!(control.upgrade().unwrap().has_pending_fence());
                    pending(fixture.admission_error());
                    assert_eq!(fixture.store().lock().unwrap().rows(OP).unwrap(), before);
                }
            } else {
                {
                    let retained = control.upgrade().unwrap();
                    assert!(exec::retry_pending_fence(fixture.store(), OP, &retained).unwrap());
                    assert!(!retained.has_pending_fence());
                }
                let committed = fixture.committed();
                assert!(committed.has_fence_cause(FenceCause::EmergencyStop));
                assert!(!committed.has_fence_cause(FenceCause::GrantRevoked));
                let before = fixture.store().lock().unwrap().rows(OP).unwrap();
                assert_eq!(fixture.status()["state"], "committed");
                assert_eq!(
                    control.strong_count(),
                    0,
                    "resolved request was not retired"
                );
                assert_eq!(fixture.admission_error().code, ErrorCode::NotFound);
                fixture.revoke().unwrap();
                assert_eq!(fixture.store().lock().unwrap().rows(OP).unwrap(), before);
            }
            assert!(!fixture._dir.path().join("source-must-not-exist").exists());
            assert!(!fixture
                ._dir
                .path()
                .join("destination-must-not-exist")
                .exists());
        }
    }
}

fn code_name<T: serde::Serialize>(code: T) -> Value {
    serde_json::to_value(code).unwrap_or(Value::Null)
}

/// Each manifest entry by its name below the selection's parent: its path below the source root and its
/// native file ID. The new folder's own source (a single selected folder) is included.
#[cfg(windows)]
fn source_entries(
    plan: &loomward_windows::effects::exec::Plan,
) -> HashMap<String, (PathBuf, [u8; 16])> {
    let m = &plan.manifest;
    let full = |name: &str| match &m.root {
        Some(top) => format!("{}/{name}", top.name),
        None => name.to_string(),
    };
    let mut out: HashMap<_, _> = m
        .items
        .iter()
        .zip(&plan.sources)
        .map(|(i, (rel, _))| (full(&i.name), (rel.clone(), i.identity.file_id)))
        .collect();
    if let (Some(top), Some(first), Some((rel, _))) =
        (&m.root, m.items.first(), plan.sources.first())
    {
        if let Some(rel) = rel.ancestors().nth(first.name.split('/').count()) {
            out.insert(top.name.clone(), (rel.to_path_buf(), top.identity.file_id));
        }
    }
    out
}

/// #526: the catalogue node of the entry at `names` (relative to the root folder `root_dir`), only
/// when every name matches exactly one live catalogue row and the last row carries `file_id`. A
/// refusal never names a guessed node: any other answer is `None`.
pub(crate) fn catalogue_node(
    conn: &rusqlite::Connection,
    root_dir: i64,
    names: &[&str],
    file_id: [u8; 16],
) -> Option<(NodeKey, i64)> {
    const DIRS: &str = "SELECT id,born_run,file_id FROM main.dir WHERE parent_id=?1 AND name=?2 AND listing_state!='absent_pending' LIMIT 2";
    const FILES: &str =
        "SELECT id,born_run,file_id FROM main.file WHERE dir_id=?1 AND name=?2 LIMIT 2";
    type Row = (i64, i64, Option<Vec<u8>>);
    let rows = |sql: &str, parent: i64, name: &str| -> Option<Vec<Row>> {
        let mut stmt = conn.prepare_cached(sql).ok()?;
        let rows = stmt
            .query_map(rusqlite::params![parent, name], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .ok()?;
        rows.collect::<rusqlite::Result<Vec<Row>>>().ok()
    };
    let (last, parents) = names.split_last()?;
    let mut dir = root_dir;
    for name in parents {
        match rows(DIRS, dir, name)?.as_slice() {
            [(id, _, _)] => dir = *id,
            _ => return None,
        }
    }
    let dirs = rows(DIRS, dir, last)?;
    let files = rows(FILES, dir, last)?;
    match (dirs.as_slice(), files.as_slice()) {
        ([(id, born, Some(fid))], []) if fid[..] == file_id => Some((NodeKey::Dir(*id), *born)),
        ([], [(id, born, Some(fid))]) if fid[..] == file_id => Some((NodeKey::File(*id), *born)),
        _ => None,
    }
}

fn not_open() -> ErrorBody {
    with_reason(
        fail(
            ErrorCode::CapabilityUnavailable,
            "copy-and-verify is not open: its effect gate (docs/47 lane C8) is closed",
        ),
        "copy_effect_gated",
    )
}

fn unknown() -> ErrorBody {
    fail(ErrorCode::NotFound, "unknown copy operation")
}

#[cfg(windows)]
fn fence_pending(code: ErrorCode, message: &str) -> ErrorBody {
    let mut error = with_reason(fail(code, message), "copy_fence_pending");
    error.retryable = true;
    error
}

impl Copies {
    /// Open the journal for a synthetic session and run recovery (docs/47 section 5): C3's reader
    /// decides, and only its decisions run, through the C2 primitives. Nothing resumes by itself.
    pub(crate) fn open(state_dir: &Path, dataset: DatasetClass) -> Copies {
        let removal = AtomicBool::new(LOOMWARD_COPY_STAGING_REMOVAL);
        let shutdown_wait_ms = AtomicU64::new(30_000);
        #[cfg(windows)]
        {
            let store = (dataset == DatasetClass::Synthetic)
                .then(|| {
                    loomward_operations::Store::open(
                        &loomward_catalog::sqlite_dir(state_dir).join("state.db"),
                    )
                    .ok()
                })
                .flatten()
                .map(|s| Arc::new(Mutex::new(s)));
            if let Some(store) = &store {
                use loomward_windows::effects::exec;
                let listed = store.lock().unwrap().operations().unwrap_or_default();
                for op in listed {
                    let control =
                        exec::Control::new(removal.load(std::sync::atomic::Ordering::SeqCst));
                    let _ = exec::recover(store, op, &control);
                }
            }
            Copies {
                store,
                pending: Mutex::default(),
                ops: Mutex::default(),
                running: Mutex::default(),
                pending_fences: Mutex::default(),
                removal,
                shutdown_wait_ms,
                #[cfg(test)]
                gate: Mutex::default(),
                #[cfg(test)]
                hook: Mutex::default(),
                #[cfg(test)]
                prepare_probe: Mutex::default(),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (state_dir, dataset);
            Copies {
                pending: Mutex::default(),
                ops: Mutex::default(),
                removal,
                shutdown_wait_ms,
            }
        }
    }
}

#[cfg(all(test, windows))]
impl Copies {
    fn at(&self, point: &str) {
        let gate = self.gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate(point);
        }
    }
}

/// Shutdown cancels every running worker (it stops at its next item or 1 MiB chunk, before publish)
/// and waits for each within a shared `shutdown_wait_ms` bound (#569). A worker stuck in a blocking
/// OS call is detached, still cancelled, and its journal is left to the recovery of the next open.
/// Store serialises appends from that worker and a reopened service against the current journal.
#[cfg(windows)]
impl Drop for Copies {
    fn drop(&mut self) {
        let runs: Vec<Run> = self
            .running
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
            .map(|(_, r)| r)
            .collect();
        for run in &runs {
            run.control.cancel();
        }
        let until = std::time::Instant::now()
            + std::time::Duration::from_millis(
                self.shutdown_wait_ms
                    .load(std::sync::atomic::Ordering::SeqCst),
            );
        for run in runs {
            // `Disconnected` is a worker that already ended; only `Timeout` leaves one behind.
            let wait = until.saturating_duration_since(std::time::Instant::now());
            if !matches!(
                run.done.recv_timeout(wait),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ) {
                let _ = run.thread.join();
            }
        }
    }
}

impl Copies {
    /// Drop previews whose time has run out (#569): a service that stays up would otherwise keep every
    /// preview that was never started.
    fn evict_expired(&self, now_ms: i64) {
        self.pending
            .lock()
            .unwrap()
            .retain(|_, p| now_ms < p.expires_ms);
    }

    /// Drop finished worker handles (#569), retaining any cause whose journal acknowledgement
    /// failed (#738). Status never retries writes or waits on the worker/journal here. Historical
    /// wire lookups use the durable plan; retained Controls confer no effect or cleanup authority.
    #[cfg(windows)]
    fn reap(&self) {
        let mut running = self.running.lock().unwrap();
        let mut pending = self.pending_fences.lock().unwrap();
        running.retain(|&op, run| {
            if !run.thread.is_finished() {
                return true;
            }
            if run.control.has_pending_fence() {
                pending.entry(op).or_insert_with(|| run.control.clone());
            }
            false
        });
        pending.retain(|_, control| control.has_pending_fence());
        self.ops
            .lock()
            .unwrap()
            .retain(|_, op| running.contains_key(op));
    }

    /// Called by the mutation-serialized fencer/canceller after taking the Run out of `running`.
    /// A reaper that removed it first transferred its pending Control before releasing `running`.
    #[cfg(windows)]
    fn fence_control(
        &self,
        op: u64,
        run: Option<&Run>,
        removal: bool,
    ) -> Arc<loomward_windows::effects::exec::Control> {
        self.pending_fences
            .lock()
            .unwrap()
            .get(&op)
            .cloned()
            .or_else(|| run.map(|run| run.control.clone()))
            .unwrap_or_else(|| Arc::new(loomward_windows::effects::exec::Control::new(removal)))
    }

    /// Every worker or temporary fencer retains unresolved requests, including first-read errors.
    /// The flag is inspected under this map lock so a worker clearing it cannot cause a stale
    /// cleanup decision to remove a later retained request. No Store/admission lock is taken here.
    #[cfg(windows)]
    fn retain_fence(&self, op: u64, control: &Arc<loomward_windows::effects::exec::Control>) {
        let mut pending = self.pending_fences.lock().unwrap();
        if control.has_pending_fence() {
            pending.entry(op).or_insert_with(|| control.clone());
        } else if pending
            .get(&op)
            .is_some_and(|old| Arc::ptr_eq(old, control))
        {
            pending.remove(&op);
        }
    }

    /// Persistent journal errors must not accumulate an unbounded queue behind new lab copies.
    /// Keep existing requests; retry releases the gate only when every request can be resolved.
    #[cfg(windows)]
    fn check_admission(&self) -> Result<(), ErrorBody> {
        let mut pending = self.pending_fences.lock().unwrap();
        pending.retain(|_, control| control.has_pending_fence());
        if pending.is_empty() {
            Ok(())
        } else {
            Err(fence_pending(
                ErrorCode::Busy,
                "an earlier copy fence is pending; retry its revocation or cancellation",
            ))
        }
    }
}

/// One resolved node: its root row, the granted root's verbatim path, and its path below it.
struct Resolved {
    root: db::RootRow,
    root_path: PathBuf,
    /// The grant's verified root identity: (volume serial, file ID).
    root_id: (u64, [u8; 16]),
    root_guid: Option<u128>,
    relative: PathBuf,
    is_dir: bool,
}

impl Inner {
    fn copy_node(
        &self,
        id: &str,
        reader: &mut loomward_catalog::Reader,
    ) -> Result<Resolved, ErrorBody> {
        let key = self.node(id, reader)?;
        let is_dir = match key {
            NodeKey::Dir(_) => true,
            NodeKey::File(_) => false,
            _ => {
                return Err(fail(
                    ErrorCode::InvalidRequest,
                    "copy selects files and folders only",
                ))
            }
        };
        let path = reader.path(key).map_err(catalog_error)?;
        if path.truncated {
            return Err(fail(ErrorCode::InvalidRequest, "the folder is too deep"));
        }
        let row: i64 = path
            .root_id
            .as_deref()
            .and_then(|r| r.strip_prefix("rt_"))
            .and_then(|r| r.parse().ok())
            .ok_or_else(|| fail(ErrorCode::NotFound, "unknown node"))?;
        let root = db::roots(&self.db(), Some(row))
            .map_err(crate::sql_error)?
            .into_iter()
            .next()
            .ok_or_else(|| fail(ErrorCode::NotFound, "unknown root"))?;
        if root.grant_state != "active" {
            return Err(fail(ErrorCode::PermissionDenied, "grant_not_active"));
        }
        // Synthetic sessions: lab roots only (the same gate the synthetic scan uses).
        self.session_root_gate(&root)
            .map_err(|e| self.root_admission_error(&root, e))?;
        let granted = self.engine.granted_root(
            RootId::new(self.root_id(row))
                .map_err(|_| fail(ErrorCode::NotFound, "unknown root"))?,
            root.grant,
            &loomward_engine::scan::CatalogRootIdentity {
                row,
                instance: self.instance.clone(),
            },
        )?;
        let root_id = match granted.expected().and_then(|e| e.id.zip(e.volume_serial)) {
            Some((loomward_windows::enumerate::FileIdObs::Id128(id), volume)) => (volume, id),
            _ => {
                return Err(with_reason(
                    fail(
                        ErrorCode::PermissionDenied,
                        "the root's identity is not verified",
                    ),
                    "identity_unverified",
                ))
            }
        };
        // Verbatim, so lab paths past 260 units stay reachable; bound to the grant's identity below.
        let root_path = std::fs::canonicalize(granted.path()).map_err(|_| {
            fail(
                ErrorCode::PermissionDenied,
                "the granted root is unavailable",
            )
        })?;
        Ok(Resolved {
            root,
            root_path,
            root_id,
            root_guid: granted.expected().and_then(|e| e.volume_guid),
            relative: path.ancestors.iter().skip(1).map(|p| &p.name).collect(),
            is_dir,
        })
    }

    /// Fresh native observations of the selection and destination (C4 inputs).
    #[cfg(windows)]
    fn copy_prepare(
        &self,
        sources: &[String],
        destination: &str,
    ) -> Result<
        (
            loomward_operations::Preview,
            loomward_windows::effects::exec::Plan,
        ),
        ErrorBody,
    > {
        use loomward_windows::effects::exec;
        let mut reader = self.catalog.reader().map_err(catalog_error)?;
        let mut resolved = Vec::with_capacity(sources.len());
        for s in sources {
            resolved.push(self.copy_node(s, &mut reader)?);
        }
        #[allow(unused_mut)]
        let mut dest = self.copy_node(destination, &mut reader)?;
        if !dest.is_dir {
            return Err(fail(
                ErrorCode::InvalidRequest,
                "the destination must be a folder",
            ));
        }
        #[cfg(test)]
        let probe = self.copies.prepare_probe.lock().unwrap().take();
        #[cfg(test)]
        if let Some(probe) = &probe {
            resolved[0].root_guid = Some(probe.source_guid);
            dest.root_guid = Some(probe.destination_guid);
        }
        let first = &resolved[0];
        if resolved.iter().any(|r| r.root.root != first.root.root) {
            return Err(fail(
                ErrorCode::InvalidRequest,
                "every source must come from one granted root",
            ));
        }
        let selected: Vec<PathBuf> = resolved.iter().map(|r| r.relative.clone()).collect();
        let dest_path = dest.root_path.join(&dest.relative);
        let state =
            std::fs::canonicalize(&self.state_dir).unwrap_or_else(|_| self.state_dir.clone());
        let date: String = serde_json::to_value(ts_ns(now_ns()))
            .ok()
            .and_then(|v| v.as_str().map(|s| s[..10].to_string()))
            .unwrap_or_default();
        let request = exec::Request {
            source_root: &first.root_path,
            selected: &selected,
            destination: &dest_path,
            destination_grant: dest.root.grant as u64,
            grants_active: true,
            synthetic_lab: self.dataset == DatasetClass::Synthetic,
            protected: dest_path.starts_with(&state),
            date: &date,
            // The pinned destination must sit under its own grant's verified root (review of #522).
            destination_root: dest.root_id,
            destination_root_guid: dest.root_guid,
        };
        #[cfg(test)]
        if let Some(probe) = probe {
            probe
                .received
                .lock()
                .unwrap()
                .push(request.destination_root_guid);
            return Err(fail(ErrorCode::PermissionDenied, "preparation intercepted"));
        }
        let (preview, mut plan) = exec::prepare(&request).map_err(|e| {
            with_reason(
                fail(
                    ErrorCode::PermissionDenied,
                    "the selection could not be observed",
                ),
                &e.to_string()
                    .chars()
                    .filter(|c| c.is_ascii_lowercase() || *c == '_')
                    .take(64)
                    .collect::<String>(),
            )
        })?;
        // The root opened for observation is the granted object, never a substitute at that path.
        if (plan.source_root_volume, plan.source_root_id) != first.root_id
            || plan.source_root_guid != first.root_guid
        {
            return Err(with_reason(
                fail(ErrorCode::PermissionDenied, "the granted root changed"),
                "root_identity_changed",
            ));
        }
        plan.operation_id.clear();
        plan.grants = vec![first.root.grant, dest.root.grant];
        Ok((preview, plan))
    }

    /// docs/47 section 5: called once a grant's revocation is durable. Every copy using the grant is
    /// fenced in the journal. A running one is stopped at its next effect step (at most the step in
    /// flight completes, waited for up to 30 s; a stuck one is answered like a pending fence and the
    /// run journals the fence once it returns, #672) and joined:
    /// before the rename it cancels itself, after it commits annotated
    /// `grant_revoked`. A run left waiting (blocked, interrupted, publishing) is
    /// then cancelled through the abandon path; `needs_attention` stays for the owner. Q1 applies.
    /// `PublishUnobserved` (a publish intent, and the destination could not settle which side of
    /// the rename the run is on) is answered like a pending fence: retryable `copy_fence_pending`.
    /// An operation whose journal tail every store refuses is skipped, not pending (#636 item 3): it
    /// can take no further effect, and no retry would change the answer.
    pub(crate) fn copies_fence(&self, grant: i64) -> Result<(), ErrorBody> {
        #[cfg(windows)]
        {
            use loomward_windows::effects::exec;
            let Some(store) = &self.copies.store else {
                return Ok(());
            };
            let mut pending = false;
            // Blocked/recovered operations may have no resident worker, but still need fencing.
            // A failed index read must not fence nothing: fall back to every journaled operation
            // (the plan filter below keeps only this grant's), and always include live workers.
            let indexed = store.lock().unwrap().operations_for_grant(grant);
            let mut ops = match indexed {
                Ok(ops) => ops,
                Err(_) => store.lock().unwrap().operations().unwrap_or_default(),
            };
            ops.extend(self.copies.running.lock().unwrap().keys().copied());
            ops.extend(self.copies.pending_fences.lock().unwrap().keys().copied());
            ops.sort_unstable();
            ops.dedup();
            for op in ops {
                let plan = store.lock().unwrap().plan(op).ok().flatten();
                let plan: Option<exec::Plan> = plan.and_then(|p| serde_json::from_str(&p).ok());
                let Some(plan) = plan.filter(|p| p.grants.contains(&grant)) else {
                    continue;
                };
                let removal = self
                    .copies
                    .removal
                    .load(std::sync::atomic::Ordering::SeqCst);
                let running = self.copies.running.lock().unwrap().remove(&op);
                let control = self.copies.fence_control(op, running.as_ref(), removal);
                let cause = loomward_operations::FenceCause::GrantRevoked;
                let stopped = match exec::fence(store, op, &control, cause) {
                    Ok(exec::Ack::StoppedBeforePublish | exec::Ack::Published) => true,
                    // Unreplayable: a refused tail, inert for every store; not a retryable fence.
                    Ok(exec::Ack::Settled | exec::Ack::Unreplayable) => false,
                    _ => {
                        pending = true;
                        false
                    }
                };
                self.copies.retain_fence(op, &control);
                if !stopped {
                    // Keep a live worker and, independently, the cause even when no worker exists.
                    if let Some(run) = running {
                        self.copies.running.lock().unwrap().insert(op, run);
                    }
                    continue;
                }
                if let Some(run) = running {
                    if let Err(run) = run.join() {
                        pending = true;
                        self.copies.running.lock().unwrap().insert(op, run);
                        continue;
                    }
                }
                match exec::status(store, op).map(|s| s.state) {
                    Ok(
                        loomward_operations::OpState::NeedsAttention
                        | loomward_operations::OpState::Cancelled
                        | loomward_operations::OpState::Committed,
                    ) => {}
                    Ok(_) => {
                        pending |=
                            exec::cancel(store, op, &plan, &exec::Control::new(removal)).is_err();
                    }
                    Err(_) => pending = true,
                }
            }
            self.copies.reap();
            if pending {
                return Err(fence_pending(
                    ErrorCode::InternalError,
                    "the grant ended, but a copy fence is pending; retry revocation",
                ));
            }
        }
        #[cfg(not(windows))]
        let _ = grant;
        Ok(())
    }

    fn copies_store(&self) -> Result<&Arc<Mutex<loomward_operations::Store>>, ErrorBody> {
        #[cfg(windows)]
        {
            self.copies.store.as_ref().ok_or_else(not_open)
        }
        #[cfg(not(windows))]
        {
            Err(not_open())
        }
    }

    /// #526: a refusal's `node_id` for the source entry at `relative` (below the plan's source root)
    /// with native `file_id`, or null when the catalogue does not hold it under that identity.
    #[cfg(windows)]
    fn refusal_node(
        &self,
        plan: &loomward_windows::effects::exec::Plan,
        relative: &Path,
        file_id: [u8; 16],
    ) -> Value {
        let conn = self.db();
        let root_dir = plan.grants.first().and_then(|&grant| {
            db::roots(&conn, None)
                .ok()?
                .into_iter()
                .find(|r| r.grant == grant && r.grant_state == "active")?
                .dir
        });
        let names: Option<Vec<&str>> = relative.iter().map(|c| c.to_str()).collect();
        root_dir
            .zip(names)
            .and_then(|(dir, names)| catalogue_node(&conn, dir, &names, file_id))
            .map_or(Value::Null, |(key, born)| {
                json!(self.ids.seal(key, born, &self.instance))
            })
    }

    pub(crate) fn copy_preview(&self, req: &RequestEnvelope) -> Result<Out, ErrorBody> {
        let request: CopyPreviewRequest = req.decode_payload()?;
        self.copies_store()?;
        #[cfg(windows)]
        {
            let sources: Vec<String> = request.sources.iter().map(|s| s.to_string()).collect();
            let destination = request.destination_node_id.to_string();
            let (preview, plan) = self.copy_prepare(&sources, &destination)?;
            self.copies.reap();
            self.copies.evict_expired(now_ns() / 1_000_000);
            let mut id = [0u8; 9];
            getrandom::fill(&mut id)
                .map_err(|_| fail(ErrorCode::InternalError, "no randomness"))?;
            let operation_id = format!("cp_{}", hex(&id));
            // Item refusals name a manifest entry by its name below the selection's parent, except the
            // two checks made on names inside the new folder (case collision, path length).
            let mut entries = None;
            let refusals: Vec<Value> = preview
                .refusals
                .iter()
                .take(256)
                .map(|r| {
                    use loomward_operations::Code;
                    let node = r.item.as_deref().and_then(|item| {
                        let key = match (&preview.manifest.root, r.code) {
                            (Some(top), Code::CaseCollision | Code::PathTooLong) => {
                                format!("{}/{item}", top.name)
                            }
                            _ => item.to_string(),
                        };
                        let entries = entries.get_or_insert_with(|| source_entries(&plan));
                        let (relative, file_id) = entries.get(&key)?;
                        Some(self.refusal_node(&plan, relative, *file_id))
                    });
                    json!({"code": code_name(r.code), "node_id": node.unwrap_or(Value::Null)})
                })
                .collect();
            let result = json!({
                "operation_id": operation_id,
                "state": if preview.refusals.is_empty() { "draft" } else { "refused" },
                "digest": digest_text(preview.manifest.digest()),
                "folder_name": preview.manifest.destination.folder_name,
                "files": preview.files,
                "folders": preview.folders,
                "bytes": preview.bytes.to_string(),
                "space_needed": preview.space_needed.to_string(),
                "space_reserve": preview.space_reserve.to_string(),
                "same_volume": preview.same_volume,
                "explicit_acl_items": preview.explicit_acl_items,
                "hard_link_names": preview.hard_link_names,
                "refusals": refusals,
                "refusal_count": preview.refusals.len(),
            });
            let now_ms = now_ns() / 1_000_000;
            self.copies.pending.lock().unwrap().insert(
                operation_id,
                Pending {
                    sources,
                    destination,
                    preview,
                    approval: None,
                    expires_ms: now_ms.saturating_add(PREVIEW_TTL_MS),
                },
            );
            Ok(Out {
                result,
                catalog_rev: None,
                state_rev: None,
            })
        }
        #[cfg(not(windows))]
        {
            let _ = request;
            Err(not_open())
        }
    }

    /// Synthetic sessions only (personal approvals over HTTP are refused before dispatch, G6).
    pub(crate) fn copy_approve(&self, req: &RequestEnvelope) -> Result<Out, ErrorBody> {
        let request: CopyConsentRequest = req.decode_payload()?;
        self.copies_store()?;
        let shown = parse_digest(request.digest.as_str())?;
        let mut pending = self.copies.pending.lock().unwrap();
        let p = pending
            .get_mut(request.operation_id.as_str())
            .ok_or_else(unknown)?;
        let now_ms = now_ns() / 1_000_000;
        if now_ms >= p.expires_ms {
            return Err(with_reason(
                fail(ErrorCode::PermissionDenied, "the copy preview has expired"),
                "preview_expired",
            ));
        }
        let approval =
            loomward_operations::approve(&p.preview, shown, 0, now_ms).map_err(|code| {
                with_reason(
                    fail(ErrorCode::PermissionDenied, "the copy cannot be approved"),
                    code_name(code).as_str().unwrap_or("refused"),
                )
            })?;
        let result = json!({
            "operation_id": request.operation_id,
            "digest": request.digest,
            "approved_at": ts_ns(approval.approved_at_ms() * 1_000_000),
            "expires_at": ts_ns(approval.expires_at_ms() * 1_000_000),
        });
        p.expires_ms = p.expires_ms.max(approval.expires_at_ms());
        p.approval = Some(approval);
        Ok(Out {
            result,
            catalog_rev: None,
            state_rev: None,
        })
    }

    pub(crate) fn copy_start(&self, req: &RequestEnvelope) -> Result<Out, ErrorBody> {
        let request: CopyConsentRequest = req.decode_payload()?;
        let store = self.copies_store()?.clone();
        #[cfg(windows)]
        {
            use loomward_windows::effects::exec;
            let id = request.operation_id.to_string();
            self.copies.reap();
            if self.copy_op(&id)?.is_some() {
                return Err(with_reason(
                    fail(
                        ErrorCode::InvalidRequest,
                        "this operation has started; abandon it to end it",
                    ),
                    "copy_already_started",
                ));
            }
            self.copies.check_admission()?;
            let shown = parse_digest(request.digest.as_str())?;
            // The approval is consumed here whatever follows: single use.
            let (sources, destination, approval) = {
                let mut pending = self.copies.pending.lock().unwrap();
                let p = pending.get_mut(&id).ok_or_else(unknown)?;
                let approval = p.approval.take().ok_or_else(|| {
                    with_reason(
                        fail(
                            ErrorCode::PermissionDenied,
                            "the copy has no unused approval",
                        ),
                        "approval_required",
                    )
                })?;
                (p.sources.clone(), p.destination.clone(), approval)
            };
            if approval.digest() != shown {
                return Err(with_reason(
                    fail(
                        ErrorCode::PermissionDenied,
                        "the digest is not the one approved",
                    ),
                    "digest_mismatch",
                ));
            }
            let (fresh, mut plan) = self.copy_prepare(&sources, &destination)?;
            #[cfg(test)]
            self.copies.at("prepared");
            // Review of #544: the grant check above ran before the tree walk, and a revocation
            // may have landed since. Revocation holds `mutation` from its commit through
            // `copies_fence`, so under it the grants are re-checked and the run is registered in
            // `ops` and `running` atomically: a revoke either refuses this start or fences this run.
            let _registering = self.mutation.lock().unwrap_or_else(|e| e.into_inner());
            if self.copy_op(&id)?.is_some() {
                return Err(with_reason(
                    fail(
                        ErrorCode::InvalidRequest,
                        "this operation has started; abandon it to end it",
                    ),
                    "copy_already_started",
                ));
            }
            // A fence may have failed while preparation was outside `mutation`. Preserve its
            // retry budget before admitting another journal/worker, just as for the grant recheck.
            self.copies.check_admission()?;
            for &grant in &plan.grants {
                let state: Option<String> = self
                    .db()
                    .query_row(
                        "SELECT state FROM st.root_grant WHERE id=?1",
                        [grant],
                        |r| r.get(0),
                    )
                    .ok();
                if state.as_deref() != Some("active") {
                    return Err(with_reason(
                        fail(ErrorCode::PermissionDenied, "the copy cannot start"),
                        "grant_revoked",
                    ));
                }
            }
            let admitted = approval
                .admit(&fresh, 0, now_ns() / 1_000_000)
                .map_err(|code| {
                    with_reason(
                        fail(ErrorCode::PermissionDenied, "the copy cannot start"),
                        code_name(code).as_str().unwrap_or("refused"),
                    )
                })?;
            plan.operation_id = id.clone();
            let op = {
                let mut s = store.lock().unwrap();
                let op = s
                    .start(0, admitted)
                    .map_err(|_| fail(ErrorCode::InternalError, "the journal refused the start"))?;
                s.set_plan(op, &serde_json::to_string(&plan).unwrap_or_default())
                    .map_err(|_| fail(ErrorCode::InternalError, "the journal refused the plan"))?;
                op
            };
            self.copies.pending.lock().unwrap().remove(&id);
            let control = exec::Control::new(
                self.copies
                    .removal
                    .load(std::sync::atomic::Ordering::SeqCst),
            );
            #[cfg(test)]
            let control = match self.copies.hook.lock().unwrap().clone() {
                Some(hook) => control.with_hook(move |p, c| hook(p, c)),
                None => control,
            };
            let control = Arc::new(control);
            let (worker_store, worker_control) = (store.clone(), control.clone());
            let (completed, done) = std::sync::mpsc::channel();
            let thread = std::thread::Builder::new()
                .name("lw-copy".into())
                .spawn(move || {
                    let _ = exec::run(&worker_store, op, &worker_control);
                    let _ = completed.send(());
                })
                .map_err(|_| {
                    // Nothing was created: settle the started operation as cancelled, never stuck.
                    let _ = exec::cancel(&store, op, &plan, &control);
                    fail(ErrorCode::ResourceBudget, "no worker")
                })?;
            #[cfg(test)]
            self.copies.at("spawned");
            {
                // Register both maps under the reaper's lock so it cannot evict a registering ID.
                let mut running = self.copies.running.lock().unwrap();
                running.insert(
                    op,
                    Run {
                        control,
                        thread,
                        done,
                    },
                );
                self.copies.ops.lock().unwrap().insert(id.clone(), op);
            }
            self.copy_status_of(&id, op)
        }
        #[cfg(not(windows))]
        {
            let _ = (request, store);
            Err(not_open())
        }
    }

    fn copy_op(&self, id: &str) -> Result<Option<u64>, ErrorBody> {
        if let Some(op) = self.copies.ops.lock().unwrap().get(id).copied() {
            return Ok(Some(op));
        }
        #[cfg(windows)]
        {
            self.copies_store()?
                .lock()
                .unwrap()
                .operation_by_wire_id(id)
                .map_err(crate::sql_error)
        }
        #[cfg(not(windows))]
        Ok(None)
    }

    /// Cancel (before publish) and abandon (blocked, needs_attention, interrupted) share one path: stop
    /// the run, journal `cancelling`, then the Q1 policy decides what happens to the staging.
    pub(crate) fn copy_cancel(&self, req: &RequestEnvelope) -> Result<Out, ErrorBody> {
        let request: CopyOperationRequest = req.decode_payload()?;
        let store = self.copies_store()?.clone();
        let id = request.operation_id.to_string();
        let op = self.copy_op(&id)?.ok_or_else(unknown)?;
        #[cfg(windows)]
        {
            use loomward_windows::effects::exec;
            // Serialised with `copies_fence` (review of #544): one of them stops and joins the run,
            // the other finds it settled.
            let _m = self.mutation.lock().unwrap_or_else(|e| e.into_inner());
            let running = self.copies.running.lock().unwrap().remove(&op);
            let removal = self
                .copies
                .removal
                .load(std::sync::atomic::Ordering::SeqCst);
            let pending_control = self.copies.fence_control(op, running.as_ref(), removal);
            self.copies.retain_fence(op, &pending_control);
            if let Some(run) = running {
                run.control.cancel();
                if let Err(run) = run.join() {
                    self.copies.running.lock().unwrap().insert(op, run);
                    return Err(with_reason(
                        fail(
                            ErrorCode::Busy,
                            "copy cancellation is pending an in-flight step",
                        ),
                        "pending_in_flight_step",
                    ));
                }
            }
            // Joining consumes Run, but cannot consume an unresolved fence request.
            // This retry is metadata-only and returns immediately if a step is still admitted.
            let retry = exec::retry_pending_fence(&store, op, &pending_control);
            self.copies.retain_fence(op, &pending_control);
            match retry {
                Ok(true) => {}
                Ok(false) => {
                    return Err(fence_pending(
                        ErrorCode::Busy,
                        "copy cancellation is pending an earlier fence",
                    ));
                }
                Err(_) => {
                    return Err(fence_pending(
                        ErrorCode::InternalError,
                        "copy cancellation is pending an earlier fence; retry cancellation",
                    ));
                }
            }
            let plan: exec::Plan = store
                .lock()
                .unwrap()
                .plan(op)
                .ok()
                .flatten()
                .and_then(|p| serde_json::from_str(&p).ok())
                .ok_or_else(unknown)?;
            let control = exec::Control::new(removal);
            exec::cancel(&store, op, &plan, &control)
                .map_err(|_| fail(ErrorCode::InternalError, "the journal refused the cancel"))?;
        }
        #[cfg(not(windows))]
        let _ = store;
        self.copy_status_of(&id, op)
    }

    pub(crate) fn copy_status(&self, req: &RequestEnvelope) -> Result<Out, ErrorBody> {
        let request: CopyOperationRequest = req.decode_payload()?;
        self.copies_store()?;
        let id = request.operation_id.to_string();
        let op = self.copy_op(&id)?.ok_or_else(unknown)?;
        self.copy_status_of(&id, op)
    }

    fn copy_status_of(&self, id: &str, op: u64) -> Result<Out, ErrorBody> {
        #[cfg(windows)]
        {
            self.copies.reap();
            let store = self.copies_store()?;
            let live = self.copies.running.lock().unwrap().contains_key(&op);
            let status = if live {
                loomward_windows::effects::exec::status
            } else {
                loomward_windows::effects::exec::status_idle
            };
            let outcome = status(store, op)
                .map_err(|_| fail(ErrorCode::InternalError, "the journal does not replay"))?;
            let digest = store
                .lock()
                .unwrap()
                .replay(op)
                .ok()
                .and_then(|r| r.ok())
                .map(|r| r.digest)
                .unwrap_or_default();
            Ok(Out {
                result: json!({
                    "operation_id": id,
                    "state": code_name(outcome.state),
                    "digest": digest_text(digest),
                    "items_total": outcome.items_total,
                    "items_verified": outcome.items_verified,
                    "staging_folder": outcome.staging_left,
                    "leftover_staging_folders": outcome.leftover_staging.iter().take(64).collect::<Vec<_>>(),
                    "unjournaled_items": outcome.unjournaled_items.iter().take(64).collect::<Vec<_>>(),
                    "unjournaled_item_count": outcome.unjournaled_items.len(),
                    "refusal": outcome.refusal.map(|(code, item)| {
                        let node = item.and_then(|i| {
                            let plan: loomward_windows::effects::exec::Plan = store
                                .lock()
                                .unwrap()
                                .plan(op)
                                .ok()
                                .flatten()
                                .and_then(|p| serde_json::from_str(&p).ok())?;
                            let item = plan.manifest.items.get(i as usize)?;
                            let (relative, _) = plan.sources.get(i as usize)?;
                            Some(self.refusal_node(&plan, relative, item.identity.file_id))
                        });
                        json!({"code": code_name(code), "node_id": node.unwrap_or(Value::Null)})
                    }),
                }),
                catalog_rev: None,
                state_rev: None,
            })
        }
        #[cfg(not(windows))]
        {
            let _ = (id, op);
            Err(not_open())
        }
    }
}
