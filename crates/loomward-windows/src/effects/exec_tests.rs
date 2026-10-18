//! docs/47 lane C5 lab suite: synthetic trees copied, verified and published; receipts checked against
//! the independent C1 tree verifier; refusals, cancel at each state under both Q1 policies, crash points
//! followed by recovery. Disposable temp directories only.
use super::*;
use loomward_operations::{approve, tree, Code};
use std::{fs, sync::atomic::AtomicU32};

#[test]
fn copy_fence_append_failure_still_stops_admission() {
    use loomward_operations::store::fault::{Kind, Stage};
    for stage in [Stage::Row, Stage::Head, Stage::Ack] {
        let Some(lab) = Lab::new() else { return };
        fs::write(lab.src().join("f"), b"payload").unwrap();
        let op = lab.start(&["f"]);
        lab.store
            .lock()
            .unwrap()
            .set_fault(Some(Box::new(move |record, at| {
                (matches!(record, Record::Fenced { .. }) && at == stage).then_some(Kind::Full)
            })));
        let control = Control::new(false);
        assert!(fence(
            &lab.store,
            op,
            &control,
            loomward_operations::FenceCause::GrantRevoked
        )
        .is_err());
        assert!(
            control.fenced(),
            "{stage:?}: journal failure left admission open"
        );
        assert!(matches!(control.admit(), Err(Stop::Cancel)));
    }
}

#[test]
fn copy_recovery_persists_replaced_staged_file_attention() {
    for state in [OpState::Copying, OpState::Blocked, OpState::Staged] {
        let Some(mut lab) = Lab::new() else { return };
        fs::write(lab.src().join("f"), b"payload").unwrap();
        let op = lab.start(&["f"]);
        let point = if state == OpState::Staged {
            Point::Verified(0)
        } else {
            Point::Identities(0)
        };
        let control = Control::new(false).with_hook(move |p, _| {
            if p == point {
                Flow::Crash
            } else {
                Flow::Continue
            }
        });
        assert!(matches!(run(&lab.store, op, &control), Err(Fault::Crashed)));
        if state != OpState::Copying {
            let mut store = lab.store.lock().unwrap();
            let epoch = store.replay(op).unwrap().unwrap().epoch;
            store
                .append(
                    op,
                    epoch,
                    Record::Transition {
                        to: state,
                        code: (state == OpState::Blocked).then_some(Refusal::SourceInUse),
                    },
                )
                .unwrap();
        }
        let staging = lab.dest().join(&staging_dirs(&lab)[0]);
        fs::rename(staging.join("f"), lab.dest().join("displaced")).unwrap();
        fs::write(staging.join("f"), b"replacement").unwrap();
        lab.reopen();
        assert_eq!(
            recover(&lab.store, op, &Control::new(false)).unwrap().state,
            OpState::NeedsAttention
        );
        lab.reopen();
        assert_eq!(
            status(&lab.store, op).unwrap().state,
            OpState::NeedsAttention
        );
        assert_eq!(fs::read(staging.join("f")).unwrap(), b"replacement");
    }
}

#[test]
fn copy_old_worker_checks_durable_stop_before_preallocation() {
    for attention in [true, false] {
        let Some(lab) = Lab::new() else { return };
        fs::write(lab.src().join("f"), b"payload").unwrap();
        let op = lab.start(&["f"]);
        let db = lab.tmp.path().join("state/state.db");
        let dest = lab.dest();
        let control = Control::new(false).with_hook(move |p, _| {
            if p == Point::OpeningSource(0) {
                // Gate the source open while a reopened service uses a different Control.
                let reopened = Mutex::new(Store::open(&db).unwrap());
                if attention {
                    let mut store = reopened.lock().unwrap();
                    let epoch = store.replay(op).unwrap().unwrap().epoch;
                    store
                        .append(
                            op,
                            epoch,
                            Record::Transition {
                                to: OpState::NeedsAttention,
                                code: None,
                            },
                        )
                        .unwrap();
                } else {
                    fence(
                        &reopened,
                        op,
                        &Control::new(false),
                        loomward_operations::FenceCause::GrantRevoked,
                    )
                    .unwrap();
                }
            }
            Flow::Continue
        });
        let _ = run(&lab.store, op, &control);
        if attention {
            assert_eq!(
                status(&lab.store, op).unwrap().state,
                OpState::NeedsAttention,
                "old worker overwrote the reopened service's decision"
            );
        }
        let staging = fs::read_dir(dest).unwrap().next().unwrap().unwrap().path();
        let file = fs::File::open(staging.join("f")).unwrap();
        let info = info::<FILE_STANDARD_INFO>(&file, FileStandardInfo).unwrap();
        assert_eq!(
            info.AllocationSize, 0,
            "stale worker preallocated after durable fence"
        );
        assert_eq!(info.EndOfFile, 0);
    }
}

#[test]
fn copy_cancelled_worker_cannot_begin_preallocation() {
    let control = Control::new(false);
    control.cancel();
    assert!(matches!(
        Cancellable::new(&mut std::io::empty(), &control),
        Err(Stop::Cancel)
    ));
}

pub(super) const DATE: &str = "2026-10-17";

#[test]
fn prepare_bounds_pending_before_observing_a_wide_tree() {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    let Some(lab) = Lab::new() else { return };
    let (src, dest) = (lab.src(), lab.dest());
    fs::create_dir(src.join("wide")).unwrap();
    for n in 0..8 {
        fs::write(src.join(format!("wide/{n}")), b"").unwrap();
    }
    let invalid = src
        .join("wide")
        .join(OsString::from_wide(&[b'z' as u16, 0xD800]));
    fs::write(&invalid, b"").unwrap();
    // NTFS lists the synthetic invalid name after the valid children: stop before decoding it.
    assert!(fs::read_dir(src.join("wide")).unwrap().take(8).all(|e| e
        .unwrap()
        .file_name()
        .to_str()
        .is_some()));
    let selected = [PathBuf::from("wide")];
    let error = prepare_with_limit(&lab.request(&src, &dest, &selected), 8).unwrap_err();
    assert_eq!(error.to_string(), "manifest_limit");
    assert!(lab.dest_names().is_empty());
    fs::remove_file(invalid).unwrap();
    let (preview, _) = prepare_with_limit(&lab.request(&src, &dest, &selected), 9).unwrap();
    assert!(preview.refusals.is_empty(), "{:?}", preview.refusals);
    assert_eq!((preview.files, preview.folders), (8, 1));
}

#[test]
fn prepare_bounds_the_initial_selection_before_observing_it() {
    let Some(lab) = Lab::new() else { return };
    let (src, dest) = (lab.src(), lab.dest());
    let selected = vec![PathBuf::from("absent"); 9];
    let error = prepare_with_limit(&lab.request(&src, &dest, &selected), 8).unwrap_err();
    assert_eq!(error.to_string(), "manifest_limit");
    assert!(lab.dest_names().is_empty());
}

#[test]
fn unpaired_surrogate_staging_entries_are_never_committed() {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    for inject in [Point::Verified(0), Point::AfterPublish] {
        let Some(lab) = Lab::new() else { return };
        fs::write(lab.src().join("f"), b"payload").unwrap();
        let op = lab.start(&["f"]);
        let dest = lab.dest();
        let control = Control::new(false).with_hook(move |point, _| {
            if point == inject {
                let staging = fs::read_dir(&dest).unwrap().next().unwrap().unwrap().path();
                let name = OsString::from_wide(&[b'x' as u16, 0xD800]);
                fs::write(staging.join(name), b"extra").unwrap();
                assert_incomplete_listing(&staging);
            }
            Flow::Continue
        });
        let outcome = run(&lab.store, op, &control).unwrap();
        assert_eq!(
            outcome.state,
            OpState::NeedsAttention,
            "{inject:?}: {outcome:?}"
        );
        assert_eq!(
            outcome.refusal,
            Some((Refusal::ChangedAfterVerification, None))
        );
        assert_eq!(fs::read(lab.src().join("f")).unwrap(), b"payload");
    }
}

#[test]
#[cfg(feature = "fixtures")]
fn over_limit_staging_entries_are_never_committed() {
    let Some(lab) = Lab::new() else { return };
    fs::write(lab.src().join("f"), b"payload").unwrap();
    let op = lab.start(&["f"]);
    let dest = lab.dest();
    let control = Control::new(false).with_hook(move |point, _| {
        if point == Point::BeforePublish {
            let staging = fs::read_dir(&dest).unwrap().next().unwrap().unwrap().path();
            for n in 0..loomward_operations::MAX_ITEMS {
                fs::write(staging.join(format!("extra-{n:06}")), b"").unwrap();
            }
            assert_eq!(
                fs::read_dir(&staging).unwrap().count(),
                loomward_operations::MAX_ITEMS as usize + 1
            );
            assert_incomplete_listing(&staging);
        }
        Flow::Continue
    });
    let outcome = run(&lab.store, op, &control).unwrap();
    assert_eq!(outcome.state, OpState::NeedsAttention, "{outcome:?}");
    assert_eq!(
        outcome.refusal,
        Some((Refusal::ChangedAfterVerification, None))
    );
    assert_eq!(fs::read(lab.src().join("f")).unwrap(), b"payload");
}

fn assert_incomplete_listing(folder: &Path) {
    let parent = folder.parent().unwrap();
    let destination =
        Destination::open(parent, &identity::observe(parent).unwrap().object).unwrap();
    let key = identity::observe(folder).unwrap().object;
    let staging = Staging::attach_published(
        &destination,
        folder.file_name().unwrap().to_str().unwrap(),
        key.file_id,
        &[key.creation_time.parse().unwrap()],
    )
    .unwrap();
    assert_eq!(
        staging.entries(&[]).unwrap_err().to_string(),
        "incomplete staging listing"
    );
}

pub(super) struct Lab {
    pub(super) tmp: tempfile::TempDir,
    pub(super) store: Mutex<Store>,
}

impl Lab {
    pub(super) fn new() -> Option<Lab> {
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
    pub(super) fn src(&self) -> PathBuf {
        fs::canonicalize(self.tmp.path()).unwrap().join("src")
    }
    pub(super) fn dest(&self) -> PathBuf {
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
            destination_root: self.grant(),
        }
    }
    /// The lab's own folder stands for the destination's lab-root grant: (volume serial, file ID).
    fn grant(&self) -> (u64, [u8; 16]) {
        let o = identity::observe(&fs::canonicalize(self.tmp.path()).unwrap())
            .unwrap()
            .object;
        (o.volume_serial.parse().unwrap(), o.file_id)
    }
    /// Preview, approve, start (fresh rebuild plus admit), journal the plan: the C4 to C3 hand-over.
    pub(super) fn start(&self, selected: &[&str]) -> u64 {
        self.start_with(selected, |_| {})
    }
    fn start_with(&self, selected: &[&str], edit: impl FnOnce(&mut Plan)) -> u64 {
        let selected: Vec<PathBuf> = selected.iter().map(PathBuf::from).collect();
        let (root, dest) = (self.src(), self.dest());
        let (preview, _) = prepare(&self.request(&root, &dest, &selected)).unwrap();
        assert!(preview.refusals.is_empty(), "{:?}", preview.refusals);
        let approval = approve(&preview, preview.manifest.digest(), 0, 0).unwrap();
        let (fresh, mut plan) = prepare(&self.request(&root, &dest, &selected)).unwrap();
        edit(&mut plan);
        let admitted = approval.admit(&fresh, 0, 1).unwrap();
        let mut store = self.store.lock().unwrap();
        let op = store.start(0, admitted).unwrap();
        store
            .set_plan(op, &serde_json::to_string(&plan).unwrap())
            .unwrap();
        op
    }
    pub(super) fn plan(&self, op: u64) -> Plan {
        serde_json::from_str(&self.store.lock().unwrap().plan(op).unwrap().unwrap()).unwrap()
    }
    pub(super) fn dest_names(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.dest())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }
    /// A fresh store over the same `state.db`: what a restarted service sees.
    pub(super) fn reopen(&mut self) {
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

pub(super) fn set_attributes(path: &Path, attributes: u32) {
    let wide = win::wide(path).unwrap();
    assert_ne!(unsafe { SetFileAttributesW(wide.as_ptr(), attributes) }, 0);
}

pub(super) fn c1(root: &Path) -> tree::Tree {
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
pub(super) fn source_snapshot(root: &Path) -> Vec<(PathBuf, identity::ObservedIdentity, Vec<u8>)> {
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
fn extra_nested_entries_prevent_publish_commit_and_crash_confirmation() {
    // Before publish, after publish, and recovery after the rename lost all holds.
    for phase in 0..3 {
        let Some(mut lab) = Lab::new() else { return };
        fs::create_dir_all(lab.src().join("top/sub")).unwrap();
        fs::write(lab.src().join("top/sub/file"), b"data").unwrap();
        let before = source_snapshot(&lab.src());
        let op = lab.start(&["top"]);
        let dest = lab.dest();
        let control = Control::new(false).with_hook(move |point, _| {
            if (phase == 0 && matches!(point, Point::Verified(_)))
                || (phase == 1 && point == Point::AfterPublish)
            {
                let folder = fs::read_dir(&dest).unwrap().next().unwrap().unwrap().path();
                fs::write(folder.join("sub/extra"), b"keep").unwrap();
            }
            if phase == 2 && point == Point::AfterPublish {
                Flow::Crash
            } else {
                Flow::Continue
            }
        });
        let result = run(&lab.store, op, &control);
        let outcome = if phase == 2 {
            assert!(matches!(result, Err(Fault::Crashed)));
            fs::write(lab.dest().join("top/sub/extra"), b"keep").unwrap();
            lab.reopen();
            recover(&lab.store, op, &Control::new(false)).unwrap()
        } else {
            result.unwrap()
        };
        assert_eq!(
            outcome.state,
            OpState::NeedsAttention,
            "{phase}: {outcome:?}"
        );
        assert_eq!(
            outcome.refusal,
            Some((Refusal::ChangedAfterVerification, None))
        );
        assert_eq!(source_snapshot(&lab.src()), before);
        let folder = fs::read_dir(lab.dest())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(fs::read(folder.join("sub/extra")).unwrap(), b"keep");
        assert_eq!(lab.dest().join("top").exists(), phase != 0);
    }
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

/// Review #664: the durable-stop hot path over a 20,000-item synthetic copy.
#[test]
#[ignore]
fn durable_stop_20k_lab() {
    let Some(lab) = Lab::new() else { return };
    let top = lab.src().join("top");
    fs::create_dir(&top).unwrap();
    for n in 0..20_000 {
        fs::write(top.join(format!("f{n:05}")), b"").unwrap();
    }
    let op = lab.start(&["top"]);
    let started = std::time::Instant::now();
    let outcome = run(&lab.store, op, &Control::new(false)).unwrap();
    let seconds = started.elapsed().as_secs_f64();
    assert_eq!(outcome.state, OpState::Committed);
    println!("DURABLE_STOP_20K: 20000 empty files, {seconds:.3} s");
    let Lab { store, tmp } = lab;
    drop(store);
    tmp.close().unwrap();
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

// ---- Review round 1 of #522 ----

/// (1) The destination must lie under its own lab-root grant, by identity: preview and run both refuse a
/// destination whose pinned ancestry does not contain the grant's verified root, before any write.
#[test]
fn a_destination_outside_its_grant_identity_is_refused_before_any_write() {
    let Some(lab) = Lab::new() else { return };
    fs::write(lab.src().join("f"), b"x").unwrap();
    let sel = [PathBuf::from("f")];
    let (src, dest) = (lab.src(), lab.dest());
    let mut request = lab.request(&src, &dest, &sel);
    request.destination_root = (request.destination_root.0, [0xAB; 16]);
    let e = prepare(&request).unwrap_err();
    assert_eq!(e.to_string(), "destination_outside_grant");
    let mut request = lab.request(&src, &dest, &sel);
    request.destination_root = (
        request.destination_root.0.wrapping_add(1),
        request.destination_root.1,
    );
    assert_eq!(
        prepare(&request).unwrap_err().to_string(),
        "destination_outside_grant"
    );
    // At run time (an ancestor swapped after start): refused, nothing created.
    let op = lab.start_with(&["f"], |plan| plan.destination_root.1 = [0xCD; 16]);
    let outcome = run(&lab.store, op, &Control::new(false)).unwrap();
    assert_eq!(outcome.state, OpState::Refused, "{outcome:?}");
    assert_eq!(outcome.refusal, Some((Refusal::DestinationChanged, None)));
    assert!(lab.dest_names().is_empty());
}

/// #526: the destination parent replaced between approval and the run (renamed away, a new folder under
/// its name) is refused with a cause, `destination_changed`, before any write to either folder.
#[test]
fn a_replaced_destination_parent_is_refused_with_destination_changed() {
    let Some(lab) = Lab::new() else { return };
    fs::write(lab.src().join("f"), b"x").unwrap();
    let op = lab.start(&["f"]);
    let moved = lab.dest().with_file_name("dest-moved");
    fs::rename(lab.dest(), &moved).unwrap();
    fs::create_dir(lab.dest()).unwrap();
    let outcome = run(&lab.store, op, &Control::new(false)).unwrap();
    assert_eq!(outcome.state, OpState::Refused, "{outcome:?}");
    assert_eq!(outcome.refusal, Some((Refusal::DestinationChanged, None)));
    assert!(lab.dest_names().is_empty());
    assert_eq!(fs::read_dir(&moved).unwrap().count(), 0);
    // Gone altogether: the same cause.
    let op = lab.start(&["f"]);
    fs::remove_dir(lab.dest()).unwrap();
    let outcome = run(&lab.store, op, &Control::new(false)).unwrap();
    assert_eq!(outcome.refusal, Some((Refusal::DestinationChanged, None)));
    assert!(!lab.dest().exists());
}

/// #526, the window before `committed`: an owner cancel that lands after the rename never unpublishes.
/// The run confirms and commits; the copy is kept under either Q1 policy, never presented as cancelled.
#[test]
fn a_cancel_after_the_rename_commits_and_never_removes_the_copy() {
    for removal in [false, true] {
        let Some(lab) = Lab::new() else { return };
        fs::create_dir(lab.src().join("top")).unwrap();
        fs::write(lab.src().join("top/f.bin"), b"payload").unwrap();
        let op = lab.start(&["top"]);
        let control = Control::new(removal).with_hook(|p, c| {
            if p == Point::AfterPublish {
                c.cancel();
            }
            Flow::Continue
        });
        let outcome = run(&lab.store, op, &control).unwrap();
        assert_eq!(outcome.state, OpState::Committed, "{removal} {outcome:?}");
        // An abandon after it is a no-op on a committed run.
        let again = cancel(&lab.store, op, &lab.plan(op), &Control::new(removal)).unwrap();
        assert_eq!(again.state, OpState::Committed, "{removal} {again:?}");
        assert_eq!(fs::read(lab.dest().join("top/f.bin")).unwrap(), b"payload");
    }
}

/// (2) A named stream added to a staged file after verification (stream holds are released for the
/// rename) is caught before `committed`.
#[test]
fn a_stream_added_after_verification_is_never_committed() {
    let Some(lab) = Lab::new() else { return };
    fs::create_dir(lab.src().join("top")).unwrap();
    fs::write(lab.src().join("top/f.txt"), b"data").unwrap();
    fs::write(lab.src().join("top/f.txt:note"), b"kept").unwrap();
    let op = lab.start(&["top"]);
    let dest = lab.dest();
    let control = Control::new(false).with_hook(move |p, _| {
        if p == Point::BeforePublish {
            let staging = fs::read_dir(&dest).unwrap().next().unwrap().unwrap().path();
            fs::write(staging.join("f.txt:planted"), b"new").unwrap();
        }
        Flow::Continue
    });
    let outcome = run(&lab.store, op, &control).unwrap();
    assert_eq!(outcome.state, OpState::NeedsAttention, "{outcome:?}");
    assert_eq!(
        outcome.refusal,
        Some((Refusal::ChangedAfterVerification, None))
    );
}

/// Deny "create folder" on the test's own destination for everyone; removed again on drop.
pub(super) struct DenyCreate(PathBuf);
impl DenyCreate {
    pub(super) fn new(path: &Path) -> Self {
        Self::with(path, "(AD)")
    }
    /// Deny `rights` (icacls syntax, inheritance flags included) to everyone.
    pub(super) fn with(path: &Path, rights: &str) -> Self {
        let ok = std::process::Command::new("icacls")
            .arg(path)
            .args(["/deny", &format!("*S-1-1-0:{rights}")])
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "icacls deny");
        DenyCreate(path.to_path_buf())
    }
}
impl Drop for DenyCreate {
    fn drop(&mut self) {
        let _ = std::process::Command::new("icacls")
            .arg(&self.0)
            .args(["/remove:d", "*S-1-1-0"])
            .output();
    }
}

/// (3) A staging folder that cannot be created ends `refused` with its cause, never `preparing`.
#[test]
fn a_failed_staging_create_is_a_journaled_refusal() {
    let Some(lab) = Lab::new() else { return };
    fs::write(lab.src().join("f"), b"x").unwrap();
    let op = lab.start(&["f"]);
    let deny = DenyCreate::new(&lab.dest());
    let outcome = run(&lab.store, op, &Control::new(false)).unwrap();
    drop(deny);
    assert_eq!(outcome.state, OpState::Refused, "{outcome:?}");
    assert_eq!(outcome.refusal, Some((Refusal::AccessDenied, None)));
    assert!(lab.dest_names().is_empty());
}

/// (4) Recovery after the rename tells an unverifiable read (the source held by a writer) from a real
/// mismatch. Unverifiable: recovery leaves `publishing` to retry; abandon settles it as needs_attention
/// with the cause and never touches the published copy. Mismatch: `changed_after_verification`.
#[test]
fn confirm_tells_unverifiable_from_changed_and_abandon_settles_it() {
    for changed in [false, true] {
        let Some(mut lab) = Lab::new() else { return };
        fs::create_dir(lab.src().join("top")).unwrap();
        fs::write(lab.src().join("top/f.bin"), b"payload").unwrap();
        let op = lab.start(&["top"]);
        let crash = Control::new(false).with_hook(|p, _| {
            if p == Point::AfterPublish {
                Flow::Crash
            } else {
                Flow::Continue
            }
        });
        assert!(matches!(run(&lab.store, op, &crash), Err(Fault::Crashed)));
        lab.reopen();
        let published = lab.dest().join("top").join("f.bin");
        if changed {
            fs::write(&published, b"PAYLOAD").unwrap();
            let r = recover(&lab.store, op, &Control::new(false)).unwrap();
            assert_eq!(r.state, OpState::NeedsAttention, "{r:?}");
            assert_eq!(r.refusal, Some((Refusal::ChangedAfterVerification, None)));
        } else {
            let writer = fs::OpenOptions::new()
                .append(true)
                .open(lab.src().join("top/f.bin"))
                .unwrap();
            let r = recover(&lab.store, op, &Control::new(false)).unwrap();
            assert_eq!(r.state, OpState::Publishing, "retried later: {r:?}");
            let settled = cancel(&lab.store, op, &lab.plan(op), &Control::new(true)).unwrap();
            drop(writer);
            assert_eq!(settled.state, OpState::NeedsAttention, "{settled:?}");
            assert_eq!(settled.refusal, Some((Refusal::SourceInUse, None)));
            assert_eq!(
                fs::read(&published).unwrap(),
                b"payload",
                "published copy kept"
            );
        }
    }
}

/// (5) A staging token intended but never identity-committed is reported, not silent.
#[test]
fn a_leftover_staging_token_is_reported_in_status() {
    let Some(lab) = Lab::new() else { return };
    fs::write(lab.src().join("f"), b"x").unwrap();
    let op = lab.start(&["f"]);
    let epoch = replay(&lab.store, op).unwrap().epoch;
    lab.store
        .lock()
        .unwrap()
        .append(
            op,
            epoch,
            Record::StagingIntent {
                token: "~lw-staging-orphan".into(),
            },
        )
        .unwrap();
    let s = status(&lab.store, op).unwrap();
    assert_eq!(s.leftover_staging, vec!["~lw-staging-orphan".to_string()]);
}

#[test]
fn recovery_records_unjournaled_batch_damage() {
    for blocked in [false, true] {
        let Some(mut lab) = Lab::new() else { return };
        fs::write(lab.src().join("f"), b"payload").unwrap();
        let op = lab.start(&["f"]);
        let crash = Control::new(false).with_hook(|p, _| {
            if p == Point::StagingCreated {
                Flow::Crash
            } else {
                Flow::Continue
            }
        });
        assert!(matches!(run(&lab.store, op, &crash), Err(Fault::Crashed)));
        let r = replay(&lab.store, op).unwrap();
        lab.store
            .lock()
            .unwrap()
            .append(
                op,
                r.epoch,
                Record::CreateBatch {
                    items: vec![(0, "f".into())],
                },
            )
            .unwrap();
        let leftover = lab.dest().join(r.staging_tokens.last().unwrap()).join("f");
        fs::write(&leftover, b"").unwrap();
        if blocked {
            lab.store
                .lock()
                .unwrap()
                .append(
                    op,
                    r.epoch,
                    Record::Transition {
                        to: OpState::Blocked,
                        code: Some(Refusal::SourceInUse),
                    },
                )
                .unwrap();
        }
        lab.reopen();
        let recovered = recover(&lab.store, op, &Control::new(true)).unwrap();
        assert_eq!(recovered.state, OpState::NeedsAttention, "{recovered:?}");
        assert_eq!(
            status(&lab.store, op).unwrap().state,
            OpState::NeedsAttention
        );
        assert_eq!(recovered.unjournaled_items, ["f"]);
        lab.reopen();
        assert_eq!(status(&lab.store, op).unwrap().unjournaled_items, ["f"]);
        // Blocked -> needs_attention is a journal edge (#634): the damage is durable in both cases.
        assert_eq!(
            replay(&lab.store, op).unwrap().state,
            OpState::NeedsAttention
        );
        assert!(
            leftover.exists(),
            "unjournaled entries must never be removed"
        );
    }
}

#[test]
fn recovery_observation_failure_does_not_record_damage() {
    recovery_observation_failure(false);
}

#[test]
fn published_recovery_observation_failure_does_not_record_damage() {
    recovery_observation_failure(true);
}

#[test]
fn idle_status_keeps_unjournaled_names_during_observation_failure() {
    let Some(mut lab) = Lab::new() else { return };
    for name in ["a", "b", "c"] {
        fs::write(lab.src().join(name), b"payload").unwrap();
    }
    let op = lab.start(&["a", "b", "c"]);
    let crash = Control::new(false).with_hook(|p, _| {
        if p == Point::Creating(2) {
            Flow::Crash
        } else {
            Flow::Continue
        }
    });
    assert!(matches!(run(&lab.store, op, &crash), Err(Fault::Crashed)));
    let r = replay(&lab.store, op).unwrap();
    let staging = lab.dest().join(r.staging_tokens.last().unwrap());
    let deny = DenyCreate::with(&staging.join("a"), "(R)");
    let before = lab.store.lock().unwrap().rows(op).unwrap();
    for _ in 0..2 {
        lab.reopen();
        let recovered = recover(&lab.store, op, &Control::new(false)).unwrap();
        assert_eq!(recovered.state, OpState::NeedsAttention);
        assert_eq!(recovered.unjournaled_items, ["b"]);
        let status = status_idle(&lab.store, op).unwrap();
        assert_eq!(status.state, OpState::NeedsAttention);
        assert_eq!(status.unjournaled_items, ["b"]);
        assert_eq!(lab.store.lock().unwrap().rows(op).unwrap(), before);
    }
    drop(deny);
    lab.reopen();
    assert_eq!(
        recover(&lab.store, op, &Control::new(false))
            .unwrap()
            .unjournaled_items,
        ["a", "b"]
    );
}

#[test]
fn cancel_unreadable_published_copy_does_not_withdraw_or_journal() {
    cancel_unreadable_published_copy(false);
}

#[test]
fn revocation_cancel_unreadable_published_copy_does_not_withdraw_or_journal() {
    cancel_unreadable_published_copy(true);
}

fn cancel_unreadable_published_copy(revoked: bool) {
    let Some(mut lab) = Lab::new() else { return };
    fs::write(lab.src().join("f"), b"payload").unwrap();
    let op = lab.start(&["f"]);
    let crash = Control::new(false).with_hook(|p, _| {
        if p == Point::AfterPublish {
            Flow::Crash
        } else {
            Flow::Continue
        }
    });
    assert!(matches!(run(&lab.store, op, &crash), Err(Fault::Crashed)));
    lab.reopen();
    let control = Control::new(true);
    if revoked {
        fence(
            &lab.store,
            op,
            &control,
            loomward_operations::FenceCause::GrantRevoked,
        )
        .unwrap();
    }
    let before = lab.store.lock().unwrap().rows(op).unwrap();
    let plan = lab.plan(op);
    let published = lab.dest().join(&plan.manifest.destination.folder_name);
    let deny = DenyCreate::with(&published, "(R)");
    assert!(identity::observe(&published).is_err());
    let outcome = cancel(&lab.store, op, &plan, &control).unwrap();
    assert_eq!(lab.store.lock().unwrap().rows(op).unwrap(), before);
    assert_eq!(outcome.state, OpState::NeedsAttention);
    drop(deny);
    lab.reopen();
    assert_eq!(
        recover(&lab.store, op, &Control::new(true)).unwrap().state,
        OpState::Committed
    );
    assert_eq!(fs::read(published.join("f")).unwrap(), b"payload");
}

#[test]
fn missing_destination_parent_is_retryable_not_staging_damage() {
    for (point, replaced) in [
        (Point::Verified(0), false),
        (Point::AfterPublish, false),
        (Point::Verified(0), true),
        (Point::AfterPublish, true),
    ] {
        let Some(mut lab) = Lab::new() else { return };
        fs::write(lab.src().join("f"), b"payload").unwrap();
        let op = lab.start(&["f"]);
        let crash = Control::new(false).with_hook(move |p, _| {
            if p == point {
                Flow::Crash
            } else {
                Flow::Continue
            }
        });
        assert!(matches!(run(&lab.store, op, &crash), Err(Fault::Crashed)));
        lab.reopen();
        let before = lab.store.lock().unwrap().rows(op).unwrap();
        let absent = lab.tmp.path().join("detached");
        fs::rename(lab.dest(), &absent).unwrap();
        if replaced {
            fs::create_dir(lab.dest()).unwrap();
        }
        assert_eq!(
            recover(&lab.store, op, &Control::new(false)).unwrap().state,
            OpState::NeedsAttention
        );
        assert_eq!(
            lab.store.lock().unwrap().rows(op).unwrap(),
            before,
            "{point:?}"
        );
        if replaced {
            fs::remove_dir(lab.dest()).unwrap();
        }
        fs::rename(absent, lab.dest()).unwrap();
        lab.reopen();
        let outcome = recover(&lab.store, op, &Control::new(false)).unwrap();
        assert_eq!(
            outcome.state,
            if point == Point::AfterPublish {
                OpState::Committed
            } else {
                OpState::Copying
            }
        );
    }
}

#[test]
fn admitted_step_rechecks_epoch_even_when_control_is_fenced() {
    let Some(lab) = Lab::new() else { return };
    fs::write(lab.src().join("f"), b"payload").unwrap();
    let op = lab.start(&["f"]);
    let crash = Control::new(false).with_hook(|p, _| {
        if p == Point::Identities(0) {
            Flow::Crash
        } else {
            Flow::Continue
        }
    });
    assert!(matches!(run(&lab.store, op, &crash), Err(Fault::Crashed)));
    let epoch = replay(&lab.store, op).unwrap().epoch;
    let journal = Journal {
        store: &lab.store,
        op,
        epoch,
    };
    let control = Control::new(false);
    let step = control
        .admit()
        .unwrap_or_else(|_| panic!("step was refused"));
    lab.store
        .lock()
        .unwrap()
        .append(
            op,
            epoch + 1,
            Record::Fenced {
                cause: loomward_operations::FenceCause::GrantRevoked,
            },
        )
        .unwrap();
    control.fenced.store(true, Ordering::SeqCst);
    assert!(matches!(journal.check_copying(), Err(Stop::Superseded)));
    drop(step);
}

fn recovery_observation_failure(published: bool) {
    let Some(mut lab) = Lab::new() else { return };
    fs::create_dir(lab.src().join("top")).unwrap();
    fs::write(lab.src().join("top/f"), b"payload").unwrap();
    let op = lab.start(&["top"]);
    let crash = Control::new(false).with_hook(move |p, _| {
        if p == if published {
            Point::AfterPublish
        } else {
            Point::Verified(0)
        } {
            Flow::Crash
        } else {
            Flow::Continue
        }
    });
    assert!(matches!(run(&lab.store, op, &crash), Err(Fault::Crashed)));
    lab.reopen();
    let before = replay(&lab.store, op).unwrap().state;
    let blocked = if published {
        lab.dest().join("top")
    } else {
        let r = replay(&lab.store, op).unwrap();
        lab.dest().join(r.staging_tokens.last().unwrap()).join("f")
    };
    let hold = DenyCreate::with(&blocked, "(R)");
    assert!(
        identity::observe(&blocked).is_err(),
        "observation really failed"
    );
    let unavailable = recover(&lab.store, op, &Control::new(false)).unwrap();
    assert_eq!(unavailable.state, OpState::NeedsAttention);
    assert_eq!(
        replay(&lab.store, op).unwrap().state,
        before,
        "unreadable is not damage"
    );
    drop(hold);
    lab.reopen();
    let outcome = recover(&lab.store, op, &Control::new(false)).unwrap();
    assert_eq!(
        outcome.state,
        if published {
            OpState::Committed
        } else {
            before
        }
    );
    assert_eq!(
        fs::read(if published {
            blocked.join("f")
        } else {
            blocked
        })
        .unwrap(),
        b"payload"
    );
}

#[test]
fn recovery_persists_confirmed_staging_damage() {
    for replaced in [false, true] {
        let Some(mut lab) = Lab::new() else { return };
        fs::write(lab.src().join("f"), b"payload").unwrap();
        let op = lab.start(&["f"]);
        let control = Control::new(false).with_hook(|p, _| {
            if p == Point::Identities(0) {
                Flow::Crash
            } else {
                Flow::Continue
            }
        });
        assert!(matches!(run(&lab.store, op, &control), Err(Fault::Crashed)));
        let staging = lab.dest().join(&staging_dirs(&lab)[0]);
        fs::rename(&staging, lab.dest().join("displaced")).unwrap();
        if replaced {
            fs::create_dir(&staging).unwrap();
        }
        lab.reopen();
        assert_eq!(
            recover(&lab.store, op, &Control::new(false)).unwrap().state,
            OpState::NeedsAttention
        );
        lab.reopen();
        assert_eq!(
            replay(&lab.store, op).unwrap().state,
            OpState::NeedsAttention
        );
        assert_eq!(fs::read(lab.dest().join("displaced/f")).unwrap(), b"");
    }
}

#[test]
fn detached_worker_checks_durable_stop_before_creation() {
    detached_worker_checks_durable_stop(Point::Creating(0));
}

#[test]
fn detached_worker_checks_durable_stop_before_stream_creation() {
    detached_worker_checks_durable_stop(Point::CreatingStream(0));
}

#[test]
fn detached_worker_checks_durable_stop_before_write() {
    detached_worker_checks_durable_stop(Point::Writing(0));
}

fn detached_worker_checks_durable_stop(point: Point) {
    let Some(lab) = Lab::new() else { return };
    fs::write(lab.src().join("f.bin"), b"payload").unwrap();
    fs::write(lab.src().join("f.bin:note"), b"stream").unwrap();
    let op = lab.start(&["f.bin"]);
    let db = lab.tmp.path().join("state/state.db");
    let control = Control::new(false).with_hook(move |p, _| {
        if p == point {
            let mut reopened = Store::open(&db).unwrap();
            let r = reopened.replay(op).unwrap().unwrap();
            reopened
                .append(
                    op,
                    r.epoch + 1,
                    Record::Fenced {
                        cause: loomward_operations::FenceCause::GrantRevoked,
                    },
                )
                .unwrap();
        }
        Flow::Continue
    });
    let _ = run(&lab.store, op, &control);
    assert!(
        !control.fenced(),
        "detached control never received the in-memory fence"
    );
    let r = replay(&lab.store, op).unwrap();
    assert!(r.fenced);
    let file = lab
        .dest()
        .join(r.staging_tokens.last().unwrap())
        .join("f.bin");
    match point {
        Point::Creating(_) => assert!(!file.exists(), "created after durable stop"),
        Point::CreatingStream(_) => assert!(
            !file.with_file_name("f.bin:note").exists(),
            "stream created after durable stop"
        ),
        Point::Writing(_) => assert_eq!(
            fs::metadata(&file).unwrap().len(),
            0,
            "written after durable stop"
        ),
        _ => unreachable!(),
    }
}

#[test]
fn detached_worker_check_and_create_exclude_a_reopened_fence() {
    detached_worker_check_and_create_exclude_fence(Point::Checked(0));
}

#[test]
fn detached_worker_check_and_stream_create_exclude_a_reopened_fence() {
    detached_worker_check_and_create_exclude_fence(Point::CheckedStream(0));
}

/// #670: a reopened service fences on its own store and `Control` while the detached worker sits
/// between its durable stop check and the create call. The fence must not be acknowledged while
/// the create can still follow: once acknowledged, the object either already existed or never will.
fn detached_worker_check_and_create_exclude_fence(point: Point) {
    let Some(lab) = Lab::new() else { return };
    fs::write(lab.src().join("f.bin"), b"payload").unwrap();
    fs::write(lab.src().join("f.bin:note"), b"stream").unwrap();
    let op = lab.start(&["f.bin"]);
    let db = lab.tmp.path().join("state/state.db");
    let dest = lab.dest();
    let target = move |token: &str| {
        let file = dest.join(token).join("f.bin");
        match point {
            Point::Checked(_) => file,
            _ => file.with_file_name("f.bin:note"),
        }
    };
    let fencer = std::sync::Arc::new(Mutex::new(None));
    let slot = fencer.clone();
    let control = Control::new(false).with_hook(move |p, _| {
        if p == point {
            let (db, target) = (db.clone(), target.clone());
            let (tx, rx) = std::sync::mpsc::channel();
            *slot.lock().unwrap() = Some(std::thread::spawn(move || {
                let reopened = Mutex::new(Store::open(&db).unwrap());
                let ack = fence(
                    &reopened,
                    op,
                    &Control::new(false),
                    loomward_operations::FenceCause::GrantRevoked,
                )
                .unwrap();
                let token = replay(&reopened, op).unwrap().staging_tokens.pop().unwrap();
                let _ = tx.send(());
                (ack, target(&token).exists(), target(&token))
            }));
            // Give an unserialized fence ample time to land inside the window.
            let _ = rx.recv_timeout(Duration::from_millis(500));
        }
        Flow::Continue
    });
    let _ = run(&lab.store, op, &control);
    let fencer = fencer.lock().unwrap().take().expect("seam reached");
    let (ack, existed_at_ack, path) = fencer.join().unwrap();
    assert_eq!(ack, Ack::StoppedBeforePublish);
    assert!(replay(&lab.store, op).unwrap().fenced);
    assert!(
        existed_at_ack || !path.exists(),
        "{point:?}: created after the reopened service's fence was acknowledged"
    );
}

#[test]
fn explicit_destination_aces_prevent_normal_commit() {
    destination_aces_prevent_commit(false);
}

#[test]
fn explicit_destination_aces_prevent_recovery_commit() {
    destination_aces_prevent_commit(true);
}

fn destination_aces_prevent_commit(recovery: bool) {
    for selection in [&["top"][..], &["top/sub", "top/g"][..]] {
        for target in ["", "sub", "sub/f"] {
            let Some(mut lab) = Lab::new() else { return };
            fs::create_dir_all(lab.src().join("top/sub")).unwrap();
            fs::write(lab.src().join("top/sub/f"), b"payload").unwrap();
            fs::write(lab.src().join("top/g"), b"another file").unwrap();
            let before = source_snapshot(&lab.src());
            let op = lab.start(selection);
            let published = lab
                .dest()
                .join(&lab.plan(op).manifest.destination.folder_name);
            let changed = published.join(target);
            let inject = move || {
                assert!(std::process::Command::new("icacls")
                    .arg(&changed)
                    .args(["/grant", "*S-1-1-0:(R)"])
                    .output()
                    .unwrap()
                    .status
                    .success());
            };
            let control = Control::new(false).with_hook(move |p, _| {
                if p == Point::AfterPublish {
                    if recovery {
                        return Flow::Crash;
                    }
                    inject();
                }
                Flow::Continue
            });
            let result = run(&lab.store, op, &control);
            let outcome = if recovery {
                assert!(matches!(result, Err(Fault::Crashed)));
                assert!(std::process::Command::new("icacls")
                    .arg(published.join(target))
                    .args(["/grant", "*S-1-1-0:(R)"])
                    .output()
                    .unwrap()
                    .status
                    .success());
                lab.reopen();
                recover(&lab.store, op, &Control::new(false)).unwrap()
            } else {
                result.unwrap()
            };
            let report = tree::compare(&c1(&lab.src().join("top")), &c1(&published));
            assert!(
                report.differences.iter().any(|d| d.kind
                    == tree::DifferenceKind::Metadata(tree::MetadataClass::SecurityDescriptor)),
                "C1 must reject the explicit ACE: {report:?}"
            );
            assert_eq!(
                outcome.state,
                OpState::NeedsAttention,
                "recovery={recovery}, target={target}: {outcome:?}"
            );
            assert_eq!(
                outcome.refusal,
                Some((Refusal::ChangedAfterVerification, None))
            );
            assert_eq!(fs::read(published.join("sub/f")).unwrap(), b"payload");
            assert_eq!(source_snapshot(&lab.src()), before);
        }
    }
}

/// #526: the bounded publish retry (10 x 3 s) ends at once when the owner cancels or the service shuts
/// down: the run waits `blocked` (`publish_in_use`, resumable) with the intent kept, never ~30 s later.
#[test]
fn cancel_ends_the_publish_retry_wait_at_once() {
    let Some(lab) = Lab::new() else { return };
    fs::create_dir(lab.src().join("top")).unwrap();
    fs::write(lab.src().join("top/f.txt"), b"data").unwrap();
    fs::write(lab.src().join("top/f.txt:note"), b"kept").unwrap();
    let op = lab.start(&["top"]);
    let dest = lab.dest();
    // A held named-stream handle under staging refuses the rename (H2 negative for streams).
    let blocker: std::sync::Arc<Mutex<Option<fs::File>>> = Default::default();
    let held = blocker.clone();
    let control = Control::new(false).with_hook(move |p, c| {
        if p == Point::BeforePublish {
            let staging = fs::read_dir(&dest).unwrap().next().unwrap().unwrap().path();
            *held.lock().unwrap() = Some(fs::File::open(staging.join("f.txt:note")).unwrap());
            c.cancel();
        }
        Flow::Continue
    });
    let started = std::time::Instant::now();
    let outcome = run(&lab.store, op, &control).unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "cancel waited {:?}",
        started.elapsed()
    );
    assert_eq!(outcome.state, OpState::Blocked, "{outcome:?}");
    assert_eq!(outcome.refusal, Some((Refusal::PublishInUse, None)));
    drop(blocker);
    assert!(!lab.dest().join("top").exists(), "never published");
}

/// Run `op` on its own thread; when `at` first fires, the run waits there while the test revokes it
/// (`fence`), then goes on. Returns the run's outcome and the revocation's acknowledgement.
fn fenced_at(lab: &Lab, op: u64, at: fn(Point) -> bool) -> (Outcome, Ack) {
    let (reached, wait) = std::sync::mpsc::channel::<()>();
    let (go, resume) = std::sync::mpsc::channel::<()>();
    let resume = Mutex::new(resume);
    let fired = AtomicBool::new(false);
    let control = Control::new(false).with_hook(move |p, _| {
        if at(p) && !fired.swap(true, Ordering::SeqCst) {
            reached.send(()).unwrap();
            resume.lock().unwrap().recv().unwrap();
        }
        Flow::Continue
    });
    std::thread::scope(|s| {
        let worker = s.spawn(|| run(&lab.store, op, &control).unwrap());
        wait.recv().unwrap();
        let ack = fence(
            &lab.store,
            op,
            &control,
            loomward_operations::FenceCause::GrantRevoked,
        )
        .unwrap();
        go.send(()).unwrap();
        (worker.join().unwrap(), ack)
    })
}

fn journal_kinds(lab: &Lab, op: u64) -> Vec<String> {
    lab.store
        .lock()
        .unwrap()
        .rows(op)
        .unwrap()
        .iter()
        .map(|r| {
            let e: loomward_operations::Entry = serde_json::from_str(&r.body).unwrap();
            let v = serde_json::to_value(e.record).unwrap();
            v["kind"].as_str().unwrap_or("?").to_string()
        })
        .collect()
}

/// #526 revocation wiring (docs/47 section 5): a revocation acknowledged before the rename step stops
/// the run at its next effect step; nothing more is created, written or renamed, a journaled publish
/// intent is withdrawn, and the run is cancelled with `grant_revoked` (Q1 default: staging left).
#[test]
fn a_revocation_before_the_rename_cancels_the_run() {
    let points: [fn(Point) -> bool; 3] = [
        |p| p == Point::StagingCreated,
        |p| matches!(p, Point::Identities(_)),
        |p| p == Point::BeforePublish,
    ];
    for (n, at) in points.into_iter().enumerate() {
        let Some(lab) = Lab::new() else { return };
        tree_fixture(&lab.src());
        let before = source_snapshot(&lab.src());
        let op = lab.start(&["top"]);
        let (outcome, ack) = fenced_at(&lab, op, at);
        assert_eq!(ack, Ack::StoppedBeforePublish, "point {n}");
        assert_eq!(outcome.state, OpState::Cancelled, "point {n} {outcome:?}");
        assert_eq!(
            outcome.refusal,
            Some((Refusal::GrantRevoked, None)),
            "point {n}"
        );
        assert!(
            !lab.dest().join("top").exists(),
            "point {n}: never published"
        );
        assert_eq!(staging_dirs(&lab).len(), 1, "point {n}: staging left");
        // Nothing advanced the run after the fence: no create, write or publish record follows it.
        let kinds = journal_kinds(&lab, op);
        let fenced = kinds.iter().position(|k| k == "fenced").unwrap();
        assert!(
            kinds[fenced + 1..].iter().all(|k| matches!(
                k.as_str(),
                "transition" | "publish_withdrawn" | "staging_left"
            )),
            "point {n}: {kinds:?}"
        );
        assert_eq!(
            kinds.contains(&"publish_withdrawn".to_string()),
            n == 2,
            "point {n}: {kinds:?}"
        );
        if n == 1 {
            // The first batch's files exist empty: no byte was written after the fence.
            let staging = lab.dest().join(&staging_dirs(&lab)[0]);
            let written: u64 = fs::read_dir(&staging)
                .unwrap()
                .map(|e| e.unwrap().metadata().unwrap())
                .filter(|m| m.is_file())
                .map(|m| m.len())
                .sum();
            assert_eq!(written, 0, "bytes written after the fence");
        }
        assert_eq!(
            source_snapshot(&lab.src()),
            before,
            "point {n}: the source changed"
        );
    }
}

/// After the rename step read an unchanged epoch, the revocation only annotates: the post-publish
/// confirmation still runs and the copy commits, reported with `grant_revoked`.
#[test]
fn a_revocation_after_the_rename_commits_annotated() {
    let Some(lab) = Lab::new() else { return };
    tree_fixture(&lab.src());
    let op = lab.start(&["top"]);
    let (outcome, ack) = fenced_at(&lab, op, |p| p == Point::AfterPublish);
    assert_eq!(ack, Ack::Published);
    assert_eq!(outcome.state, OpState::Committed, "{outcome:?}");
    assert_eq!(outcome.refusal, Some((Refusal::GrantRevoked, None)));
    let report = tree::compare(&c1(&lab.src().join("top")), &c1(&lab.dest().join("top")));
    assert!(report.equal(), "{report:#?}");
}

/// Review of #544 (HIGH 2): a fence or a cancel landing inside a file's chunk loop stops the run. It once
/// spun forever, because `read_exact` retries `Interrupted`. Run on a detached thread with a timeout, so
/// a regression fails instead of hanging the suite.
#[test]
fn a_fence_or_cancel_mid_file_stops_the_run() {
    for fenced in [true, false] {
        let Some(lab) = Lab::new() else { return };
        fs::create_dir(lab.src().join("top")).unwrap();
        fs::write(lab.src().join("top/big.bin"), vec![5u8; 4 << 20]).unwrap();
        let op = lab.start(&["top"]);
        let lab = std::sync::Arc::new(lab);
        let (reached, wait) = std::sync::mpsc::channel::<()>();
        let (go, resume) = std::sync::mpsc::channel::<()>();
        let resume = Mutex::new(resume);
        let fired = AtomicBool::new(false);
        let control = std::sync::Arc::new(Control::new(false).with_hook(move |p, _| {
            if p == Point::Chunk(1 << 20) && !fired.swap(true, Ordering::SeqCst) {
                reached.send(()).unwrap();
                resume.lock().unwrap().recv().unwrap();
            }
            Flow::Continue
        }));
        let (done, outcome) = std::sync::mpsc::channel();
        {
            let (lab, control) = (lab.clone(), control.clone());
            std::thread::spawn(move || {
                let _ = done.send(run(&lab.store, op, &control).unwrap());
            });
        }
        wait.recv_timeout(Duration::from_secs(60)).unwrap();
        if fenced {
            let ack = fence(
                &lab.store,
                op,
                &control,
                loomward_operations::FenceCause::GrantRevoked,
            )
            .unwrap();
            assert_eq!(ack, Ack::StoppedBeforePublish);
        } else {
            control.cancel();
        }
        go.send(()).unwrap();
        let outcome = outcome
            .recv_timeout(Duration::from_secs(30))
            .unwrap_or_else(|_| panic!("fenced {fenced}: the run never stopped"));
        assert_eq!(outcome.state, OpState::Cancelled, "{fenced} {outcome:?}");
        if fenced {
            assert_eq!(outcome.refusal, Some((Refusal::GrantRevoked, None)));
        }
        // No chunk after the stop: only the first 1 MiB reached the staged file.
        let staging = lab.dest().join(&staging_dirs(&lab)[0]);
        let len = fs::metadata(staging.join("big.bin")).unwrap().len();
        assert!(len <= 1 << 20, "fenced {fenced}: {len} bytes");
    }
}

/// The admission lock: a revocation waits for the one effect step in flight, and only for it; a step
/// that does not return in time leaves the revocation `pending_in_flight_step`.
#[test]
fn a_fence_waits_for_the_in_flight_step_only() {
    let control = Control::new(false);
    std::thread::scope(|s| {
        let (entered, wait) = std::sync::mpsc::channel();
        let control = &control;
        s.spawn(move || {
            let _step = control.admit().ok().unwrap();
            entered.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(300));
        });
        wait.recv().unwrap();
        assert_eq!(control.fence(Duration::from_millis(20)), None, "pending");
        let started = std::time::Instant::now();
        assert_eq!(control.fence(Duration::from_secs(30)), Some(false));
        assert!(started.elapsed() < Duration::from_secs(5));
    });
    // Every later step reads the fence under the lock and stops.
    assert!(control.admit().is_err());
}

/// #526: folders are checked for named streams and EAs too (section 6 refuses both on folders at
/// preview, so any found later was added). Before publish (the staging folder) and after the rename
/// (a subfolder, checked by the post-publish confirmation): never committed.
#[test]
fn a_stream_planted_on_a_staged_folder_is_never_committed() {
    for after_rename in [false, true] {
        let Some(lab) = Lab::new() else { return };
        fs::create_dir_all(lab.src().join("top/sub")).unwrap();
        fs::write(lab.src().join("top/sub/f.txt"), b"data").unwrap();
        let op = lab.start(&["top"]);
        let dest = lab.dest();
        let control = Control::new(false).with_hook(move |p, _| {
            let folder = || fs::read_dir(&dest).unwrap().next().unwrap().unwrap().path();
            match p {
                Point::Verified(_) if !after_rename => {
                    let mut stream = folder().into_os_string();
                    stream.push(":planted");
                    fs::write(stream, b"x").unwrap()
                }
                Point::AfterPublish if after_rename => {
                    fs::write(folder().join("sub:planted"), b"x").unwrap()
                }
                _ => {}
            }
            Flow::Continue
        });
        let outcome = run(&lab.store, op, &control).unwrap();
        assert_eq!(
            outcome.state,
            OpState::NeedsAttention,
            "{after_rename} {outcome:?}"
        );
        assert_eq!(
            outcome.refusal,
            Some((Refusal::ChangedAfterVerification, None))
        );
    }
}
