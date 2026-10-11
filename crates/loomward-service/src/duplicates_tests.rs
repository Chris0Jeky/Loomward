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

#[test]
fn duplicates_personal_refusal_precedes_any_root_or_content_access_and_replays() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path(), DatasetClass::Personal);
    let req = request(
        "duplicates.inspect",
        duplicate_request("rt_nonexistent", 1024),
    );
    let refused = svc.call(req.clone(), &CallContext::http());
    let error = err(refused.clone());
    assert_eq!(error.code, ErrorCode::CapabilityUnavailable);
    assert_eq!(reason(&error), Some("content_read_gated"));
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
