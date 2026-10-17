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
        let (s, _) = ok(
            "copy.status",
            call(&svc, "copy.status", json!({"operation_id": op})),
        );
        assert_eq!(s["state"], "committed");
        assert!(path.join("b").join("a").join("deep").join("x.bin").exists());
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
        let deadline = Instant::now() + Duration::from_secs(60);
        while !svc.inner.copies.running.lock().unwrap().is_empty() {
            assert!(
                Instant::now() < deadline,
                "the settled run was never dropped"
            );
            std::thread::sleep(Duration::from_millis(10));
            settle_copy(svc, &op);
        }
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
