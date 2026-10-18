//! docs/47 lanes C4 and C5: personal sessions reach no copy effect; every copy command answers them
//! `capability_unavailable` (`copy_effect_gated`), and a personal approval over HTTP is refused by its
//! own rule (`copy_personal_approval_http`, G6). Synthetic sessions are served on registered lab roots
//! (C5). Session effect capabilities stay all false: the contract cannot say "synthetic only".
use super::*;
use std::path::PathBuf;

struct NoDialogs;
impl NativeDialogs for NoDialogs {
    fn pick_folder(&self) -> Option<PathBuf> {
        panic!("a copy command opened the folder picker")
    }
    fn confirm_disclosure(&self, _: &DisclosureSummary) -> Result<bool, ErrorBody> {
        panic!("a copy command opened the disclosure dialog")
    }
}

fn example(command: Command) -> Value {
    let path = format!(
        "{}/../../contracts/v3/examples/commands/{}.request.json",
        env!("CARGO_MANIFEST_DIR"),
        command.as_str()
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn personal_copy_is_gated_on_every_adapter_and_synthetic_is_served() {
    let copy: Vec<_> = Command::ALL
        .iter()
        .copied()
        .filter(|c| c.as_str().starts_with("copy."))
        .collect();
    assert_eq!(copy.len(), 6);
    for dataset in [DatasetClass::Synthetic, DatasetClass::Personal] {
        let tmp = temp_root();
        let svc = open(tmp.path(), dataset);
        let tauri = CallContext::tauri(Arc::new(NoDialogs));
        for ctx in [CallContext::http(), tauri] {
            let (hello, _) = ok(
                "session.hello",
                svc.call(request("session.hello", json!({})), &ctx),
            );
            let effects = hello["capabilities"]["effects"].as_object().unwrap();
            assert!(effects.values().all(|v| v == &json!(false)), "{effects:?}");
            for &c in &copy {
                let e = err(svc.call(request(c.as_str(), example(c)), &ctx));
                if dataset == DatasetClass::Synthetic {
                    // Served: the example node and operation IDs are simply unknown here.
                    if cfg!(windows) {
                        assert_ne!(reason(&e), Some("copy_effect_gated"), "{}", c.as_str());
                        assert_ne!(e.code, ErrorCode::CapabilityUnavailable, "{}", c.as_str());
                    }
                    continue;
                }
                assert_eq!(e.code, ErrorCode::CapabilityUnavailable, "{}", c.as_str());
                assert!(!e.retryable);
                let personal_http =
                    c == Command::CopyApprove && ctx.adapter == loomward_protocol::Adapter::Http;
                let want = if personal_http {
                    "copy_personal_approval_http"
                } else {
                    "copy_effect_gated"
                };
                assert_eq!(reason(&e), Some(want), "{} {dataset:?}", c.as_str());
            }
        }
    }
}

#[cfg(windows)]
mod lab_copy {
    use super::*;

    fn digest(v: &Payload) -> String {
        v["digest"].as_str().unwrap().to_owned()
    }

    fn settle_copy(svc: &Service, op: &str) -> Payload {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let (s, _) = ok(
                "copy.status",
                call(svc, "copy.status", json!({"operation_id": op})),
            );
            if matches!(
                s["state"].as_str().unwrap(),
                "committed" | "cancelled" | "needs_attention" | "blocked" | "refused"
            ) {
                return s;
            }
            assert!(Instant::now() < deadline, "{s:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn reap_copy(svc: &Service, op: &str) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !svc.inner.copies.running.lock().unwrap().is_empty() {
            assert!(
                Instant::now() < deadline,
                "the settled run was never dropped"
            );
            std::thread::sleep(Duration::from_millis(10));
            settle_copy(svc, op);
        }
        settle_copy(svc, op);
        assert!(
            svc.inner.copies.ops.lock().unwrap().is_empty(),
            "settled operation IDs must not accumulate in memory"
        );
    }

    fn count_plan_reads(svc: &Service, path: &Path) -> Arc<std::sync::atomic::AtomicUsize> {
        use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
        let conn = rusqlite::Connection::open(path).unwrap();
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = reads.clone();
        conn.authorizer(Some(move |ctx: AuthContext<'_>| {
            if matches!(
                ctx.action,
                AuthAction::Read {
                    table_name: "op_plan",
                    column_name: "plan"
                }
            ) {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            Authorization::Allow
        }))
        .unwrap();
        *svc.inner.copies.store.as_ref().unwrap().lock().unwrap() =
            loomward_operations::Store::new(conn).unwrap();
        reads
    }

    /// Scan the lab root and return the lab with the nodes of its folders `a` and `b`.
    fn scanned() -> Option<(Lab, String, String)> {
        let lab = lab()?;
        let job = start(&lab.svc, &lab.root, "full");
        assert_eq!(settle(&lab.svc, &job)["state"], "completed");
        // Freshness quiescence before any catalogue read: the per-root watcher may still be catching up
        // after the scan, and a read in that window fails with PartialCoverage under parallel load.
        f3_wait_live(&lab.svc);
        let slice = lab_slice(&lab.svc, &lab.root);
        let a = child_named(&slice, "a").unwrap();
        let b = child_named(&slice, "b").unwrap();
        // A checkout inside a cloud-synced folder (OneDrive) puts the lab there, and the copy rightly
        // refuses a cloud-synced destination: skip with the reason rather than fail.
        let probe = preview(&lab.svc, &a, &b);
        if probe["refusals"]
            .as_array()
            .is_some_and(|r| r.iter().any(|x| x["code"] == "destination_cloud_synced"))
        {
            eprintln!("skipped: this checkout's lab destination is cloud-synced (destination_cloud_synced)");
            return None;
        }
        Some((lab, a, b))
    }

    fn preview(svc: &Service, a: &str, b: &str) -> Payload {
        ok(
            "copy.preview",
            call(
                svc,
                "copy.preview",
                json!({"sources": [a], "destination_node_id": b}),
            ),
        )
        .0
    }

    /// Relative names and bytes under `p`. The C1 verifier's agreement is proven in the windows lab
    /// suite; the content reader stays engine-only here (`content_reader_is_feature_gated_and_engine_only`).
    fn contents(p: &Path) -> std::collections::BTreeMap<PathBuf, Option<Vec<u8>>> {
        let mut out = std::collections::BTreeMap::new();
        let mut pending = vec![p.to_path_buf()];
        while let Some(dir) = pending.pop() {
            for e in std::fs::read_dir(&dir).unwrap() {
                let path = e.unwrap().path();
                let key = path.strip_prefix(p).unwrap().to_path_buf();
                if path.is_dir() {
                    pending.push(path);
                    out.insert(key, None);
                } else {
                    out.insert(key, Some(std::fs::read(&path).unwrap()));
                }
            }
        }
        out
    }

    #[test]
    fn synthetic_lab_copy_previews_approves_over_http_and_commits() {
        let Some((lab, a, b)) = scanned() else { return };
        let svc = &lab.svc;
        let p = preview(svc, &a, &b);
        assert_eq!(p["state"], "draft", "{p:?}");
        assert_eq!(p["folder_name"], "a");
        assert_eq!(
            (p["files"].as_u64(), p["folders"].as_u64()),
            (Some(4), Some(2))
        );
        let op = p["operation_id"].as_str().unwrap().to_owned();
        let consent = json!({"operation_id": op, "digest": digest(&p)});
        // Starting without an approval is refused and starts nothing.
        let e = err(call(svc, "copy.start", consent.clone()));
        assert_eq!(reason(&e), Some("approval_required"));
        ok("copy.approve", call(svc, "copy.approve", consent.clone()));
        ok("copy.start", call(svc, "copy.start", consent.clone()));
        let done = settle_copy(svc, &op);
        assert_eq!(done["state"], "committed", "{done:?}");
        assert_eq!(done["items_verified"], done["items_total"]);
        assert_eq!(done["staging_folder"], Value::Null);
        assert_eq!(done["leftover_staging_folders"], json!([]));
        // A started operation never starts again, approved or not.
        let e = err(call(svc, "copy.start", consent));
        assert_eq!(reason(&e), Some("copy_already_started"));
        assert_eq!(
            contents(&lab.path.join("a")),
            contents(&lab.path.join("b").join("a"))
        );
        // The journal and the wire ID survive a restart; recovery leaves a committed copy alone.
        let state = lab.path.parent().unwrap().join("state");
        let Lab {
            _tmp, path, svc, ..
        } = lab;
        drop(svc);
        let svc = Service::open(Config {
            state_dir: state,
            dataset: DatasetClass::Synthetic,
            allow_personal: false,
            allow_identity_unverified: false,
            grant_roots: vec![],
        })
        .unwrap();
        assert!(
            svc.inner.copies.ops.lock().unwrap().is_empty(),
            "reopening must not reload historical operation IDs into memory"
        );
        let (s, _) = ok(
            "copy.status",
            call(&svc, "copy.status", json!({"operation_id": op})),
        );
        assert_eq!(s["state"], "committed");
        assert!(path.join("b").join("a").join("deep").join("x.bin").exists());
    }

    #[test]
    fn restart_reports_unjournaled_copy_items_without_removing_them() {
        use crate::copy::{Flow, Point};
        use loomward_operations::Record;
        let Some((lab, a, b)) = scanned() else { return };
        let svc = &lab.svc;
        let p = preview(svc, &a, &b);
        let id = p["operation_id"].as_str().unwrap().to_owned();
        let consent = json!({"operation_id": id, "digest": digest(&p)});
        ok("copy.approve", call(svc, "copy.approve", consent.clone()));
        *svc.inner.copies.hook.lock().unwrap() = Some(Arc::new(|p, _| {
            if p == Point::StagingCreated {
                Flow::Crash
            } else {
                Flow::Continue
            }
        }));
        ok("copy.start", call(svc, "copy.start", consent));
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            ok(
                "copy.status",
                call(svc, "copy.status", json!({"operation_id": id})),
            );
            if svc.inner.copies.running.lock().unwrap().is_empty() {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let store = svc.inner.copies.store.as_ref().unwrap();
        let mut journal = store.lock().unwrap();
        let op = journal.operations().unwrap()[0];
        let r = journal.replay(op).unwrap().unwrap();
        journal
            .append(
                op,
                r.epoch,
                Record::CreateBatch {
                    items: vec![(0, "orphan".into())],
                },
            )
            .unwrap();
        let leftover = lab
            .path
            .join("b")
            .join(r.staging_tokens.last().unwrap())
            .join("orphan");
        std::fs::write(&leftover, b"").unwrap();
        drop(journal);
        let state = lab.path.parent().unwrap().join("state");
        let Lab {
            _tmp, path: _, svc, ..
        } = lab;
        drop(svc);
        let svc = Service::open(Config {
            state_dir: state,
            dataset: DatasetClass::Synthetic,
            allow_personal: false,
            allow_identity_unverified: false,
            grant_roots: vec![],
        })
        .unwrap();
        let (status, _) = ok(
            "copy.status",
            call(&svc, "copy.status", json!({"operation_id": id})),
        );
        assert_eq!(status["state"], "needs_attention", "{status:?}");
        assert_eq!(status["unjournaled_items"], json!(["orphan"]));
        assert_eq!(status["unjournaled_item_count"], 1);
        assert!(leftover.exists());
    }

    #[test]
    fn restart_status_reports_unreadable_staged_copy_attention_without_journaling() {
        restart_unreadable_copy(false);
    }

    #[test]
    fn restart_status_reports_unreadable_published_copy_attention_without_journaling() {
        restart_unreadable_copy(true);
    }

    fn restart_unreadable_copy(published: bool) {
        use crate::copy::{Flow, Point};
        let Some((lab, a, b)) = scanned() else { return };
        let svc = &lab.svc;
        let p = preview(svc, &a, &b);
        let id = p["operation_id"].as_str().unwrap().to_owned();
        let consent = json!({"operation_id": id, "digest": digest(&p)});
        ok("copy.approve", call(svc, "copy.approve", consent.clone()));
        *svc.inner.copies.hook.lock().unwrap() = Some(Arc::new(move |p, _| {
            if p == if published {
                Point::AfterPublish
            } else {
                Point::Verified(0)
            } {
                Flow::Crash
            } else {
                Flow::Continue
            }
        }));
        ok("copy.start", call(svc, "copy.start", consent));
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            ok(
                "copy.status",
                call(svc, "copy.status", json!({"operation_id": id})),
            );
            if svc.inner.copies.running.lock().unwrap().is_empty() {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let store = svc.inner.copies.store.as_ref().unwrap();
        let journal = store.lock().unwrap();
        let op = journal.operations().unwrap()[0];
        let r = journal.replay(op).unwrap().unwrap();
        let before = journal.rows(op).unwrap();
        let blocked = if published {
            lab.path.join("b/a")
        } else {
            let item = r
                .items
                .iter()
                .find(|i| i.verified && i.name.as_deref().is_some_and(|n| n.ends_with(".bin")))
                .unwrap();
            lab.path
                .join("b")
                .join(r.staging_tokens.last().unwrap())
                .join(item.name.as_ref().unwrap())
        };
        drop(journal);
        let state = lab.path.parent().unwrap().join("state");
        let Lab { _tmp, svc, .. } = lab;
        drop(svc);
        struct DenyRead(PathBuf);
        impl Drop for DenyRead {
            fn drop(&mut self) {
                let _ = std::process::Command::new("icacls")
                    .arg(&self.0)
                    .args(["/remove:d", "*S-1-1-0"])
                    .output();
            }
        }
        assert!(std::process::Command::new("icacls")
            .arg(&blocked)
            .args(["/deny", "*S-1-1-0:(R)"])
            .output()
            .unwrap()
            .status
            .success());
        let denied = DenyRead(blocked.clone());
        assert!(loomward_windows::identity::observe(&blocked).is_err());
        let reopen = || {
            Service::open(Config {
                state_dir: state.clone(),
                dataset: DatasetClass::Synthetic,
                allow_personal: false,
                allow_identity_unverified: false,
                grant_roots: vec![],
            })
            .unwrap()
        };
        let svc = reopen();
        for _ in 0..2 {
            let (status, _) = ok(
                "copy.status",
                call(&svc, "copy.status", json!({"operation_id": id})),
            );
            assert_eq!(status["state"], "needs_attention", "{status:?}");
            assert_eq!(
                svc.inner
                    .copies
                    .store
                    .as_ref()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .rows(op)
                    .unwrap(),
                before
            );
        }
        drop(svc);
        drop(denied);
        let svc = reopen();
        let (status, _) = ok(
            "copy.status",
            call(&svc, "copy.status", json!({"operation_id": id})),
        );
        assert_eq!(
            status["state"],
            if published { "committed" } else { "copying" }
        );
    }

    /// #567: a lost acknowledgement at `committed` makes the executor return `Fault::Journal` for a copy
    /// that did commit (the journal is the truth, evidence/v3/copy-faults.md). The service reports the
    /// journal's state, never a failure.
    #[test]
    fn a_lost_acknowledgement_at_committed_reports_committed() {
        use loomward_operations::store::fault::{Kind, Stage};
        use loomward_operations::Record;
        let Some((lab, a, b)) = scanned() else { return };
        let svc = &lab.svc;
        let p = preview(svc, &a, &b);
        let op = p["operation_id"].as_str().unwrap().to_owned();
        let consent = json!({"operation_id": op, "digest": digest(&p)});
        ok("copy.approve", call(svc, "copy.approve", consent.clone()));
        let lost = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = lost.clone();
        svc.inner
            .copies
            .store
            .as_ref()
            .unwrap()
            .lock()
            .unwrap()
            .set_fault(Some(Box::new(move |record, stage| {
                let hit = matches!(record, Record::Committed) && stage == Stage::Ack;
                flag.fetch_or(hit, std::sync::atomic::Ordering::SeqCst);
                hit.then_some(Kind::Busy)
            })));
        ok("copy.start", call(svc, "copy.start", consent));
        let done = settle_copy(svc, &op);
        assert!(
            lost.load(std::sync::atomic::Ordering::SeqCst),
            "the acknowledgement was never lost: the test proves nothing"
        );
        assert_eq!(done["state"], "committed", "{done:?}");
        assert_eq!(done["items_verified"], done["items_total"]);
        assert_eq!(
            contents(&lab.path.join("a")),
            contents(&lab.path.join("b").join("a"))
        );
    }

    /// #569 item 1: a preview nobody approved is evicted once its TTL has passed (the next preview
    /// sweeps), so a long-lived service does not keep every abandoned preview.
    #[test]
    fn an_expired_preview_is_evicted() {
        let Some((lab, a, b)) = scanned() else { return };
        let svc = &lab.svc;
        let old = preview(svc, &a, &b);
        let (old_id, old_digest) = (
            old["operation_id"].as_str().unwrap().to_owned(),
            digest(&old),
        );
        let live = preview(svc, &a, &b);
        let live_id = live["operation_id"].as_str().unwrap().to_owned();
        svc.inner
            .copies
            .pending
            .lock()
            .unwrap()
            .get_mut(&old_id)
            .unwrap()
            .expires_ms = 0;
        let last = preview(svc, &a, &b)["operation_id"]
            .as_str()
            .unwrap()
            .to_owned();
        {
            let pending = svc.inner.copies.pending.lock().unwrap();
            assert!(!pending.contains_key(&old_id), "the expired preview stayed");
            assert!(pending.contains_key(&live_id) && pending.contains_key(&last));
        }
        let e = err(call(
            svc,
            "copy.approve",
            json!({"operation_id": old_id, "digest": old_digest}),
        ));
        assert_eq!(e.code, ErrorCode::NotFound, "{e:?}");
    }

    #[test]
    fn an_expired_copy_preview_cannot_be_approved_without_an_eviction_sweep() {
        let Some((lab, a, b)) = scanned() else { return };
        let svc = &lab.svc;
        let p = preview(svc, &a, &b);
        let op = p["operation_id"].as_str().unwrap();
        svc.inner
            .copies
            .pending
            .lock()
            .unwrap()
            .get_mut(op)
            .unwrap()
            .expires_ms = 0;
        let consent = json!({"operation_id": op, "digest": digest(&p)});
        let e = err(call(svc, "copy.approve", consent));
        assert_eq!(reason(&e), Some("preview_expired"));
        assert!(std::fs::read_dir(lab.path.join("b"))
            .unwrap()
            .next()
            .is_none());
    }

    #[test]
    fn an_expired_approved_copy_cannot_start_without_an_eviction_sweep() {
        let Some((lab, a, b)) = scanned() else { return };
        let svc = &lab.svc;
        let p = preview(svc, &a, &b);
        let op = p["operation_id"].as_str().unwrap();
        let consent = json!({"operation_id": op, "digest": digest(&p)});
        ok("copy.approve", call(svc, "copy.approve", consent.clone()));
        svc.inner
            .copies
            .pending
            .lock()
            .unwrap()
            .get_mut(op)
            .unwrap()
            .expire_approval();
        let e = err(call(svc, "copy.start", consent));
        assert_eq!(reason(&e), Some("approval_expired"));
        assert!(std::fs::read_dir(lab.path.join("b"))
            .unwrap()
            .next()
            .is_none());
    }

    /// #569 item 1: a settled run is dropped from `running` (its status is durable in the journal; a
    /// finished worker holds nothing), while `copy.status` keeps answering from the journal.
    #[test]
    fn a_settled_run_is_dropped_from_the_running_map() {
        let Some((lab, a, b)) = scanned() else { return };
        let svc = &lab.svc;
        let p = preview(svc, &a, &b);
        let op = p["operation_id"].as_str().unwrap().to_owned();
        let consent = json!({"operation_id": op, "digest": digest(&p)});
        ok("copy.approve", call(svc, "copy.approve", consent.clone()));
        ok("copy.start", call(svc, "copy.start", consent));
        assert_eq!(settle_copy(svc, &op)["state"], "committed");
        // The worker returns just after its last record; every status call reaps what has finished.
        // A status call reaps both the worker and its resident wire-ID entry. Historical status and
        // start-once still come from the durable plan after the entry has gone.
        reap_copy(svc, &op);
        let reads = count_plan_reads(
            svc,
            &loomward_catalog::sqlite_dir(&lab.path.parent().unwrap().join("state"))
                .join("state.db"),
        );
        assert_eq!(settle_copy(svc, &op)["state"], "committed");
        let e = err(call(
            svc,
            "copy.start",
            json!({"operation_id": op, "digest": digest(&p)}),
        ));
        assert_eq!(reason(&e), Some("copy_already_started"));
        assert_eq!(
            reads.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "historical status and start-once must use the wire-ID index, not deserialize plans"
        );
    }

    #[test]
    fn fencing_an_unrelated_grant_does_not_read_historical_plans() {
        let Some((lab, a, b)) = scanned() else { return };
        let svc = &lab.svc;
        let p = preview(svc, &a, &b);
        let op = p["operation_id"].as_str().unwrap().to_owned();
        let consent = json!({"operation_id": op, "digest": digest(&p)});
        ok("copy.approve", call(svc, "copy.approve", consent.clone()));
        ok("copy.start", call(svc, "copy.start", consent));
        assert_eq!(settle_copy(svc, &op)["state"], "committed");
        reap_copy(svc, &op);
        let reads = count_plan_reads(
            svc,
            &loomward_catalog::sqlite_dir(&lab.path.parent().unwrap().join("state"))
                .join("state.db"),
        );
        svc.inner.copies_fence(-1).unwrap();
        assert_eq!(
            reads.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "fencing must use the grant index, not deserialize unrelated history"
        );
        assert_eq!(settle_copy(svc, &op)["state"], "committed");
    }

    /// #569 item 2: shutdown cancels the workers and waits a bounded time; a worker stuck in a blocking
    /// call is detached (its journal left to recovery), never joined forever.
    #[test]
    fn dropping_the_service_does_not_wait_forever_for_a_stuck_worker() {
        use crate::copy::{Flow, Point};
        let Some((lab, a, b)) = scanned() else { return };
        let p = preview(&lab.svc, &a, &b);
        let op = p["operation_id"].as_str().unwrap().to_owned();
        let consent = json!({"operation_id": op, "digest": digest(&p)});
        ok(
            "copy.approve",
            call(&lab.svc, "copy.approve", consent.clone()),
        );
        let (reached, wait) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel::<()>();
        let (alive, gone) = std::sync::mpsc::channel::<()>();
        let (resume, alive) = (Mutex::new(resume), Mutex::new(alive));
        let fired = std::sync::atomic::AtomicBool::new(false);
        *lab.svc.inner.copies.hook.lock().unwrap() = Some(Arc::new(move |p, _| {
            // `alive` lives as long as the control, i.e. until the detached worker is done with it.
            let _ = &alive;
            if matches!(p, Point::Chunk(_))
                && !fired.swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                reached.send(()).unwrap();
                let _ = resume
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(120));
            }
            Flow::Continue
        }));
        lab.svc
            .inner
            .copies
            .shutdown_wait_ms
            .store(300, std::sync::atomic::Ordering::SeqCst);
        ok("copy.start", call(&lab.svc, "copy.start", consent));
        wait.recv_timeout(Duration::from_secs(60)).unwrap();
        let Lab { _tmp, svc, .. } = lab;
        let (dropped, finished) = std::sync::mpsc::channel();
        let closer = std::thread::spawn(move || {
            drop(svc);
            let _ = dropped.send(());
        });
        let bounded = finished.recv_timeout(Duration::from_secs(20));
        // Always release the worker before asserting, including against the unbounded implementation.
        release.send(()).unwrap();
        closer.join().unwrap();
        bounded.expect("dropping the service waited for the stuck worker");
        // The detached worker stops at its next chunk; wait for it before the temp folder goes.
        assert!(matches!(
            gone.recv_timeout(Duration::from_secs(60)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
        ));
    }

    #[test]
    fn copy_detached_source_open_cannot_write_after_reopened_service_cancels() {
        use crate::copy::{Flow, Point};
        let Some((lab, a, b)) = scanned() else { return };
        let p = preview(&lab.svc, &a, &b);
        let op = p["operation_id"].as_str().unwrap().to_owned();
        let consent = json!({"operation_id": op, "digest": digest(&p)});
        ok(
            "copy.approve",
            call(&lab.svc, "copy.approve", consent.clone()),
        );
        let (reached, wait) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel::<()>();
        let (alive, gone) = std::sync::mpsc::channel::<()>();
        let (resume, alive) = (Mutex::new(resume), Mutex::new(alive));
        let wrote = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writing = wrote.clone();
        *lab.svc.inner.copies.hook.lock().unwrap() = Some(Arc::new(move |p, _| {
            let _ = &alive;
            if matches!(p, Point::Chunk(_)) {
                writing.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            if matches!(p, Point::OpeningSource(_)) {
                reached.send(()).unwrap();
                let _ = resume
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(120));
            }
            Flow::Continue
        }));
        lab.svc
            .inner
            .copies
            .shutdown_wait_ms
            .store(0, std::sync::atomic::Ordering::SeqCst);
        ok("copy.start", call(&lab.svc, "copy.start", consent));
        wait.recv_timeout(Duration::from_secs(60)).unwrap();
        let state = lab.path.parent().unwrap().join("state");
        let Lab {
            _tmp, path, svc, ..
        } = lab;
        drop(svc);
        let svc = Service::open(Config {
            state_dir: state,
            dataset: DatasetClass::Synthetic,
            allow_personal: false,
            allow_identity_unverified: false,
            grant_roots: vec![],
        })
        .unwrap();
        ok(
            "copy.cancel",
            call(&svc, "copy.cancel", json!({"operation_id": op})),
        );
        let staging = std::fs::read_dir(path.join("b"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let before = contents(&staging);
        release.send(()).unwrap();
        assert!(matches!(
            gone.recv_timeout(Duration::from_secs(60)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
        ));
        assert!(
            !wrote.load(std::sync::atomic::Ordering::SeqCst),
            "detached worker entered writer after new service cancelled"
        );
        assert_eq!(
            contents(&staging),
            before,
            "detached worker wrote after new service cancelled"
        );
        assert_eq!(settle_copy(&svc, &op)["state"], "cancelled");
    }

    #[test]
    fn a_source_changed_after_approval_refuses_the_start() {
        let Some((lab, a, b)) = scanned() else { return };
        let svc = &lab.svc;
        let p = preview(svc, &a, &b);
        let consent = json!({"operation_id": p["operation_id"], "digest": digest(&p)});
        ok("copy.approve", call(svc, "copy.approve", consent.clone()));
        std::fs::write(lab.path.join("a").join("1.bin"), [1u8; 10]).unwrap();
        let e = err(call(svc, "copy.start", consent.clone()));
        assert_eq!(reason(&e), Some("source_changed"), "{e:?}");
        assert_eq!(std::fs::read_dir(lab.path.join("b")).unwrap().count(), 0);
        // The approval was consumed by that attempt (single use).
        let e = err(call(svc, "copy.start", consent));
        assert_eq!(reason(&e), Some("approval_required"));
    }

    #[test]
    fn a_wrong_digest_is_never_approved() {
        let Some((lab, a, b)) = scanned() else { return };
        let p = preview(&lab.svc, &a, &b);
        let e = err(call(
            &lab.svc,
            "copy.approve",
            json!({"operation_id": p["operation_id"], "digest": format!("sha256:{}", "0".repeat(64))}),
        ));
        assert_eq!(reason(&e), Some("digest_mismatch"));
    }

    /// Q1 at the service: abandoning a blocked operation stops and LEAVES the staging by default,
    /// reporting its name; with `loomward-copy-staging-removal` on, removes it by identity.
    #[test]
    fn abandon_applies_the_staging_removal_setting() {
        for removal in [false, true] {
            let Some((lab, a, b)) = scanned() else { return };
            let svc = &lab.svc;
            svc.inner
                .copies
                .removal
                .store(removal, std::sync::atomic::Ordering::SeqCst);
            let p = preview(svc, &a, &b);
            let op = p["operation_id"].as_str().unwrap().to_owned();
            let consent = json!({"operation_id": op, "digest": digest(&p)});
            ok("copy.approve", call(svc, "copy.approve", consent.clone()));
            // A writer on one source file stops the run `blocked` (source_in_use, resumable).
            let writer = std::fs::OpenOptions::new()
                .append(true)
                .open(lab.path.join("a").join("deep").join("x.bin"))
                .unwrap();
            ok("copy.start", call(svc, "copy.start", consent));
            let stopped = settle_copy(svc, &op);
            assert_eq!(stopped["state"], "blocked", "{stopped:?}");
            assert_eq!(stopped["refusal"]["code"], "source_in_use");
            reap_copy(svc, &op);
            drop(writer);
            let (gone, _) = ok(
                "copy.abandon",
                call(svc, "copy.abandon", json!({"operation_id": op})),
            );
            assert_eq!(gone["state"], "cancelled", "{gone:?}");
            let left: Vec<String> = std::fs::read_dir(lab.path.join("b"))
                .unwrap()
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect();
            if removal {
                assert!(left.is_empty(), "{left:?}");
                assert_eq!(gone["staging_folder"], Value::Null);
            } else {
                assert_eq!(left.len(), 1, "{left:?}");
                assert_eq!(gone["staging_folder"], json!(left[0]));
            }
        }
    }

    /// Releases a copy hook that waits for the fence once its test has already failed: the guard
    /// drops on a panic, so a failing test never leaves the worker waiting.
    struct GiveUp(Arc<std::sync::atomic::AtomicBool>);
    impl Drop for GiveUp {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn lab_grant(svc: &Service) -> i64 {
        svc.inner
            .db()
            .query_row("SELECT id FROM st.root_grant", [], |r| r.get(0))
            .unwrap()
    }

    /// Bytes in every file under `p` (0 when `p` holds only empty files and folders).
    fn bytes_under(p: &Path) -> u64 {
        contents(p).values().flatten().map(|b| b.len() as u64).sum()
    }

    /// Review of #544 (HIGH 1, scenario A): a revoke that lands while `copy.start` walks the tree,
    /// after its first grant check, refuses the start: nothing is journaled, created or published.
    #[test]
    fn a_revoke_during_the_start_walk_refuses_the_start() {
        let Some((lab, a, b)) = scanned() else { return };
        let svc = &lab.svc;
        let grant = lab_grant(svc);
        let p = preview(svc, &a, &b);
        let consent = json!({"operation_id": p["operation_id"], "digest": digest(&p)});
        ok("copy.approve", call(svc, "copy.approve", consent.clone()));
        let inner = Arc::downgrade(&svc.inner);
        *svc.inner.copies.gate.lock().unwrap() = Some(Arc::new(move |point: &str| {
            if point == "prepared" {
                inner.upgrade().unwrap().revoke(grant).unwrap();
            }
        }));
        let e = err(call(svc, "copy.start", consent));
        assert_eq!(reason(&e), Some("grant_revoked"), "{e:?}");
        assert_eq!(std::fs::read_dir(lab.path.join("b")).unwrap().count(), 0);
    }

    /// Review of #544 (HIGH 1, scenario B): a revoke between the run's registration in `ops` and in
    /// `running` (its worker already live) must fence the live worker, not a throwaway control. The
    /// worker waits after its first identity batch until fenced (10 s at most): unfenced, it would go
    /// on to write bytes after the revoke.
    #[test]
    fn a_revoke_while_the_run_registers_fences_the_live_worker() {
        let Some((lab, a, b)) = scanned() else { return };
        let svc = &lab.svc;
        let grant = lab_grant(svc);
        let p = preview(svc, &a, &b);
        let op = p["operation_id"].as_str().unwrap().to_owned();
        let consent = json!({"operation_id": op, "digest": digest(&p)});
        ok("copy.approve", call(svc, "copy.approve", consent.clone()));
        let (reached, wait) = std::sync::mpsc::channel::<()>();
        let (after, was_fenced) = std::sync::mpsc::channel::<bool>();
        let fired = std::sync::atomic::AtomicBool::new(false);
        // As in the identity test (#571): the hook waits for the fence itself, with no deadline sized for
        // an idle host. The test's own bounded receive below ends the wait, and a failing test drops
        // `_release`, which lets the hook go unfenced.
        let give_up = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let _release = GiveUp(give_up.clone());
        *svc.inner.copies.hook.lock().unwrap() = Some(Arc::new(move |p, c| {
            use crate::copy::{Flow, Point};
            if matches!(p, Point::Identities(_))
                && !fired.swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                reached.send(()).unwrap();
                while !c.fenced() && !give_up.load(std::sync::atomic::Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                after.send(c.fenced()).unwrap();
            }
            Flow::Continue
        }));
        let revoker: Arc<Mutex<Option<std::thread::JoinHandle<()>>>> = Default::default();
        let (inner, slot) = (Arc::downgrade(&svc.inner), revoker.clone());
        let wait = Mutex::new(wait);
        *svc.inner.copies.gate.lock().unwrap() = Some(Arc::new(move |point: &str| {
            if point != "spawned" {
                return;
            }
            wait.lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(60))
                .unwrap();
            let inner = inner.upgrade().unwrap();
            let (done, finished) = std::sync::mpsc::channel();
            *slot.lock().unwrap() = Some(std::thread::spawn(move || {
                inner.revoke(grant).unwrap();
                let _ = done.send(());
            }));
            // Unserialised, the revoke completes inside this window; serialised, it waits.
            let _ = finished.recv_timeout(Duration::from_secs(2));
        }));
        ok("copy.start", call(svc, "copy.start", consent));
        // The live worker itself must see the fence (a throwaway control leaves it running).
        assert!(
            was_fenced.recv_timeout(Duration::from_secs(30)).unwrap(),
            "the revoke fenced a control the live worker does not read"
        );
        // Serialised, the revoke joins the stopped worker before it returns.
        revoker.lock().unwrap().take().unwrap().join().unwrap();
        let s = settle_copy(svc, &op);
        assert_eq!(s["state"], "cancelled", "{s:?}");
        assert_eq!(s["refusal"]["code"], "grant_revoked", "{s:?}");
        assert_eq!(
            bytes_under(&lab.path.join("b")),
            0,
            "bytes written after the revoke"
        );
    }

    #[test]
    fn identity_change_fences_a_running_copy_before_and_after_publish() {
        use crate::copy::{Flow, Point};
        for point in [
            Point::StagingCreated,
            Point::Identities(0),
            Point::BeforePublish,
            Point::AfterPublish,
        ] {
            let Some((lab, a, b)) = scanned() else { return };
            let svc = &lab.svc;
            let grant = lab_grant(svc);
            let p = preview(svc, &a, &b);
            let op = p["operation_id"].as_str().unwrap().to_owned();
            let consent = json!({"operation_id": op, "digest": digest(&p)});
            ok("copy.approve", call(svc, "copy.approve", consent.clone()));
            let (reached, wait) = std::sync::mpsc::channel();
            let (after, was_fenced) = std::sync::mpsc::channel();
            // #571: the hook waits for the fence itself, with no deadline sized for an idle host; the
            // fence lands whenever `identity_changed` gets there, however loaded the host is. Only a
            // test that is already failing (the guard drops on a panic) lets the hook go unfenced.
            let give_up = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let _release = GiveUp(give_up.clone());
            *svc.inner.copies.hook.lock().unwrap() = Some(Arc::new(move |p, c| {
                if p == point {
                    reached.send(()).unwrap();
                    while !c.fenced() && !give_up.load(std::sync::atomic::Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    after.send(c.fenced()).unwrap();
                }
                Flow::Continue
            }));
            ok("copy.start", call(svc, "copy.start", consent));
            wait.recv_timeout(Duration::from_secs(60)).unwrap();
            let before = contents(&lab.path.join("b"));
            let root = RootId::new(lab.root.clone()).unwrap();
            svc.inner.identity_changed(grant, &root, false);
            let fenced = was_fenced.recv_timeout(Duration::from_secs(10)).unwrap();
            let s = settle_copy(svc, &op);
            assert!(
                fenced,
                "identity change did not fence the worker at {point:?}"
            );
            assert_eq!(
                s["state"],
                if point == Point::AfterPublish {
                    "committed"
                } else {
                    "cancelled"
                },
                "{point:?}: {s:?}"
            );
            assert_eq!(s["refusal"]["code"], "grant_revoked", "{s:?}");
            assert_eq!(
                contents(&lab.path.join("b")),
                before,
                "effects after identity change at {point:?}"
            );
        }
    }

    #[test]
    fn copy_cancel_times_out_a_hung_read_without_claiming_cancelled() {
        use crate::copy::{Flow, Point};
        let Some((lab, a, b)) = scanned() else { return };
        let svc = &lab.svc;
        let p = preview(svc, &a, &b);
        let op = p["operation_id"].as_str().unwrap().to_owned();
        let consent = json!({"operation_id": op, "digest": digest(&p)});
        ok("copy.approve", call(svc, "copy.approve", consent.clone()));
        let (reached, wait) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let resume = Mutex::new(resume);
        let fired = std::sync::atomic::AtomicBool::new(false);
        *svc.inner.copies.hook.lock().unwrap() = Some(Arc::new(move |p, _| {
            if matches!(p, Point::Chunk(_))
                && !fired.swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                reached.send(()).unwrap();
                resume
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(60))
                    .unwrap();
            }
            Flow::Continue
        }));
        ok("copy.start", call(svc, "copy.start", consent));
        wait.recv_timeout(Duration::from_secs(60)).unwrap();
        let (done, response) = std::sync::mpsc::channel();
        let (inner, operation) = (svc.inner.clone(), op.clone());
        let canceller = std::thread::spawn(move || {
            let _ = done.send(
                inner.copy_cancel(&request("copy.cancel", json!({"operation_id": operation}))),
            );
        });
        let bounded = response.recv_timeout(Duration::from_secs(35));
        // Always release the worker before asserting, including against the unbounded implementation.
        let mutation_free = svc.inner.mutation.try_lock().is_ok();
        let (pending, _) = ok(
            "copy.status",
            call(svc, "copy.status", json!({"operation_id": op})),
        );
        release.send(()).unwrap();
        canceller.join().unwrap();
        let e = match bounded.expect("copy.cancel waited for the hung read") {
            Err(e) => e,
            Ok(_) => panic!("copy.cancel acknowledged an unfinished worker"),
        };
        assert_eq!(e.code, ErrorCode::Busy);
        assert_eq!(reason(&e), Some("pending_in_flight_step"));
        assert!(mutation_free, "timed-out copy.cancel still holds mutation");
        assert_ne!(pending["state"], "cancelled");
        assert_eq!(settle_copy(svc, &op)["state"], "cancelled");
        assert!(!lab.path.join("b").join("a").exists());
        ok(
            "copy.cancel",
            call(svc, "copy.cancel", json!({"operation_id": op})),
        );
    }

    /// #526 revocation wiring: revoking the lab root a copy uses fences it in the journal and cancels a
    /// waiting run (docs/47 section 5: blocked -> cancelling), reported `grant_revoked`; the Q1 default
    /// leaves the staging. A running run stops at its next effect step (the windows lab suite).
    #[test]
    fn revoking_the_root_cancels_a_blocked_copy() {
        let Some((lab, a, b)) = scanned() else { return };
        let svc = &lab.svc;
        let p = preview(svc, &a, &b);
        let op = p["operation_id"].as_str().unwrap().to_owned();
        let consent = json!({"operation_id": op, "digest": digest(&p)});
        ok("copy.approve", call(svc, "copy.approve", consent.clone()));
        let writer = std::fs::OpenOptions::new()
            .append(true)
            .open(lab.path.join("a").join("deep").join("x.bin"))
            .unwrap();
        ok("copy.start", call(svc, "copy.start", consent));
        assert_eq!(settle_copy(svc, &op)["state"], "blocked");
        reap_copy(svc, &op);
        drop(writer);
        ok(
            "roots.revoke",
            call(
                svc,
                "roots.revoke",
                json!({"root_id": lab.root, "purge_catalog": false}),
            ),
        );
        let (s, _) = ok(
            "copy.status",
            call(svc, "copy.status", json!({"operation_id": op})),
        );
        assert_eq!(s["state"], "cancelled", "{s:?}");
        assert_eq!(s["refusal"]["code"], "grant_revoked", "{s:?}");
        let left: Vec<_> = std::fs::read_dir(lab.path.join("b")).unwrap().collect();
        assert_eq!(left.len(), 1, "staging left (Q1 default)");
        assert!(s["staging_folder"].as_str().is_some(), "{s:?}");
    }

    #[test]
    fn copy_fence_failure_is_reported_by_revoke_and_retried() {
        use loomward_operations::{
            store::fault::{Kind, Stage},
            Record,
        };
        let Some((lab, a, b)) = scanned() else { return };
        let svc = &lab.svc;
        let p = preview(svc, &a, &b);
        let op = p["operation_id"].as_str().unwrap().to_owned();
        let consent = json!({"operation_id": op, "digest": digest(&p)});
        ok("copy.approve", call(svc, "copy.approve", consent.clone()));
        let writer = std::fs::OpenOptions::new()
            .append(true)
            .open(lab.path.join("a/deep/x.bin"))
            .unwrap();
        ok("copy.start", call(svc, "copy.start", consent));
        assert_eq!(settle_copy(svc, &op)["state"], "blocked");
        drop(writer);
        let mut fail_once = true;
        svc.inner
            .copies
            .store
            .as_ref()
            .unwrap()
            .lock()
            .unwrap()
            .set_fault(Some(Box::new(move |record, stage| {
                if fail_once && matches!(record, Record::Fenced { .. }) && stage == Stage::Row {
                    fail_once = false;
                    Some(Kind::Full)
                } else {
                    None
                }
            })));
        let request = json!({"root_id": lab.root, "purge_catalog": false});
        let e = err(call(svc, "roots.revoke", request.clone()));
        assert_eq!(reason(&e), Some("copy_fence_pending"));
        ok("roots.revoke", call(svc, "roots.revoke", request));
        assert_eq!(settle_copy(svc, &op)["state"], "cancelled");
    }

    /// The ID of the child named `name` in a depth-3 slice, at any depth.
    fn node_named(slice: &Payload, name: &str) -> String {
        slice["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["name"] == name)
            .map(|n| n["node_id"].as_str().unwrap().to_owned())
            .unwrap()
    }

    /// Two granted lab roots in one store, both scanned: the first holds `a` and `b`, the second `c`.
    /// Returns the temp folder (kept alive), the service and the two root IDs.
    fn two_roots() -> Option<(tempfile::TempDir, Service, String, String)> {
        if loomward_windows::enumerate::running_elevated() {
            eprintln!("skipped: elevated token; the native scan refuses elevation (invariant 2)");
            return None;
        }
        let tmp = temp_root();
        let state = tmp.path().join("state");
        let seed = "sha256:0000000000000000000000000000000000000000000000000000000000000000";
        let mut roots = Vec::new();
        for name in ["lab-scan", "lab-other"] {
            let root = tmp.path().join(name);
            crate::lab::register(&state, &root, &format!("seed-{name}"), "S", seed).unwrap();
            roots.push(root.canonicalize().unwrap());
        }
        let (first, second) = (roots[0].clone(), roots[1].clone());
        std::fs::create_dir_all(first.join("a")).unwrap();
        std::fs::write(first.join("a").join("1.bin"), [0u8; 10]).unwrap();
        std::fs::create_dir(first.join("b")).unwrap();
        std::fs::create_dir(second.join("c")).unwrap();
        std::fs::write(second.join("c").join("1.bin"), [0u8; 7]).unwrap();
        let svc = Service::open(Config {
            state_dir: state,
            dataset: DatasetClass::Synthetic,
            allow_personal: false,
            allow_identity_unverified: false,
            grant_roots: roots,
        })
        .unwrap();
        let (listed, _) = ok("roots.list", call(&svc, "roots.list", json!({})));
        let id_of = |root: &Path| {
            let name = root.file_name().unwrap().to_str().unwrap();
            listed["roots"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["display_path"]["text"].as_str().unwrap().ends_with(name))
                .unwrap()["root_id"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let (one, two) = (id_of(&first), id_of(&second));
        for root in [&one, &two] {
            let job = start(&svc, root, "full");
            assert_eq!(settle(&svc, &job)["state"], "completed");
        }
        f3_wait_live(&svc);
        Some((tmp, svc, one, two))
    }

    /// copy_prepare (~374): a selection drawn from two granted roots is refused as one InvalidRequest
    /// before anything is observed. Each source on its own is served.
    #[test]
    fn sources_from_two_granted_roots_are_refused() {
        let Some((_tmp, svc, first, second)) = two_roots() else {
            return;
        };
        let a = child_named(&lab_slice(&svc, &first), "a").unwrap();
        let b = child_named(&lab_slice(&svc, &first), "b").unwrap();
        let c = child_named(&lab_slice(&svc, &second), "c").unwrap();
        let (alone, _) = ok(
            "copy.preview",
            call(
                &svc,
                "copy.preview",
                json!({"sources": [a], "destination_node_id": b}),
            ),
        );
        assert_eq!(alone["state"], "draft", "{alone:?}");
        let e = err(call(
            &svc,
            "copy.preview",
            json!({"sources": [a, c], "destination_node_id": b}),
        ));
        assert_eq!(e.code, ErrorCode::InvalidRequest, "{e:?}");
        assert_eq!(
            e.message.as_str(),
            "every source must come from one granted root"
        );
    }

    /// copy_start (~627): a start naming a digest other than the approved one is refused
    /// `digest_mismatch`. That attempt spends the approval (single use): nothing is journaled or written,
    /// and even the right digest no longer starts.
    #[test]
    fn a_start_naming_another_digest_than_the_approved_one_is_refused() {
        let Some((lab, a, b)) = scanned() else { return };
        let svc = &lab.svc;
        let p = preview(svc, &a, &b);
        let op = p["operation_id"].as_str().unwrap().to_owned();
        let other = format!("sha256:{}", "0".repeat(64));
        assert_ne!(other, digest(&p));
        ok(
            "copy.approve",
            call(
                svc,
                "copy.approve",
                json!({"operation_id": op, "digest": digest(&p)}),
            ),
        );
        let e = err(call(
            svc,
            "copy.start",
            json!({"operation_id": op, "digest": other}),
        ));
        assert_eq!(e.code, ErrorCode::PermissionDenied, "{e:?}");
        assert_eq!(reason(&e), Some("digest_mismatch"), "{e:?}");
        assert_eq!(std::fs::read_dir(lab.path.join("b")).unwrap().count(), 0);
        let e = err(call(
            svc,
            "copy.start",
            json!({"operation_id": op, "digest": digest(&p)}),
        ));
        assert_eq!(reason(&e), Some("approval_required"), "{e:?}");
    }

    #[test]
    fn copy_approval_retry_replays_without_restoring_consumed_approval() {
        let Some((lab, a, b)) = scanned() else { return };
        let svc = &lab.svc;
        let p = preview(svc, &a, &b);
        let op = p["operation_id"].as_str().unwrap();
        let consent = json!({"operation_id": op, "digest": digest(&p)});
        let approve = request("copy.approve", consent.clone());
        let first = ok(
            "copy.approve",
            svc.call(approve.clone(), &CallContext::http()),
        )
        .0;
        // A refused start still consumes approval, before it creates any staging.
        let refused = err(call(
            svc,
            "copy.start",
            json!({"operation_id": op, "digest": format!("sha256:{}", "0".repeat(64))}),
        ));
        assert_eq!(reason(&refused), Some("digest_mismatch"));
        let replay = ok("copy.approve", svc.call(approve, &CallContext::http())).0;
        assert_eq!(
            svc.inner
                .hooks
                .runs
                .lock()
                .unwrap()
                .get(&Command::CopyApprove),
            Some(&1),
            "a retry must not run approval again"
        );
        assert_eq!(
            replay, first,
            "replay preserves the original approval timestamps"
        );
        assert_eq!(
            reason(&err(call(svc, "copy.start", consent))),
            Some("approval_required")
        );
        assert_eq!(std::fs::read_dir(lab.path.join("b")).unwrap().count(), 0);
    }

    #[test]
    fn copy_approval_request_id_cannot_be_reused_for_another_operation() {
        let Some((lab, a, b)) = scanned() else { return };
        let svc = &lab.svc;
        let first = preview(svc, &a, &b);
        let second = preview(svc, &a, &b);
        assert_ne!(first["operation_id"], second["operation_id"]);
        let mut approve = request(
            "copy.approve",
            json!({"operation_id": first["operation_id"], "digest": digest(&first)}),
        );
        ok(
            "copy.approve",
            svc.call(approve.clone(), &CallContext::http()),
        );
        approve.payload = serde_json::from_value(
            json!({"operation_id": second["operation_id"], "digest": digest(&second)}),
        )
        .unwrap();
        let e = err(svc.call(approve, &CallContext::http()));
        assert_eq!(e.code, ErrorCode::InvalidRequest);
        assert_eq!(reason(&e), Some("idempotency_conflict"));
        assert_eq!(
            reason(&err(call(
                svc,
                "copy.start",
                json!({"operation_id": second["operation_id"], "digest": digest(&second)})
            ))),
            Some("approval_required")
        );
    }

    /// A digest that is not `sha256:` followed by exactly 64 lowercase hex digits is refused at
    /// copy_approve and copy_start as InvalidRequest, before any approval is looked at. The wire type
    /// `Digest` checks it on decode, so parse_digest's own `malformed digest` branch is not reachable
    /// from the wire. A malformed start spends no approval, so the right digest still starts and commits.
    #[test]
    fn a_malformed_digest_is_refused_and_spends_no_approval() {
        let Some((lab, a, b)) = scanned() else { return };
        let svc = &lab.svc;
        let p = preview(svc, &a, &b);
        let op = p["operation_id"].as_str().unwrap().to_owned();
        let malformed = [
            String::new(),
            "sha256:".to_owned(),
            format!("sha256:{}", "a".repeat(62)),
            format!("sha256:{}", "a".repeat(63)),
            format!("sha256:{}", "g".repeat(64)),
            "a".repeat(64),
            format!("md5:{}", "a".repeat(64)),
        ];
        for text in &malformed {
            for command in ["copy.approve", "copy.start"] {
                let e = err(call(
                    svc,
                    command,
                    json!({"operation_id": op, "digest": text}),
                ));
                assert_eq!(
                    e.code,
                    ErrorCode::InvalidRequest,
                    "{command} {text:?}: {e:?}"
                );
                assert!(
                    e.message.as_str().starts_with("expected sha256:"),
                    "{command} {text:?}: {e:?}"
                );
            }
        }
        ok(
            "copy.approve",
            call(
                svc,
                "copy.approve",
                json!({"operation_id": op, "digest": digest(&p)}),
            ),
        );
        for text in &malformed {
            let e = err(call(
                svc,
                "copy.start",
                json!({"operation_id": op, "digest": text}),
            ));
            assert_eq!(e.code, ErrorCode::InvalidRequest, "{text:?}");
        }
        ok(
            "copy.start",
            call(
                svc,
                "copy.start",
                json!({"operation_id": op, "digest": digest(&p)}),
            ),
        );
        assert_eq!(settle_copy(svc, &op)["state"], "committed");
    }

    /// copy_op (~723): `copy.status` and `copy.cancel` answer NotFound `unknown copy operation` for an
    /// operation ID this service never issued.
    #[test]
    fn an_unknown_copy_operation_id_is_not_found() {
        let Some((lab, _a, _b)) = scanned() else {
            return;
        };
        for command in ["copy.status", "copy.cancel"] {
            let e = err(call(
                &lab.svc,
                command,
                json!({"operation_id": "cp_000000000000000000"}),
            ));
            assert_eq!(e.code, ErrorCode::NotFound, "{command}: {e:?}");
            assert_eq!(e.message.as_str(), "unknown copy operation", "{command}");
        }
    }

    /// copy_preview (~516-537): a refused preview is reported `refused`, never `draft`, with its refusal
    /// count. An item refusal names the refused entry's catalogue node (#526); an operation-wide one is
    /// null. Here the destination sits inside the selected folder `a`, which is the refused entry.
    #[test]
    fn a_refused_preview_is_reported_refused_with_catalogue_node_ids() {
        let Some((lab, a, _b)) = scanned() else {
            return;
        };
        let svc = &lab.svc;
        let deep = node_named(&lab_slice(svc, &lab.root), "deep");
        let (p, _) = ok(
            "copy.preview",
            call(
                svc,
                "copy.preview",
                json!({"sources": [a], "destination_node_id": deep}),
            ),
        );
        assert_eq!(p["state"], "refused", "{p:?}");
        let refusals = p["refusals"].as_array().unwrap();
        assert!(!refusals.is_empty(), "{p:?}");
        assert_eq!(p["refusal_count"].as_u64(), Some(refusals.len() as u64));
        let inside = refusals
            .iter()
            .find(|r| r["code"] == "destination_inside_source")
            .unwrap_or_else(|| panic!("{p:?}"));
        assert_eq!(inside["node_id"], json!(a), "{p:?}");
    }

    /// #526: a runtime item refusal carries the refused entry's catalogue node. A writer holding `1.bin`
    /// makes the run `blocked` with `source_in_use` on that item.
    #[test]
    fn a_runtime_item_refusal_names_the_catalogue_node() {
        let Some((lab, a, b)) = scanned() else { return };
        let svc = &lab.svc;
        let one = node_named(&lab_slice(svc, &lab.root), "1.bin");
        let p = preview(svc, &a, &b);
        let op = p["operation_id"].as_str().unwrap().to_owned();
        let consent = json!({"operation_id": op, "digest": digest(&p)});
        ok("copy.approve", call(svc, "copy.approve", consent.clone()));
        let writer = std::fs::OpenOptions::new()
            .append(true)
            .open(lab.path.join("a").join("1.bin"))
            .unwrap();
        ok("copy.start", call(svc, "copy.start", consent));
        let done = settle_copy(svc, &op);
        drop(writer);
        assert_eq!(done["state"], "blocked", "{done:?}");
        assert_eq!(
            done["refusal"],
            json!({"code": "source_in_use", "node_id": one}),
            "{done:?}"
        );
    }

    /// #526: the catalogue lookup behind refusal node IDs never invents a node. A name the catalogue
    /// does not hold, or a held name whose native file ID differs, is `None`.
    #[test]
    fn a_refusal_node_is_null_unless_name_and_identity_match() {
        let Some((lab, _a, _b)) = scanned() else {
            return;
        };
        let inner = &lab.svc.inner;
        let row = inner.root_row(&lab.root).unwrap();
        let dir = crate::db::roots(&inner.db(), Some(row)).unwrap()[0]
            .dir
            .unwrap();
        let id = loomward_windows::identity::observe(&lab.path.join("a").join("1.bin"))
            .unwrap()
            .object
            .file_id;
        let at = |names: &[&str], file_id: [u8; 16]| {
            crate::copy::catalogue_node(&inner.db(), dir, names, file_id)
        };
        assert!(matches!(
            at(&["a", "1.bin"], id),
            Some((loomward_catalog::NodeKey::File(_), _))
        ));
        assert_eq!(at(&["a", "1.bin"], [0xEE; 16]), None);
        assert_eq!(at(&["a", "absent.bin"], id), None);
        assert_eq!(at(&["absent", "1.bin"], id), None);
    }

    /// copy_preview (~516): a preview with more refusals than the wire bound answers the first 256 and
    /// the full count, and is never `draft`. Each name has the device stem `NUL`, which
    /// `name_unrepresentable` refuses (Win32 refuses `?` in a name before the copy could see it).
    #[test]
    fn a_preview_with_more_than_256_refusals_is_capped_with_the_full_count() {
        const BAD: usize = 300;
        let Some(lab) = lab() else { return };
        for i in 0..BAD {
            std::fs::write(lab.path.join("a").join(format!("NUL.{i}")), [0u8; 1]).unwrap();
        }
        let job = start(&lab.svc, &lab.root, "full");
        assert_eq!(settle(&lab.svc, &job)["state"], "completed");
        f3_wait_live(&lab.svc);
        let slice = lab_slice(&lab.svc, &lab.root);
        let (a, b) = (
            child_named(&slice, "a").unwrap(),
            child_named(&slice, "b").unwrap(),
        );
        let (p, _) = ok(
            "copy.preview",
            call(
                &lab.svc,
                "copy.preview",
                json!({"sources": [a], "destination_node_id": b}),
            ),
        );
        assert_eq!(p["state"], "refused", "{p:?}");
        assert!(p["refusal_count"].as_u64().unwrap() >= BAD as u64, "{p:?}");
        let refusals = p["refusals"].as_array().unwrap();
        assert_eq!(refusals.len(), 256);
        // Each refused file is catalogued under its name and file ID, so each names its own node.
        assert!(refusals.iter().all(|r| r["code"] == "name_unrepresentable"));
        let nodes: std::collections::BTreeSet<_> = refusals
            .iter()
            .map(|r| r["node_id"].as_str().unwrap_or_else(|| panic!("{r:?}")))
            .collect();
        assert_eq!(nodes.len(), 256);
    }
}

/// Every command whose contract declares an effect is a mutation: a ticketed, retained, serialised
/// call, so opening the copy gate (docs/47 C5) can never route an effect onto the concurrent read path.
#[test]
fn every_effectful_command_is_a_mutation() {
    let path = format!(
        "{}/../../contracts/v3/commands.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut effectful = 0;
    for (name, row) in doc["commands"].as_object().unwrap() {
        if row["effects"] != "none" {
            effectful += 1;
            let c = Command::from_name(name).unwrap();
            assert!(
                crate::is_mutation(c),
                "{name} declares effects but is not a mutation"
            );
        }
    }
    assert_eq!(effectful, 4);
}
