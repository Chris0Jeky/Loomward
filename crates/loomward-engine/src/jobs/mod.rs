//! Kind-dispatched jobs, immediate cancellation and ten-minute terminal retention.
use crate::{
    events::{random_id, timestamp, Bus},
    Component, Engine, EngineError, EngineResult,
};
use loomward_protocol::{
    CoverageState, EventName, Job, JobId, JobKind, JobList, JobListRequest, JobState, RootId,
};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
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
            crate::scan::source::Strategy::Both => return,
        };
        let progress=serde_json::from_value(serde_json::json!({"phase":"done","strategy":strategy,"examined":report.examined,"indexed_files":report.totals.files,"indexed_dirs":report.totals.dirs,"skipped":report.skipped,"failed":report.failed,"logical_bytes":report.totals.logical.to_string(),"allocated_bytes":report.totals.allocated.map(|n|n.to_string()),"pending_dirs":0,"writer_queue_depth":0,"entries_per_second":report.examined as f64/report.elapsed_seconds.max(0.000001),"elapsed_ms":(report.elapsed_seconds*1000.) as u64,"limit_hit":report.limit_hit})).expect("bounded scan progress");
        if let Some(registry) = self.registry.upgrade() {
            if let Some(r) = registry.records.lock().unwrap().get_mut(&self.job_id) {
                r.job.progress = Some(progress);
                if let Some(root_id) = &r.job.root_id {
                    let _=self.events.publish(EventName::ScanProgress,serde_json::json!({"job_id":self.job_id,"root_id":root_id,"progress":r.job.progress}).as_object().unwrap().clone(),None,None);
                }
                emit(&self.events, &r.job);
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
    created: Instant,
    cancel: Arc<AtomicBool>,
    terminal: Option<Instant>,
    interrupt: Option<Arc<dyn Fn() + Send + Sync>>,
}
#[derive(Default)]
pub(crate) struct Registry {
    records: Mutex<HashMap<JobId, Record>>,
    runners: Mutex<HashMap<JobKind, JobRunner>>,
}
impl Registry {
    fn prune(records: &mut HashMap<JobId, Record>) {
        records.retain(|_, r| r.terminal.is_none_or(|t| t.elapsed() < TERMINAL_RETENTION));
    }
    pub fn submit(
        self: &Arc<Self>,
        spec: JobSpec,
        runner: JobRunner,
        events: Arc<Bus>,
        interrupt: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> EngineResult<Job> {
        let job = Job {
            job_id: JobId::new(random_id("jb_")).unwrap(),
            kind: spec.kind,
            state: JobState::Queued,
            root_id: spec.root_id.clone(),
            created_at: timestamp(),
            started_at: None,
            finished_at: None,
            progress: None,
            coverage: None,
            error: None,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut records = self.records.lock().unwrap();
            Self::prune(&mut records);
            if spec.kind == JobKind::Scan {
                if let Some(r) = records.values().find(|r| {
                    r.job.kind == JobKind::Scan
                        && r.job.root_id == spec.root_id
                        && r.terminal.is_none()
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
            records.insert(
                job.job_id.clone(),
                Record {
                    job: job.clone(),
                    created: Instant::now(),
                    cancel: cancel.clone(),
                    terminal: None,
                    interrupt,
                },
            );
        }
        let registry = self.clone();
        let id = job.job_id.clone();
        let spawn = std::thread::Builder::new()
            .name(format!("loomward-{}", id.as_str()))
            .spawn(move || {
                {
                    let mut records = registry.records.lock().unwrap();
                    let r = records.get_mut(&id).unwrap();
                    if !r.cancel.load(Ordering::Acquire) {
                        r.job.state = JobState::Running;
                    }
                    r.job.started_at = Some(timestamp());
                    emit(&events, &r.job);
                }
                let context = JobContext {
                    job_id: id.clone(),
                    cancel: cancel.clone(),
                    registry: Arc::downgrade(&registry),
                    events: events.clone(),
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
                if cancel.load(Ordering::Acquire) {
                    r.job.state = JobState::Cancelled;
                    r.job.coverage = Some(CoverageState::Cancelled);
                } else {
                    match result {
                        Ok(coverage) => {
                            r.job.state = JobState::Completed;
                            r.job.coverage = Some(coverage)
                        }
                        Err(e) => {
                            r.job.state = JobState::Failed;
                            r.job.coverage = Some(CoverageState::Partial);
                            r.job.error = Some(e.to_body());
                        }
                    }
                }
                r.job.finished_at = Some(timestamp());
                r.terminal = Some(Instant::now());
                emit(&events, &r.job);
            });
        if spawn.is_err() {
            let mut records = self.records.lock().unwrap();
            let r = records.get_mut(&job.job_id).unwrap();
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
            return Err(EngineError::Internal {
                message: "job thread unavailable".into(),
                detail: None,
            });
        }
        Ok(job)
    }
    pub fn cancel(&self, id: &JobId) -> EngineResult<Job> {
        let (job, interrupt) = {
            let mut records = self.records.lock().unwrap();
            let r = records.get_mut(id).ok_or_else(|| EngineError::NotFound {
                message: "job not found".into(),
            })?;
            if r.terminal.is_none() {
                r.cancel.store(true, Ordering::Release);
                r.job.state = JobState::CancelRequested;
            }
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
                r.job.kind == JobKind::Scan
                    && r.job.root_id.as_ref() == Some(root)
                    && r.terminal.is_none()
            })
            .map(|r| r.job.job_id.clone())
            .collect()
    }
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
        let job = self.jobs.cancel(id)?;
        emit(&self.events, &job);
        Ok(job)
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
        recent.sort_by_key(|r| std::cmp::Reverse(r.created));
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
}
