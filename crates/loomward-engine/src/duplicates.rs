use crate::{
    jobs::{JobContext, JobRunner, JobSpec},
    scan::GrantedRoot,
    Engine, EngineError, EngineResult,
};
use loomward_catalog::{DuplicateCandidate, DuplicateCandidates};
use loomward_protocol::{CoverageState, DuplicateBudget, DuplicateReport, Job, JobKind};
use loomward_windows::{
    duplicate_reader::{Budget, Entry},
    identity,
};
use std::{
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};

impl Engine {
    /// Submit only already-resolved catalogue observations under a synthetic durable grant.
    pub fn duplicates_start(
        self: &Arc<Self>,
        root: GrantedRoot,
        candidates: DuplicateCandidates,
        budget: DuplicateBudget,
    ) -> EngineResult<Job> {
        self.duplicate_read_admission(&root)?;
        let workers = self.budgets.reserve_scan_workers(Some(1))?;
        let spec = JobSpec::new(JobKind::Duplicates, Some(root.root_id().clone()));
        let runner = self.duplicate_runner(root, candidates, budget);
        let runner: JobRunner = Arc::new(move |ctx, spec| {
            let _workers = &workers;
            runner(ctx, spec)
        });
        self.jobs.submit(spec, runner, self.events.clone(), None)
    }

    fn duplicate_runner(
        self: &Arc<Self>,
        root: GrantedRoot,
        candidates: DuplicateCandidates,
        budget: DuplicateBudget,
    ) -> JobRunner {
        let engine = self.clone();
        Arc::new(move |ctx, _| engine.duplicates_run(&root, &candidates, &budget, ctx))
    }

    fn duplicates_run(
        &self,
        root: &GrantedRoot,
        candidates: &DuplicateCandidates,
        budget: &DuplicateBudget,
        ctx: JobContext,
    ) -> EngineResult<CoverageState> {
        let root_cancel = self.duplicate_read_admission(root)?;
        let deadline = Instant::now() + Duration::from_millis(budget.max_ms.get() as u64);
        let mut entries = Vec::new();
        let mut indices = Vec::new();
        let mut skips = std::collections::BTreeMap::new();
        let mut stop = None;
        for (index, candidate) in candidates.entries.iter().enumerate() {
            if ctx.is_cancelled() || root_cancel.load(Ordering::Acquire) {
                stop = Some("cancelled");
                break;
            }
            if Instant::now() >= deadline {
                stop = Some("time_budget");
                break;
            }
            if candidate.file_id.as_ref().is_none_or(|id| id.len() != 16)
                || candidate.created.is_none()
                || candidate.modified.is_none()
                || candidate.changed.is_none()
            {
                skips.insert(index, "unsupported");
                continue;
            }
            // Metadata-only hydration supplies nlink/ancestor bindings that directory listings omit.
            // Content is opened only by the existing reader, after matching the scan observation.
            match loomward_windows::duplicate_reader::observe_candidate(
                root.path(),
                root.expected.as_ref().unwrap(),
                std::path::Path::new(&candidate.relative),
                Budget {
                    bytes: 0,
                    time: deadline.saturating_duration_since(Instant::now()),
                },
                ctx.cancel.clone(),
            ) {
                Ok(observed) if matches_scan(candidate, &observed) => {
                    entries.push(Entry {
                        relative: candidate.relative.clone().into(),
                        expected: observed,
                    });
                    indices.push(index);
                }
                Ok(_) => {
                    skips.insert(index, "changed_during_read");
                }
                Err(reason) => {
                    skips.insert(index, reason.as_str());
                }
            }
        }
        let report = self.inspect_duplicates_cancel(
            root,
            &entries,
            Budget {
                bytes: budget.max_bytes.get(),
                time: deadline.saturating_duration_since(Instant::now()),
            },
            Some(ctx.cancel.clone()),
        )?;
        for skip in &report.skipped {
            skips.insert(indices[skip.entry], skip.reason.as_str());
        }
        let groups = |sets: &[Vec<usize>]| {
            sets.iter()
                .map(|set| set.iter().map(|&i| indices[i]).collect::<Vec<_>>())
                .collect::<Vec<_>>()
        };
        let stop = report
            .stop
            .map(|s| s.as_str())
            .or(stop)
            .or(candidates.truncated.then_some("entry_budget"));
        let partial = candidates.partial || report.partial || !skips.is_empty() || stop.is_some();
        let wire: DuplicateReport = serde_json::from_value(serde_json::json!({
            "entries": candidates.entries.iter().map(|e| serde_json::json!({"relative_path":e.relative,"logical_bytes":e.logical.to_string()})).collect::<Vec<_>>(),
            "groups": groups(&report.groups), "links": groups(&report.links), "links_not_compared":report.links_not_compared,
            "skipped":skips.iter().map(|(entry,reason)|serde_json::json!({"entry":entry,"reason":reason})).collect::<Vec<_>>(),
            "bytes_read": report.bytes_read.to_string(), "partial":partial, "truncated":candidates.truncated, "stop":stop
        })).map_err(|_| EngineError::Internal { message: "duplicate report bounds".into(), detail: None })?;
        ctx.set_duplicates(wire);
        Ok(if partial {
            CoverageState::Partial
        } else {
            CoverageState::Complete
        })
    }
}

fn matches_scan(c: &DuplicateCandidate, o: &identity::ObservedIdentity) -> bool {
    c.file_id.as_deref() == Some(&o.object.file_id[..])
        && c.volume_key == format!("vsn_{}", o.object.volume_serial)
        && c.logical.to_string() == o.size
        && c.attributes == o.attributes
        && c.created.map(|n| n.to_string()).as_ref() == Some(&o.object.creation_time)
        && c.modified.map(|n| n.to_string()).as_ref() == Some(&o.last_write_time)
        && c.changed.map(|n| n.to_string()).as_ref() == Some(&o.change_time)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::EngineConfig;
    use loomward_protocol::{DatasetClass, JobState, RootId};
    use loomward_windows::enumerate::{DirSource, NativeSource};

    #[test]
    fn duplicate_job_cancel_and_root_revocation_reach_the_worker_and_terminal_state() {
        if loomward_windows::enumerate::running_elevated() {
            return;
        }
        for (revoke, after_read) in [(false, false), (true, false), (false, true), (true, true)] {
            let tmp =
                tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
            let path = tmp.path().join("fixture");
            std::fs::create_dir(&path).unwrap();
            let expected = NativeSource::default().open_root(&path).unwrap().1;
            let root = GrantedRoot::new(
                RootId::new("rt_fixture").unwrap(),
                path,
                DatasetClass::Synthetic,
                expected,
            );
            let db = rusqlite::Connection::open(tmp.path().join("state.db")).unwrap();
            db.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT); INSERT INTO meta VALUES('dataset_class','synthetic'); CREATE TABLE root_grant(display_path TEXT,volume_key TEXT,root_file_id BLOB,state TEXT);").unwrap();
            let crate::scan::source::FileIdObs::Id128(id) = expected.id.unwrap() else {
                panic!()
            };
            db.execute(
                "INSERT INTO root_grant VALUES(?1,?2,?3,'active')",
                rusqlite::params![
                    root.path().to_str(),
                    format!("vsn_{}", expected.volume_serial.unwrap()),
                    id.to_vec()
                ],
            )
            .unwrap();
            let engine = Arc::new(
                Engine::open(EngineConfig::new(
                    tmp.path().to_owned(),
                    DatasetClass::Synthetic,
                ))
                .unwrap(),
            );
            let stopped = engine
                .inspect_duplicates_cancel(
                    &root,
                    &[],
                    Budget {
                        bytes: 100,
                        time: Duration::from_secs(1),
                    },
                    Some(Arc::new(std::sync::atomic::AtomicBool::new(true))),
                )
                .unwrap();
            assert_eq!(
                stopped.stop,
                Some(loomward_windows::duplicate_reader::Reason::Cancelled)
            );
            assert!(stopped.partial);
            let budget = serde_json::from_value(
                serde_json::json!({"max_candidates":200,"max_bytes":"100","max_ms":1000}),
            )
            .unwrap();
            let runner = engine.duplicate_runner(root.clone(), Default::default(), budget);
            let (entered, started) = std::sync::mpsc::channel();
            let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
            let worker_gate = gate.clone();
            let gated: JobRunner = Arc::new(move |ctx, spec| {
                let outcome = after_read.then(|| runner(ctx.clone(), spec.clone()));
                entered.send(()).unwrap();
                let mut go = worker_gate.0.lock().unwrap();
                while !*go {
                    go = worker_gate.1.wait(go).unwrap();
                }
                outcome.unwrap_or_else(|| runner(ctx, spec))
            });
            let job = engine
                .jobs
                .submit(
                    JobSpec::new(JobKind::Duplicates, Some(root.root_id().clone())),
                    gated,
                    engine.events.clone(),
                    None,
                )
                .unwrap();
            started.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(
                engine.job_status(&job.job_id).unwrap().duplicates.is_none(),
                "reports publish atomically with the terminal revision"
            );
            let cancel = if revoke {
                db.execute("UPDATE root_grant SET state='revoked'", [])
                    .unwrap();
                let jobs = engine.scan_cancel_root(root.root_id()).unwrap();
                assert_eq!(jobs.len(), 1);
                jobs[0].clone()
            } else {
                engine.job_cancel(&job.job_id).unwrap()
            };
            assert_eq!(cancel.state, JobState::CancelRequested);
            assert!(cancel.rev > job.rev);
            assert_eq!(engine.job_cancel(&job.job_id).unwrap().rev, cancel.rev);
            *gate.0.lock().unwrap() = true;
            gate.1.notify_all();
            let until = Instant::now() + Duration::from_secs(2);
            let terminal = loop {
                let latest = engine.job_status(&job.job_id).unwrap();
                if latest.finished_at.is_some() {
                    break latest;
                }
                assert!(Instant::now() < until);
                std::thread::sleep(Duration::from_millis(2));
            };
            assert_eq!(terminal.state, JobState::Cancelled);
            assert!(terminal.rev > cancel.rev);
            if revoke {
                assert!(terminal.duplicates.is_none());
                assert_eq!(
                    terminal.error.unwrap().code,
                    loomward_protocol::ErrorCode::PermissionDenied
                );
            } else {
                let report = terminal.duplicates.unwrap();
                assert!(report.partial && report.groups.is_empty());
                assert_eq!(report.stop.as_ref().map(|s| s.as_str()), Some("cancelled"));
                assert_eq!(report.bytes_read.get(), 0);
            }
        }
    }
}
