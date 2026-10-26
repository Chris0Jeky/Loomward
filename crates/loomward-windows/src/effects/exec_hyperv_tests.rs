//! docs/53 H2: persistent, synthetic guest steps. This entire module is test-build only.
use super::crash_tests::{
    abandon, cancelled_oracle, dest_snapshot, fixed_tree, pattern, Snap, EXISTING,
};
use super::tests::{c1, set_attributes, Lab};
use super::*;
use loomward_operations::{tree, Entry};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    panic::{catch_unwind, AssertUnwindSafe},
    process::{Command, ExitStatus},
    time::Instant,
};

const CHILD: &str = "effects::exec::hyperv_tests::hyperv_guest";

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct StreamRecord {
    name: String,
    size: u64,
    sha256: [u8; 32],
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ObjectRecord {
    relative: PathBuf,
    identity: identity::ObservedIdentity,
    streams: Vec<StreamRecord>,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct SourceSnap {
    // Every sibling has the same ancestry. Keep it once per relative parent, without losing
    // any ObservedIdentity fields or repeating the entire ancestor chain thousands of times.
    ancestors: BTreeMap<PathBuf, Vec<ObjectKey>>,
    objects: Vec<ObjectRecord>,
}

fn object_record(root: &Path, path: &Path) -> ObjectRecord {
    let mut observed = identity::observe(path).expect("hyperv: object missing");
    observed.last_access_time.clear();
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .unwrap();
    let mut named = match streams(&file) {
        Ok(named) => named,
        Err(error) if error.raw_os_error() == Some(ERROR_HANDLE_EOF as i32) => Vec::new(),
        Err(error) => panic!("hyperv: stream observation failed: {error}"),
    };
    if !path.is_dir() {
        named.push(("::$DATA".to_string(), file.metadata().unwrap().len()));
    }
    named.sort();
    let streams = named
        .into_iter()
        .map(|(name, size)| {
            if name != "::$DATA" {
                let mut stream = path.as_os_str().to_os_string();
                stream.push(&name);
                file = fs::File::open(stream).unwrap();
            }
            let mut hash = Sha256::new();
            let mut bytes = [0; 64 * 1024];
            let mut read = 0;
            loop {
                let n = file.read(&mut bytes).unwrap();
                if n == 0 {
                    break;
                }
                read += n as u64;
                hash.update(&bytes[..n]);
            }
            assert_eq!(read, size, "hyperv: stream size changed");
            StreamRecord {
                name,
                size,
                sha256: hash.finalize().into(),
            }
        })
        .collect();
    let mut after = identity::observe(path).unwrap();
    after.last_access_time.clear();
    assert_eq!(observed, after, "hyperv: object changed during observation");
    ObjectRecord {
        relative: path.strip_prefix(root).unwrap().to_path_buf(),
        identity: observed,
        streams,
    }
}

fn source_record(root: &Path) -> SourceSnap {
    let mut snapshot = SourceSnap::default();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path.clone());
            }
            let mut record = object_record(root, &path);
            let ancestors = std::mem::take(&mut record.identity.ancestors);
            let parent = record.relative.parent().unwrap().to_path_buf();
            if let Some(previous) = snapshot.ancestors.insert(parent, ancestors.clone()) {
                assert_eq!(previous, ancestors, "hyperv: ancestor changed");
            }
            snapshot.objects.push(record);
        }
    }
    snapshot.objects.sort_by(|a, b| a.relative.cmp(&b.relative));
    snapshot
}

#[derive(Serialize, Deserialize)]
struct Prepared {
    scenario: String,
    removal: bool,
    op: u64,
    final_name: String,
    source: SourceSnap,
    neighbour: SourceSnap,
    sentinel: ObjectRecord,
    c1_sha256: [u8; 32],
}

/// Hash precisely the equality-relevant C1 fields; reported metadata (e.g. access time) can change
/// when read. Source identities, signatures and all stream hashes are persisted separately.
fn c1_record(value: &tree::Tree) -> [u8; 32] {
    let report = tree::compare(value, value);
    assert!(
        report.equal(),
        "C1 observation incomplete: {:?}; {:?}",
        report.differences,
        report.unknowns
    );
    let mut nodes = Vec::new();
    for (path, node) in &value.nodes {
        let node = node.as_ref().unwrap();
        let metadata: Vec<_> = node
            .metadata
            .iter()
            .filter(|(class, _)| class.policy() != tree::Policy::Reported)
            .collect();
        let content = if node.kind == tree::Kind::File {
            let digest = node.content.unwrap();
            Some((node.size.unwrap(), digest))
        } else {
            None
        };
        nodes.push(json!([path, node.kind, content, metadata]));
    }
    Sha256::digest(serde_json::to_vec(&nodes).unwrap()).into()
}

/// The signal is on its own device in the VM. Never flush a lab-volume handle here.
fn write_signal(signal: &Path, name: &str, value: &impl Serialize) {
    fs::create_dir_all(signal).unwrap();
    let temp = signal.join(format!("{name}.tmp"));
    let target = signal.join(name);
    let mut file = io::BufWriter::new(fs::File::create(&temp).unwrap());
    serde_json::to_writer(&mut file, value).unwrap();
    file.flush().unwrap();
    file.get_ref().sync_all().unwrap();
    drop(file);
    fs::rename(temp, &target).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(target)
        .unwrap()
        .sync_all()
        .unwrap();
    // FlushFileBuffers needs write access, including for a directory handle on NTFS.
    fs::OpenOptions::new()
        .write(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(signal)
        .unwrap()
        .sync_all()
        .unwrap();
}

fn read_prepared(signal: &Path) -> Prepared {
    serde_json::from_slice(
        &fs::read(signal.join("prepare.json")).expect("hyperv: missing prepare.json"),
    )
    .expect("hyperv: invalid prepare.json")
}

fn flush_file(path: &Path) {
    let attributes = fs::metadata(path).unwrap().file_attributes();
    if attributes & FILE_ATTRIBUTE_READONLY != 0 {
        set_attributes(path, attributes & !FILE_ATTRIBUTE_READONLY);
    }
    let file = fs::OpenOptions::new().write(true).open(path).unwrap();
    if attributes & FILE_ATTRIBUTE_READONLY != 0 {
        set_attributes(path, attributes);
    }
    file.sync_all().unwrap();
}

fn flush_tree(root: &Path) {
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            flush_tree(&path);
        } else {
            flush_file(&path);
        }
    }
}

fn prepare_step(dir: &Path, signal: &Path, scenario: &str, removal: bool, large: bool) {
    assert!(
        !dir.exists(),
        "hyperv: prepare requires a fresh lab directory"
    );
    let lab = Lab::at(dir).expect("hyperv: elevated token refused");
    let top = lab.src().join("top");
    fixed_tree(&top);
    if large {
        for d in 0..20 {
            fs::create_dir(top.join(format!("batch-{d:02}"))).unwrap();
        }
        for n in 0..2000 {
            fs::write(
                top.join(format!("batch-{:02}/small-{n:04}.bin", n % 20)),
                pattern((1 + n % 64) * 1024, n as u64),
            )
            .unwrap();
        }
        for n in 0..8 {
            fs::write(
                top.join(format!("batch-{n:02}/large-{n}.bin")),
                pattern(8 * 1024 * 1024, n as u64),
            )
            .unwrap();
        }
    }
    fs::write(lab.dest().join(EXISTING), b"pre-existing").unwrap();
    let outside = dir.join("neighbour");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep.txt"), b"not a lab root").unwrap();
    flush_tree(&lab.src());
    flush_file(&top.join("streamed.txt:note"));
    flush_tree(&lab.dest());
    flush_tree(&outside);

    let source = source_record(&lab.src());
    let neighbour = source_record(&outside);
    let sentinel_path = lab.dest().join(EXISTING);
    let sentinel = object_record(&lab.dest(), &sentinel_path);
    let c1_sha256 = c1_record(&c1(&top));
    // A fresh store starts at 1; a single selected folder keeps its name in this empty parent.
    // Check both predictions against the real admission before any executor effects.
    assert!(lab.store.lock().unwrap().operations().unwrap().is_empty());
    let op = 1;
    write_signal(
        signal,
        "prepare.json",
        &Prepared {
            scenario: scenario.to_string(),
            removal,
            op,
            final_name: "top".to_string(),
            source,
            neighbour,
            sentinel,
            c1_sha256,
        },
    );
    assert_eq!(lab.start(&["top"]), op, "hyperv: unexpected operation ID");
    assert_eq!(
        lab.plan(op).manifest.destination.folder_name,
        "top",
        "hyperv: unexpected final folder name"
    );
    if scenario == "abandon" {
        let stop = Control::new(removal).with_hook(|p, _| {
            if matches!(p, Point::Verified(_)) {
                Flow::Crash
            } else {
                Flow::Continue
            }
        });
        assert!(matches!(run(&lab.store, op, &stop), Err(Fault::Crashed)));
    }
    println!("{}", json!({"step": "prepare", "op": op, "ok": true}));
}

fn act_step(dir: &Path, signal: &Path) {
    let before = read_prepared(signal);
    let lab = Lab::at(dir).expect("hyperv: elevated token refused");
    let control = Control::new(before.removal);
    let outcome = match before.scenario.as_str() {
        "run" => run(&lab.store, before.op, &control).unwrap(),
        "abandon" => {
            let recovered = recover(&lab.store, before.op, &control).unwrap();
            abandon(&lab, before.op, &control, "act", &recovered)
        }
        _ => panic!("hyperv: invalid persisted scenario"),
    };
    write_signal(signal, "act-done.json", &json!({"state": outcome.state}));
}

fn oracle(lab: &Lab, before: &Prepared, baseline: &Snap, outcome: &Outcome) -> Snap {
    assert!(
        source_record(&lab.src()) == before.source,
        "hyperv: source changed"
    );
    let source = c1(&lab.src().join("top"));
    let digest = c1_record(&source);
    assert_eq!(digest, before.c1_sha256, "hyperv: source changed (C1)");
    assert!(
        source_record(&lab.tmp.path().join("neighbour")) == before.neighbour,
        "hyperv: neighbour changed"
    );
    let path = lab.dest().join(EXISTING);
    assert!(path.is_file(), "hyperv: sentinel missing");
    assert_eq!(
        object_record(&lab.dest(), &path),
        before.sentinel,
        "hyperv: sentinel changed"
    );
    let now = reboot_destination_oracle(lab, before, baseline, outcome);
    if outcome.state == OpState::Committed {
        let report = tree::compare(&source, &c1(&lab.dest().join(&before.final_name)));
        assert!(report.equal(), "hyperv: FALSE COMMIT {report:#?}");
    }
    now
}

/// Unlike C6's pre-crash allowlist, a post-boot baseline can include unrelated entries already
/// present before recovery. Still reject every later addition and every unjournaled removal.
fn reboot_destination_oracle(
    lab: &Lab,
    before: &Prepared,
    baseline: &Snap,
    outcome: &Outcome,
) -> Snap {
    assert_eq!(
        replay(&lab.store, before.op).unwrap().state,
        outcome.state,
        "hyperv: receipt and journal disagree"
    );
    let now = dest_snapshot(&lab.dest());
    for (path, id) in &now {
        assert_eq!(
            baseline.get(path),
            Some(id),
            "hyperv: {path} appeared after baseline"
        );
    }
    let mut journaled = BTreeSet::new();
    for row in lab.store.lock().unwrap().rows(before.op).unwrap() {
        match serde_json::from_str::<Entry>(&row.body).unwrap().record {
            Record::StagingCreated { id } => {
                journaled.insert(id.file_id);
            }
            Record::Identities { items } => {
                journaled.extend(items.into_iter().map(|(_, id)| id.file_id))
            }
            _ => {}
        }
    }
    for (path, id) in baseline {
        if now.get(path) != Some(id) {
            assert!(
                before.removal,
                "hyperv: removal off, yet {path} is gone or replaced"
            );
            assert!(
                journaled.contains(id),
                "hyperv: {path} removed without a journaled identity"
            );
        }
    }
    now
}

fn panic_text(error: Box<dyn std::any::Any + Send>) -> String {
    if let Some(text) = error.downcast_ref::<String>() {
        text.clone()
    } else if let Some(text) = error.downcast_ref::<&str>() {
        (*text).to_string()
    } else {
        "hyperv: non-text panic".to_string()
    }
}

/// Catch configuration, token and step panics alike, and persist failure before libtest exits.
/// Hard termination at a crash hook intentionally bypasses unwinding and cannot produce a receipt.
fn step_receipt(signal: &Path, step: &str, body: impl FnOnce(&mut Value)) {
    let mut result = json!({
        "step": step, "ok": false, "recovered": null, "abandoned": null, "journal_records": 0,
        "last_record": null, "copy_ran": false, "failure": null,
        "baseline_attempted": false, "lab_open_attempted": false,
    });
    let checked = catch_unwind(AssertUnwindSafe(|| {
        if step == "prepare" {
            clear_signals(signal);
        }
        body(&mut result);
    }));
    match checked {
        Ok(()) => result["ok"] = json!(true),
        Err(error) => result["failure"] = json!(panic_text(error)),
    }
    if step == "recover" || result["ok"] != true {
        write_signal(signal, "result.json", &result);
    }
    assert!(result["ok"] == true, "hyperv: {}", result["failure"]);
}

fn clear_signals(signal: &Path) {
    if !signal.exists() {
        return;
    }
    for entry in fs::read_dir(signal).unwrap() {
        let path = entry.unwrap().path();
        let name = path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_ascii_lowercase();
        if matches!(name.as_str(), "result.json" | "act-done.json" | "trace.txt")
            || name.ends_with(".tmp")
        {
            fs::remove_file(path).unwrap();
        }
    }
}

fn recover_step(dir: &Path, signal: &Path, result: &mut Value) {
    let before = read_prepared(signal);
    // Read the post-boot destination before Lab::at can create or open anything on the lab.
    result["baseline_attempted"] = json!(true);
    let baseline = dest_snapshot(&dir.join("dest"));
    result["lab_open_attempted"] = json!(true);
    let lab = Lab::at(dir).expect("hyperv: elevated token refused");
    let rows = lab.store.lock().unwrap().rows(before.op).unwrap();
    result["journal_records"] = json!(rows.len());
    if let Some(last) = rows.last() {
        let entry: Entry = serde_json::from_str(&last.body).unwrap();
        let record = serde_json::to_value(&entry.record).unwrap();
        result["last_record"] = match record["to"].as_str() {
            Some(to) => json!(format!("{}:{to}", record["kind"].as_str().unwrap())),
            None => record["kind"].clone(),
        };
    }
    // Started puts the operation in preparing. Count only evidence durable before recovery.
    let copy_ran = rows.len() > 1;
    result["copy_ran"] = json!(copy_ran);
    // The host credits a cut to the G5 floor only when the act had not already finished.
    let state_before = replay(&lab.store, before.op).ok().map(|r| r.state);
    result["state_before_recover"] = json!(state_before);
    result["terminal_before_recover"] = json!(state_before.is_some_and(|s| s.is_terminal()));
    let control = Control::new(before.removal);
    let recovered = recover(&lab.store, before.op, &control).unwrap();
    result["recovered"] = json!(recovered.state);
    let after_recovery = oracle(&lab, &before, &baseline, &recovered);
    let abandoned = abandon(&lab, before.op, &control, "hyperv", &recovered);
    result["abandoned"] = json!(abandoned.state);
    let after = oracle(&lab, &before, &after_recovery, &abandoned);
    // C6's cancellation check owns Loomward names; unrelated baseline entries are retained
    // and already checked above, but are not staging leftovers.
    let replayed = replay(&lab.store, before.op).unwrap();
    let owned: Snap = after
        .into_iter()
        .filter(|(path, _)| {
            let name = path.split('/').next().unwrap();
            name == EXISTING
                || name == before.final_name
                || replayed.staging_tokens.iter().any(|token| token == name)
        })
        .collect();
    cancelled_oracle(
        &lab,
        before.op,
        before.removal,
        "hyperv",
        &abandoned,
        &owned,
    );
    assert!(copy_ran, "hyperv: no durable copy record after preparing");
}

#[test]
fn hyperv_guest() {
    let step = std::env::var("LOOMWARD_HV_STEP");
    // Ordinary cargo tests do not activate the runner. A partial guest configuration does.
    if matches!(step, Err(std::env::VarError::NotPresent))
        && std::env::var_os("LOOMWARD_HV_SIGNAL").is_none()
        && std::env::var_os("LOOMWARD_HV_LAB").is_none()
    {
        return;
    }
    let step_name = step.as_deref().unwrap_or("unknown");
    // With no signal argument there is no caller-selected directory. Keep an error receipt
    // at the deterministic local fallback, <working directory>/signal/result.json.
    let signal = std::env::var_os("LOOMWARD_HV_SIGNAL")
        .map_or_else(|| PathBuf::from("signal"), PathBuf::from);
    step_receipt(&signal, step_name, |result| {
        let step = step
            .as_deref()
            .unwrap_or_else(|_| panic!("hyperv: missing or invalid LOOMWARD_HV_STEP"));
        guest_step(step, &signal, result);
    });
}

fn guest_step(step: &str, signal: &Path, result: &mut Value) {
    assert!(
        !enumerate::skip_elevated_test(),
        "hyperv: elevated token refused"
    );
    let required =
        |name| std::env::var(name).unwrap_or_else(|_| panic!("hyperv: missing or invalid {name}"));
    let required_path = |name| {
        PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("hyperv: missing {name}")))
    };
    let dir = required_path("LOOMWARD_HV_LAB");
    required_path("LOOMWARD_HV_SIGNAL");
    assert!(
        dir.is_absolute() && signal.is_absolute(),
        "hyperv: paths must be absolute"
    );
    assert!(
        !dir.starts_with(signal) && !signal.starts_with(&dir),
        "hyperv: signal and lab must be separate"
    );
    match step {
        "prepare" => {
            let scenario = required("LOOMWARD_HV_SCENARIO");
            assert!(
                matches!(scenario.as_str(), "run" | "abandon"),
                "hyperv: invalid scenario"
            );
            let removal = match std::env::var("LOOMWARD_HV_REMOVAL").as_deref() {
                Ok("0") => false,
                Ok("1") => true,
                _ => panic!("hyperv: removal must be 0 or 1"),
            };
            let large = match std::env::var("LOOMWARD_HV_TREE").as_deref() {
                Ok("small") => false,
                Ok("large") | Err(std::env::VarError::NotPresent) => true,
                _ => panic!("hyperv: tree must be small or large"),
            };
            prepare_step(&dir, signal, &scenario, removal, large);
        }
        "act" => act_step(&dir, signal),
        "recover" => recover_step(&dir, signal, result),
        _ => panic!("hyperv: unknown step"),
    }
}

fn host_lab() -> Option<tempfile::TempDir> {
    if enumerate::skip_elevated_test() {
        let temp = tempfile::tempdir().unwrap();
        assert!(identity::observe(temp.path()).is_err());
        eprintln!("skipped: elevated token; copy primitives refuse elevation (invariant 2)");
        return None;
    }
    Some(tempfile::tempdir().unwrap())
}

fn child_step(
    root: &Path,
    step: &str,
    scenario: &str,
    removal: bool,
    cut: Option<u64>,
) -> ExitStatus {
    child_step_with(root, step, scenario, removal, cut, |_| {})
}

fn child_step_with(
    root: &Path,
    step: &str,
    scenario: &str,
    removal: bool,
    cut: Option<u64>,
    configure: impl FnOnce(&mut Command),
) -> ExitStatus {
    let signal = root.join("signal");
    fs::create_dir_all(&signal).unwrap();
    let log = signal.join(format!("{step}.log"));
    let file = fs::File::create(&log).unwrap();
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args([CHILD, "--exact", "--test-threads=1", "--nocapture"])
        .env("LOOMWARD_HV_STEP", step)
        .env("LOOMWARD_HV_LAB", root.join("lab"))
        .env("LOOMWARD_HV_SIGNAL", &signal)
        .env("LOOMWARD_HV_SCENARIO", scenario)
        .env("LOOMWARD_HV_REMOVAL", if removal { "1" } else { "0" })
        .env("LOOMWARD_HV_TREE", "small")
        .env_remove("LOOMWARD_COPY_CHILD")
        .env_remove("LOOMWARD_COPY_CRASH_TRACE")
        .env_remove("LOOMWARD_COPY_CRASH_AT")
        .env_remove("LOOMWARD_COPY_PAUSE_AT")
        .stdout(file.try_clone().unwrap())
        .stderr(file);
    if step == "act" {
        cmd.env("LOOMWARD_COPY_CRASH_TRACE", signal.join("trace.txt"));
    }
    if let Some(n) = cut {
        cmd.env("LOOMWARD_COPY_CRASH_AT", n.to_string());
    }
    configure(&mut cmd);
    let mut child = cmd.spawn().unwrap();
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if start.elapsed() > Duration::from_secs(120) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("hyperv: {step} child timed out");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn successful_step(root: &Path, step: &str, scenario: &str, removal: bool) {
    let status = child_step(root, step, scenario, removal, None);
    assert!(
        status.success(),
        "{step}: {}",
        fs::read_to_string(root.join(format!("signal/{step}.log"))).unwrap()
    );
}

fn result(root: &Path) -> Value {
    serde_json::from_slice(&fs::read(root.join("signal/result.json")).unwrap()).unwrap()
}

#[test]
fn hyperv_failure_receipt_is_durable_even_without_prepare() {
    let Some(temp) = host_lab() else { return };
    let signal = temp.path().join("signal");
    successful_step(temp.path(), "prepare", "run", false);
    // The persisted record is valid; the absent destination must fail before Lab::at can
    // create a lab, rather than failing early while reading a missing prepare.json.
    read_prepared(&signal);
    write_signal(&signal, "probe.json", &json!({"version": 1}));
    write_signal(&signal, "probe.json", &json!({"version": 2}));
    let probe: Value =
        serde_json::from_slice(&fs::read(signal.join("probe.json")).unwrap()).unwrap();
    assert_eq!(probe["version"], 2);
    assert!(!signal.join("probe.json.tmp").exists());
    assert!(catch_unwind(|| step_receipt(&signal, "recover", |result| {
        recover_step(&temp.path().join("absent-lab"), &signal, result);
    }))
    .is_err());
    let receipt = result(temp.path());
    assert_eq!(receipt["ok"], false);
    assert_eq!(receipt["copy_ran"], false);
    assert_eq!(receipt["baseline_attempted"], true);
    assert_eq!(receipt["lab_open_attempted"], false);
    assert!(receipt["failure"].is_string());
    assert!(!temp.path().join("absent-lab").exists());
    assert!(!signal.join("result.json.tmp").exists());
}

#[test]
fn hyperv_steps_run_end_to_end_in_process() {
    let Some(temp) = host_lab() else { return };
    for scenario in ["run", "abandon"] {
        for removal in [false, true] {
            let root = temp.path().join(format!("{scenario}-{removal}"));
            successful_step(&root, "prepare", scenario, removal);
            successful_step(&root, "act", scenario, removal);
            successful_step(&root, "recover", scenario, removal);
            for name in ["prepare.json", "act-done.json", "result.json"] {
                assert!(root.join("signal").join(name).is_file());
                assert!(!root.join("signal").join(format!("{name}.tmp")).exists());
            }
            let receipt = result(&root);
            assert_eq!(receipt["ok"], true, "{receipt}");
            assert_eq!(receipt["copy_ran"], true);
            assert!(receipt["journal_records"].as_u64().unwrap() > 1);
            let state = if scenario == "run" {
                "committed"
            } else {
                "cancelled"
            };
            assert_eq!(receipt["recovered"], state);
            assert_eq!(receipt["abandoned"], state);
            assert_eq!(receipt["terminal_before_recover"], true, "{receipt}");

            // Resolve a real mid-write / mid-cleanup boundary from this scenario's clean trace.
            let trace = fs::read_to_string(root.join("signal/trace.txt")).unwrap();
            let label = if scenario == "run" {
                "write@1048576"
            } else {
                "after transition:cancelling"
            };
            let n = trace
                .lines()
                .find_map(|line| {
                    let (n, found) = line.split_once('\t').unwrap();
                    (found == label).then(|| n.parse::<u64>().unwrap())
                })
                .expect("mid-act boundary missing");
            let cut_root = temp.path().join(format!("cut-{scenario}-{removal}"));
            successful_step(&cut_root, "prepare", scenario, removal);
            let status = child_step(&cut_root, "act", scenario, removal, Some(n));
            assert_eq!(status.code(), Some(crash::EXIT as i32));
            assert!(!cut_root.join("signal/act-done.json").exists());
            successful_step(&cut_root, "recover", scenario, removal);
            assert_eq!(result(&cut_root)["ok"], true);
            assert_eq!(result(&cut_root)["copy_ran"], true);
            assert_eq!(result(&cut_root)["terminal_before_recover"], false);
        }
    }
}

#[test]
fn hyperv_source_corruption_reports_failure() {
    let Some(temp) = host_lab() else { return };
    let root = temp.path();
    successful_step(root, "prepare", "run", false);
    successful_step(root, "act", "run", false);
    let path = root.join("lab/src/top/big.bin");
    let mut bytes = fs::read(&path).unwrap();
    bytes[1024] ^= 1;
    fs::write(path, bytes).unwrap();
    assert!(!child_step(root, "recover", "run", false, None).success());
    let receipt = result(root);
    assert_eq!(receipt["ok"], false);
    assert!(
        receipt["failure"]
            .as_str()
            .unwrap()
            .contains("source changed"),
        "{receipt}"
    );
}

#[test]
fn hyperv_prepare_clears_stale_signals_before_validation() {
    let Some(temp) = host_lab() else { return };
    let signal = temp.path().join("signal");
    for name in [
        "result.json",
        "act-done.json",
        "trace.txt",
        "prepare.json.tmp",
        "other.tmp",
        ".tmp",
        "upper.TMP",
    ] {
        fs::create_dir_all(&signal).unwrap();
        fs::write(signal.join(name), b"stale").unwrap();
    }
    assert!(!child_step(temp.path(), "prepare", "invalid", false, None).success());
    let receipt = result(temp.path());
    assert_eq!(receipt["ok"], false);
    assert_eq!(receipt["step"], "prepare");
    assert!(receipt["failure"]
        .as_str()
        .unwrap()
        .contains("invalid scenario"));
    for name in [
        "act-done.json",
        "trace.txt",
        "prepare.json.tmp",
        "other.tmp",
        ".tmp",
        "upper.TMP",
    ] {
        assert!(!signal.join(name).exists(), "stale {name}");
    }
}

#[test]
fn hyperv_configuration_and_missing_prepare_failures_have_receipts() {
    let Some(temp) = host_lab() else { return };
    for (n, (name, value, failure)) in [
        ("LOOMWARD_HV_LAB", None, "LOOMWARD_HV_LAB"),
        ("LOOMWARD_HV_SIGNAL", None, "LOOMWARD_HV_SIGNAL"),
        ("LOOMWARD_HV_STEP", None, "LOOMWARD_HV_STEP"),
        ("LOOMWARD_HV_SCENARIO", None, "LOOMWARD_HV_SCENARIO"),
        ("LOOMWARD_HV_REMOVAL", None, "removal must"),
        ("LOOMWARD_HV_REMOVAL", Some("bad"), "removal must"),
        ("LOOMWARD_HV_TREE", Some("bad"), "tree must"),
        (
            "LOOMWARD_HV_LAB",
            Some("relative-lab"),
            "paths must be absolute",
        ),
        ("LOOMWARD_HV_STEP", Some("bad"), "unknown step"),
    ]
    .into_iter()
    .enumerate()
    {
        let root = temp.path().join(n.to_string());
        let status = child_step_with(&root, "prepare", "run", false, None, |cmd| {
            cmd.current_dir(&root);
            if let Some(value) = value {
                cmd.env(name, value);
            } else {
                cmd.env_remove(name);
            }
        });
        assert!(!status.success(), "{name} unexpectedly succeeded");
        let receipt = result(&root);
        assert_eq!(receipt["ok"], false);
        assert!(receipt["step"].is_string());
        assert!(
            receipt["failure"].as_str().unwrap().contains(failure),
            "{receipt}"
        );
        assert!(!root.join("lab").exists());
    }
    for step in ["act", "recover"] {
        let root = temp.path().join(step);
        assert!(!child_step(&root, step, "run", false, None).success());
        let receipt = result(&root);
        assert_eq!(receipt["ok"], false);
        assert_eq!(receipt["step"], step);
        assert!(
            receipt["failure"]
                .as_str()
                .unwrap()
                .contains("missing prepare.json"),
            "{receipt}"
        );
    }
}

#[test]
fn hyperv_panics_in_every_step_replace_success_with_failure() {
    let temp = tempfile::tempdir().unwrap();
    let signal = temp.path().join("signal");
    for step in ["prepare", "act", "recover"] {
        write_signal(&signal, "result.json", &json!({"ok": true}));
        assert!(catch_unwind(|| step_receipt(&signal, step, |_| {
            panic!("hyperv: injected {step} panic");
        }))
        .is_err());
        let receipt = result(temp.path());
        assert_eq!(receipt["ok"], false);
        assert_eq!(receipt["step"], step);
        assert_eq!(receipt["failure"], format!("hyperv: injected {step} panic"));
        assert!(!signal.join("result.json.tmp").exists());
    }
}

#[test]
fn hyperv_sentinel_loss_or_replacement_reports_failure() {
    let Some(temp) = host_lab() else { return };
    for replace in [false, true] {
        let root = temp.path().join(format!("replace-{replace}"));
        successful_step(&root, "prepare", "run", false);
        successful_step(&root, "act", "run", false);
        let sentinel = root.join("lab/dest").join(EXISTING);
        fs::remove_file(&sentinel).unwrap();
        if replace {
            fs::write(sentinel, b"different sentinel bytes").unwrap();
        }
        assert!(!child_step(&root, "recover", "run", false, None).success());
        let receipt = result(&root);
        assert_eq!(receipt["ok"], false);
        assert!(
            receipt["failure"].as_str().unwrap().contains("sentinel"),
            "{receipt}"
        );
    }
}

#[test]
fn hyperv_stream_records_preserve_hashes_and_full_identity_without_file_bytes() {
    let Some(temp) = host_lab() else { return };
    fs::create_dir(temp.path().join("empty")).unwrap();
    let path = temp.path().join("streamed.txt");
    fs::write(&path, b"default stream").unwrap();
    fs::write(temp.path().join("streamed.txt:note"), b"named stream").unwrap();
    let snapshot = source_record(temp.path());
    let encoded = serde_json::to_vec(&snapshot).unwrap();
    let decoded: SourceSnap = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(snapshot, decoded);
    assert_eq!(decoded.objects.len(), 2);
    assert!(decoded.objects[0].streams.is_empty());
    let record = &decoded.objects[1];
    assert_eq!(record.relative, Path::new("streamed.txt"));
    assert!(!record.relative.is_absolute());
    assert_eq!(record.streams.len(), 2);
    for (stream, name, bytes) in [
        (&record.streams[0], "::$DATA", b"default stream".as_slice()),
        (
            &record.streams[1],
            ":note:$DATA",
            b"named stream".as_slice(),
        ),
    ] {
        assert_eq!(stream.name, name);
        assert_eq!(stream.size, bytes.len() as u64);
        assert_eq!(stream.sha256, <[u8; 32]>::from(Sha256::digest(bytes)));
    }
    let mut restored = record.identity.clone();
    restored.ancestors = decoded.ancestors[Path::new("")].clone();
    let mut observed = identity::observe(&path).unwrap();
    observed.last_access_time.clear();
    assert_eq!(restored, observed);
    let wire: Value = serde_json::from_slice(&encoded).unwrap();
    assert!(wire["objects"][1].get("bytes").is_none());
    assert!(wire["objects"][1].get("content").is_none());
}

#[test]
fn hyperv_pre_recovery_addition_is_baseline_but_later_addition_fails() {
    let Some(temp) = host_lab() else { return };
    let root = temp.path();
    successful_step(root, "prepare", "run", false);
    successful_step(root, "act", "run", false);
    fs::write(root.join("lab/dest/baseline.txt"), b"before recovery").unwrap();
    successful_step(root, "recover", "run", false);
    assert_eq!(result(root)["ok"], true);

    let before = read_prepared(&root.join("signal"));
    let lab = Lab::at(&root.join("lab")).unwrap();
    let baseline = dest_snapshot(&lab.dest());
    let outcome = recover(&lab.store, before.op, &Control::new(false)).unwrap();
    fs::write(lab.dest().join("late.txt"), b"after baseline").unwrap();
    let failure = catch_unwind(AssertUnwindSafe(|| {
        reboot_destination_oracle(&lab, &before, &baseline, &outcome)
    }))
    .expect_err("a name appearing after the baseline must fail");
    let failure = panic_text(failure);
    assert!(
        failure.contains("late.txt") && failure.contains("appeared"),
        "{failure}"
    );
}

#[test]
fn hyperv_skipping_act_is_not_a_copy_run() {
    let Some(temp) = host_lab() else { return };
    successful_step(temp.path(), "prepare", "run", false);
    assert!(!child_step(temp.path(), "recover", "run", false, None).success());
    let receipt = result(temp.path());
    assert_eq!(receipt["ok"], false);
    assert_eq!(receipt["copy_ran"], false);
    assert!(receipt["failure"]
        .as_str()
        .unwrap()
        .contains("no durable copy record"));
}

#[test]
fn hyperv_large_tree_smoke_and_bounded_prepare_signal() {
    let Some(temp) = host_lab() else { return };
    let dir = temp.path().join("lab");
    let signal = temp.path().join("signal");
    let start = Instant::now();
    // Thousands of individual durability flushes can exceed the small-child watchdog.
    // Exercise the same steps directly in this disposable host lab.
    step_receipt(&signal, "prepare", |_| {
        prepare_step(&dir, &signal, "run", false, true)
    });
    let prepare = start.elapsed();
    let size = fs::metadata(signal.join("prepare.json")).unwrap().len();
    println!("hyperv large: prepare={prepare:.3?}, prepare.json={size} bytes");
    assert!(size < 2 * 1024 * 1024, "prepare.json is {size} bytes");
    let start = Instant::now();
    step_receipt(&signal, "act", |_| act_step(&dir, &signal));
    let act = start.elapsed();
    let start = Instant::now();
    step_receipt(&signal, "recover", |result| {
        recover_step(&dir, &signal, result)
    });
    println!(
        "hyperv large: act={act:.3?}, recover={:.3?}",
        start.elapsed()
    );
    assert_eq!(result(temp.path())["ok"], true);
}
