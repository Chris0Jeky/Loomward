//! Bounded native scan workers, staging, run/revision finals and grant revocation hooks.
mod pipeline;
pub mod sink;
pub mod source;
#[cfg(test)]
mod tests;
use crate::{Component, Engine, EngineError, EngineResult};
use loomward_protocol::{DatasetClass, Job, JobId, JobKind, RootId, ScanBudget};
pub use pipeline::{run_scan, ScanOptions, ScanReport};
pub use sink::{Entry, ListingTicket, MemorySink, RunScope, ScanMessage, ScanSink, Sums};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

/// Trusted root resolved from an active durable grant by the service, never from wire paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantedRoot {
    root_id: RootId,
    path: PathBuf,
    dataset_class: DatasetClass,
    expected: Option<source::OpenedIdentity>,
}
impl GrantedRoot {
    // L8 must wire construction from its durable grant row inside the trusted engine boundary.
    #[allow(dead_code)]
    pub(crate) fn new(
        root_id: RootId,
        path: PathBuf,
        dataset_class: DatasetClass,
        expected: source::OpenedIdentity,
    ) -> Self {
        Self {
            root_id,
            path,
            dataset_class,
            expected: Some(expected),
        }
    }
    /// Construct a synthetic grant only for a marked G:/E: scale-lab data directory.
    /// This fixture-only API is absent from production builds; L8 resolves personal grants.
    #[cfg(all(windows, feature = "lab"))]
    pub fn for_lab(root_id: RootId, path: PathBuf) -> EngineResult<Self> {
        use source::DirSource;
        let text = path.to_str().ok_or_else(|| EngineError::PermissionDenied {
            message: "invalid lab root".into(),
        })?;
        let normal = text.strip_prefix(r"\\?\").unwrap_or(text);
        let prefix = [r"G:\loomward-lab\scale\", r"E:\loomward-lab\scale\"]
            .into_iter()
            .find(|p| normal.starts_with(p))
            .ok_or_else(|| EngineError::PermissionDenied {
                message: "outside scale lab".into(),
            })?;
        let suffix = &normal[prefix.len()..];
        let name = suffix
            .strip_suffix(r"\data")
            .ok_or_else(|| EngineError::PermissionDenied {
                message: "not lab data root".into(),
            })?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            return Err(EngineError::PermissionDenied {
                message: "invalid lab name".into(),
            });
        }
        let source = loomward_windows::enumerate::NativeSource::default();
        let mut pins = Vec::new();
        for ancestor in path.ancestors().filter(|p| !p.as_os_str().is_empty()) {
            pins.push(
                source
                    .open_root(ancestor)
                    .map_err(|_| EngineError::PermissionDenied {
                        message: "lab ancestor refused".into(),
                    })?
                    .0,
            );
        }
        let marker = path.parent().unwrap().join(".loomward-lab-marker");
        if std::fs::read_to_string(marker).ok().as_deref()
            != Some("Loomward disposable scale lab v1\n")
        {
            return Err(EngineError::PermissionDenied {
                message: "unmarked lab root".into(),
            });
        }
        let (_, identity) = source
            .open_root(&path)
            .map_err(|_| EngineError::PermissionDenied {
                message: "lab root refused".into(),
            })?;
        Ok(Self::new(root_id, path, DatasetClass::Synthetic, identity))
    }
    /// Opaque grant root id.
    pub fn root_id(&self) -> &RootId {
        &self.root_id
    }
    /// Granted native directory; never exposed as a wire input.
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Dataset class of its durable grant.
    pub fn dataset_class(&self) -> DatasetClass {
        self.dataset_class
    }
}
impl Engine {
    fn check_root(&self, root: &GrantedRoot) -> EngineResult<()> {
        if root.dataset_class != self.config.dataset_class {
            return Err(EngineError::PermissionDenied {
                message: "dataset_class_mismatch".into(),
            });
        }
        Ok(())
    }
    /// Install the catalogue writer after restart recovery has marked interrupted roots repairing.
    pub fn set_scan_sink(&self, sink: Arc<dyn ScanSink>) -> EngineResult<()> {
        let mut installed = self.scan_sink.lock().unwrap();
        if installed.is_some() {
            return Err(EngineError::Busy {
                message: "scan writer already installed".into(),
                detail: None,
            });
        }
        sink.recover_interrupted()?;
        *installed = Some(sink);
        Ok(())
    }
    /// Full read-only traversal of one granted root.
    pub fn scan_start(&self, root: &GrantedRoot, budget: Option<ScanBudget>) -> EngineResult<Job> {
        self.start_scan(root, budget, RunScope::FullRoot)
    }
    /// Conservative refresh: until watcher targets are supplied, relist the full granted root.
    pub fn scan_refresh(
        &self,
        root: &GrantedRoot,
        budget: Option<ScanBudget>,
    ) -> EngineResult<Job> {
        self.start_scan(root, budget, RunScope::FullRoot)
    }
    fn start_scan(
        &self,
        root: &GrantedRoot,
        budget: Option<ScanBudget>,
        scope: RunScope,
    ) -> EngineResult<Job> {
        self.check_root(root)?;
        let sink = self
            .scan_sink
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| EngineError::unavailable(Component::Scan))?;
        if let Some(job_id) = self.jobs.root_jobs(root.root_id()).first() {
            return Err(EngineError::Busy {
                message: "root already scanning".into(),
                detail: Some(
                    loomward_protocol::Detail::new(std::collections::BTreeMap::from([(
                        "job_id".into(),
                        serde_json::json!(job_id),
                    )]))
                    .unwrap(),
                ),
            });
        }
        let worker_permit = self.budgets.reserve_scan_workers(
            budget
                .as_ref()
                .and_then(|b| b.threads)
                .map(|n| n.get() as usize),
        )?;
        let options = ScanOptions {
            workers: worker_permit.count,
            max_entries: budget
                .as_ref()
                .and_then(|b| b.max_entries)
                .map_or(50_000_000, |n| n.get() as u64),
            max_dirs: budget
                .as_ref()
                .and_then(|b| b.max_dirs)
                .map_or(8_000_000, |n| n.get() as usize),
            scope,
            excluded_path: Some(self.config.state_dir.clone()),
            ..ScanOptions::default()
        };
        let root = root.clone();
        let bytes = self.budgets.bytes.clone();
        static NEXT_RUN: AtomicU64 = AtomicU64::new(1);
        let run = NEXT_RUN.fetch_add(1, Ordering::Relaxed);
        #[cfg(windows)]
        {
            let io = source::native_cancel();
            let interrupt = io.clone();
            let root_id = root.root_id().clone();
            let workers = std::sync::Mutex::new(Some(worker_permit));
            let runner: crate::jobs::JobRunner = Arc::new(move |ctx, _| {
                let _workers = workers
                    .lock()
                    .unwrap()
                    .take()
                    .expect("scan runner executes once");
                let source = loomward_windows::enumerate::NativeSource::new(io.clone());
                let report = run_scan(
                    &source,
                    &root,
                    run,
                    sink.clone(),
                    bytes.clone(),
                    ctx.cancel.clone(),
                    options.clone(),
                )?;
                ctx.set_scan_report(&report);
                Ok(if report.complete {
                    loomward_protocol::CoverageState::Complete
                } else {
                    loomward_protocol::CoverageState::Partial
                })
            });
            self.jobs.submit(
                crate::jobs::JobSpec::new(JobKind::Scan, Some(root_id)),
                runner,
                self.events.clone(),
                Some(Arc::new(move || interrupt.cancel())),
            )
        }
        #[cfg(not(windows))]
        {
            let _ = (options, root, bytes, run, sink);
            Err(EngineError::unavailable(Component::Scan))
        }
    }
    /// Immediate cancellation acknowledgement; no waiting for directory I/O or writer joins.
    pub fn scan_cancel(&self, id: &JobId) -> EngineResult<Job> {
        if self.job_status(id)?.kind != JobKind::Scan {
            return Err(EngineError::InvalidRequest {
                message: "not a scan job".into(),
            });
        }
        self.job_cancel(id)
    }
    /// Cancel all active scan jobs for a revoked root. Commit revocation before calling this.
    pub fn scan_cancel_root(&self, root: &RootId) -> EngineResult<Vec<Job>> {
        self.jobs
            .root_jobs(root)
            .iter()
            .map(|id| self.job_cancel(id))
            .collect()
    }
    /// Writer-fence hook, called after durable revocation and root cancellation by L8.
    pub fn scan_writer_fence(&self, root: &RootId) -> EngineResult<()> {
        self.scan_sink
            .lock()
            .unwrap()
            .as_ref()
            .ok_or_else(|| EngineError::unavailable(Component::Scan))?
            .fence_root(root)
    }
}
