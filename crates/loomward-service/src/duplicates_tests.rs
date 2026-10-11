use super::*;

fn duplicate_request(root: &str, bytes: u64) -> Value {
    json!({"root_id":root,"budget":{"max_candidates":200,"max_bytes":bytes.to_string(),"max_ms":5000}})
}
#[cfg(windows)]
fn terminal(svc: &Service, id: &JobId) -> Job {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let (result, _) = ok("jobs.get", call(svc, "jobs.get", json!({"job_id":id})));
        let job = serde_json::from_value::<JobResult>(json!(result))
            .unwrap()
            .job;
        if matches!(
            job.state,
            JobState::Completed | JobState::Failed | JobState::Cancelled
        ) {
            return job;
        }
        assert!(Instant::now() < until, "job never ended");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// q-10: personal inspection is open, so an unknown root is simply `not_found`; the refusal
/// replays without a second execution and starts no job.
#[test]
fn duplicates_personal_unknown_root_is_refused_before_any_job_and_replays() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path(), DatasetClass::Personal);
    let req = request(
        "duplicates.inspect",
        duplicate_request("rt_nonexistent", 1024),
    );
    let refused = svc.call(req.clone(), &CallContext::http());
    let error = err(refused.clone());
    assert_eq!(error.code, ErrorCode::NotFound, "{error:?}");
    assert_eq!(svc.call(req, &CallContext::http()), refused);
    assert_eq!(
        svc.inner.hooks.runs.lock().unwrap()[&Command::DuplicatesInspect],
        1
    );
    assert!(svc
        .inner
        .engine
        .job_list(&serde_json::from_value(json!({"limit":50})).unwrap())
        .unwrap()
        .jobs
        .is_empty());
}

/// Review on #376: a folder revoked and then granted again leaves a revoked row beside the new
/// active one; inspection must follow the active grant instead of refusing forever.
#[cfg(windows)]
#[test]
fn duplicates_follow_a_fresh_grant_after_the_same_folder_was_revoked() {
    let tmp = crate::tests::temp_root();
    let root = tmp.path().join("regranted-folder");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"same").unwrap();
    std::fs::write(root.join("b.txt"), b"same").unwrap();
    let root = root.canonicalize().unwrap();
    let open = || {
        Service::open(Config {
            state_dir: tmp.path().join("state"),
            dataset: DatasetClass::Personal,
            allow_personal: true,
            allow_identity_unverified: false,
            grant_roots: vec![root.clone()],
        })
    };
    let first = open();
    if loomward_windows::enumerate::running_elevated() {
        let e = first.err().expect("an elevated grant is refused");
        assert!(e.contains("identity_unavailable"), "{e}");
        return;
    }
    let svc = first.unwrap();
    let (roots, _) = ok("roots.list", call(&svc, "roots.list", json!({})));
    let old = roots["roots"][0]["root_id"].as_str().unwrap().to_owned();
    ok(
        "roots.revoke",
        call(
            &svc,
            "roots.revoke",
            json!({"root_id": old, "purge_catalog": false}),
        ),
    );
    drop(svc);
    let svc = open().unwrap();
    let (roots, _) = ok("roots.list", call(&svc, "roots.list", json!({})));
    let active = roots["roots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["grant_state"] == "active")
        .expect("the fresh grant is active")["root_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (scan, _) = ok(
        "scan.start",
        call(&svc, "scan.start", json!({"root_id":active,"mode":"full"})),
    );
    let scan: JobResult = serde_json::from_value(json!(scan)).unwrap();
    assert_eq!(terminal(&svc, &scan.job.job_id).state, JobState::Completed);
    let (start, _) = ok(
        "duplicates.inspect",
        call(&svc, "duplicates.inspect", duplicate_request(&active, 1024)),
    );
    let start: JobResult = serde_json::from_value(json!(start)).unwrap();
    let job = terminal(&svc, &start.job.job_id);
    assert_eq!(job.state, JobState::Completed, "{:?}", job.error);
    assert_eq!(job.duplicates.unwrap().groups.len(), 1);
}

/// q-10 end to end: a personal session confirms duplicates in its own owner-granted folder
/// (synthetic files in a disposable root), reads no byte it does not need, changes nothing, and a
/// revoked root's report is redacted.
#[cfg(windows)]
#[test]
fn duplicates_personal_session_confirms_owner_duplicates_and_revocation_redacts() {
    let tmp = crate::tests::temp_root();
    let root = tmp.path().join("owner-folder");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    let files: [(&str, &[u8]); 4] = [
        ("a.txt", b"same"),
        (r"sub\copy.txt", b"same"),
        ("different.txt", b"else"),
        ("lonely.txt", b"unique size"),
    ];
    for (name, bytes) in files {
        std::fs::write(root.join(name), bytes).unwrap();
    }
    let root = root.canonicalize().unwrap();
    let snapshot = || {
        files
            .iter()
            .map(|(name, _)| {
                let f = root.join(name);
                (
                    std::fs::read(&f).unwrap(),
                    f.metadata().unwrap().modified().unwrap(),
                )
            })
            .collect::<Vec<_>>()
    };
    let before = snapshot();
    let opened = Service::open(Config {
        state_dir: tmp.path().join("state"),
        dataset: DatasetClass::Personal,
        allow_personal: true,
        allow_identity_unverified: false,
        grant_roots: vec![root.clone()],
    });
    if loomward_windows::enumerate::running_elevated() {
        let e = opened.err().expect("an elevated grant is refused");
        assert!(e.contains("identity_unavailable"), "{e}");
        return;
    }
    let svc = opened.unwrap();
    let (roots, _) = ok("roots.list", call(&svc, "roots.list", json!({})));
    assert_eq!(roots["roots"][0]["origin"], "owner_granted");
    let root_id = roots["roots"][0]["root_id"].as_str().unwrap().to_owned();
    let (scan, _) = ok(
        "scan.start",
        call(&svc, "scan.start", json!({"root_id":root_id,"mode":"full"})),
    );
    let scan: JobResult = serde_json::from_value(json!(scan)).unwrap();
    assert_eq!(terminal(&svc, &scan.job.job_id).state, JobState::Completed);
    let (start, _) = ok(
        "duplicates.inspect",
        call(
            &svc,
            "duplicates.inspect",
            duplicate_request(&root_id, 1024),
        ),
    );
    let start: JobResult = serde_json::from_value(json!(start)).unwrap();
    let job = terminal(&svc, &start.job.job_id);
    assert_eq!(job.state, JobState::Completed, "{:?}", job.error);
    let report = job.duplicates.unwrap();
    let mut equal = report.groups[0]
        .iter()
        .map(|i| report.entries[i.get() as usize].relative_path.as_str())
        .collect::<Vec<_>>();
    equal.sort();
    assert_eq!(report.groups.len(), 1);
    assert_eq!(equal, vec!["a.txt", r"sub\copy.txt"]);
    assert!(report.skipped.is_empty() && report.stop.is_none());
    // Three same-size names across the stages, never the lone size; well under the budget.
    assert!(report.bytes_read.get() > 0 && report.bytes_read.get() <= 3 * 4 * 3);
    assert_eq!(snapshot(), before, "inspection reads; it never writes");
    ok(
        "roots.revoke",
        call(
            &svc,
            "roots.revoke",
            json!({"root_id":root_id,"purge_catalog":false}),
        ),
    );
    let (revoked, _) = ok(
        "jobs.get",
        call(&svc, "jobs.get", json!({"job_id":start.job.job_id})),
    );
    assert_eq!(revoked["job"]["state"], "completed");
    assert!(
        revoked["job"].get("duplicates").is_none_or(Value::is_null),
        "a revoked root's report is redacted: {revoked:?}"
    );
    let refused = err(call(
        &svc,
        "duplicates.inspect",
        duplicate_request(&root_id, 1024),
    ));
    assert_eq!(refused.code, ErrorCode::PermissionDenied);
    assert_eq!(snapshot(), before);
}

#[cfg(windows)]
#[test]
fn duplicates_native_service_job_reads_synthetic_content_and_never_counts_links_as_duplicates() {
    if !native_identity_available() {
        return;
    }
    let tmp = tempfile::tempdir_in(std::fs::canonicalize(".").unwrap()).unwrap();
    let state = tmp.path().join("state");
    let root = tmp.path().join("fixture");
    crate::lab::register(
        &state,
        &root,
        "duplicates-fixture",
        "S",
        "sha256:0000000000000000000000000000000000000000000000000000000000000000",
    )
    .unwrap();
    let root = root.canonicalize().unwrap();
    for name in ["a.txt", "b.txt", "different.txt", "linked.txt"] {
        std::fs::write(
            root.join(name),
            if name == "different.txt" {
                b"else"
            } else {
                b"same"
            },
        )
        .unwrap();
    }
    std::fs::hard_link(root.join("linked.txt"), root.join("alias.txt")).unwrap();
    let svc = Service::open(Config {
        state_dir: state,
        dataset: DatasetClass::Synthetic,
        allow_personal: false,
        allow_identity_unverified: false,
        grant_roots: vec![root.clone()],
    })
    .unwrap();
    let (roots, _) = ok("roots.list", call(&svc, "roots.list", json!({})));
    let root_id = roots["roots"][0]["root_id"].as_str().unwrap();
    let (scan, _) = ok(
        "scan.start",
        call(&svc, "scan.start", json!({"root_id":root_id,"mode":"full"})),
    );
    let scan: JobResult = serde_json::from_value(json!(scan)).unwrap();
    assert_eq!(terminal(&svc, &scan.job.job_id).state, JobState::Completed);
    let mut events = svc.inner.engine.subscribe_events(None).unwrap();
    let RecvOutcome::Event(hello) = events.recv_timeout(Duration::ZERO) else {
        panic!("missing hello")
    };
    let last_event_id = format!("{}.{}", hello.epoch, hello.seq.get());
    let req = request("duplicates.inspect", duplicate_request(root_id, 1024));
    let submitted = svc.call(req.clone(), &CallContext::http());
    assert_eq!(svc.call(req, &CallContext::http()), submitted);
    let start: JobResult =
        serde_json::from_value(json!(ok("duplicates.inspect", submitted).0)).unwrap();
    assert_eq!(start.job.kind, JobKind::Duplicates);
    let job = terminal(&svc, &start.job.job_id);
    assert_eq!(job.state, JobState::Completed, "{:?}", job.error);
    assert!(job.seq > scan.job.seq && job.rev > start.job.rev);
    let report = job.duplicates.unwrap();
    let names = |sets: &BoundedVec<UniqueVec<Int<0, 199>, 2, 200>, 0, 100>| {
        sets.iter()
            .map(|set| {
                set.iter()
                    .map(|i| report.entries[i.get() as usize].relative_path.as_str())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(report.groups.len(), 1);
    let mut equal = names(&report.groups)[0].clone();
    equal.sort();
    assert_eq!(equal, vec!["a.txt", "b.txt"]);
    let mut linked = report.links[0]
        .iter()
        .map(|i| report.entries[i.get() as usize].relative_path.as_str())
        .collect::<Vec<_>>();
    linked.sort();
    assert_eq!(linked, vec!["alias.txt", "linked.txt"]);
    assert_eq!(report.links_not_compared.get(), 2);
    assert!(report.bytes_read.get() > 0 && report.partial);
    assert!(report.skipped.is_empty());
    assert_eq!(report.stop, None);
    // The content budget is the reader's shared meter, not a UI estimate.
    let (limited, _) = ok(
        "duplicates.inspect",
        call(&svc, "duplicates.inspect", duplicate_request(root_id, 0)),
    );
    let limited: JobResult = serde_json::from_value(json!(limited)).unwrap();
    let limited = terminal(&svc, &limited.job.job_id).duplicates.unwrap();
    assert!(limited.groups.is_empty() && limited.partial);
    assert_eq!(limited.bytes_read.get(), 0);
    assert_eq!(
        limited.stop.as_ref().map(|s| s.as_str()),
        Some("byte_budget")
    );
    assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"same");
    std::fs::write(root.join("a.txt"), b"diff").unwrap();
    let (changed, _) = ok(
        "duplicates.inspect",
        call(&svc, "duplicates.inspect", duplicate_request(root_id, 1024)),
    );
    let changed: JobResult = serde_json::from_value(json!(changed)).unwrap();
    let report = terminal(&svc, &changed.job.job_id).duplicates.unwrap();
    assert!(report.groups.is_empty() && report.partial);
    assert!(report
        .skipped
        .iter()
        .any(|skip| report.entries[skip.entry.get() as usize]
            .relative_path
            .as_str()
            == "a.txt"
            && skip.reason.as_str() == "changed_during_read"));
    // Cache a cancel before revocation; neither its retry nor resumed SSE may recover names.
    let cancel = request("duplicates.cancel", json!({"job_id":start.job.job_id}));
    let cancelled = svc.call(cancel.clone(), &CallContext::http());
    assert!(ok("duplicates.cancel", cancelled.clone()).0["job"]
        .get("duplicates")
        .is_none());
    ok(
        "roots.revoke",
        call(
            &svc,
            "roots.revoke",
            json!({"root_id":root_id,"purge_catalog":false}),
        ),
    );
    let generic = request("jobs.cancel", json!({"job_id":start.job.job_id}));
    assert!(
        ok("jobs.cancel", svc.call(generic, &CallContext::http())).0["job"]
            .get("duplicates")
            .is_none()
    );
    let replay = svc.call(cancel, &CallContext::http());
    assert_eq!(replay, cancelled);
    assert!(ok("duplicates.cancel", replay).0["job"]
        .get("duplicates")
        .is_none());
    let mut resumed = svc
        .inner
        .engine
        .subscribe_events(loomward_engine::events::EventResume::from_last_event_id(
            &last_event_id,
        ))
        .unwrap();
    let mut states = 0;
    while let RecvOutcome::Event(event) = resumed.recv_timeout(Duration::ZERO) {
        event.validate().unwrap();
        if event.event == EventName::JobState && event.data["job"]["kind"] == "duplicates" {
            states += 1;
            assert!(event.data["job"].get("duplicates").is_none(), "{event:?}");
        }
    }
    assert!(
        states > 0,
        "resume must exercise retained duplicate job states"
    );
}
