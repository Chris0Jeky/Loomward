use super::*;
use loomward_protocol::{DatasetClass, RecvOutcome};

#[test]
fn durable_commit_releases_records_and_preserves_cancel_order() {
    for fail_commit in [false, true] {
        let e = engine();
        let (entered, started) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let wait = Mutex::new(wait);
        let job = e
            .jobs
            .submit(
                JobSpec::new(JobKind::Refit, None),
                Arc::new(move |ctx, _| {
                    ctx.commit_if_not_cancelled(|| {
                        let registry = ctx.registry.upgrade().unwrap();
                        entered.send(registry.records.try_lock().is_ok()).unwrap();
                        wait.lock()
                            .unwrap()
                            .recv_timeout(Duration::from_secs(5))
                            .unwrap();
                        if fail_commit {
                            Err(EngineError::Internal {
                                message: "fixture commit failed".into(),
                                detail: None,
                            })
                        } else {
                            Ok(())
                        }
                    })?;
                    Ok(CoverageState::Complete)
                }),
                e.events.clone(),
                None,
            )
            .unwrap();
        let unlocked = started.recv_timeout(Duration::from_secs(5)).unwrap();
        let cancellation = if !fail_commit {
            let registry = e.jobs.clone();
            let events = e.events.clone();
            let id = job.job_id.clone();
            let (starting, ready) = std::sync::mpsc::channel();
            let (cancelled, outcome) = std::sync::mpsc::channel();
            let thread = std::thread::spawn(move || {
                starting.send(()).unwrap();
                cancelled.send(registry.cancel(&id, &events)).unwrap();
            });
            ready.recv_timeout(Duration::from_secs(5)).unwrap();
            let early = outcome.recv_timeout(Duration::from_millis(100));
            Some((thread, outcome, early))
        } else {
            None
        };
        release.send(()).unwrap();
        if let Some((thread, outcome, early)) = cancellation {
            let cancel = match early {
                Ok(result) => result,
                Err(_) => outcome.recv_timeout(Duration::from_secs(5)).unwrap(),
            }
            .unwrap();
            thread.join().unwrap();
            assert_ne!(cancel.state, JobState::CancelRequested);
        }
        let done = terminal(&e, &job.job_id);
        assert!(unlocked, "durable I/O held the records lock");
        assert_eq!(
            done.state,
            if fail_commit {
                JobState::Failed
            } else {
                JobState::Completed
            }
        );
        assert_eq!(
            e.jobs.records.lock().unwrap()[&job.job_id].committed,
            !fail_commit
        );
    }
}

#[test]
fn root_revocation_visits_jobs_after_missing_and_failed_ids() {
    for missing in [false, true] {
        let e = engine();
        let mut stream = e.subscribe_events(None).unwrap();
        let root = RootId::new("rt_fixture").unwrap();
        let mut ids = Vec::new();
        for _ in 0..2 {
            let job = e
                .jobs
                .submit(
                    JobSpec::new(JobKind::Duplicates, Some(root.clone())),
                    Arc::new(|ctx, _| {
                        ctx.set_duplicates(report(None));
                        Ok(CoverageState::Complete)
                    }),
                    e.events.clone(),
                    None,
                )
                .unwrap();
            terminal(&e, &job.job_id);
            ids.push(job.job_id);
        }
        {
            let mut records = e.jobs.records.lock().unwrap();
            if missing {
                records.remove(&ids[0]);
            } else {
                records.get_mut(&ids[0]).unwrap().job.rev =
                    Count::new(loomward_protocol::MAX_SAFE_INTEGER).unwrap();
            }
        }
        let result = e.jobs.revoke_jobs(ids.iter().cloned(), &e.events);
        assert_eq!(result.is_ok(), missing);
        assert!(e.job_status(&ids[1]).unwrap().duplicates.is_none());
        if !missing {
            assert!(e.job_status(&ids[0]).unwrap().duplicates.is_none());
        }
        if missing {
            assert_eq!(result.unwrap().len(), 1);
            let mut retained_jobs = 0;
            while let RecvOutcome::Event(event) = stream.recv_timeout(Duration::ZERO) {
                if event.event == EventName::JobState {
                    retained_jobs += 1;
                }
                assert!(event
                    .data
                    .get("job")
                    .and_then(|job| job.get("duplicates"))
                    .is_none_or(serde_json::Value::is_null));
            }
            assert!(retained_jobs >= 6);
        }
    }
}

#[test]
fn revocation_redacts_a_report_published_while_cancel_waits_for_the_gate() {
    // Codex P1 on #351: a worker that publishes its terminal report between the early
    // redaction and `revoked = true` must not leave that report in the replay ring.
    let e = engine();
    let mut stream = e.subscribe_events(None).unwrap();
    let (started, running) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel::<()>();
    let wait = Mutex::new(wait);
    let job = e
        .jobs
        .submit(
            JobSpec::new(
                JobKind::Duplicates,
                Some(RootId::new("rt_fixture").unwrap()),
            ),
            Arc::new(move |ctx, _| {
                ctx.set_duplicates(report(None));
                started.send(()).unwrap();
                wait.lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
                Ok(CoverageState::Complete)
            }),
            e.events.clone(),
            None,
        )
        .unwrap();
    running.recv_timeout(Duration::from_secs(5)).unwrap();
    let gate = e.jobs.records.lock().unwrap()[&job.job_id]
        .commit_gate
        .clone();
    let held = gate.lock().unwrap();
    let registry = e.jobs.clone();
    let events = e.events.clone();
    let id = job.job_id.clone();
    let cancel = std::thread::spawn(move || registry.cancel_revoked(&id, &events));
    // Let the revocation pass its early redaction and park on the held gate.
    std::thread::sleep(Duration::from_millis(100));
    release.send(()).unwrap();
    let done = terminal(&e, &job.job_id);
    assert_eq!(done.state, JobState::Completed);
    drop(held);
    cancel.join().unwrap().unwrap();
    let mut job_events = 0;
    while let RecvOutcome::Event(event) = stream.recv_timeout(Duration::ZERO) {
        if event.event == EventName::JobState {
            job_events += 1;
            assert!(
                event
                    .data
                    .get("job")
                    .and_then(|job| job.get("duplicates"))
                    .is_none_or(serde_json::Value::is_null),
                "a revoked root's report survived in the replay ring"
            );
        }
    }
    assert!(job_events >= 2);
    assert!(e.job_status(&job.job_id).unwrap().duplicates.is_none());
}

fn engine() -> Engine {
    Engine::open(crate::EngineConfig::new(
        "unused".into(),
        DatasetClass::Synthetic,
    ))
    .unwrap()
}
fn snapshot(stream: &mut Box<dyn loomward_protocol::EventStream>) -> Job {
    let mut last = None;
    while let RecvOutcome::Event(event) = stream.recv_timeout(Duration::ZERO) {
        if event.event == EventName::JobState {
            last = Some(serde_json::from_value(serde_json::json!(event.data["job"])).unwrap());
        }
    }
    last.expect("published job snapshot")
}
fn terminal(e: &Engine, id: &JobId) -> Job {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let job = e.job_status(id).unwrap();
        if job.finished_at.is_some() {
            return job;
        }
        assert!(Instant::now() < until, "worker did not finish");
        std::thread::yield_now();
    }
}
fn report(stop: Option<&str>) -> loomward_protocol::DuplicateReport {
    serde_json::from_value(serde_json::json!({
        "entries":[{"relative_path":"a","logical_bytes":"4"},{"relative_path":"b","logical_bytes":"4"}],
        "groups":[[0,1]],"links":[],"links_not_compared":0,"skipped":[],"bytes_read":"24",
        "partial":false,"truncated":false,"stop":stop
    })).unwrap()
}
fn list_bytes(list: &JobList) -> usize {
    let envelope = loomward_protocol::ResponseEnvelope::ok(
        loomward_protocol::RequestId::new("rq_fixture").unwrap(),
        list,
        loomward_protocol::ResponseMeta {
            served_at: timestamp(),
            elapsed_ms: Count::ZERO,
            dataset_class: DatasetClass::Synthetic,
            budget_hit: false,
            catalog_rev: None,
            state_rev: None,
        },
    );
    serde_json::to_vec(&envelope).unwrap().len()
}

#[test]
fn cancelled_before_start_has_no_start_time_or_unpublished_revision() {
    let e = engine();
    let mut stream = e.subscribe_events(None).unwrap();
    let (entered, started) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let wait = Mutex::new(wait);
    let runner: JobRunner = Arc::new(move |ctx, _| {
        assert!(ctx.is_cancelled());
        entered.send(()).unwrap();
        wait.lock().unwrap().recv().unwrap();
        Ok(CoverageState::Cancelled)
    });
    let job = e
        .jobs
        .submit_with_spawn(
            JobSpec::new(JobKind::Refit, None),
            runner,
            e.events.clone(),
            None,
            |_, worker| {
                let id = e
                    .jobs
                    .records
                    .lock()
                    .unwrap()
                    .keys()
                    .next()
                    .unwrap()
                    .clone();
                e.job_cancel(&id).unwrap();
                std::thread::spawn(worker);
                Ok(())
            },
        )
        .unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let current = e.job_status(&job.job_id).unwrap();
    let published = snapshot(&mut stream);
    // Release before assertions so a mutation failure never leaves a blocked worker.
    release.send(()).unwrap();
    assert_eq!(current.started_at, None);
    assert_eq!(current.rev.get(), 1);
    assert_eq!(current, published);
    let done = terminal(&e, &job.job_id);
    assert_eq!(done.state, JobState::Cancelled);
    assert_eq!(done.started_at, None);
    assert_eq!(done.rev.get(), 2);
}

#[test]
fn submit_spawn_failure_publishes_terminal_and_preserves_pending_cancel() {
    for cancel in [false, true] {
        let e = engine();
        let mut stream = e.subscribe_events(None).unwrap();
        let err = e
            .jobs
            .submit_with_spawn(
                JobSpec::new(JobKind::Refit, None),
                Arc::new(|_, _| panic!("failed spawn must not run")),
                e.events.clone(),
                None,
                |_, _| {
                    if cancel {
                        let id = e
                            .jobs
                            .records
                            .lock()
                            .unwrap()
                            .keys()
                            .next()
                            .unwrap()
                            .clone();
                        e.job_cancel(&id).unwrap();
                    }
                    Err(std::io::Error::other("synthetic spawn refusal"))
                },
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::Internal { .. }));
        let published = snapshot(&mut stream);
        let current = e.job_status(&published.job_id).unwrap();
        assert_eq!(current, published);
        assert_eq!(
            current.state,
            if cancel {
                JobState::Cancelled
            } else {
                JobState::Failed
            }
        );
        assert_eq!(current.started_at, None);
        assert!(current.finished_at.is_some());
        assert_eq!(current.error.is_some(), !cancel);
    }
}

#[test]
fn exhausted_revision_returns_internal_without_changing_or_poisoning_registry() {
    let e = engine();
    let (entered, started) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let wait = Mutex::new(wait);
    let job = e
        .jobs
        .submit(
            JobSpec::new(JobKind::Refit, None),
            Arc::new(move |_, _| {
                entered.send(()).unwrap();
                wait.lock().unwrap().recv().unwrap();
                Ok(CoverageState::Complete)
            }),
            e.events.clone(),
            None,
        )
        .unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    e.jobs
        .records
        .lock()
        .unwrap()
        .get_mut(&job.job_id)
        .unwrap()
        .job
        .rev = Count::new(loomward_protocol::MAX_SAFE_INTEGER).unwrap();
    let outcome = std::panic::catch_unwind(|| e.job_cancel(&job.job_id));
    // Restore a representable revision so the worker can finish even under mutation.
    let mut records = e
        .jobs
        .records
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let r = records.get_mut(&job.job_id).unwrap();
    let unchanged = r.job.state == JobState::Running && !r.cancel.load(Ordering::Acquire);
    r.job.rev = Count::new(1).unwrap();
    drop(records);
    release.send(()).unwrap();
    assert!(matches!(outcome, Ok(Err(EngineError::Internal { .. }))));
    assert!(unchanged);
    assert!(!e.jobs.records.is_poisoned());
    terminal(&e, &job.job_id);
}

#[test]
fn duplicate_revocation_is_silent_until_terminal_and_clears_retained_names() {
    for ordinary_cancel in [false, true] {
        let e = engine();
        let root = RootId::new("rt_fixture").unwrap();
        let mut stream = e.subscribe_events(None).unwrap();
        let (entered, started) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let wait = Mutex::new(wait);
        let job = e
            .jobs
            .submit(
                JobSpec::new(JobKind::Duplicates, Some(root.clone())),
                Arc::new(move |ctx, _| {
                    ctx.set_duplicates(report(None));
                    entered.send(ctx.clone()).unwrap();
                    wait.lock().unwrap().recv().unwrap();
                    ctx.set_duplicates(report(None)); // staging after revocation cannot restore names
                    Ok(CoverageState::Complete)
                }),
                e.events.clone(),
                None,
            )
            .unwrap();
        let ctx = started.recv_timeout(Duration::from_secs(5)).unwrap();
        if ordinary_cancel {
            e.job_cancel(&job.job_id).unwrap();
        }
        let first = e.jobs.cancel_revoked(&job.job_id, &e.events).unwrap();
        let published = snapshot(&mut stream);
        let second = e.jobs.cancel_revoked(&job.job_id, &e.events).unwrap();
        let silent = matches!(stream.recv_timeout(Duration::ZERO), RecvOutcome::Timeout);
        let no_staged = e.jobs.records.lock().unwrap()[&job.job_id]
            .duplicates
            .is_none();
        release.send(()).unwrap();
        assert_eq!(first, second);
        assert_eq!(first, published);
        assert!(first.error.is_none() && silent && no_staged);
        assert!(ctx.is_cancelled());
        let done = terminal(&e, &job.job_id);
        assert_eq!(done.state, JobState::Cancelled);
        assert_eq!(
            done.error.unwrap().code,
            loomward_protocol::ErrorCode::PermissionDenied
        );
        assert!(done.duplicates.is_none());
    }
    let e = engine();
    let root = RootId::new("rt_fixture").unwrap();
    let job = e
        .jobs
        .submit(
            JobSpec::new(JobKind::Duplicates, Some(root.clone())),
            Arc::new(|ctx, _| {
                ctx.set_duplicates(report(None));
                Ok(CoverageState::Complete)
            }),
            e.events.clone(),
            None,
        )
        .unwrap();
    let done = terminal(&e, &job.job_id);
    assert!(done.duplicates.is_some());
    let mut stream = e.subscribe_events(None).unwrap();
    while let RecvOutcome::Event(_) = stream.recv_timeout(Duration::ZERO) {}
    let key = crate::scan::RootKey::Fixture(root.clone());
    e.scan_cancel_root(&key, &root).unwrap();
    let revoked = e.job_status(&job.job_id).unwrap();
    assert!(revoked.duplicates.is_none());
    assert_eq!(revoked.state, done.state);
    assert_eq!(revoked.rev.get(), done.rev.get() + 1);
    assert_eq!(snapshot(&mut stream), revoked);
    e.scan_cancel_root(&key, &root).unwrap();
    assert_eq!(e.job_status(&job.job_id).unwrap(), revoked);
    assert!(matches!(
        stream.recv_timeout(Duration::ZERO),
        RecvOutcome::Timeout
    ));
}

#[test]
fn cancel_preserves_a_reader_stop_and_duplicates_busy_names_the_work() {
    for stop in [
        None,
        Some("byte_budget"),
        Some("time_budget"),
        Some("entry_budget"),
    ] {
        let e = engine();
        let (entered, started) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let wait = Mutex::new(wait);
        let job = e
            .jobs
            .submit(
                JobSpec::new(
                    JobKind::Duplicates,
                    Some(RootId::new("rt_fixture").unwrap()),
                ),
                Arc::new(move |ctx, _| {
                    ctx.set_duplicates(report(stop));
                    entered.send(()).unwrap();
                    wait.lock().unwrap().recv().unwrap();
                    Ok(CoverageState::Partial)
                }),
                e.events.clone(),
                None,
            )
            .unwrap();
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        let busy = e
            .jobs
            .check_conflict(&JobSpec::new(JobKind::Duplicates, job.root_id.clone()))
            .unwrap_err();
        e.job_cancel(&job.job_id).unwrap();
        release.send(()).unwrap();
        let done = terminal(&e, &job.job_id);
        assert_eq!(
            done.duplicates.unwrap().stop.unwrap().as_str(),
            stop.unwrap_or("cancelled")
        );
        assert_eq!(
            busy.to_body().message.as_str(),
            "root already inspecting duplicates"
        );
    }
}

#[test]
fn terminal_report_retention_caps_count_bytes_and_jobs_list_transport_size() {
    let e = engine();
    let mut large = report(None);
    large.entries = loomward_protocol::BoundedVec::new(
        (0..200)
            .map(|_| loomward_protocol::DuplicateEntry {
                relative_path: loomward_protocol::BoundedStr::new("\\\"\n".repeat(40)).unwrap(),
                logical_bytes: u64::MAX.into(),
            })
            .collect(),
    )
    .unwrap();
    let mut ids = Vec::new();
    for _ in 0..51 {
        let evidence = large.clone();
        let job = e
            .jobs
            .submit(
                JobSpec::new(JobKind::Duplicates, None),
                Arc::new(move |ctx, _| {
                    ctx.set_duplicates(evidence.clone());
                    Ok(CoverageState::Complete)
                }),
                e.events.clone(),
                None,
            )
            .unwrap();
        terminal(&e, &job.job_id);
        ids.push(job.job_id);
    }
    let list = e
        .job_list(&JobListRequest {
            kinds: None,
            limit: loomward_protocol::Int::new(50).unwrap(),
        })
        .unwrap();
    assert_eq!(list.jobs.len(), 50);
    assert_eq!(
        list.jobs.iter().filter(|j| j.duplicates.is_some()).count(),
        50
    );
    let oldest = e.job_status(&ids[0]).unwrap();
    assert!(
        oldest.duplicates.is_none(),
        "job ID remains recoverable without retained names"
    );
    // HTTP's client cap is 8 MiB. Tauri returns the same envelope without a response cap.
    let bytes = list_bytes(&list);
    eprintln!("50 retained reports: jobs.list envelope {bytes} bytes");
    assert!(bytes < 8 * 1024 * 1024, "jobs.list is {bytes} bytes");
    let oversized = loomward_protocol::BoundedStr::new("x".repeat(32768)).unwrap();
    large.entries = loomward_protocol::BoundedVec::new(
        (0..200)
            .map(|_| loomward_protocol::DuplicateEntry {
                relative_path: oversized.clone(),
                logical_bytes: 4.into(),
            })
            .collect(),
    )
    .unwrap();
    let job = e
        .jobs
        .submit(
            JobSpec::new(JobKind::Duplicates, None),
            Arc::new(move |ctx, _| {
                ctx.set_duplicates(large.clone());
                Ok(CoverageState::Complete)
            }),
            e.events.clone(),
            None,
        )
        .unwrap();
    terminal(&e, &job.job_id);
    let list = e
        .job_list(&JobListRequest {
            kinds: None,
            limit: loomward_protocol::Int::new(50).unwrap(),
        })
        .unwrap();
    let retained = list
        .jobs
        .iter()
        .filter_map(|j| j.duplicates.as_ref())
        .map(|r| serde_json::to_vec(r).unwrap().len())
        .sum::<usize>();
    assert!(retained <= 4 * 1024 * 1024, "retained {retained} bytes");
    assert!(list_bytes(&list) < 8 * 1024 * 1024);
}
