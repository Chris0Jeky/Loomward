use super::*;
use loomward_protocol::{
    CallContext, DatasetClass, ErrorCode, RequestEnvelope, ResponseEnvelope, ViewService,
};
use loomward_service::{Config, Service};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, time::Duration};
use windows_sys::Win32::System::{
    ProcessStatus::PROCESS_MEMORY_COUNTERS_EX, Threading::GetProcessHandleCount,
};

const STAGES: &[&str] = &[
    "grant",
    "scan",
    "tree.slice",
    "search.query",
    "watch",
    "refresh",
    "revoke",
    "forget",
    "restart",
];

fn check_root(input: &str) -> io::Result<()> {
    real_root(input)?;
    let path = Path::new(input);
    if !path
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|s| s.eq_ignore_ascii_case("lab"))
    {
        return Err(invalid(
            "soak root must be a direct child of a lab directory",
        ));
    }
    // Reuse the scale lab's leaf grammar, including its Windows device-name refusal.
    root_name(&format!(
        r"G:\loomward-lab\scale\{}",
        path.file_name().unwrap().to_string_lossy()
    ))?;
    Ok(())
}

pub(super) fn cli(args: &[String]) -> io::Result<()> {
    let mut options = BTreeMap::new();
    if args.len() != 6 {
        return Err(invalid(
            "soak --hours N --root C:\\...\\lab\\NAME --out C:\\...\\run.jsonl",
        ));
    }
    for pair in args.chunks_exact(2) {
        if !["--hours", "--root", "--out"].contains(&pair[0].as_str())
            || options.insert(pair[0].as_str(), pair[1].as_str()).is_some()
        {
            return Err(invalid("unknown or duplicate soak option"));
        }
    }
    let hours: f64 = options["--hours"]
        .parse()
        .map_err(|_| invalid("invalid hours"))?;
    if !hours.is_finite() || !(0.0..=168.0).contains(&hours) || hours == 0.0 {
        return Err(invalid("hours must be finite, positive and <=168"));
    }
    let out = Path::new(options["--out"]);
    if !out.is_absolute() {
        return Err(invalid("out must be absolute and outside Git"));
    }
    let parent =
        loomward_service::paths::prospective_dir(out.parent().unwrap()).map_err(invalid)?;
    if parent.ancestors().any(|p| p.join(".git").exists()) {
        return Err(invalid("out must be outside Git"));
    }
    let root_parent = loomward_service::paths::prospective_dir(
        Path::new(options["--root"])
            .parent()
            .ok_or_else(|| invalid("root parent missing"))?,
    )
    .map_err(invalid)?;
    if root_parent.ancestors().any(|p| p.join(".git").exists()) {
        return Err(invalid("root must be outside Git"));
    }
    run(
        Path::new(options["--root"]),
        out,
        Duration::from_secs_f64(hours * 3600.0),
        None,
    )
}

fn open(state: &Path, root: Option<&Path>) -> io::Result<Service> {
    Service::open(Config {
        state_dir: state.to_owned(),
        dataset: DatasetClass::Synthetic,
        allow_personal: false,
        allow_identity_unverified: false,
        grant_roots: root.into_iter().map(Path::to_owned).collect(),
    })
    .map_err(io::Error::other)
}

fn call(service: &Service, command: &str, payload: Value) -> io::Result<Value> {
    call_with(command, payload, Duration::from_secs(30), |request| {
        service.call(request, &CallContext::http())
    })
}

fn call_with(
    command: &str,
    payload: Value,
    timeout: Duration,
    mut invoke: impl FnMut(RequestEnvelope) -> ResponseEnvelope,
) -> io::Result<Value> {
    use std::sync::atomic::AtomicU64;
    static REQUEST: AtomicU64 = AtomicU64::new(1);
    let deadline = Instant::now() + timeout;
    loop {
        let request = serde_json::from_value(json!({
            "protocol": "loomward/3", "request_id": format!("soak-{}", REQUEST.fetch_add(1, Ordering::Relaxed)),
            "command": command, "payload": payload,
            "deadline_ms": deadline.saturating_duration_since(Instant::now()).as_millis().max(1),
        }))?;
        match invoke(request) {
            ResponseEnvelope::Ok(reply) => return Ok(json!(reply.result)),
            ResponseEnvelope::Err(reply) => {
                let error = format!("{command}: {:?}", reply.error);
                // Aggregate repair can overlap a completed scan. Only this read may wait for it.
                if command != "tree.slice"
                    || reply.error.code != ErrorCode::PartialCoverage
                    || !reply.error.retryable
                {
                    return Err(io::Error::other(error));
                }
                thread::sleep(
                    Duration::from_millis(50)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
                if Instant::now() >= deadline {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, error));
                }
            }
        }
    }
}

fn scan(service: &Service, root: &Value, mode: &str, expected: Totals) -> io::Result<()> {
    let job = call(
        service,
        "scan.start",
        json!({"root_id": root, "mode": mode}),
    )?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = call(service, "jobs.get", json!({"job_id": job["job"]["job_id"]}))?;
        match status["job"]["state"].as_str() {
            Some("completed") => {
                if status["job"]["coverage"] != "complete" {
                    return Err(io::Error::other("scan coverage incomplete"));
                }
                break;
            }
            Some("failed" | "cancelled") => {
                return Err(io::Error::other(format!("scan: {status}")))
            }
            _ if Instant::now() >= deadline => return Err(io::Error::other("scan timeout")),
            _ => thread::sleep(Duration::from_millis(10)),
        }
    }
    let slice = slice(service, root)?;
    let node = &slice["nodes"][0];
    if node["files"] != expected.files
        || node["dirs"] != expected.directories
        || node["logical_bytes"]
            .as_str()
            .and_then(|s| s.parse::<u64>().ok())
            != Some(expected.logical_bytes)
        || slice["aggregate_state"] != "consistent"
    {
        return Err(io::Error::other("scan totals disagree with lab manifest"));
    }
    Ok(())
}

fn slice(service: &Service, root: &Value) -> io::Result<Value> {
    call(
        service,
        "tree.slice",
        json!({"anchor": {"kind": "root", "root_id": root},
        "depth": 2, "max_nodes": 64, "min_share": 0.0, "basis": "logical", "include_files": true}),
    )
}

fn rows(state: &Path) -> io::Result<BTreeMap<String, i64>> {
    let dir = loomward_catalog::sqlite_dir(state);
    let mut result = BTreeMap::new();
    for (db, tables) in [
        (
            "catalog.db",
            &["root", "dir", "file", "scan_run", "stage_entry"][..],
        ),
        ("state.db", &["root_grant"][..]),
    ] {
        let conn = rusqlite::Connection::open_with_flags(
            dir.join(db),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(io::Error::other)?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(io::Error::other)?;
        for table in tables {
            let filter = if *table == "root_grant" {
                " WHERE NOT (display_path='forgotten' AND volume_key='forgotten' AND root_file_id=zeroblob(8) AND lab_root_id IS NULL AND state='revoked' AND id IN (SELECT grant_id FROM root_forgotten))"
            } else {
                ""
            };
            let n = conn
                .query_row(
                    &format!("SELECT count(*) FROM {table}{filter}"),
                    [],
                    |row| row.get(0),
                )
                .map_err(io::Error::other)?;
            result.insert(
                if *table == "root_grant" {
                    "identifiable_grant"
                } else {
                    table
                }
                .to_string(),
                n,
            );
        }
    }
    Ok(result)
}

fn sample(state: &Path) -> io::Result<Value> {
    let mut handles = 0;
    let mut memory: PROCESS_MEMORY_COUNTERS_EX = unsafe { zeroed() };
    memory.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    if unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut handles) } == 0
        || unsafe {
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                (&mut memory as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
                memory.cb,
            )
        } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut sizes = BTreeMap::new();
    for name in ["catalog.db", "state.db", "catalog.db-wal", "state.db-wal"] {
        let bytes = match fs::metadata(loomward_catalog::sqlite_dir(state).join(name)) {
            Ok(m) => m.len(),
            Err(e) if e.kind() == io::ErrorKind::NotFound && name.ends_with("-wal") => 0,
            Err(e) => return Err(e),
        };
        sizes.insert(name, bytes);
    }
    let conn = rusqlite::Connection::open_with_flags(
        loomward_catalog::sqlite_dir(state).join("catalog.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(io::Error::other)?;
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(io::Error::other)?;
    let logical_bytes: i64 = conn
        .query_row("SELECT coalesce(sum(logical),0) FROM file", [], |r| {
            r.get(0)
        })
        .map_err(io::Error::other)?;
    Ok(
        json!({"handles": handles, "rss_bytes": memory.WorkingSetSize,
        "private_bytes": memory.PrivateUsage, "sizes": sizes, "rows": rows(state)?, "logical_bytes": logical_bytes}),
    )
}

fn append(out: &mut File, record: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *out, record)?;
    out.write_all(b"\n")?;
    out.flush()
}

// Wall-clock milliseconds since the Unix epoch. Unlike Instant, SystemTime keeps counting
// through a suspend, so soak_summary.py can tell a sleep from a stall.
fn unix_ms() -> io::Result<u64> {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?;
    u64::try_from(since_epoch.as_millis()).map_err(io::Error::other)
}

fn operation(
    out: &mut File,
    state: &Path,
    start: Instant,
    cycle: u64,
    name: &str,
    action: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let before = Instant::now();
    let result = action();
    let latency_ms = before.elapsed().as_secs_f64() * 1000.0;
    let measured = sample(state);
    let mut record = measured.as_ref().cloned().unwrap_or_else(|_| json!({}));
    let error = result
        .as_ref()
        .err()
        .or_else(|| measured.as_ref().err())
        .map(ToString::to_string);
    record["kind"] = json!("sample");
    record["cycle"] = json!(cycle);
    record["operation"] = json!(name);
    record["elapsed_s"] = json!(start.elapsed().as_secs_f64());
    record["unix_ms"] = json!(unix_ms()?);
    record["latency_ms"] = json!(latency_ms);
    record["error"] = json!(error);
    append(out, &record)?;
    result?;
    measured.map(|_| ())
}

fn run(root: &Path, out: &Path, duration: Duration, cycles: Option<u64>) -> io::Result<()> {
    require_non_elevated()?;
    let input = root
        .to_str()
        .ok_or_else(|| invalid("non Unicode root"))?
        .trim_start_matches(r"\\?\");
    check_root(input)?;
    let root = loomward_service::paths::prospective_dir(root).map_err(invalid)?;
    check_root(root.to_str().unwrap().trim_start_matches(r"\\?\"))?;
    let out_parent = loomward_service::paths::prospective_dir(
        out.parent().ok_or_else(|| invalid("out parent missing"))?,
    )
    .map_err(invalid)?;
    if out_parent.starts_with(&root) || root.starts_with(out_parent.join(out.file_name().unwrap()))
    {
        return Err(invalid("out must be outside the generated root"));
    }
    // The out file is created before any root: a pre-existing out stops the run with no orphan root.
    fs::create_dir_all(&out_parent)?;
    let mut out = OpenOptions::new().write(true).create_new(true).open(out)?;
    // Pin checked ancestors for the whole run, as the existing scale generator does.
    fs::create_dir_all(root.parent().unwrap())?;
    let mut pins = Vec::new();
    for path in root
        .parent()
        .unwrap()
        .ancestors()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        pins.push(pin_directory(path)?);
    }
    fs::create_dir(&root)?; // Never accept or overwrite an existing folder.
    fs::write(root.join(".loomward-lab-marker"), MARKER)?;
    pins.push(pin_directory(&root)?);
    let scope = Scope {
        root: root.clone(),
        pins,
        marker: None,
    };
    let _busy = Busy::take(&scope)?;
    let expected = expected(128, 42, Profile::Dev)?;
    let manifest = json!({"schema_version": 1, "seed": 42, "profile": "dev", "expected": expected});
    let bytes = serde_json::to_vec_pretty(&manifest)?;
    let digest = format!("sha256:{:x}", Sha256::digest(&bytes));
    let state = root.join("state");
    let data = root.join("data");
    loomward_service::lab::register(&state, &data, "42", "soak", &digest)
        .map_err(io::Error::other)?;
    for i in 0..expected.files {
        let dir = data.join(bucket_path(
            i / Profile::Dev.bucket_size(),
            42,
            Profile::Dev,
        ));
        fs::create_dir_all(&dir)?;
        let (name, size) = file_spec(i, 42, Profile::Dev);
        make_sparse(&dir.join(name), size)?;
    }
    fs::write(root.join("manifest.json"), bytes)?;
    let start = Instant::now();
    append(
        &mut out,
        &json!({"kind": "start", "schema_version": 1, "manifest": manifest,
        "requested_seconds": duration.as_secs_f64(), "pid": std::process::id(),
        "unix_seconds": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(io::Error::other)?.as_secs(),
        "cadence_seconds": 5, "wall_clock": true, "watch_mode": "native hint then explicit service refresh"}),
    )?;
    out.sync_all()?;
    let mut service = None;
    let mut cycle = 0;
    let result = (|| -> io::Result<()> {
        while cycles.map_or(start.elapsed() < duration, |n| cycle < n) {
            cycle += 1;
            let cycle_start = Instant::now();
            let mut id = Value::Null;
            for &stage in STAGES {
                operation(&mut out, &state, start, cycle, stage, || {
                    match stage {
                        "grant" => {
                            drop(service.take());
                            service = Some(open(&state, Some(&data))?);
                            id = call(service.as_ref().unwrap(), "roots.list", json!({}))?["roots"]
                                [0]["root_id"]
                                .clone();
                            if !id.is_string() {
                                return Err(io::Error::other("missing lab grant"));
                            }
                        }
                        "scan" | "refresh" => {
                            scan(
                                service.as_ref().unwrap(),
                                &id,
                                if stage == "scan" { "full" } else { "refresh" },
                                expected,
                            )?;
                            let counts = rows(&state)?;
                            if counts["file"] != expected.files as i64
                                || counts["dir"] != expected.directories as i64 + 1
                                || counts["root"] != 1
                            {
                                return Err(io::Error::other(
                                    "catalogue rows disagree with manifest",
                                ));
                            }
                        }
                        "tree.slice" => {
                            slice(service.as_ref().unwrap(), &id)?;
                        }
                        "search.query" => {
                            let found = call(
                                service.as_ref().unwrap(),
                                "search.query",
                                json!({"root_id": id, "text": "Sample", "extension": null,
                                "min_bytes": null, "max_bytes": null, "modified_after": null, "modified_before": null,
                                "kind": "file", "limit": 10, "cursor": null}),
                            )?;
                            if found["hits"].as_array().is_none_or(Vec::is_empty) {
                                return Err(io::Error::other("search returned no generated files"));
                            }
                        }
                        "watch" => {
                            let (tx, rx) = std::sync::mpsc::sync_channel(1);
                            let watch = loomward_windows::watch::Watcher::start(
                                &data,
                                None,
                                move |changes| {
                                    let _ = tx.try_send(changes);
                                },
                            )
                            .map_err(|e| io::Error::other(format!("watch start: {e:?}")))?;
                            let probe = data.join("soak-watch-probe.bin");
                            OpenOptions::new()
                                .write(true)
                                .create_new(true)
                                .open(&probe)?
                                .write_all(&[0; 8])?;
                            rx.recv_timeout(Duration::from_secs(5))
                                .map_err(io::Error::other)?;
                            fs::remove_file(probe)?;
                            watch.checkpoint().map_err(|e| {
                                io::Error::other(format!("watch checkpoint: {e:?}"))
                            })?;
                            watch.stop();
                            if watch.failed() {
                                return Err(io::Error::other("watch lost coverage"));
                            }
                        }
                        "revoke" => {
                            call(
                                service.as_ref().unwrap(),
                                "roots.revoke",
                                json!({"root_id": id, "purge_catalog": false}),
                            )?;
                        }
                        "forget" => {
                            call(
                                service.as_ref().unwrap(),
                                "roots.forget",
                                json!({"root_id": id}),
                            )?;
                            let remaining = rows(&state)?;
                            if remaining.values().any(|&n| n != 0) {
                                return Err(io::Error::other(format!(
                                    "forget retained rows: {remaining:?}"
                                )));
                            }
                        }
                        "restart" => {
                            drop(service.take());
                            service = Some(open(&state, None)?);
                            if call(service.as_ref().unwrap(), "roots.list", json!({}))?["roots"]
                                != json!([])
                            {
                                return Err(io::Error::other(
                                    "forgotten root returned after restart",
                                ));
                            }
                        }
                        _ => unreachable!(),
                    }
                    Ok(())
                })?;
            }
            out.sync_all()?;
            if cycles.is_none() {
                thread::sleep(
                    Duration::from_secs(5)
                        .saturating_sub(cycle_start.elapsed())
                        .min(duration.saturating_sub(start.elapsed())),
                );
            }
        }
        Ok(())
    })();
    drop(service);
    let end_unix_ms = unix_ms()?;
    append(
        &mut out,
        &json!({"kind": "end", "cycles": cycle, "elapsed_s": start.elapsed().as_secs_f64(),
        "unix_ms": end_unix_ms, "success": result.is_ok(),
        "error": result.as_ref().err().map(ToString::to_string)}),
    )?;
    out.sync_all()?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use loomward_protocol::ErrorBody;

    #[test]
    fn retryable_partial_coverage_settles_before_returning_the_slice() {
        let expected = json!({"aggregate_state": "consistent", "nodes": []});
        let mut calls = 0;
        let mut request_ids = Vec::new();
        let mut deadlines = Vec::new();
        let result = call_with("tree.slice", json!({}), Duration::from_secs(1), |request| {
            calls += 1;
            assert!(!request_ids.contains(&request.request_id));
            request_ids.push(request.request_id.clone());
            deadlines.push(request.deadline_ms);
            if calls <= 2 {
                ResponseEnvelope::error(
                    Some(request.request_id),
                    ErrorBody::new(
                        ErrorCode::PartialCoverage,
                        "aggregates are being repaired",
                        true,
                    ),
                    None,
                )
            } else {
                ResponseEnvelope::ok(
                    request.request_id,
                    &expected,
                    serde_json::from_value(json!({
                        "served_at": "2026-10-18T00:00:00Z", "elapsed_ms": 0,
                        "dataset_class": "synthetic", "budget_hit": false,
                        "catalog_rev": null, "state_rev": null
                    }))
                    .unwrap(),
                )
            }
        });
        assert_eq!(result.unwrap(), expected);
        assert_eq!(calls, 3);
        assert!(
            deadlines[0] > deadlines[2],
            "retries share the original budget"
        );
    }

    #[test]
    fn partial_coverage_that_never_settles_fails_within_the_budget() {
        for timeout in [Duration::ZERO, Duration::from_millis(100)] {
            let error = call_with("tree.slice", json!({}), timeout, |request| {
                ResponseEnvelope::error(
                    Some(request.request_id),
                    ErrorBody::new(
                        ErrorCode::PartialCoverage,
                        "aggregates are being repaired",
                        true,
                    ),
                    None,
                )
            })
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
            assert!(error.to_string().contains("PartialCoverage"), "{error}");
        }
    }

    #[test]
    fn other_errors_and_mutations_are_never_retried() {
        for (command, code, retryable) in [
            ("tree.slice", ErrorCode::PartialCoverage, false),
            ("tree.slice", ErrorCode::InternalError, true),
            ("scan.start", ErrorCode::PartialCoverage, true),
        ] {
            let mut calls = 0;
            let error = call_with(command, json!({}), Duration::from_secs(1), |request| {
                calls += 1;
                ResponseEnvelope::error(
                    Some(request.request_id),
                    ErrorBody::new(code, "synthetic failure", retryable),
                    None,
                )
            })
            .unwrap_err();
            assert_eq!(calls, 1, "{command}: {code:?}");
            assert_eq!(error.kind(), io::ErrorKind::Other);
            assert!(error.to_string().contains("synthetic failure"));
        }
    }

    #[test]
    fn soak_refuses_non_lab_and_ambiguous_roots() {
        assert!(check_root(r"C:\lw\lab\soak").is_ok());
        for root in [
            r"C:\lw\real",
            r"C:\Users\lab\soak",
            r"C:\lw\lab\..\real",
            r"C:\lw\lab\soak\nested",
            r"C:\lw\lab\con",
        ] {
            assert!(check_root(root).is_err(), "{root}");
        }
    }

    #[test]
    fn samples_carry_wall_clock_unix_ms_even_when_sampling_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("samples.jsonl");
        let mut out = File::create(&path).unwrap();
        // No state directory exists, so sample() fails on purpose; the record must still be stamped.
        let missing_state = dir.path().join("no-such-state");
        let before = unix_ms().unwrap();
        let _ = operation(&mut out, &missing_state, Instant::now(), 1, "grant", || {
            Ok(())
        });
        let after = unix_ms().unwrap();
        drop(out);
        let line = fs::read_to_string(&path).unwrap();
        let record: Value = serde_json::from_str(line.lines().next().unwrap()).unwrap();
        assert_eq!(record["kind"], "sample");
        let stamped = record["unix_ms"].as_u64().expect("sample carries unix_ms");
        assert!(
            before <= stamped && stamped <= after,
            "{before} <= {stamped} <= {after}"
        );
    }

    #[test]
    fn cli_refuses_a_root_inside_a_git_tree_before_touching_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let root = repo.join("lab").join("soak");
        let out = tmp.path().join("run.jsonl");
        let args: Vec<String> = [
            "--hours",
            "0.01",
            "--root",
            root.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ]
        .map(String::from)
        .to_vec();
        let refused = cli(&args).unwrap_err();
        assert!(refused.to_string().contains("outside Git"), "{refused}");
        assert!(!root.exists() && !out.exists());
    }

    #[test]
    fn existing_out_file_is_refused_before_any_root_is_created() {
        if let Err(refused) = require_non_elevated() {
            eprintln!("skipped under an elevated token: {refused}");
            return;
        }
        let tmp = tempfile::tempdir_in(loomward_service::paths::test_roots_base()).unwrap();
        let lab = tmp.path().join("lab");
        fs::create_dir(&lab).unwrap();
        let root = lab.join("soak");
        let out = tmp.path().join("cycles.jsonl");
        fs::write(&out, b"earlier run\n").unwrap();
        let refused = run(&root, &out, Duration::ZERO, Some(1)).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::AlreadyExists, "{refused}");
        assert!(
            !root.exists(),
            "a refused out must not leave an orphan root"
        );
        assert_eq!(fs::read(&out).unwrap(), b"earlier run\n");
    }

    #[test]
    fn three_cycles_scan_query_watch_forget_and_restart() {
        if let Err(refused) = require_non_elevated() {
            // Hosted Windows runners are elevated, and the harness refuses elevated tokens by design.
            eprintln!("skipped under an elevated token: {refused}");
            return;
        }
        let tmp = tempfile::tempdir_in(loomward_service::paths::test_roots_base()).unwrap();
        let lab = tmp.path().join("lab");
        fs::create_dir(&lab).unwrap();
        let root = lab.join("soak");
        let out = tmp.path().join("cycles.jsonl");
        match run(&root, &out, std::time::Duration::ZERO, Some(3)) {
            // A checkout under a user profile puts target/ there, which the harness refuses by design.
            Err(refused) if refused.to_string().contains("profile, credential, browser") => {
                eprintln!("skipped: this checkout's test root is refused: {refused}");
                return;
            }
            other => other.unwrap(),
        }
        let records: Vec<serde_json::Value> = fs::read_to_string(out)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(records.last().unwrap()["cycles"], 3);
        assert_eq!(records.last().unwrap()["success"], true);
        assert!(records.last().unwrap()["unix_ms"].as_u64().is_some());
        for cycle in 1..=3 {
            let samples: Vec<_> = records
                .iter()
                .filter(|r| r["cycle"] == cycle && r["kind"] == "sample")
                .collect();
            assert_eq!(samples.len(), STAGES.len());
            assert!(samples
                .iter()
                .all(|s| s["error"].is_null() && s["handles"].as_u64().unwrap() > 0));
            assert!(samples.iter().all(|s| s["unix_ms"].as_u64().is_some()));
            assert_eq!(
                samples.iter().find(|s| s["operation"] == "forget").unwrap()["rows"]["file"],
                0
            );
        }
    }
}
