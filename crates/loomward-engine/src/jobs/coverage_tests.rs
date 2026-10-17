//! Coverage pins for the job registry: strategy mapping, exact final scan counters, runner panics,
//! revocation before a worker starts, and a cancelled duplicates runner that fails (#190).
use super::*;
use crate::scan::sink::Sums;
use crate::scan::source::Strategy as ScanStrategy;
use crate::scan::ScanReport;
use loomward_protocol::{EnumerationStrategy, ErrorCode, RecvOutcome};

fn engine() -> Engine {
    Engine::open(crate::EngineConfig::new(
        "unused".into(),
        loomward_protocol::DatasetClass::Synthetic,
    ))
    .unwrap()
}

fn counters(strategy: ScanStrategy, allocated: Option<u64>) -> ScanReport {
    ScanReport {
        totals: Sums {
            files: 3,
            dirs: 2,
            logical: 4096,
            allocated,
        },
        complete: true,
        strategy,
        limit_hit: true,
        examined: 7,
        enumeration_attempts: 9,
        duplicate_identity: 0,
        skipped: 2,
        failed: 1,
        elapsed_seconds: 0.5,
        enumeration_worker_seconds: 0.,
        enumeration_wall_seconds: 0.,
        staging_seconds: 0.,
        aggregation_seconds: 0.,
        persistence_seconds: 0.,
    }
}

fn duplicate_report(stop: Option<&str>) -> loomward_protocol::DuplicateReport {
    serde_json::from_value(serde_json::json!({
        "entries":[{"relative_path":"a","logical_bytes":"4"},{"relative_path":"b","logical_bytes":"4"}],
        "groups":[[0,1]],"links":[],"links_not_compared":0,"skipped":[],"bytes_read":"24",
        "partial":false,"truncated":false,"stop":stop
    }))
    .unwrap()
}

fn wait_terminal(e: &Engine, id: &JobId) -> Job {
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

/// The last published job snapshot; the terminal transition is always the last one.
fn last_job(stream: &mut Box<dyn loomward_protocol::EventStream>) -> Job {
    let mut last = None;
    while let RecvOutcome::Event(event) = stream.recv_timeout(Duration::ZERO) {
        if event.event == EventName::JobState {
            last = Some(serde_json::from_value(event.data["job"].clone()).unwrap());
        }
    }
    last.expect("published job snapshot")
}

#[test]
fn scan_progress_maps_extended_and_find_strategies_to_their_wire_values() {
    let cases = [
        (
            ScanStrategy::Extended,
            EnumerationStrategy::FileIdExtdDirectoryInfo,
            "file_id_extd_directory_info",
        ),
        (
            ScanStrategy::Find,
            EnumerationStrategy::FindFirstFileExLargeFetch,
            "find_first_file_ex_large_fetch",
        ),
        (
            ScanStrategy::Portable,
            EnumerationStrategy::StdReadDir,
            "std_read_dir",
        ),
    ];
    for (strategy, wire, text) in cases {
        let e = engine();
        e.register_job_runner(
            JobKind::Scan,
            Arc::new(move |ctx, _| {
                ctx.set_scan_report(&counters(strategy, Some(8192)))?;
                Ok(CoverageState::Complete)
            }),
        );
        let job = e
            .job_submit(JobSpec::new(
                JobKind::Scan,
                Some(RootId::new("rt_fixture").unwrap()),
            ))
            .unwrap();
        let done = wait_terminal(&e, &job.job_id);
        let progress = done.progress.expect("final progress");
        assert_eq!(progress.strategy, wire, "strategy {strategy:?}");
        assert_eq!(
            serde_json::to_value(progress.strategy).unwrap(),
            serde_json::json!(text)
        );
    }
}

#[test]
fn final_scan_counters_publish_the_exact_payload_and_unknown_allocation_stays_null() {
    for (allocated, expected_allocated) in [
        (Some(8192), serde_json::json!("8192")),
        (None, serde_json::Value::Null),
    ] {
        let e = engine();
        e.register_job_runner(
            JobKind::Scan,
            Arc::new(move |ctx, _| {
                ctx.set_scan_report(&counters(ScanStrategy::Extended, allocated))?;
                Ok(CoverageState::Complete)
            }),
        );
        let mut stream = e.subscribe_events(None).unwrap();
        let job = e
            .job_submit(JobSpec::new(
                JobKind::Scan,
                Some(RootId::new("rt_fixture").unwrap()),
            ))
            .unwrap();
        let done = wait_terminal(&e, &job.job_id);
        assert_eq!(done.state, JobState::Completed);
        let expected = serde_json::json!({
            "phase": "done",
            "strategy": "file_id_extd_directory_info",
            "examined": 7,
            "indexed_files": 3,
            "indexed_dirs": 2,
            "skipped": 2,
            "failed": 1,
            "logical_bytes": "4096",
            "allocated_bytes": expected_allocated,
            "pending_dirs": 0,
            "writer_queue_depth": 0,
            "entries_per_second": 14.0,
            "elapsed_ms": 500,
            "limit_hit": true
        });
        assert_eq!(
            serde_json::to_value(done.progress.expect("final progress")).unwrap(),
            expected
        );
        let mut progress_events = Vec::new();
        while let RecvOutcome::Event(event) = stream.recv_timeout(Duration::ZERO) {
            if event.event == EventName::ScanProgress {
                progress_events.push(event.data["progress"].clone());
            }
        }
        assert_eq!(progress_events, vec![expected]);
    }
}

#[test]
fn a_panicking_runner_fails_the_job_as_partial_with_a_runner_panic_error() {
    let e = engine();
    let runner: JobRunner = Arc::new(|_, _| panic!("synthetic runner panic"));
    e.register_job_runner(JobKind::Refit, runner);
    let mut stream = e.subscribe_events(None).unwrap();
    let job = e.job_submit(JobSpec::new(JobKind::Refit, None)).unwrap();
    let done = wait_terminal(&e, &job.job_id);
    assert_eq!(done.state, JobState::Failed);
    assert_eq!(done.coverage, Some(CoverageState::Partial));
    let error = done.error.clone().expect("panic recorded as the job error");
    assert_eq!(error.code, ErrorCode::InternalError);
    assert_eq!(error.message.as_str(), "job runner panicked");
    assert_eq!(last_job(&mut stream), done);
}

#[test]
fn revoking_a_duplicates_job_before_its_thread_starts_records_the_revocation() {
    let e = engine();
    let mut stream = e.subscribe_events(None).unwrap();
    let err = e
        .jobs
        .submit_with_spawn(
            JobSpec::new(
                JobKind::Duplicates,
                Some(RootId::new("rt_fixture").unwrap()),
            ),
            Arc::new(|_, _| panic!("failed spawn must not run")),
            e.events.clone(),
            None,
            |_, _| {
                let id = e
                    .jobs
                    .records
                    .lock()
                    .unwrap()
                    .keys()
                    .next()
                    .unwrap()
                    .clone();
                e.jobs.cancel_revoked(&id, &e.events).unwrap();
                Err(std::io::Error::other("synthetic spawn refusal"))
            },
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::Internal { .. }));
    let published = last_job(&mut stream);
    let current = e.job_status(&published.job_id).unwrap();
    assert_eq!(current, published);
    assert_eq!(current.state, JobState::Cancelled);
    assert_eq!(current.coverage, Some(CoverageState::Cancelled));
    assert_eq!(current.started_at, None);
    assert!(current.finished_at.is_some());
    assert!(current.duplicates.is_none());
    let error = current.error.expect("revocation recorded");
    assert_eq!(error.code, ErrorCode::PermissionDenied);
    assert_eq!(error.message.as_str(), "cancelled");
}

#[test]
fn a_cancelled_duplicates_job_whose_runner_errors_clears_retained_names_and_keeps_the_error() {
    let e = engine();
    let (entered, started) = std::sync::mpsc::channel();
    let runner: JobRunner = Arc::new(move |ctx, _| {
        ctx.set_duplicates(duplicate_report(None));
        entered.send(()).unwrap();
        while !ctx.is_cancelled() {
            std::thread::sleep(Duration::from_millis(1));
        }
        Err(EngineError::Internal {
            message: "synthetic duplicate failure".into(),
            detail: None,
        })
    });
    e.register_job_runner(JobKind::Duplicates, runner);
    let job = e
        .job_submit(JobSpec::new(
            JobKind::Duplicates,
            Some(RootId::new("rt_fixture").unwrap()),
        ))
        .unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    e.job_cancel(&job.job_id).unwrap();
    let done = wait_terminal(&e, &job.job_id);
    assert_eq!(done.state, JobState::Cancelled);
    assert_eq!(done.coverage, Some(CoverageState::Cancelled));
    assert!(
        done.duplicates.is_none(),
        "retained names survived a failed cancel"
    );
    let error = done.error.expect("runner error recorded");
    assert_eq!(error.code, ErrorCode::InternalError);
    assert_eq!(error.message.as_str(), "synthetic duplicate failure");
}

#[test]
#[ignore = "#190: strategy Both has no wire enumeration value, so its scan publishes no final progress"]
fn a_scan_whose_actual_strategy_is_both_publishes_final_progress() {
    let e = engine();
    e.register_job_runner(
        JobKind::Scan,
        Arc::new(|ctx, _| {
            ctx.set_scan_report(&counters(ScanStrategy::Both, Some(8192)))?;
            Ok(CoverageState::Complete)
        }),
    );
    let job = e
        .job_submit(JobSpec::new(
            JobKind::Scan,
            Some(RootId::new("rt_fixture").unwrap()),
        ))
        .unwrap();
    let done = wait_terminal(&e, &job.job_id);
    assert!(
        done.progress.is_some(),
        "final progress missing for strategy Both"
    );
}
