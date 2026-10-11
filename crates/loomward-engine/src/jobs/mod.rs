//! Kind-dispatched jobs, immediate cancellation and ten-minute terminal retention.
use crate::{
    events::{random_id, timestamp, Bus},
    Component, Engine, EngineError, EngineResult,
};
use loomward_protocol::{
    Count, CoverageState, EventName, Job, JobId, JobKind, JobList, JobListRequest, JobState, RootId,
};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, Weak,
    },
    time::{Duration, Instant},
};

/// Minimum terminal retention, matching the service idempotency window (#117 N4).
pub const TERMINAL_RETENTION: Duration = Duration::from_secs(600);
/// Runner selection; never contains a shell command or arbitrary caller path.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct JobSpec {
    /// Engine-owned runner.
    pub kind: JobKind,
    /// Resolved root, if any.
    pub root_id: Option<RootId>,
}
impl JobSpec {
    /// Select a runner and its already-resolved root.
    pub fn new(kind: JobKind, root_id: Option<RootId>) -> Self {
        Self { kind, root_id }
    }
}
/// Per-job cancellation shared with the runner. A cancel request never joins its worker.
#[derive(Clone)]
pub struct JobContext {
    /// Opaque job id.
    pub job_id: JobId,
    /// Cancellation flag, also used by staging waits.
    pub cancel: Arc<AtomicBool>,
    registry: Weak<Registry>,
    events: Arc<Bus>,
}
impl JobContext {
    /// Store and publish exact final scan counters. A missing wire strategy stays unknown.
    pub fn set_scan_report(&self, report: &crate::scan::ScanReport) {
        let strategy = match report.strategy {
            crate::scan::source::Strategy::Extended => {
                loomward_protocol::EnumerationStrategy::FileIdExtdDirectoryInfo
            }
            crate::scan::source::Strategy::Find => {
                loomward_protocol::EnumerationStrategy::FindFirstFileExLargeFetch
            }
            crate::scan::source::Strategy::Portable => {
                loomward_protocol::EnumerationStrategy::StdReadDir
            }
            // FileIdBothDirectoryInfo has no wire value yet (#190 item 3): stay unknown.
            crate::scan::source::Strategy::Both => return,
        };
        let progress=serde_json::from_value(serde_json::json!({"phase":"done","strategy":strategy,"examined":report.examined,"indexed_files":report.totals.files,"indexed_dirs":report.totals.dirs,"skipped":report.skipped,"failed":report.failed,"logical_bytes":report.totals.logical.to_string(),"allocated_bytes":report.totals.allocated.map(|n|n.to_string()),"pending_dirs":0,"writer_queue_depth":0,"entries_per_second":report.examined as f64/report.elapsed_seconds.max(0.000001),"elapsed_ms":(report.elapsed_seconds*1000.) as u64,"limit_hit":report.limit_hit})).expect("bounded scan progress");
        if let Some(registry) = self.registry.upgrade() {
            if let Some(r) = registry.records.lock().unwrap().get_mut(&self.job_id) {
                if r.job.progress.as_ref() == Some(&progress) {
                    return;
                }
                r.job.progress = Some(progress);
                revise(&mut r.job);
                if let Some(root_id) = &r.job.root_id {
                    let _=self.events.publish(EventName::ScanProgress,serde_json::json!({"job_id":self.job_id,"root_id":root_id,"progress":r.job.progress}).as_object().unwrap().clone(),None,None);
                }
                emit(&self.events, &r.job);
            }
        }
    }
    /// Retain bounded evidence; the terminal transition publishes it with the job revision.
    pub(crate) fn set_duplicates(&self, report: loomward_protocol::DuplicateReport) {
        if let Some(registry) = self.registry.upgrade() {
            if let Some(r) = registry.records.lock().unwrap().get_mut(&self.job_id) {
                if r.job.error.is_none() {
                    r.duplicates = Some(report);
                }
            }
        }
    }
    /// Whether the caller requested cancellation.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
}
/// Engine-owned runner registered by the lane responsible for this job kind.
pub type JobRunner = Arc<dyn Fn(JobContext, JobSpec) -> EngineResult<CoverageState> + Send + Sync>;
struct Record {
    job: Job,
    duplicates: Option<loomward_protocol::DuplicateReport>,
    cancel: Arc<AtomicBool>,
    terminal: Option<Instant>,
    interrupt: Option<Arc<dyn Fn() + Send + Sync>>,
}
#[derive(Default)]
pub(crate) struct Registry {
    records: Mutex<HashMap<JobId, Record>>,
    runners: Mutex<HashMap<JobKind, JobRunner>>,
    seq: AtomicU64,
}
impl Registry {
    fn prune(records: &mut HashMap<JobId, Record>) {
        records.retain(|_, r| r.terminal.is_none_or(|t| t.elapsed() < TERMINAL_RETENTION));
    }
    /// Mark a submitted job failed because its worker thread could not start, and
    /// publish the terminal state so subscribers never wait forever. This is the
    /// single owner of the spawn-failure transition; `submit` calls it.
    pub(crate) fn mark_spawn_failed(&self, job_id: &JobId, events: &Bus) -> EngineError {
        let mut records = self.records.lock().unwrap();
        let r = records.get_mut(job_id).unwrap();
        r.job.state = JobState::Failed;
        r.job.finished_at = Some(timestamp());
        r.terminal = Some(Instant::now());
        r.job.error = Some(
            EngineError::Internal {
                message: "job thread unavailable".into(),
                detail: None,
            }
            .to_body(),
        );
        revise(&mut r.job);
        emit(events, &r.job);
        EngineError::Internal {
            message: "job thread unavailable".into(),
            detail: None,
        }
    }
    pub fn submit(
        self: &Arc<Self>,
        spec: JobSpec,
        runner: JobRunner,
        events: Arc<Bus>,
        interrupt: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> EngineResult<Job> {
        let mut job = Job {
            job_id: JobId::new(random_id("jb_")).unwrap(),
            seq: Count::ZERO,
            rev: Count::ZERO,
            kind: spec.kind,
            state: JobState::Queued,
            root_id: spec.root_id.clone(),
            created_at: timestamp(),
            started_at: None,
            finished_at: None,
            progress: None,
            coverage: None,
            error: None,
            duplicates: None,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut records = self.records.lock().unwrap();
            Self::prune(&mut records);
            if matches!(spec.kind, JobKind::Scan | JobKind::Duplicates) {
                if let Some(r) = records.values().find(|r| {
                    r.job.kind == spec.kind && r.job.root_id == spec.root_id && r.terminal.is_none()
                }) {
                    return Err(EngineError::Busy {
                        message: "root already scanning".into(),
                        detail: Some(
                            loomward_protocol::Detail::new(std::collections::BTreeMap::from([(
                                "job_id".into(),
                                serde_json::json!(r.job.job_id),
                            )]))
                            .unwrap(),
                        ),
                    });
                }
            }
            // Allocate under the records lock so submission events and listing order agree.
            // The records lock serialises allocation, so a load and store is enough (and avoids
            // `fetch_update`, deprecated on newer toolchains).
            let next = self
                .seq
                .load(Ordering::Relaxed)
                .checked_add(1)
                .filter(|next| Count::new(*next).is_ok())
                .ok_or_else(|| EngineError::Internal {
                    message: "job sequence exhausted".into(),
                    detail: None,
                })?;
            self.seq.store(next, Ordering::Relaxed);
            job.seq = Count::new(next).unwrap();
            records.insert(
                job.job_id.clone(),
                Record {
                    job: job.clone(),
                    duplicates: None,
                    cancel: cancel.clone(),
                    terminal: None,
                    interrupt,
                },
            );
            emit(&events, &job);
        }
        let registry = self.clone();
        let id = job.job_id.clone();
        let thread_events = events.clone();
        let spawn = std::thread::Builder::new()
            .name(format!("loomward-{}", id.as_str()))
            .spawn(move || {
                {
                    let mut records = registry.records.lock().unwrap();
                    let r = records.get_mut(&id).unwrap();
                    if r.cancel.load(Ordering::Acquire) {
                        // Cancelled before the worker started: CancelRequested was
                        // already published by `cancel`, so only record the start
                        // attempt without re-emitting the same state.
                        r.job.started_at = Some(timestamp());
                        revise(&mut r.job);
                    } else {
                        r.job.state = JobState::Running;
                        r.job.started_at = Some(timestamp());
                        revise(&mut r.job);
                        emit(&thread_events, &r.job);
                    }
                }
                let context = JobContext {
                    job_id: id.clone(),
                    cancel: cancel.clone(),
                    registry: Arc::downgrade(&registry),
                    events: thread_events.clone(),
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    runner(context, spec)
                }))
                .unwrap_or_else(|_| {
                    Err(EngineError::Internal {
                        message: "job runner panicked".into(),
                        detail: None,
                    })
                });
                let mut records = registry.records.lock().unwrap();
                let r = records.get_mut(&id).unwrap();
                if r.job.kind == JobKind::Duplicates && r.job.error.is_none() {
                    r.job.duplicates = r.duplicates.take();
                } else {
                    r.duplicates = None;
                }
                if cancel.load(Ordering::Acquire) {
                    r.job.state = JobState::Cancelled;
                    r.job.coverage = Some(CoverageState::Cancelled);
                    if r.job.kind == JobKind::Duplicates {
                        if r.job.error.is_some() {
                            r.job.duplicates = None;
                        } else if let Some(report) = &mut r.job.duplicates {
                            report.partial = true;
                            report.stop =
                                Some(loomward_protocol::BoundedStr::new("cancelled").unwrap());
                        }
                        if let Err(e) = result {
                            r.job.duplicates = None;
                            r.job.error = Some(e.to_body());
                        }
                    }
                } else {
                    match result {
                        Ok(coverage) => {
                            r.job.state = JobState::Completed;
                            r.job.coverage = Some(coverage)
                        }
                        Err(e) => {
                            r.job.duplicates = None;
                            r.job.state = JobState::Failed;
                            r.job.coverage = Some(CoverageState::Partial);
                            r.job.error = Some(e.to_body());
                        }
                    }
                }
                r.job.finished_at = Some(timestamp());
                r.terminal = Some(Instant::now());
                revise(&mut r.job);
                emit(&thread_events, &r.job);
            });
        if spawn.is_err() {
            return Err(self.mark_spawn_failed(&job.job_id, &events));
        }
        Ok(job)
    }
    pub fn cancel(&self, id: &JobId, events: &Bus) -> EngineResult<Job> {
        self.cancel_inner(id, events, false)
    }
    pub(crate) fn cancel_revoked(&self, id: &JobId, events: &Bus) -> EngineResult<Job> {
        self.cancel_inner(id, events, true)
    }
    fn cancel_inner(&self, id: &JobId, events: &Bus, revoked: bool) -> EngineResult<Job> {
        let (job, interrupt) = {
            let mut records = self.records.lock().unwrap();
            let r = records.get_mut(id).ok_or_else(|| EngineError::NotFound {
                message: "job not found".into(),
            })?;
            if revoked && r.terminal.is_none() && r.job.kind == JobKind::Duplicates {
                r.job.duplicates = None;
                r.duplicates = None;
                r.job.error = Some(
                    EngineError::PermissionDenied {
                        message: "cancelled".into(),
                    }
                    .to_body(),
                );
            }
            if r.terminal.is_some() || r.job.state == JobState::CancelRequested {
                if revoked && r.terminal.is_none() && r.job.kind == JobKind::Duplicates {
                    revise(&mut r.job);
                    emit(events, &r.job);
                }
                // Idempotent: an already-terminal job or an already-requested
                // cancel publishes no duplicate event.
                return Ok(r.job.clone());
            }
            r.cancel.store(true, Ordering::Release);
            r.job.state = JobState::CancelRequested;
            revise(&mut r.job);
            emit(events, &r.job);
            (r.job.clone(), r.interrupt.clone())
        };
        if let Some(f) = interrupt {
            f();
        }
        Ok(job)
    }
    pub fn root_jobs(&self, root: &RootId) -> Vec<JobId> {
        self.records
            .lock()
            .unwrap()
            .values()
            .filter(|r| {
                matches!(r.job.kind, JobKind::Scan | JobKind::Duplicates)
                    && r.job.root_id.as_ref() == Some(root)
                    && r.terminal.is_none()
            })
            .map(|r| r.job.job_id.clone())
            .collect()
    }
}
fn revise(job: &mut Job) {
    job.rev = Count::new(job.rev.get() + 1).expect("job revision exhausted");
}
fn emit(events: &Bus, job: &Job) {
    let data = serde_json::json!({"job":job}).as_object().unwrap().clone();
    let _ = events.publish(EventName::JobState, data, None, None);
}
impl Engine {
    /// Install one engine-owned runner for a kind; active jobs retain their original runner.
    pub fn register_job_runner(&self, kind: JobKind, runner: JobRunner) {
        self.jobs.runners.lock().unwrap().insert(kind, runner);
    }
    /// Dispatch by kind. A missing runner stays explicitly unavailable.
    pub fn job_submit(&self, spec: JobSpec) -> EngineResult<Job> {
        let runner = self
            .jobs
            .runners
            .lock()
            .unwrap()
            .get(&spec.kind)
            .cloned()
            .ok_or_else(|| EngineError::unavailable(Component::Jobs))?;
        self.jobs.submit(spec, runner, self.events.clone(), None)
    }
    /// Acknowledge cancellation immediately; synchronous I/O completion remains separate.
    pub fn job_cancel(&self, id: &JobId) -> EngineResult<Job> {
        self.jobs.cancel(id, &self.events)
    }
    /// Retained state of a job, including terminal outcomes inside the idempotency window.
    pub fn job_status(&self, id: &JobId) -> EngineResult<Job> {
        self.jobs
            .records
            .lock()
            .unwrap()
            .get(id)
            .map(|r| r.job.clone())
            .ok_or_else(|| EngineError::NotFound {
                message: "job not found".into(),
            })
    }
    /// Recent retained jobs, newest first, optionally filtered by kind.
    pub fn job_list(&self, request: &JobListRequest) -> EngineResult<JobList> {
        let mut records = self.jobs.records.lock().unwrap();
        Registry::prune(&mut records);
        let mut recent: Vec<_> = records
            .values()
            .filter(|r| {
                request
                    .kinds
                    .as_ref()
                    .is_none_or(|k| k.contains(&r.job.kind))
            })
            .collect();
        recent.sort_by_key(|r| std::cmp::Reverse(r.job.seq));
        let jobs = recent
            .into_iter()
            .take(request.limit.get() as usize)
            .map(|r| r.job.clone())
            .collect();
        Ok(JobList {
            jobs: loomward_protocol::BoundedVec::new(jobs).unwrap(),
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrent_submissions_have_unique_monotonic_sequences() {
        let e = Arc::new(engine());
        e.register_job_runner(JobKind::Refit, Arc::new(|_, _| Ok(CoverageState::Complete)));
        e.register_job_runner(
            JobKind::Teacher,
            Arc::new(|_, _| Ok(CoverageState::Complete)),
        );
        let barrier = Arc::new(std::sync::Barrier::new(16));
        let mut stream = e.subscribe_events(None).unwrap();
        let handles: Vec<_> = (0..16)
            .map(|i| {
                let e = e.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    e.job_submit(JobSpec::new(
                        if i % 2 == 0 {
                            JobKind::Refit
                        } else {
                            JobKind::Teacher
                        },
                        None,
                    ))
                    .unwrap()
                })
            })
            .collect();
        let mut seqs: Vec<_> = handles
            .into_iter()
            .map(|h| h.join().unwrap().seq.get())
            .collect();
        seqs.sort_unstable();
        assert_eq!(seqs, (1..=16).collect::<Vec<_>>());
        let mut published = Vec::new();
        while let loomward_protocol::RecvOutcome::Event(event) = stream.recv_timeout(Duration::ZERO)
        {
            if event.event == EventName::JobState && event.data["job"]["state"] == "queued" {
                published.push(event.data["job"]["seq"].as_u64().unwrap());
            }
        }
        assert_eq!(published, seqs);
        let list = e
            .job_list(&JobListRequest {
                kinds: None,
                limit: loomward_protocol::Int::new(50).unwrap(),
            })
            .unwrap();
        assert_eq!(
            list.jobs.iter().map(|j| j.seq.get()).collect::<Vec<_>>(),
            (1..=16).rev().collect::<Vec<_>>()
        );
        e.jobs
            .seq
            .store(loomward_protocol::MAX_SAFE_INTEGER - 1, Ordering::Relaxed);
        assert_eq!(
            e.job_submit(JobSpec::new(JobKind::Refit, None))
                .unwrap()
                .seq
                .get(),
            loomward_protocol::MAX_SAFE_INTEGER
        );
        assert!(e.job_submit(JobSpec::new(JobKind::Refit, None)).is_err());
        let fresh = engine();
        fresh.register_job_runner(JobKind::Refit, Arc::new(|_, _| Ok(CoverageState::Complete)));
        assert_eq!(
            fresh
                .job_submit(JobSpec::new(JobKind::Refit, None))
                .unwrap()
                .seq
                .get(),
            1
        );
    }

    #[test]
    fn revisions_track_state_and_progress_changes_not_reads_or_repeated_reports() {
        let e = engine();
        let (tx, rx) = std::sync::mpsc::channel();
        let (finish, wait) = std::sync::mpsc::channel();
        let wait = Mutex::new(wait);
        e.register_job_runner(
            JobKind::Scan,
            Arc::new(move |ctx, _| {
                tx.send(ctx.clone()).unwrap();
                wait.lock().unwrap().recv().unwrap();
                Ok(CoverageState::Complete)
            }),
        );
        let job = e
            .job_submit(JobSpec::new(
                JobKind::Scan,
                Some(RootId::new("rt_fixture").unwrap()),
            ))
            .unwrap();
        assert_eq!(job.rev.get(), 0);
        let ctx = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(e.job_status(&job.job_id).unwrap().rev.get(), 1);
        let mut report = crate::scan::ScanReport {
            totals: Default::default(),
            complete: true,
            strategy: crate::scan::source::Strategy::Portable,
            limit_hit: false,
            examined: 1,
            enumeration_attempts: 1,
            duplicate_identity: 0,
            skipped: 0,
            failed: 0,
            elapsed_seconds: 1.,
            enumeration_worker_seconds: 0.,
            enumeration_wall_seconds: 0.,
            staging_seconds: 0.,
            aggregation_seconds: 0.,
            persistence_seconds: 0.,
        };
        ctx.set_scan_report(&report);
        assert_eq!(e.job_status(&job.job_id).unwrap().rev.get(), 2);
        ctx.set_scan_report(&report);
        assert_eq!(e.job_status(&job.job_id).unwrap().rev.get(), 2);
        report.examined = 2;
        ctx.set_scan_report(&report);
        assert_eq!(e.job_status(&job.job_id).unwrap().rev.get(), 3);
        finish.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let current = e.job_status(&job.job_id).unwrap();
            if current.state == JobState::Completed {
                assert_eq!(current.rev.get(), 4);
                assert_eq!(current.seq, job.seq);
                assert_eq!(e.job_cancel(&job.job_id).unwrap().rev, current.rev);
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    #[test]
    fn cancel_complete_race_never_publishes_nonterminal_after_terminal() {
        // The interrupt callback lets completion win before cancellation returns.
        let e = Arc::new(engine());
        let mut stale = 0;
        for _ in 0..128 {
            let registry = Arc::downgrade(&e.jobs);
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            let runner: JobRunner = Arc::new(move |ctx, _| {
                tx.send(ctx.job_id.clone()).unwrap();
                while !ctx.is_cancelled() {
                    std::thread::yield_now();
                }
                Ok(CoverageState::Complete)
            });
            let id = Arc::new(Mutex::new(None::<JobId>));
            let interrupt_id = id.clone();
            let interrupt = Arc::new(move || {
                let registry = registry.upgrade().unwrap();
                let id = interrupt_id.lock().unwrap().clone().unwrap();
                let deadline = Instant::now() + Duration::from_secs(2);
                while registry.records.lock().unwrap()[&id].terminal.is_none() {
                    assert!(Instant::now() < deadline);
                    std::thread::yield_now();
                }
            });
            let mut stream = e.subscribe_events(None).unwrap();
            stream.recv_timeout(Duration::ZERO);
            let job = e
                .jobs
                .submit(
                    JobSpec::new(JobKind::Refit, None),
                    runner,
                    e.events.clone(),
                    Some(interrupt),
                )
                .unwrap();
            *id.lock().unwrap() = Some(rx.recv_timeout(Duration::from_secs(2)).unwrap());
            e.job_cancel(&job.job_id).unwrap();
            let mut terminal = false;
            let mut saw_cancel = false;
            while let loomward_protocol::RecvOutcome::Event(event) =
                stream.recv_timeout(Duration::ZERO)
            {
                if event.event != EventName::JobState {
                    continue;
                }
                let state = &event.data["job"]["state"];
                if state == "cancelled" {
                    terminal = true;
                } else if terminal {
                    stale += 1;
                }
                saw_cancel |= state == "cancel_requested";
            }
            assert!(terminal && saw_cancel);
        }
        assert_eq!(
            stale, 0,
            "late cancellation transitions overtake terminal state"
        );
    }
    fn engine() -> Engine {
        Engine::open(crate::EngineConfig::new(
            "unused".into(),
            loomward_protocol::DatasetClass::Synthetic,
        ))
        .unwrap()
    }
    #[test]
    fn kind_dispatch_terminal_retention_and_cancel_ack() {
        let e = engine();
        e.register_job_runner(
            JobKind::Refit,
            Arc::new(|ctx, _| {
                while !ctx.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Ok(CoverageState::Cancelled)
            }),
        );
        let job = e.job_submit(JobSpec::new(JobKind::Refit, None)).unwrap();
        let now = Instant::now();
        assert_eq!(
            e.job_cancel(&job.job_id).unwrap().state,
            JobState::CancelRequested
        );
        assert!(now.elapsed() < Duration::from_millis(250));
        let deadline = Instant::now() + Duration::from_secs(1);
        while e.job_status(&job.job_id).unwrap().state != JobState::Cancelled {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(e.job_submit(JobSpec::new(JobKind::Teacher, None)).is_err());
        let mut records = e.jobs.records.lock().unwrap();
        Registry::prune(&mut records);
        assert!(records.contains_key(&job.job_id));
        records.get_mut(&job.job_id).unwrap().terminal = Some(Instant::now() - TERMINAL_RETENTION);
        Registry::prune(&mut records);
        assert!(!records.contains_key(&job.job_id));
    }
    fn drain(stream: &mut Box<dyn loomward_protocol::EventStream>) {
        while let loomward_protocol::RecvOutcome::Event(_) = stream.recv_timeout(Duration::ZERO) {}
    }
    fn job_states(
        stream: &mut Box<dyn loomward_protocol::EventStream>,
        deadline: Instant,
    ) -> Vec<String> {
        let mut states = Vec::new();
        while Instant::now() < deadline {
            while let loomward_protocol::RecvOutcome::Event(event) =
                stream.recv_timeout(Duration::ZERO)
            {
                if event.event != EventName::JobState {
                    continue;
                }
                states.push(
                    event.data["job"]["state"]
                        .as_str()
                        .unwrap_or("?")
                        .to_string(),
                );
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        states
    }
    #[test]
    fn submit_publishes_queued_before_running() {
        let e = engine();
        let mut stream = e.subscribe_events(None).unwrap();
        drain(&mut stream);
        let runner: JobRunner = Arc::new(|ctx, _| {
            while !ctx.is_cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(CoverageState::Cancelled)
        });
        let job = e
            .jobs
            .submit(
                JobSpec::new(JobKind::Refit, None),
                runner,
                e.events.clone(),
                None,
            )
            .unwrap();
        assert_eq!(job.state, JobState::Queued);
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut states = Vec::new();
        while Instant::now() < deadline {
            while let loomward_protocol::RecvOutcome::Event(event) =
                stream.recv_timeout(Duration::ZERO)
            {
                if event.event != EventName::JobState {
                    continue;
                }
                states.push(
                    event.data["job"]["state"]
                        .as_str()
                        .unwrap_or("?")
                        .to_string(),
                );
            }
            if states.contains(&"queued".to_string()) && states.contains(&"running".to_string()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            states.contains(&"queued".to_string()),
            "submission published no Queued event: {:?}",
            states
        );
        assert_eq!(
            states.first().map(|s| s.as_str()),
            Some("queued"),
            "first event was not Queued: {:?}",
            states
        );
        let _ = e.job_cancel(&job.job_id);
    }
    #[test]
    fn spawn_failure_marks_failed_and_publishes_terminal_event() {
        let e = engine();
        let mut stream = e.subscribe_events(None).unwrap();
        drain(&mut stream);
        let runner: JobRunner = Arc::new(|_, _| {
            std::thread::sleep(Duration::from_millis(500));
            Ok(CoverageState::Complete)
        });
        let job = e
            .jobs
            .submit(
                JobSpec::new(JobKind::Refit, None),
                runner,
                e.events.clone(),
                None,
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while e.job_status(&job.job_id).unwrap().state != JobState::Running {
            assert!(Instant::now() < deadline, "job never reached Running");
            std::thread::sleep(Duration::from_millis(2));
        }
        drain(&mut stream);
        let _ = e.jobs.mark_spawn_failed(&job.job_id, &e.events);
        assert_eq!(
            e.job_status(&job.job_id).unwrap().state,
            JobState::Failed,
            "spawn failure did not mark the job Failed"
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut saw_failed = false;
        while Instant::now() < deadline {
            while let loomward_protocol::RecvOutcome::Event(event) =
                stream.recv_timeout(Duration::ZERO)
            {
                if event.event != EventName::JobState {
                    continue;
                }
                if &event.data["job"]["state"] == "failed" {
                    saw_failed = true;
                }
            }
            if saw_failed {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            saw_failed,
            "spawn failure published no Failed terminal event"
        );
    }
    #[test]
    fn cancel_is_idempotent_with_no_duplicate_event() {
        let e = engine();
        // The runner lingers after a cancel request so a second cancel lands
        // while the job is still non-terminal.
        let runner: JobRunner = Arc::new(|ctx, _| {
            while !ctx.is_cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            std::thread::sleep(Duration::from_millis(300));
            Ok(CoverageState::Cancelled)
        });
        let mut stream = e.subscribe_events(None).unwrap();
        drain(&mut stream);
        let job = e
            .jobs
            .submit(
                JobSpec::new(JobKind::Refit, None),
                runner,
                e.events.clone(),
                None,
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while e.job_status(&job.job_id).unwrap().state != JobState::Running {
            assert!(Instant::now() < deadline, "job never reached Running");
            std::thread::sleep(Duration::from_millis(2));
        }
        drain(&mut stream);
        assert_eq!(
            e.job_cancel(&job.job_id).unwrap().state,
            JobState::CancelRequested
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut saw_requested = false;
        while Instant::now() < deadline {
            while let loomward_protocol::RecvOutcome::Event(event) =
                stream.recv_timeout(Duration::ZERO)
            {
                if event.event != EventName::JobState {
                    continue;
                }
                if &event.data["job"]["state"] == "cancel_requested" {
                    saw_requested = true;
                }
            }
            if saw_requested {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(saw_requested, "first cancel published no request");
        drain(&mut stream);
        // Still non-terminal: the runner lingers 300ms after the request.
        assert_eq!(
            e.job_status(&job.job_id).unwrap().state,
            JobState::CancelRequested
        );
        assert_eq!(
            e.job_cancel(&job.job_id).unwrap().state,
            JobState::CancelRequested
        );
        std::thread::sleep(Duration::from_millis(100));
        let states = job_states(&mut stream, Instant::now() + Duration::from_millis(50));
        assert!(
            states.is_empty(),
            "second cancel re-emitted state: {:?}",
            states
        );
        // After terminal completion a further cancel stays silent.
        let deadline = Instant::now() + Duration::from_secs(2);
        while e.job_status(&job.job_id).unwrap().state != JobState::Cancelled {
            assert!(Instant::now() < deadline, "job never reached Cancelled");
            std::thread::sleep(Duration::from_millis(2));
        }
        drain(&mut stream);
        assert_eq!(
            e.job_cancel(&job.job_id).unwrap().state,
            JobState::Cancelled
        );
        std::thread::sleep(Duration::from_millis(100));
        let states = job_states(&mut stream, Instant::now() + Duration::from_millis(50));
        assert!(
            states.is_empty(),
            "terminal cancel re-emitted state: {:?}",
            states
        );
    }
}
