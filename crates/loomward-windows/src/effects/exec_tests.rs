//! docs/47 lane C5 lab suite: synthetic trees copied, verified and published; receipts checked against
//! the independent C1 tree verifier; refusals, cancel at each state under both Q1 policies, crash points
//! followed by recovery. Disposable temp directories only.
use super::*;
use loomward_operations::{approve, tree, Code};
use std::{fs, sync::atomic::AtomicU32};

const DATE: &str = "2026-10-17";

struct Lab {
    tmp: tempfile::TempDir,
    store: Mutex<Store>,
}

impl Lab {
    fn new() -> Option<Lab> {
        if enumerate::running_elevated() {
            eprintln!("skipped: elevated token (invariant 2)");
            return None;
        }
        let tmp = tempfile::tempdir().unwrap();
        for d in ["src", "dest", "state"] {
            fs::create_dir(tmp.path().join(d)).unwrap();
        }
        let store = Mutex::new(Store::open(&tmp.path().join("state/state.db")).unwrap());
        Some(Lab { tmp, store })
    }
    fn src(&self) -> PathBuf {
        fs::canonicalize(self.tmp.path()).unwrap().join("src")
    }
    fn dest(&self) -> PathBuf {
        fs::canonicalize(self.tmp.path()).unwrap().join("dest")
    }
    fn request<'a>(&'a self, root: &'a Path, dest: &'a Path, sel: &'a [PathBuf]) -> Request<'a> {
        Request {
            source_root: root,
            selected: sel,
            destination: dest,
            destination_grant: 2,
            grants_active: true,
            protected: false,
            date: DATE,
        }
    }
    /// Preview, approve, start (fresh rebuild plus admit), journal the plan: the C4 to C3 hand-over.
    fn start(&self, selected: &[&str]) -> u64 {
        let selected: Vec<PathBuf> = selected.iter().map(PathBuf::from).collect();
        let (root, dest) = (self.src(), self.dest());
        let (preview, _) = prepare(&self.request(&root, &dest, &selected)).unwrap();
        assert!(preview.refusals.is_empty(), "{:?}", preview.refusals);
        let approval = approve(&preview, preview.manifest.digest(), 0, 0).unwrap();
        let (fresh, plan) = prepare(&self.request(&root, &dest, &selected)).unwrap();
        let admitted = approval.admit(&fresh, 0, 1).unwrap();
        let mut store = self.store.lock().unwrap();
        let op = store.start(0, admitted).unwrap();
        store
            .set_plan(op, &serde_json::to_string(&plan).unwrap())
            .unwrap();
        op
    }
    fn plan(&self, op: u64) -> Plan {
        serde_json::from_str(&self.store.lock().unwrap().plan(op).unwrap().unwrap()).unwrap()
    }
    fn dest_names(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.dest())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }
    /// A fresh store over the same `state.db`: what a restarted service sees.
    fn reopen(&mut self) {
        self.store = Mutex::new(Store::open(&self.tmp.path().join("state/state.db")).unwrap());
    }
}

/// Synthetic tree: sizes around sector and chunk edges, a named stream, read-only and hidden files,
/// Unicode, a 200-unit name and a deep chain whose full path passes 260 units.
fn tree_fixture(root: &Path) {
    let top = root.join("top");
    fs::create_dir(&top).unwrap();
    for (name, size) in [
        ("zero.bin", 0usize),
        ("one.bin", 1),
        ("sector.bin", 512),
        ("odd.bin", 4097),
        ("chunk-plus.bin", 1024 * 1024 + 1),
    ] {
        fs::write(
            top.join(name),
            (0..size).map(|i| (i * 7 % 251) as u8).collect::<Vec<_>>(),
        )
        .unwrap();
    }
    fs::write(top.join("streamed.txt"), b"default stream").unwrap();
    fs::write(top.join("streamed.txt:note"), b"zone mark survives").unwrap();
    fs::write(top.join("readonly.txt"), b"read only").unwrap();
    let mut p = fs::metadata(top.join("readonly.txt"))
        .unwrap()
        .permissions();
    p.set_readonly(true);
    fs::set_permissions(top.join("readonly.txt"), p).unwrap();
    fs::write(top.join("hidden.txt"), b"hidden").unwrap();
    set_attributes(&top.join("hidden.txt"), FILE_ATTRIBUTE_HIDDEN);
    fs::write(top.join("na\u{ef}ve-\u{fc}nicode.txt"), b"unicode").unwrap();
    fs::write(top.join("l".repeat(200)), b"long name").unwrap();
    let mut deep = top.join("deep");
    for n in 0..12 {
        deep = deep.join(format!("level-{n:02}-{}", "x".repeat(20)));
    }
    fs::create_dir_all(&deep).unwrap();
    fs::write(deep.join("leaf.bin"), b"deep leaf").unwrap();
    fs::create_dir(top.join("empty")).unwrap();
}

fn set_attributes(path: &Path, attributes: u32) {
    let wide = win::wide(path).unwrap();
    assert_ne!(unsafe { SetFileAttributesW(wide.as_ptr(), attributes) }, 0);
}

fn c1(root: &Path) -> tree::Tree {
    let (_, expected) =
        <enumerate::NativeSource as enumerate::DirSource>::open_root(&Default::default(), root)
            .unwrap();
    duplicate_reader::read_tree(
        root,
        &expected,
        duplicate_reader::Budget {
            bytes: 1 << 30,
            time: Duration::from_secs(120),
        },
        10_000,
        Default::default(),
    )
    .unwrap()
    .0
}

/// Every source object's identity, signature and bytes: must be unchanged by any copy.
fn source_snapshot(root: &Path) -> Vec<(PathBuf, identity::ObservedIdentity, Vec<u8>)> {
    let mut out = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for e in fs::read_dir(&dir).unwrap() {
            let path = e.unwrap().path();
            let mut o = identity::observe(&path).unwrap();
            o.last_access_time.clear();
            let bytes = if path.is_dir() {
                pending.push(path.clone());
                Vec::new()
            } else {
                fs::read(&path).unwrap()
            };
            out.push((path, o, bytes));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn lab_tree_is_copied_verified_published_and_c1_agrees() {
    let Some(lab) = Lab::new() else { return };
    tree_fixture(&lab.src());
    let before = source_snapshot(&lab.src());
    let op = lab.start(&["top"]);
    let outcome = run(&lab.store, op, &Control::new(false)).unwrap();
    assert_eq!(outcome.state, OpState::Committed, "{outcome:?}");
    assert_eq!(outcome.refusal, None);
    assert_eq!(outcome.items_verified, outcome.items_total);
    // One new folder under the source folder's name, nothing else in the parent.
    assert_eq!(lab.dest_names(), ["top"]);
    // The receipt agrees with the independent C1 verifier: content and every section 6 class.
    let report = tree::compare(&c1(&lab.src().join("top")), &c1(&lab.dest().join("top")));
    assert!(report.equal(), "{report:#?}");
    assert_eq!(source_snapshot(&lab.src()), before, "the source changed");
}

#[test]
fn a_multi_item_selection_lands_in_one_new_dated_folder() {
    let Some(lab) = Lab::new() else { return };
    fs::write(lab.src().join("a.txt"), b"alpha").unwrap();
    fs::write(lab.src().join("b.txt"), b"beta").unwrap();
    fs::create_dir(lab.dest().join(format!("Loomward copy {DATE}"))).unwrap();
    let op = lab.start(&["a.txt", "b.txt"]);
    let outcome = run(&lab.store, op, &Control::new(false)).unwrap();
    assert_eq!(outcome.state, OpState::Committed, "{outcome:?}");
    // The taken name gets " (2)"; the existing folder is never written into (Q3 default).
    let folder = lab.dest().join(format!("Loomward copy {DATE} (2)"));
    assert_eq!(fs::read(folder.join("a.txt")).unwrap(), b"alpha");
    assert_eq!(fs::read(folder.join("b.txt")).unwrap(), b"beta");
    assert_eq!(
        fs::read_dir(lab.dest().join(format!("Loomward copy {DATE}")))
            .unwrap()
            .count(),
        0
    );
}

fn staging_dirs(lab: &Lab) -> Vec<String> {
    lab.dest_names()
        .into_iter()
        .filter(|n| n.starts_with("~lw-staging-"))
        .collect()
}

#[test]
fn source_changed_mid_run_is_needs_attention_with_staging_kept_and_never_published() {
    for removal in [false, true] {
        let Some(lab) = Lab::new() else { return };
        tree_fixture(&lab.src());
        let op = lab.start(&["top"]);
        let victim = lab.src().join("top").join("sector.bin");
        let hit = AtomicU32::new(0);
        let control = Control::new(removal).with_hook(move |p, _| {
            if matches!(p, Point::Identities(_)) && hit.fetch_add(1, Ordering::SeqCst) == 0 {
                fs::write(&victim, vec![9u8; 512]).unwrap();
            }
            Flow::Continue
        });
        let outcome = run(&lab.store, op, &control).unwrap();
        assert_eq!(outcome.state, OpState::NeedsAttention, "{outcome:?}");
        assert!(
            matches!(outcome.refusal, Some((Refusal::SourceChanged, Some(_)))),
            "{outcome:?}"
        );
        assert!(!lab.dest().join("top").exists(), "never published");
        assert_eq!(staging_dirs(&lab).len(), 1, "staging kept");
        // The only exit is Abandon; the Q1 policy decides what happens to the staging.
        let abandoned = cancel(&lab.store, op, &lab.plan(op), &control).unwrap();
        assert_eq!(abandoned.state, OpState::Cancelled, "{abandoned:?}");
        if removal {
            assert_eq!(abandoned.staging_left, None);
            assert!(
                lab.dest_names().is_empty(),
                "own staging removed by identity"
            );
        } else {
            assert_eq!(
                abandoned.staging_left.as_deref(),
                Some(&*staging_dirs(&lab)[0])
            );
        }
    }
}

#[test]
fn cancel_at_each_state_stops_and_applies_the_q1_policy() {
    let points: [fn(Point) -> bool; 4] = [
        |p| p == Point::StagingCreated,
        |p| matches!(p, Point::Identities(_)),
        |p| matches!(p, Point::Written(_)),
        |p| matches!(p, Point::Verified(_)),
    ];
    for (n, at) in points.into_iter().enumerate() {
        for removal in [false, true] {
            let Some(lab) = Lab::new() else { return };
            tree_fixture(&lab.src());
            let op = lab.start(&["top"]);
            let control = Control::new(removal).with_hook(move |p, c| {
                if at(p) {
                    c.cancel();
                }
                Flow::Continue
            });
            let outcome = run(&lab.store, op, &control).unwrap();
            assert_eq!(outcome.state, OpState::Cancelled, "point {n} {outcome:?}");
            assert!(
                !lab.dest().join("top").exists(),
                "point {n}: never published"
            );
            if removal {
                assert!(
                    lab.dest_names().is_empty(),
                    "point {n}: {:?}",
                    lab.dest_names()
                );
                assert_eq!(outcome.staging_left, None);
            } else {
                let left = staging_dirs(&lab);
                assert_eq!(left.len(), 1, "point {n}");
                assert_eq!(outcome.staging_left.as_deref(), Some(&*left[0]));
            }
        }
    }
}

#[test]
fn crash_points_recover_to_wait_abandon_or_confirm() {
    let points: [fn(Point) -> bool; 6] = [
        |p| p == Point::StagingCreated,
        |p| matches!(p, Point::Identities(_)),
        |p| matches!(p, Point::Written(_)),
        |p| matches!(p, Point::Verified(_)),
        |p| p == Point::BeforePublish,
        |p| p == Point::AfterPublish,
    ];
    for (n, at) in points.into_iter().enumerate() {
        let Some(mut lab) = Lab::new() else { return };
        tree_fixture(&lab.src());
        let before = source_snapshot(&lab.src());
        let op = lab.start(&["top"]);
        let crash = Control::new(true)
            .with_hook(move |p, _| if at(p) { Flow::Crash } else { Flow::Continue });
        assert!(
            matches!(run(&lab.store, op, &crash), Err(Fault::Crashed)),
            "point {n}"
        );
        lab.reopen();
        let control = Control::new(true);
        let recovered = recover(&lab.store, op, &control).unwrap();
        if n == 5 {
            // Renamed before `committed`: re-hash by ID against the source, then commit. Never cleanup.
            assert_eq!(recovered.state, OpState::Committed, "{recovered:?}");
            let report = tree::compare(&c1(&lab.src().join("top")), &c1(&lab.dest().join("top")));
            assert!(report.equal(), "{report:#?}");
        } else {
            // Nothing resumes by itself.
            assert!(!recovered.state.is_terminal(), "point {n}: {recovered:?}");
            assert!(!lab.dest().join("top").exists());
            let abandoned = cancel(&lab.store, op, &lab.plan(op), &control).unwrap();
            assert_eq!(
                abandoned.state,
                OpState::Cancelled,
                "point {n}: {abandoned:?}"
            );
            assert!(
                lab.dest_names().is_empty(),
                "point {n}: {:?}",
                lab.dest_names()
            );
        }
        assert_eq!(
            source_snapshot(&lab.src()),
            before,
            "point {n}: source changed"
        );
    }
}

#[test]
fn a_flipped_staged_byte_is_redone_once_with_removal_and_needs_attention_without() {
    for removal in [true, false] {
        let Some(lab) = Lab::new() else { return };
        fs::write(lab.src().join("f.bin"), vec![3u8; 9000]).unwrap();
        let op = lab.start(&["f.bin"]);
        let dest = lab.dest();
        let flipped = AtomicU32::new(0);
        let control = Control::new(removal).with_hook(move |p, _| {
            if matches!(p, Point::Written(_)) && flipped.fetch_add(1, Ordering::SeqCst) == 0 {
                let staging = fs::read_dir(&dest).unwrap().next().unwrap().unwrap().path();
                let mut f = fs::OpenOptions::new()
                    .write(true)
                    .open(staging.join("f.bin"))
                    .unwrap();
                f.write_all(&[4u8]).unwrap();
            }
            Flow::Continue
        });
        let outcome = run(&lab.store, op, &control).unwrap();
        if removal {
            assert_eq!(outcome.state, OpState::Committed, "{outcome:?}");
            let folder = lab.dest().join(format!("Loomward copy {DATE}"));
            assert_eq!(fs::read(folder.join("f.bin")).unwrap(), vec![3u8; 9000]);
        } else {
            assert_eq!(outcome.state, OpState::NeedsAttention, "{outcome:?}");
            assert!(matches!(
                outcome.refusal,
                Some((Refusal::VerificationMismatch, _))
            ));
        }
    }
}

#[test]
fn preflight_refuses_before_any_effect() {
    let Some(lab) = Lab::new() else { return };
    fs::create_dir(lab.src().join("top")).unwrap();
    fs::write(lab.src().join("top/x"), b"x").unwrap();
    fs::write(lab.src().join("top/sys"), b"s").unwrap();
    set_attributes(&lab.src().join("top/sys"), FILE_ATTRIBUTE_SYSTEM);
    let sel = [PathBuf::from("top")];
    let (src, dest) = (lab.src(), lab.dest());
    let (preview, _) = prepare(&lab.request(&src, &dest, &sel)).unwrap();
    assert!(preview
        .refusals
        .iter()
        .any(|r| r.code == Code::UnsupportedAttribute));
    assert!(approve(&preview, preview.manifest.digest(), 0, 0).is_err());
    // Destination inside the source.
    let inner = src.join("top");
    let (preview, _) = prepare(&lab.request(&src, &inner, &sel)).unwrap();
    assert!(preview
        .refusals
        .iter()
        .any(|r| r.code == Code::DestinationInsideSource));
    assert!(lab.dest_names().is_empty());
}

#[test]
fn a_changed_source_between_approval_and_start_is_source_changed() {
    let Some(lab) = Lab::new() else { return };
    fs::write(lab.src().join("f"), b"one").unwrap();
    let sel = [PathBuf::from("f")];
    let (src, dest) = (lab.src(), lab.dest());
    let (preview, _) = prepare(&lab.request(&src, &dest, &sel)).unwrap();
    let approval = approve(&preview, preview.manifest.digest(), 0, 0).unwrap();
    fs::write(lab.src().join("f"), b"two").unwrap();
    let (fresh, _) = prepare(&lab.request(&src, &dest, &sel)).unwrap();
    assert_eq!(approval.admit(&fresh, 0, 1), Err(Code::SourceChanged));
}

/// The insufficient-space seam (no small lab volume on this host): a preallocation or write failing
/// with ERROR_DISK_FULL or ERROR_HANDLE_DISK_FULL is the resumable `insufficient_space`.
#[test]
fn disk_full_maps_to_resumable_insufficient_space() {
    for code in [39, 112] {
        assert!(matches!(
            io_refusal(&io::Error::from_raw_os_error(code)),
            Stop::Refused(Refusal::InsufficientSpace, None)
        ));
    }
    assert!(Refusal::InsufficientSpace.resumable());
}

#[test]
fn destination_case_sensitivity_is_the_folder_flag() {
    let Some(lab) = Lab::new() else { return };
    let key = identity::observe(&lab.dest()).unwrap().object;
    let dest = Destination::open(&lab.dest(), &key).unwrap();
    assert!(!dest.case_sensitive().unwrap());
    assert!(!dest.cloud_synced().unwrap());
}

#[test]
fn attach_is_by_journaled_id_and_creation_time_never_by_name() {
    let Some(lab) = Lab::new() else { return };
    let key = identity::observe(&lab.dest()).unwrap().object;
    let dest = Destination::open(&lab.dest(), &key).unwrap();
    let mut stage = dest.stage("tok").unwrap();
    let original = stage.identity().clone();
    let created: i64 = original.creation_time.parse().unwrap();
    let planned = 133_000_000_000_000_000;
    stage
        .set_metadata(BasicMetadata {
            creation_time: planned,
            last_write_time: planned,
            attributes: 0,
        })
        .unwrap();
    drop(stage);
    // Original or planned creation time: attached. Any other: refused.
    assert!(Staging::attach(&dest, "tok", original.file_id, &[created, planned]).is_ok());
    assert!(Staging::attach(&dest, "tok", original.file_id, &[created]).is_err());
    assert!(Staging::attach(&dest, "tok", original.file_id, &[planned + 1]).is_err());
    // The name now names another folder: the journaled ID no longer resolves under it.
    fs::rename(lab.dest().join("tok"), lab.dest().join("moved")).unwrap();
    fs::create_dir(lab.dest().join("tok")).unwrap();
    assert!(Staging::attach(&dest, "tok", original.file_id, &[created, planned]).is_err());
    let other = identity::observe(&lab.dest().join("tok")).unwrap().object;
    let other_ft: i64 = other.creation_time.parse().unwrap();
    // ...and the impostor's own ID is not journaled, so a caller holding the journal never asks for it.
    assert!(Staging::attach(&dest, "tok", original.file_id, &[other_ft]).is_err());
    // Publishing to the staging name itself, or a case-only variant, is refused.
    let mut stage = Staging::attach(&dest, "tok", other.file_id, &[other_ft]).unwrap();
    assert!(stage.publish("TOK").is_err());
}

/// Throughput on this host (evidence/v3/copy-executor-lab.md). `--ignored --nocapture`.
#[test]
#[ignore]
fn throughput_lab() {
    for (label, files, size) in [
        ("large", 64usize, 4 * 1024 * 1024usize),
        ("small", 2000, 4096),
    ] {
        let Some(lab) = Lab::new() else { return };
        let top = lab.src().join("top");
        fs::create_dir(&top).unwrap();
        let bytes: Vec<u8> = (0..size).map(|i| (i * 31 % 253) as u8).collect();
        for n in 0..files {
            fs::write(top.join(format!("f{n:05}.bin")), &bytes).unwrap();
        }
        let op = lab.start(&["top"]);
        let started = std::time::Instant::now();
        let outcome = run(&lab.store, op, &Control::new(false)).unwrap();
        let secs = started.elapsed().as_secs_f64();
        assert_eq!(outcome.state, OpState::Committed);
        let mib = (files * size) as f64 / (1024.0 * 1024.0);
        println!(
            "THROUGHPUT {label}: {files} files x {size} B = {mib:.1} MiB in {secs:.2} s: {:.1} MiB/s, {:.0} files/s",
            mib / secs,
            files as f64 / secs
        );
    }
}

/// H2 for named streams (measured here, C5): a held named-stream handle blocks renaming its ancestor
/// folder even with delete sharing, while a held default stream does not. Hence the executor releases
/// stream holds just before publish and re-hashes the streams after (docs/47 section 4, step 2).
#[test]
fn h2_named_stream_hold_blocks_publish_default_hold_does_not() {
    let Some(lab) = Lab::new() else { return };
    let key = identity::observe(&lab.dest()).unwrap().object;
    let dest = Destination::open(&lab.dest(), &key).unwrap();
    let mut stage = dest.stage("tok").unwrap();
    let file = stage.create_file(&[], "f").unwrap();
    file.write_and_flush(&mut &b"data"[..], 4).unwrap();
    let stream = file.create_stream("note").unwrap();
    stream.write_and_flush(&mut &b"n"[..], 1).unwrap();
    let id = file.identity().unwrap();
    drop((stream, file));
    let held = stage.verify(&[], "f", &id).unwrap();
    let stream_hold = held.verify_stream("note").unwrap();
    assert_eq!(stage.publish("done").unwrap_err().raw_os_error(), Some(5));
    drop(stream_hold);
    stage.publish("done").unwrap();
    assert_eq!(held.verify_stream("note").unwrap().size, 1);
}

/// Regression (found by the C5 service lab run): a one-character final name was refused with
/// ERROR_INVALID_PARAMETER because the rename buffer was sized below the Win32 minimum.
#[test]
fn publish_accepts_short_final_names() {
    let Some(lab) = Lab::new() else { return };
    let key = identity::observe(&lab.dest()).unwrap().object;
    let dest = Destination::open(&lab.dest(), &key).unwrap();
    for name in ["a", "ab", "abc"] {
        let mut stage = dest.stage(&format!("tok-{name}")).unwrap();
        stage.publish(name).unwrap();
        assert!(lab.dest().join(name).is_dir());
    }
}

/// Review of #522: a cleanup that crashed after removing an entry but before its `item_removed` record
/// finishes on recovery (the entry is journaled gone), instead of stopping in `needs_attention`.
#[test]
fn cleanup_after_a_crash_mid_removal_journals_the_gone_entry_and_finishes() {
    let Some(mut lab) = Lab::new() else { return };
    fs::create_dir(lab.src().join("top")).unwrap();
    fs::write(lab.src().join("top/a.bin"), b"aaaa").unwrap();
    fs::write(lab.src().join("top/b.bin"), b"bbbb").unwrap();
    let op = lab.start(&["top"]);
    let crash = Control::new(true).with_hook(|p, _| {
        if matches!(p, Point::Verified(_)) {
            Flow::Crash
        } else {
            Flow::Continue
        }
    });
    assert!(matches!(run(&lab.store, op, &crash), Err(Fault::Crashed)));
    let staging = fs::read_dir(lab.dest())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    // Abandon was journaled; the first removal happened; the process died before its record.
    {
        let r = replay(&lab.store, op).unwrap();
        lab.store
            .lock()
            .unwrap()
            .append(
                op,
                r.epoch,
                Record::Transition {
                    to: OpState::Cancelling,
                    code: None,
                },
            )
            .unwrap();
    }
    fs::remove_file(staging.join("a.bin")).unwrap();
    lab.reopen();
    let outcome = recover(&lab.store, op, &Control::new(true)).unwrap();
    assert_eq!(outcome.state, OpState::Cancelled, "{outcome:?}");
    assert!(lab.dest_names().is_empty(), "{:?}", lab.dest_names());
}
