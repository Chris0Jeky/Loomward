//! #1106 `coverage.start` / `coverage.result`: synthetic lab folders only. Reads nothing it is not
//! granted, writes nothing, and a verdict that is not proven is `unknown`, never "covered" or
//! "not covered".
use super::*;
use crate::covers::summarise;
use loomward_operations::containment::{Coverage, Item, Report};
use loomward_operations::tree::Side;

fn root_a() -> RootId {
    RootId::new("rt_a").unwrap()
}

fn item(path: &str, size: Option<u64>, coverage: Coverage) -> Item {
    Item {
        path: path.into(),
        size,
        coverage,
    }
}

fn unknown(side: Side, reason: &'static str) -> Coverage {
    Coverage::Unknown { side, reason }
}

fn wire(report: &Report) -> CoverageReport {
    summarise(
        &JobId::new("jb_1").unwrap(),
        (&root_a(), &RootId::new("rt_b").unwrap()),
        report,
        7,
        &|_| None,
    )
    .unwrap()
}

#[test]
fn summary_counts_everything_lists_the_largest_first_and_keeps_unknown_sizes_last() {
    let mut report = Report::default();
    report.items.push(item(
        "same.bin",
        Some(10),
        Coverage::Covered { by: "x".into() },
    ));
    for (path, size) in [("small", Some(1)), ("unsized", None), ("big", Some(900))] {
        report.items.push(item(path, size, Coverage::NotCovered));
    }
    let wire = wire(&report);
    assert_eq!(wire.outcome, CoverageOutcome::Complete);
    assert!(wire.reasons.is_empty());
    let order: Vec<_> = wire.not_covered.iter().map(|i| i.path.as_str()).collect();
    assert_eq!(order, ["big", "small", "unsized"]);
    assert_eq!(
        wire.not_covered[2].bytes, None,
        "an unknown size is null, not zero"
    );
    assert_eq!(wire.totals.covered.files.get(), 1);
    assert_eq!(wire.totals.not_covered.files.get(), 3);
    assert_eq!(wire.totals.not_covered.bytes.get(), 901);
    assert_eq!(wire.totals.not_covered.size_unknown.get(), 1);
    assert!(wire.note.contains("not permission to delete"));
    assert!(!wire.note.to_lowercase().contains("safe to delete"));
}

#[test]
fn any_unknown_makes_the_result_partial_with_its_reasons_and_never_a_clean_verdict() {
    let mut report = Report::default();
    for (path, coverage) in [
        ("a", unknown(Side::Destination, "byte_budget")),
        ("b", unknown(Side::Source, "time_budget")),
        ("c", unknown(Side::Destination, "changed_during_compare")),
        ("d", unknown(Side::Source, "sharing_violation")),
        ("e", unknown(Side::Destination, "absence_unproven")),
    ] {
        report.items.push(item(path, Some(5), coverage));
    }
    let wire = wire(&report);
    assert_eq!(wire.outcome, CoverageOutcome::Partial);
    assert_eq!(
        wire.reasons.iter().copied().collect::<Vec<_>>(),
        [
            CoverageStop::Budget,
            CoverageStop::Refused,
            CoverageStop::ChangedDuringRead
        ]
    );
    assert_eq!(wire.totals.unknown.files.get(), 5);
    let sides: Vec<_> = wire.unknown.iter().map(|u| u.side).collect();
    assert_eq!(
        sides,
        [
            CoverageSide::B,
            CoverageSide::A,
            CoverageSide::B,
            CoverageSide::A,
            CoverageSide::B
        ]
    );
    // Equal sizes tie-break by path.
    assert_eq!(wire.unknown[0].reason.as_str(), "byte_budget");
    assert_eq!(wire.unknown[4].reason.as_str(), "absence_unproven");
}

#[test]
fn lists_are_capped_and_say_so_while_totals_count_everything() {
    let mut report = Report::default();
    for n in 0..205u64 {
        report
            .items
            .push(item(&format!("gone{n}"), Some(n + 1), Coverage::NotCovered));
        report.items.push(item(
            &format!("odd{n}"),
            Some(1),
            unknown(Side::Source, "io"),
        ));
    }
    let wire = wire(&report);
    assert_eq!(wire.not_covered.len(), 200);
    assert!(wire.not_covered_truncated && wire.unknown_truncated);
    assert_eq!(wire.not_covered[0].path.as_str(), "gone204");
    assert_eq!(wire.totals.not_covered.files.get(), 205);
    assert_eq!(wire.totals.unknown.files.get(), 205);
    // The A folder itself, when it could not be read at all, is `.`.
    let root = Report {
        items: vec![item("", None, unknown(Side::Source, "root_not_observed"))],
    };
    assert_eq!(self::wire(&root).unknown[0].path.as_str(), ".");
}

#[test]
fn a_folder_inside_the_other_is_an_overlap_in_either_direction() {
    let (outer, inner) = (Path::new(r"D:\Backups"), Path::new(r"D:\Backups\2019"));
    assert!(crate::paths::overlaps_stored(outer, inner));
    assert!(crate::paths::overlaps_stored(inner, outer));
    assert!(!crate::paths::overlaps_stored(
        outer,
        Path::new(r"D:\Backups2")
    ));
}

#[test]
fn unknown_roots_are_refused_before_any_job_and_the_same_root_twice_is_never_compared() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path(), DatasetClass::Personal);
    let refused = err(call(
        &svc,
        "coverage.start",
        json!({"a_root_id":"rt_nonexistent","b_root_id":"rt_other"}),
    ));
    assert_eq!(refused.code, ErrorCode::NotFound, "{refused:?}");
    let same = err(call(
        &svc,
        "coverage.start",
        json!({"a_root_id":"rt_same","b_root_id":"rt_same"}),
    ));
    assert_eq!(same.code, ErrorCode::InvalidRequest);
    assert_eq!(reason(&same), Some("roots_must_differ"));
    let none = err(call(
        &svc,
        "coverage.result",
        json!({"job_id":"jb_nothing"}),
    ));
    assert_eq!(none.code, ErrorCode::NotFound);
    assert!(svc
        .inner
        .engine
        .job_list(&serde_json::from_value(json!({"limit":50})).unwrap())
        .unwrap()
        .jobs
        .is_empty());
}

#[test]
fn a_client_principal_reaches_neither_command() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    assert!(!crate::clients::open_to_clients(Command::CoverageStart));
    assert!(!crate::clients::open_to_clients(Command::CoverageResult));
    for client in ["ask:local", "mcp:reader"] {
        for (command, payload) in [
            (
                "coverage.start",
                json!({"a_root_id":"rt_a","b_root_id":"rt_b"}),
            ),
            ("coverage.result", json!({"job_id":"jb_1"})),
        ] {
            assert_eq!(
                err(svc.call(request(command, payload), &CallContext::client(client))).code,
                ErrorCode::PermissionDenied,
                "{client} {command}"
            );
        }
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::path::PathBuf;

    struct Pair {
        _tmp: tempfile::TempDir,
        svc: Service,
        a: String,
        b: String,
        a_path: PathBuf,
        b_path: PathBuf,
    }

    /// Two scanned synthetic lab folders, filled by `fill` before the service opens them.
    fn pair(fill: impl FnOnce(&Path, &Path)) -> Option<Pair> {
        if !native_identity_available() {
            return None;
        }
        let tmp = crate::tests::temp_root();
        let state = tmp.path().join("state");
        let (a_path, b_path) = (tmp.path().join("cover-a"), tmp.path().join("cover-b"));
        for (path, seed) in [(&a_path, "cover-a"), (&b_path, "cover-b")] {
            crate::lab::register(
                &state,
                path,
                seed,
                "S",
                "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            )
            .unwrap();
        }
        let (a_path, b_path) = (
            a_path.canonicalize().unwrap(),
            b_path.canonicalize().unwrap(),
        );
        fill(&a_path, &b_path);
        let svc = Service::open(Config {
            state_dir: state,
            dataset: DatasetClass::Synthetic,
            allow_personal: false,
            allow_identity_unverified: false,
            grant_roots: vec![a_path.clone(), b_path.clone()],
        })
        .unwrap();
        let (roots, _) = ok("roots.list", call(&svc, "roots.list", json!({})));
        let id = |suffix: &str| {
            roots["roots"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| {
                    r["display_path"]["text"]
                        .as_str()
                        .is_some_and(|t| t.ends_with(suffix))
                })
                .unwrap_or_else(|| panic!("no root ending {suffix}: {roots:?}"))["root_id"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let (a, b) = (id("cover-a"), id("cover-b"));
        for root in [&a, &b] {
            let (scan, _) = ok(
                "scan.start",
                call(&svc, "scan.start", json!({"root_id":root,"mode":"full"})),
            );
            let scan: JobResult = serde_json::from_value(json!(scan)).unwrap();
            assert_eq!(finished(&svc, &scan.job.job_id).state, JobState::Completed);
        }
        crate::tests::f3_wait_live(&svc);
        // Later changes reach the catalogue only through an explicit rescan.
        svc.inner.freshness.stop();
        Some(Pair {
            _tmp: tmp,
            svc,
            a,
            b,
            a_path,
            b_path,
        })
    }

    fn finished(svc: &Service, id: &JobId) -> Job {
        let until = Instant::now() + Duration::from_secs(20);
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

    fn start(p: &Pair, extra: Value) -> Job {
        let mut payload = json!({"a_root_id":p.a,"b_root_id":p.b});
        for (key, value) in extra.as_object().unwrap() {
            payload[key] = value.clone();
        }
        let (started, _) = ok("coverage.start", call(&p.svc, "coverage.start", payload));
        let started: JobResult = serde_json::from_value(json!(started)).unwrap();
        assert_eq!(started.job.kind, JobKind::Coverage);
        assert_eq!(
            started.job.root_id.as_ref().map(RootId::as_str),
            Some(p.a.as_str())
        );
        started.job
    }

    fn report(p: &Pair, job: &Job) -> Value {
        json!(
            ok(
                "coverage.result",
                call(&p.svc, "coverage.result", json!({"job_id":job.job_id}))
            )
            .0
        )
    }

    fn put(dir: &Path, name: &str, bytes: &[u8]) {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn names(list: &Value) -> Vec<String> {
        list.as_array()
            .unwrap()
            .iter()
            .map(|i| i["path"].as_str().unwrap().replace('\\', "/"))
            .collect()
    }

    #[test]
    fn renamed_and_moved_files_are_covered_by_content_and_nothing_is_written() {
        let Some(p) = pair(|a, b| {
            put(a, "photos/one.jpg", b"first picture");
            put(a, "two.txt", b"second file, longer");
            put(b, "archive/renamed-picture.jpg", b"first picture");
            put(b, "archive/deep/two-copy.txt", b"second file, longer");
            put(b, "unrelated.bin", b"something else entirely");
        }) else {
            return;
        };
        let before: Vec<_> = [
            p.a_path.join("two.txt"),
            p.b_path.join("unrelated.bin"),
            p.a_path.join("photos/one.jpg"),
        ]
        .iter()
        .map(|f| {
            (
                std::fs::read(f).unwrap(),
                f.metadata().unwrap().modified().unwrap(),
            )
        })
        .collect();
        let started = start(&p, json!({}));
        let job = finished(&p.svc, &started.job_id);
        assert_eq!(job.state, JobState::Completed, "{:?}", job.error);
        assert_eq!(job.coverage, Some(CoverageState::Complete));
        let r = report(&p, &job);
        assert_eq!(r["outcome"], "complete");
        assert_eq!(r["reasons"], json!([]));
        assert_eq!(r["totals"]["covered"]["files"], 2);
        assert_eq!(r["totals"]["covered"]["bytes"], "32");
        assert_eq!(r["totals"]["not_covered"]["files"], 0);
        assert_eq!(r["totals"]["unknown"]["files"], 0);
        assert_eq!(r["not_covered"], json!([]));
        assert!(r.get("covered").is_none(), "covered files are counts only");
        assert!(r["bytes_read"].as_str().unwrap().parse::<u64>().unwrap() > 0);
        assert_eq!(r["a_root_id"], p.a);
        assert_eq!(r["b_root_id"], p.b);
        assert!(r["note"]
            .as_str()
            .unwrap()
            .contains("not permission to delete"));
        let after: Vec<_> = [
            p.a_path.join("two.txt"),
            p.b_path.join("unrelated.bin"),
            p.a_path.join("photos/one.jpg"),
        ]
        .iter()
        .map(|f| {
            (
                std::fs::read(f).unwrap(),
                f.metadata().unwrap().modified().unwrap(),
            )
        })
        .collect();
        assert_eq!(before, after, "covers reads; it never writes");
    }

    #[test]
    fn an_uncovered_file_is_listed_largest_first_with_a_node_id_only_where_the_catalogue_has_it() {
        let Some(p) = pair(|a, b| {
            put(a, "kept.txt", b"present in both");
            // Same size as a B file but different bytes: hashed, then not covered.
            put(a, "docs/changed.txt", b"AAAAAAAAAA");
            // No B file of this size at all: not covered without a single content read.
            put(a, "huge-unique.bin", &vec![7u8; 5000]);
            put(b, "moved/kept-elsewhere.txt", b"present in both");
            put(b, "other.txt", b"BBBBBBBBBB");
        }) else {
            return;
        };
        // Created after the scan: real, but not in the catalogue yet.
        put(&p.a_path, "late.bin", &vec![9u8; 3000]);
        let job = finished(&p.svc, &start(&p, json!({})).job_id);
        assert_eq!(job.state, JobState::Completed, "{:?}", job.error);
        let r = report(&p, &job);
        assert_eq!(
            r["outcome"], "complete",
            "not covered is a complete verdict: {r}"
        );
        assert_eq!(r["totals"]["covered"]["files"], 1);
        assert_eq!(r["totals"]["not_covered"]["files"], 3);
        assert_eq!(
            names(&r["not_covered"]),
            ["huge-unique.bin", "late.bin", "docs/changed.txt"]
        );
        assert_eq!(r["not_covered"][0]["bytes"], "5000");
        assert!(r["not_covered"][0]["node_id"].is_string());
        assert!(r["not_covered"][2]["node_id"].is_string());
        assert!(
            r["not_covered"][1]["node_id"].is_null(),
            "the catalogue has not seen late.bin: {r}"
        );
        // The sealed id opens the catalogue row Explorer shows.
        let (detail, _) = ok(
            "node.inspect",
            call(
                &p.svc,
                "node.inspect",
                json!({"node_id": r["not_covered"][0]["node_id"]}),
            ),
        );
        assert_eq!(detail["row"]["name"], "huge-unique.bin");
        // Replays from the retained report until it expires.
        assert_eq!(report(&p, &job), r);
    }

    #[test]
    fn a_spent_budget_is_partial_and_never_a_clean_not_covered_verdict() {
        let Some(p) = pair(|a, b| {
            put(a, "needs-reading.txt", b"identical bytes");
            put(b, "elsewhere/copy.txt", b"identical bytes");
        }) else {
            return;
        };
        let job = finished(
            &p.svc,
            &start(&p, json!({"budget_bytes":"0","budget_secs":30})).job_id,
        );
        assert_eq!(job.state, JobState::Completed, "{:?}", job.error);
        assert_eq!(job.coverage, Some(CoverageState::Partial));
        let r = report(&p, &job);
        assert_eq!(r["outcome"], "partial");
        assert!(r["reasons"].as_array().unwrap().contains(&json!("budget")));
        assert_eq!(r["bytes_read"], "0");
        assert_eq!(r["totals"]["covered"]["files"], 0);
        assert_eq!(r["totals"]["not_covered"]["files"], 0, "{r}");
        assert_eq!(r["totals"]["unknown"]["files"], 1);
        assert_eq!(r["unknown"][0]["path"], "needs-reading.txt");
        assert!(r["unknown"][0]["reason"].is_string());
    }

    #[test]
    fn cancelling_ends_the_job_cancelled_and_keeps_no_result() {
        let Some(p) = pair(|a, b| {
            put(a, "one.txt", b"same");
            put(b, "two.txt", b"same");
        }) else {
            return;
        };
        let hooks = &p.svc.inner.hooks;
        hooks
            .closed
            .lock()
            .unwrap()
            .push((Command::CoverageStart, "reading"));
        let started = start(&p, json!({}));
        hooks.until("the job reading", |h| {
            h.arrived
                .lock()
                .unwrap()
                .contains(&(Command::CoverageStart, "reading"))
        });
        // One at a time: a second start is busy and names the running job.
        let busy = err(call(
            &p.svc,
            "coverage.start",
            json!({"a_root_id":p.b,"b_root_id":p.a}),
        ));
        assert_eq!(busy.code, ErrorCode::Busy);
        assert_eq!(
            busy.detail.as_ref().unwrap()["job_id"],
            json!(started.job_id)
        );
        let (cancelled, _) = ok(
            "jobs.cancel",
            call(&p.svc, "jobs.cancel", json!({"job_id":started.job_id})),
        );
        assert_eq!(cancelled["job"]["state"], "cancel_requested");
        hooks.open(Command::CoverageStart, "reading");
        let job = finished(&p.svc, &started.job_id);
        assert_eq!(job.state, JobState::Cancelled);
        assert_eq!(job.coverage, Some(CoverageState::Cancelled));
        let none = err(call(
            &p.svc,
            "coverage.result",
            json!({"job_id":started.job_id}),
        ));
        assert_eq!(none.code, ErrorCode::NotFound);
        // The slot is free again and a fresh run completes.
        // The cancelled read frees the slot when it ends, a moment after the job did.
        let until = Instant::now() + Duration::from_secs(20);
        let again = loop {
            let r = call(
                &p.svc,
                "coverage.start",
                json!({"a_root_id":p.a,"b_root_id":p.b}),
            );
            if let ResponseEnvelope::Ok(_) = r {
                let r: JobResult =
                    serde_json::from_value(json!(ok("coverage.start", r).0)).unwrap();
                break finished(&p.svc, &r.job.job_id);
            }
            assert!(Instant::now() < until, "the slot never freed");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(again.state, JobState::Completed, "{:?}", again.error);
    }

    /// The orphan path: a cancel lands while the engine read is still running. The job is terminal
    /// at once, but the read goes on, so a new check is refused until it ends, and the orphan keeps
    /// no report.
    #[test]
    fn a_cancelled_read_still_running_keeps_the_slot_busy_and_stores_nothing() {
        let Some(p) = pair(|a, b| {
            put(a, "one.txt", b"same");
            put(b, "two.txt", b"same");
        }) else {
            return;
        };
        let hooks = &p.svc.inner.hooks;
        let held = (Command::CoverageStart, "orphan_read");
        hooks.closed.lock().unwrap().push(held);
        let started = start(&p, json!({}));
        hooks.until("the read thread holding", |h| {
            h.arrived.lock().unwrap().contains(&held)
        });
        ok(
            "jobs.cancel",
            call(&p.svc, "jobs.cancel", json!({"job_id":started.job_id})),
        );
        // The job is terminal through the timeout branch while the read has not even begun.
        let job = finished(&p.svc, &started.job_id);
        assert_eq!(job.state, JobState::Cancelled);
        let busy = err(call(
            &p.svc,
            "coverage.start",
            json!({"a_root_id":p.a,"b_root_id":p.b}),
        ));
        assert_eq!(busy.code, ErrorCode::Busy, "{busy:?}");
        assert_eq!(reason(&busy), Some("read_still_finishing"));
        assert!(busy.retryable);
        hooks.open(held.0, held.1);
        let until = Instant::now() + Duration::from_secs(20);
        let second = loop {
            let r = call(
                &p.svc,
                "coverage.start",
                json!({"a_root_id":p.a,"b_root_id":p.b}),
            );
            match r {
                ResponseEnvelope::Ok(_) => break r,
                ResponseEnvelope::Err(ref e) => {
                    assert_eq!(e.error.code, ErrorCode::Busy);
                    assert!(Instant::now() < until, "the orphan read never ended");
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        };
        let second: JobResult =
            serde_json::from_value(json!(ok("coverage.start", second).0)).unwrap();
        assert_eq!(
            finished(&p.svc, &second.job.job_id).state,
            JobState::Completed
        );
        // The orphan stored nothing; only the second run has a report.
        assert_eq!(
            err(call(
                &p.svc,
                "coverage.result",
                json!({"job_id":started.job_id})
            ))
            .code,
            ErrorCode::NotFound
        );
        assert!(p.svc.inner.covers.get(&started.job_id).is_none());
        assert!(p.svc.inner.covers.get(&second.job.job_id).is_some());
    }

    #[test]
    fn the_same_root_is_refused_and_a_revoked_root_ends_the_result() {
        let Some(p) = pair(|a, b| {
            put(a, "one.txt", b"same");
            put(b, "two.txt", b"same");
        }) else {
            return;
        };
        let same = err(call(
            &p.svc,
            "coverage.start",
            json!({"a_root_id":p.a,"b_root_id":p.a}),
        ));
        assert_eq!(same.code, ErrorCode::InvalidRequest);
        assert_eq!(reason(&same), Some("roots_must_differ"));
        // A start replays under its request id without a second job.
        let req = request("coverage.start", json!({"a_root_id":p.a,"b_root_id":p.b}));
        let first = p.svc.call(req.clone(), &CallContext::http());
        assert_eq!(p.svc.call(req, &CallContext::http()), first);
        let job: JobResult = serde_json::from_value(json!(ok("coverage.start", first).0)).unwrap();
        let job = finished(&p.svc, &job.job.job_id);
        assert_eq!(job.state, JobState::Completed, "{:?}", job.error);
        assert_eq!(report(&p, &job)["totals"]["covered"]["files"], 1);
        ok(
            "roots.revoke",
            call(
                &p.svc,
                "roots.revoke",
                json!({"root_id":p.b,"purge_catalog":false}),
            ),
        );
        let gone = err(call(
            &p.svc,
            "coverage.result",
            json!({"job_id":job.job_id}),
        ));
        assert_eq!(
            gone.code,
            ErrorCode::NotFound,
            "a revoked folder's names are not kept"
        );
        let refused = err(call(
            &p.svc,
            "coverage.start",
            json!({"a_root_id":p.a,"b_root_id":p.b}),
        ));
        assert_eq!(refused.code, ErrorCode::PermissionDenied);
    }
}
