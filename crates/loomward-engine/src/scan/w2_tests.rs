use super::*;

#[test]
fn refs_legacy_directory_ids_are_not_deduplication_or_reopen_keys() {
    use loomward_windows::enumerate::validate_identity;

    let legacy = Some(FileIdObs::Id64(7));
    for listed in [None, legacy] {
        let (id, basis) = validate_identity(listed, legacy, true).unwrap();
        assert_eq!(basis, IdBasis::NonUnique);
        let identity = OpenedIdentity {
            id,
            basis,
            volume_guid: None,
            volume_serial: Some(1),
            attributes: 0x10,
            reparse_tag: None,
        };
        assert_eq!(directory_key(identity), None);
        assert_eq!(reopen_key(identity), None);
    }
    let extended = FileIdObs::Id128([7; 16]);
    let (id, basis) = validate_identity(Some(extended), Some(extended), true).unwrap();
    let identity = OpenedIdentity {
        id,
        basis,
        volume_guid: None,
        volume_serial: Some(1),
        attributes: 0x10,
        reparse_tag: None,
    };
    assert_eq!(directory_key(identity), Some((Some(1), extended)));
    assert_eq!(reopen_key(identity), Some((1, extended)));
}

#[cfg(windows)]
mod native {
    use super::*;
    use crate::{scan::MemorySink, Engine, EngineConfig};
    use loomward_protocol::{CoverageState, DatasetClass, Job, JobId, JobState};

    type FinishPause = (SyncSender<()>, Receiver<()>);

    struct Sink {
        memory: MemorySink,
        watched: AtomicBool,
        finish_pause: Mutex<Option<FinishPause>>,
        publication_pause: Mutex<Option<FinishPause>>,
    }

    impl Sink {
        fn new(watched: bool) -> Self {
            Self {
                memory: MemorySink::default(),
                watched: AtomicBool::new(watched),
                finish_pause: Mutex::new(None),
                publication_pause: Mutex::new(None),
            }
        }
    }

    impl ScanSink for Sink {
        fn watch_dirty(&self, root: &RootId) -> EngineResult<()> {
            if self.watched.load(Ordering::Acquire) {
                self.memory.watch_dirty(root)
            } else {
                Err(EngineError::unavailable(crate::Component::Scan))
            }
        }
        fn recover_interrupted(&self) -> EngineResult<()> {
            self.memory.recover_interrupted()
        }
        fn begin_run(&self, root: &RootId, run: u64, scope: &RunScope) -> EngineResult<()> {
            self.memory.begin_run(root, run, scope)?;
            if let Some(pause) = self.publication_pause.lock().unwrap().take() {
                // begin_run executes on the actual native job worker, as does this hook.
                let mut pause = Some(pause);
                BEFORE_PUBLISH.with_borrow_mut(|hook| {
                    assert!(hook.is_none());
                    *hook = Some(Box::new(move |point| {
                        if point == "run_scan_finish" {
                            let (reached, resume) = pause.take().unwrap();
                            reached.send(()).unwrap();
                            resume.recv_timeout(Duration::from_secs(10)).unwrap();
                        }
                    }));
                });
            }
            Ok(())
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
            BEFORE_PUBLISH.with_borrow_mut(|hook| *hook = None);
            self.memory.finish_run(root, run, scope, complete)?;
            if complete {
                if let Some((finished, resume)) = self.finish_pause.lock().unwrap().take() {
                    finished.send(()).unwrap();
                    resume.recv_timeout(Duration::from_secs(10)).unwrap();
                }
            }
            Ok(())
        }
        fn fence_root(&self, root: &RootId) -> EngineResult<()> {
            self.memory.fence_root(root)
        }
    }

    fn fixture() -> Option<(tempfile::TempDir, GrantedRoot, Arc<Engine>)> {
        use loomward_windows::enumerate::NativeSource;

        if loomward_windows::enumerate::skip_elevated_test() {
            assert_eq!(
                NativeSource::default()
                    .open_root(std::path::Path::new("synthetic"))
                    .unwrap_err(),
                SourceError::Refused
            );
            return None;
        }
        let temp =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let (_, identity) = NativeSource::default().open_root(temp.path()).unwrap();
        let root = GrantedRoot::new(
            RootId::new("rt_w2").unwrap(),
            temp.path().to_path_buf(),
            DatasetClass::Synthetic,
            identity,
        );
        let engine = Arc::new(
            // A real state directory: an unidentifiable state exclusion refuses the scan.
            Engine::open(EngineConfig::new(
                std::env::temp_dir(),
                DatasetClass::Synthetic,
            ))
            .unwrap(),
        );
        Some((temp, root, engine))
    }

    fn wait_for(engine: &Engine, id: &JobId, predicate: impl Fn(&Job) -> bool) -> Job {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let job = engine.job_status(id).unwrap();
            if predicate(&job) {
                return job;
            }
            assert!(Instant::now() < deadline, "unexpected job: {job:?}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    struct ReleasePause(SyncSender<()>);

    impl Drop for ReleasePause {
        fn drop(&mut self) {
            // A full channel already contains the release; never block while unwinding.
            let _ = self.0.try_send(());
        }
    }

    fn revoke_plain_while_paused(
        engine: &Engine,
        root: &GrantedRoot,
        resume: SyncSender<()>,
    ) -> Vec<Job> {
        std::thread::scope(|scope| {
            let release = ReleasePause(resume);
            let (returned, response) = mpsc::channel();
            let revoking = scope.spawn(move || {
                returned
                    .send(engine.scan_cancel_root(root.key(), root.root_id()))
                    .unwrap();
            });
            let early = response.recv_timeout(Duration::from_secs(2));
            // Release before joining or asserting, including a blocked-cancellation regression.
            let released = release.0.send(());
            let joined = revoking.join();
            released.unwrap();
            joined.unwrap();
            early
                .expect("root revocation waited for the paused plain scan")
                .unwrap()
        })
    }

    fn assert_revoked_root_is_fenced(engine: &Engine, root: &GrantedRoot, sink: &Sink) {
        engine
            .scan_writer_fence(root.key(), root.root_id())
            .unwrap();
        assert!(matches!(
            engine.scan_start(root, None),
            Err(EngineError::PermissionDenied { .. })
        ));
        assert!(matches!(
            sink.begin_run(root.root_id(), 999, &RunScope::FullRoot),
            Err(EngineError::PermissionDenied { .. })
        ));
        assert!(!sink.memory.sweep_allowed(root.root_id()));
    }

    #[test]
    fn revocation_after_scan_finish_cannot_publish_complete() {
        let Some((_temp, root, engine)) = fixture() else {
            return;
        };
        // Watched scans retain their separate cancellation boundary after sink completion.
        let sink = Arc::new(Sink::new(true));
        let (finished, reached_finish) = mpsc::sync_channel(1);
        let (resume, paused) = mpsc::sync_channel(1);
        *sink.finish_pause.lock().unwrap() = Some((finished, paused));
        engine.set_scan_sink(sink.clone()).unwrap();
        let job = engine.scan_start(&root, None).unwrap();
        reached_finish
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        std::thread::scope(|scope| {
            // This guard drops before the scope joins a revoker waiting for watch teardown.
            let release = ReleasePause(resume);
            let revoking =
                scope.spawn(|| engine.scan_cancel_root(root.key(), root.root_id()).unwrap());
            wait_for(&engine, &job.job_id, |job| {
                job.state == JobState::CancelRequested
            });
            release.0.send(()).unwrap();
            revoking.join().unwrap();
        });
        let terminal = wait_for(&engine, &job.job_id, |job| job.state == JobState::Cancelled);
        assert_eq!(terminal.coverage, Some(CoverageState::Cancelled));
        assert_revoked_root_is_fenced(&engine, &root, &sink);
    }

    #[test]
    fn revocation_before_plain_scan_publication_cannot_publish_complete() {
        let Some((_temp, root, engine)) = fixture() else {
            return;
        };
        let sink = Arc::new(Sink::new(false));
        let (reached, at_publication) = mpsc::sync_channel(1);
        let (resume, paused) = mpsc::sync_channel(1);
        *sink.publication_pause.lock().unwrap() = Some((reached, paused));
        engine.set_scan_sink(sink.clone()).unwrap();
        let job = engine.scan_start(&root, None).unwrap();
        at_publication
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        let revoked = revoke_plain_while_paused(&engine, &root, resume);
        assert_eq!(revoked.len(), 1);
        assert_eq!(revoked[0].job_id, job.job_id);
        assert_eq!(revoked[0].state, JobState::CancelRequested);
        let terminal = wait_for(&engine, &job.job_id, |job| job.state == JobState::Cancelled);
        assert_eq!(terminal.coverage, Some(CoverageState::Cancelled));
        assert!(!sink.memory.sweep_allowed(root.root_id()));
        assert_revoked_root_is_fenced(&engine, &root, &sink);
    }

    #[test]
    fn revocation_after_plain_scan_finish_preserves_admitted_completion() {
        let Some((_temp, root, engine)) = fixture() else {
            return;
        };
        let sink = Arc::new(Sink::new(false));
        let (finished, reached_finish) = mpsc::sync_channel(1);
        let (resume, paused) = mpsc::sync_channel(1);
        *sink.finish_pause.lock().unwrap() = Some((finished, paused));
        engine.set_scan_sink(sink.clone()).unwrap();
        let job = engine.scan_start(&root, None).unwrap();
        reached_finish
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        // Admission and MemorySink::finish_run have already succeeded. Root revocation may not
        // acknowledge cancellation of that historical result, but must fence all future work.
        let published = sink.memory.sweep_allowed(root.root_id());
        let revoked = revoke_plain_while_paused(&engine, &root, resume);
        assert!(
            published,
            "the paused plain scan had not published completion"
        );
        assert_eq!(revoked.len(), 1);
        assert_eq!(revoked[0].job_id, job.job_id);
        assert_eq!(revoked[0].state, JobState::Running);
        let terminal = wait_for(&engine, &job.job_id, |job| job.state == JobState::Completed);
        assert_eq!(terminal.coverage, Some(CoverageState::Complete));
        assert!(terminal.error.is_none());
        assert_revoked_root_is_fenced(&engine, &root, &sink);
    }

    #[test]
    fn unwatched_rescan_removes_prior_watch() {
        let Some((_temp, root, engine)) = fixture() else {
            return;
        };
        let sink = Arc::new(Sink::new(true));
        engine.set_scan_sink(sink.clone()).unwrap();
        let first = engine.scan_start(&root, None).unwrap();
        wait_for(&engine, &first.job_id, |job| {
            job.state == JobState::Completed
        });
        assert!(engine
            .scan_watches
            .lock()
            .unwrap()
            .contains_key(root.root_id()));
        sink.watched.store(false, Ordering::Release);
        let second = engine.scan_start(&root, None).unwrap();
        let terminal = wait_for(&engine, &second.job_id, |job| {
            job.state == JobState::Completed
        });
        assert_eq!(terminal.coverage, Some(CoverageState::Complete));
        assert!(engine.scan_watches.lock().unwrap().is_empty());
    }
}
