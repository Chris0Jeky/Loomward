//! Exercise the real job cancellation boundary while final scan publication is held.
use super::*;
use crate::{jobs::JobSpec, Engine, EngineConfig};
use loomward_protocol::{CoverageState, DatasetClass, Job, JobId, JobKind, JobState};

type FinishHook = Box<dyn Fn() -> EngineResult<()> + Send + Sync>;

#[derive(Default)]
struct PublicationSink {
    memory: super::super::MemorySink,
    finishes: Mutex<Vec<(bool, bool, bool)>>,
    finish: Option<FinishHook>,
}

impl ScanSink for PublicationSink {
    fn recover_interrupted(&self) -> EngineResult<()> {
        self.memory.recover_interrupted()
    }

    fn begin_run(&self, root: &RootId, run: u64, scope: &RunScope) -> EngineResult<()> {
        self.memory.begin_run(root, run, scope)
    }

    fn prepare_listing(
        &self,
        root: &RootId,
        run: u64,
        parent: Option<u64>,
        name: &[u16],
        identity: OpenedIdentity,
    ) -> EngineResult<ListingTicket> {
        self.memory
            .prepare_listing(root, run, parent, name, identity)
    }

    fn refresh_listing(
        &self,
        root: &RootId,
        run: u64,
        ticket: ListingTicket,
    ) -> EngineResult<ListingTicket> {
        self.memory.refresh_listing(root, run, ticket)
    }

    fn consume(&self, root: &RootId, message: ScanMessage) -> EngineResult<()> {
        self.memory.consume(root, message)
    }

    fn finish_run(
        &self,
        root: &RootId,
        run: u64,
        scope: &RunScope,
        complete: bool,
    ) -> EngineResult<()> {
        self.memory.finish_run(root, run, scope, complete)
    }

    fn finish_traversal(
        &self,
        root: &RootId,
        run: u64,
        scope: &RunScope,
        complete: bool,
        settled: bool,
        limited: bool,
    ) -> EngineResult<()> {
        self.finishes
            .lock()
            .unwrap()
            .push((complete, settled, limited));
        if let Some(finish) = &self.finish {
            finish()?;
        }
        self.memory.finish_run(root, run, scope, complete)
    }

    fn fence_root(&self, root: &RootId) -> EngineResult<()> {
        self.memory.fence_root(root)
    }
}

fn terminal(engine: &Engine, id: &JobId) -> Job {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let job = engine.job_status(id).unwrap();
        if matches!(
            job.state,
            JobState::Cancelled | JobState::Completed | JobState::Failed
        ) {
            return job;
        }
        assert!(Instant::now() < deadline, "job did not finish: {job:?}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn cancellation_after_the_flag_snapshot_prevents_scan_publication() {
    use super::super::tests::{root, Tree};

    for options in [
        ScanOptions {
            max_dirs: 1,
            ..ScanOptions::default()
        },
        ScanOptions {
            max_entries: 1,
            ..ScanOptions::default()
        },
        ScanOptions::default(),
    ] {
        let engine =
            Engine::open(EngineConfig::new("unused".into(), DatasetClass::Synthetic)).unwrap();
        let root = root();
        let root_id = root.root_id().clone();
        let sink = Arc::new(PublicationSink::default());
        let worker_sink = sink.clone();
        let (reached, at_publication) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let wait = Arc::new(Mutex::new(wait));
        engine.register_job_runner(
            JobKind::Scan,
            Arc::new(move |ctx, _| {
                let reached = reached.clone();
                let wait = wait.clone();
                BEFORE_PUBLISH.with_borrow_mut(|hook| {
                    *hook = Some(Box::new(move |point| {
                        if point == "run_scan_finish" {
                            reached.send(()).unwrap();
                            wait.lock()
                                .unwrap()
                                .recv_timeout(Duration::from_secs(5))
                                .unwrap();
                        }
                    }));
                });
                let result = run_job_scan(
                    &Tree,
                    &root,
                    1,
                    worker_sink.clone(),
                    Arc::new(ByteBudget::new(crate::budgets::SCAN_BYTES)),
                    &ctx,
                    options.clone(),
                );
                BEFORE_PUBLISH.with_borrow_mut(|hook| *hook = None);
                let report = result?;
                ctx.set_scan_report(&report)?;
                Ok(if report.coverage_complete() {
                    CoverageState::Complete
                } else {
                    CoverageState::Partial
                })
            }),
        );
        let job = engine
            .job_submit(JobSpec::new(JobKind::Scan, Some(root_id.clone())))
            .unwrap();
        at_publication.recv_timeout(Duration::from_secs(5)).unwrap();
        let cancelled = engine.scan_cancel(&job.job_id).unwrap();
        release.send(()).unwrap();
        let done = terminal(&engine, &job.job_id);
        assert_eq!(cancelled.state, JobState::CancelRequested);
        assert_eq!(done.state, JobState::Cancelled);
        assert_eq!(done.coverage, Some(CoverageState::Cancelled));
        assert_eq!(*sink.finishes.lock().unwrap(), vec![(false, false, false)]);
        assert!(!sink.memory.sweep_allowed(&root_id));
    }
}

#[test]
fn admitted_scan_publication_does_not_block_or_acknowledge_cancellation() {
    use super::super::tests::{root, Tree};

    // Admission owns the terminal outcome, not a promise that the durable write will succeed.
    for fail_finish in [false, true] {
        let engine = Arc::new(
            Engine::open(EngineConfig::new("unused".into(), DatasetClass::Synthetic)).unwrap(),
        );
        let root = root();
        let root_id = root.root_id().clone();
        let root_gate = engine.scan_root_gate(root.key());
        let (entered, publishing) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let wait = Mutex::new(wait);
        let sink = Arc::new(PublicationSink {
            finish: Some(Box::new(move || {
                entered.send(()).unwrap();
                wait.lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
                if fail_finish {
                    Err(EngineError::Internal {
                        message: "synthetic publication failure".into(),
                        detail: None,
                    })
                } else {
                    Ok(())
                }
            })),
            ..PublicationSink::default()
        });
        let worker_sink = sink.clone();
        engine.register_job_runner(
            JobKind::Scan,
            Arc::new(move |ctx, _| {
                let report = run_job_scan(
                    &Tree,
                    &root,
                    1,
                    worker_sink.clone(),
                    Arc::new(ByteBudget::new(crate::budgets::SCAN_BYTES)),
                    &ctx,
                    ScanOptions {
                        max_dirs: 1,
                        ..ScanOptions::default()
                    },
                )?;
                ctx.set_scan_report(&report)?;
                Ok(CoverageState::Partial)
            }),
        );
        let job = engine
            .job_submit(JobSpec::new(JobKind::Scan, Some(root_id.clone())))
            .unwrap();
        publishing.recv_timeout(Duration::from_secs(5)).unwrap();
        let root_unlocked = root_gate.try_lock().is_ok();
        let (cancelled, outcome) = mpsc::channel();
        let requester = engine.clone();
        let id = job.job_id.clone();
        let cancellation = std::thread::spawn(move || {
            cancelled.send(requester.scan_cancel(&id)).unwrap();
        });
        // This response needs the shared records lock and the job gate. Neither may be held
        // across sink I/O; release the sink even on a regression so the test leaves no worker.
        let early = outcome.recv_timeout(Duration::from_secs(2));
        release.send(()).unwrap();
        cancellation.join().unwrap();
        let done = terminal(&engine, &job.job_id);
        let response = early
            .expect("scan.cancel waited for final sink publication")
            .unwrap();
        assert!(root_unlocked, "publication held the root admission gate");
        assert_eq!(response.state, JobState::Running);
        assert_eq!(
            done.state,
            if fail_finish {
                JobState::Failed
            } else {
                JobState::Completed
            }
        );
        assert_eq!(done.coverage, Some(CoverageState::Partial));
        if fail_finish {
            assert_eq!(
                done.error.unwrap().message.as_str(),
                "synthetic publication failure"
            );
        } else {
            assert!(done.error.is_none());
        }
        assert_eq!(*sink.finishes.lock().unwrap(), vec![(false, false, true)]);
        assert!(!sink.memory.sweep_allowed(&root_id));
    }
}
