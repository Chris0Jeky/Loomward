//! #473: the C1 tree gate of `Engine::compare_trees`, pinned where the q-10 admission checks
//! could otherwise move unseen: both durable grants admitted before any read, the durable and
//! in-memory recheck after the reads, and content observed on the success path.
//!
//! Production code calls [`point`] at three places and nothing else; a test installs a hook
//! for its own thread with [`install`]. Each pin fails if the line it names is removed or moved.
#![cfg_attr(not(windows), allow(dead_code))]
#[cfg(windows)]
use super::source::FileIdObs;
#[cfg(windows)]
use super::tests::{canonical_tempdir, duplicate_budget, duplicate_grant_store, skip_elevated};
#[cfg(windows)]
use super::*;
#[cfg(windows)]
use crate::EngineConfig;
#[cfg(windows)]
use loomward_operations::tree::{DifferenceKind, Field, Report};
#[cfg(windows)]
use loomward_protocol::{DatasetClass, RootId};
#[cfg(windows)]
use loomward_windows::enumerate::{DirSource, NativeSource};
use std::cell::RefCell;
#[cfg(windows)]
use std::rc::Rc;

/// The points at which `Engine::compare_trees` pauses for a test hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TreePoint {
    /// The source observation returned; the destination read has not started.
    AfterSourceRead,
    /// The destination observation returned; the end-of-run recheck has not started.
    AfterDestinationRead,
    /// Both end-of-run durable rechecks passed; the in-memory gate guard has not run.
    BeforeGuard,
}

type Hook = Box<dyn FnMut(TreePoint)>;

thread_local! {
    static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
}

/// Called by `Engine::compare_trees`; does nothing unless this thread installed a hook.
pub(super) fn point(at: TreePoint) {
    // Take the hook out while it runs, so a hook never borrows its own slot.
    let taken = HOOK.with(|slot| slot.borrow_mut().take());
    if let Some(mut hook) = taken {
        hook(at);
        HOOK.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                *slot = Some(hook);
            }
        });
    }
}

/// Installs `hook` on this thread until the returned guard drops.
fn install(hook: impl FnMut(TreePoint) + 'static) -> HookGuard {
    HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
    HookGuard
}

struct HookGuard;

impl Drop for HookGuard {
    fn drop(&mut self) {
        HOOK.with(|slot| *slot.borrow_mut() = None);
    }
}

/// Records every point `compare_trees` passes while the returned guard is alive.
#[cfg(windows)]
fn record_points() -> (Rc<RefCell<Vec<TreePoint>>>, HookGuard) {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let guard = install({
        let seen = seen.clone();
        move |at| seen.borrow_mut().push(at)
    });
    (seen, guard)
}

/// Two granted roots, each with a `fixture` file, and the grant store that names both: the
/// source is row 1 and the destination row 2, as `compare_trees` admits them.
#[cfg(windows)]
struct Pair {
    engine: Engine,
    left: GrantedRoot,
    right: GrantedRoot,
    db: std::path::PathBuf,
    // Keeps the three directories alive for the test.
    _dirs: [tempfile::TempDir; 3],
}

#[cfg(windows)]
fn pair_of(left: &[u8], right: &[u8]) -> Pair {
    let a = canonical_tempdir();
    let b = canonical_tempdir();
    let state = canonical_tempdir();
    std::fs::write(a.path().join("fixture"), left).unwrap();
    std::fs::write(b.path().join("fixture"), right).unwrap();
    let grant = |name, path: &std::path::Path| {
        GrantedRoot::new(
            RootId::new(name).unwrap(),
            path.into(),
            DatasetClass::Synthetic,
            NativeSource::default().open_root(path).unwrap().1,
        )
    };
    let left = grant("rt_tree_left", a.path());
    let right = grant("rt_tree_right", b.path());
    let conn = duplicate_grant_store(state.path(), &left);
    let identity = right.expected.unwrap();
    let FileIdObs::Id128(id) = identity.id.unwrap() else {
        panic!("fixture needs native identity")
    };
    conn.execute(
        "INSERT INTO root_grant(id,display_path,volume_key,root_file_id,state,volume_guid) VALUES(2,?1,?2,?3,'active',?4)",
        rusqlite::params![
            b.path().to_str().unwrap(),
            format!("vsn_{}", identity.volume_serial.unwrap()),
            id.as_slice(),
            identity.volume_guid.map(|g| format!("{g:032x}")),
        ],
    )
    .unwrap();
    drop(conn);
    let engine = Engine::open(EngineConfig::new(
        state.path().into(),
        DatasetClass::Synthetic,
    ))
    .unwrap();
    Pair {
        engine,
        left,
        right,
        db: state.path().join("state.db"),
        _dirs: [a, b, state],
    }
}

#[cfg(windows)]
fn set_state(db: &std::path::Path, id: i64, state: &str) {
    rusqlite::Connection::open(db)
        .unwrap()
        .execute(
            "UPDATE root_grant SET state=?1 WHERE id=?2",
            rusqlite::params![state, id],
        )
        .unwrap();
}

#[cfg(windows)]
fn refusal(result: EngineResult<Report>) -> String {
    result.unwrap_err().to_body().message.as_str().to_owned()
}

/// (a) Both durable grants are admitted before either root is read. With the destination
/// denied, the source must not be observed first: no read point may be passed at all.
#[cfg(windows)]
#[test]
fn both_durable_grants_are_admitted_before_any_read() {
    if skip_elevated() {
        return;
    }
    let pair = pair_of(b"same", b"same");
    let (seen, _guard) = record_points();
    pair.engine
        .compare_trees(&pair.left, &pair.right, duplicate_budget(), 100)
        .unwrap();
    assert_eq!(
        *seen.borrow(),
        [
            TreePoint::AfterSourceRead,
            TreePoint::AfterDestinationRead,
            TreePoint::BeforeGuard
        ]
    );
    seen.borrow_mut().clear();
    set_state(&pair.db, 2, "revoked");
    assert_eq!(
        refusal(
            pair.engine
                .compare_trees(&pair.left, &pair.right, duplicate_budget(), 100)
        ),
        "grant_not_active"
    );
    assert!(seen.borrow().is_empty(), "{:?}", seen.borrow());
    set_state(&pair.db, 2, "active");
    set_state(&pair.db, 1, "revoked");
    assert_eq!(
        refusal(
            pair.engine
                .compare_trees(&pair.left, &pair.right, duplicate_budget(), 100)
        ),
        "grant_not_active"
    );
    assert!(seen.borrow().is_empty(), "{:?}", seen.borrow());
}

/// (b) A durable revocation that lands mid-read, after a root's observation and before the
/// end-of-run recheck, refuses the comparison. No in-memory flag is set, so only the durable
/// recheck can see it.
#[cfg(windows)]
#[test]
fn durable_revocation_mid_read_is_refused_by_the_end_of_run_recheck() {
    if skip_elevated() {
        return;
    }
    for (row, at) in [
        (1, TreePoint::AfterSourceRead),
        (2, TreePoint::AfterDestinationRead),
    ] {
        let pair = pair_of(b"same", b"same");
        let db = pair.db.clone();
        let _guard = install(move |point| {
            if point == at {
                set_state(&db, row, "revoked");
            }
        });
        assert_eq!(
            refusal(
                pair.engine
                    .compare_trees(&pair.left, &pair.right, duplicate_budget(), 100)
            ),
            "grant_not_active",
            "revoked row {row} at {at:?}"
        );
    }
}

/// (b) The in-memory revocation flag and the cancellation flag are each refused at the final
/// gate, after the durable recheck has passed.
#[cfg(windows)]
#[test]
fn in_memory_revocation_is_refused_at_the_final_gate() {
    if skip_elevated() {
        return;
    }
    type Set = fn(&mut RootGate);
    let cases: [(&str, Set); 2] = [
        ("revoked flag", |gate| gate.revoked = true),
        ("cancel flag", |gate| {
            gate.duplicate_cancel.store(true, Ordering::Release)
        }),
    ];
    for (name, set) in cases {
        let pair = pair_of(b"same", b"same");
        let gate = pair.engine.scan_root_gate(pair.left.key());
        let _guard = install(move |point| {
            if point == TreePoint::BeforeGuard {
                set(&mut gate.lock().unwrap());
            }
        });
        assert_eq!(
            refusal(
                pair.engine
                    .compare_trees(&pair.left, &pair.right, duplicate_budget(), 100)
            ),
            "cancelled",
            "{name}"
        );
    }
}

/// (c) On the success path both roots are content-hashed. Equal-size, different bytes are a
/// reported Content difference; a read that returned only error nodes would report unknowns.
#[cfg(windows)]
#[test]
fn success_path_hashes_content_on_both_roots() {
    if skip_elevated() {
        return;
    }
    let pair = pair_of(b"aaaa", b"aaab");
    let report = pair
        .engine
        .compare_trees(&pair.left, &pair.right, duplicate_budget(), 100)
        .unwrap();
    let fixture = std::path::Path::new("fixture");
    assert!(
        report
            .differences
            .iter()
            .any(|d| d.path == fixture && d.kind == DifferenceKind::Content),
        "{report:?}"
    );
    assert!(
        !report
            .unknowns
            .iter()
            .any(|u| u.path == fixture && u.field == Field::Content),
        "{report:?}"
    );
}

/// #1011: `observe_coverage` hashes a B file only when an A file of its size was hashed, so B is
/// never read in full; a renamed and moved copy is covered by content; a byte-budget stop leaves
/// A files unknown, never covered. Same admitted pair as `compare_trees`.
#[cfg(windows)]
#[test]
fn coverage_reads_only_matching_b_sizes_and_covers_a_renamed_copy() {
    use loomward_operations::containment::{containment, Coverage};
    use std::path::Path;
    if skip_elevated() {
        return;
    }
    let pair = pair_of(b"same", b"same");
    let (a, b) = (pair.left.path(), pair.right.path());
    std::fs::create_dir(a.join("photos")).unwrap();
    std::fs::write(a.join("photos").join("one.jpg"), b"photo-bytes").unwrap();
    std::fs::write(a.join("missing.txt"), b"nope!").unwrap();
    std::fs::create_dir(b.join("archive")).unwrap();
    std::fs::write(b.join("archive").join("renamed.jpg"), b"photo-bytes").unwrap();
    std::fs::write(b.join("same-size.bin"), b"other-bytes").unwrap();
    let big = 256 * 1024;
    std::fs::write(b.join("big.bin"), vec![7u8; big]).unwrap();
    let budget = |bytes| loomward_windows::duplicate_reader::Budget {
        bytes,
        time: std::time::Duration::from_secs(30),
    };
    let (a_tree, b_tree, bytes) = pair
        .engine
        .observe_coverage(&pair.left, &pair.right, budget(1024 * 1024), 100)
        .unwrap();
    let content = |tree: &loomward_operations::tree::Tree, path: &Path| {
        tree.nodes[path].as_ref().unwrap().content
    };
    assert_eq!(content(&b_tree, Path::new("big.bin")), Err("not_requested"));
    assert_eq!(
        content(&a_tree, Path::new("missing.txt")),
        Err("not_requested")
    );
    assert!(content(&b_tree, &Path::new("archive").join("renamed.jpg")).is_ok());
    assert!(
        (bytes as usize) < big,
        "B was read beyond its matching sizes: {bytes}"
    );
    let report = containment(&a_tree, &b_tree);
    let found: Vec<_> = report
        .items
        .iter()
        .map(|item| (item.path.clone(), item.coverage.clone()))
        .collect();
    assert_eq!(
        found,
        vec![
            (
                "fixture".into(),
                Coverage::Covered {
                    by: "fixture".into()
                }
            ),
            ("missing.txt".into(), Coverage::NotCovered),
            (
                Path::new("photos").join("one.jpg"),
                Coverage::Covered {
                    by: Path::new("archive").join("renamed.jpg")
                }
            ),
        ]
    );
    let (a_tree, b_tree, bytes) = pair
        .engine
        .observe_coverage(&pair.left, &pair.right, budget(0), 100)
        .unwrap();
    assert_eq!(bytes, 0);
    let report = containment(&a_tree, &b_tree);
    assert!(!report
        .items
        .iter()
        .any(|item| matches!(item.coverage, Coverage::Covered { .. })));
    assert!(report.totals().unknown.files >= 2, "{report:?}");
}

/// #1037: with a selection, `compare_selected_trees` hashes only selected source files and files
/// under a selected folder. The 1 KiB budget fits the unselected 1000-byte file, which lists
/// before `planned`, but not that file plus `planned/inside`: the selected run passes only if the
/// file costs nothing, and the default run, which hashes everything, does not.
/// The selected file's verdict is unchanged: equal stays equal and a difference is still caught.
#[cfg(windows)]
#[test]
fn selected_run_never_hashes_unselected_source_files() {
    use std::path::PathBuf;
    if skip_elevated() {
        return;
    }
    let small = loomward_windows::duplicate_reader::Budget {
        bytes: 1024,
        time: std::time::Duration::from_secs(30),
    };
    let select = |paths: &[&str]| -> std::collections::BTreeSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    };
    // A source file the budget did not reach has `Content` unknown; the report cannot show an
    // unselected file's own read (it is only `Missing`), so the observable is the files after it.
    let budget_stops = |report: &Report| report.unknowns.iter().any(|u| u.field == Field::Content);
    for (destination, differs) in [(&b"same"[..], false), (&b"diff"[..], true)] {
        let pair = pair_of(b"same", destination);
        let (a, b) = (pair.left.path(), pair.right.path());
        std::fs::create_dir(a.join("planned")).unwrap();
        std::fs::write(a.join("planned").join("inside"), [3u8; 100]).unwrap();
        std::fs::create_dir(b.join("planned")).unwrap();
        std::fs::write(b.join("planned").join("inside"), [3u8; 100]).unwrap();
        std::fs::write(a.join("left_behind"), [7u8; 1000]).unwrap();
        let run = |selection: Option<&[&str]>| {
            let selection = selection.map(select);
            pair.engine
                .compare_selected_trees(&pair.left, &pair.right, small, 100, selection.as_ref())
                .unwrap()
        };
        let content_diffs = |report: &Report| -> Vec<PathBuf> {
            report
                .differences
                .iter()
                .filter(|d| d.kind == DifferenceKind::Content)
                .map(|d| d.path.clone())
                .collect()
        };
        // The default path hashes `left_behind` (listed before `planned`) and exhausts the budget.
        let whole = run(None);
        assert!(budget_stops(&whole), "{whole:?}");
        for selection in [
            &["fixture", "planned"][..],
            &["planned/inside", "fixture"][..],
        ] {
            let selected = run(Some(selection));
            assert!(!budget_stops(&selected), "{selection:?}: {selected:?}");
            assert!(
                !selected
                    .differences
                    .iter()
                    .any(|d| d.path.as_os_str() == "left_behind"),
                "{selected:?}"
            );
            let expect: Vec<PathBuf> = if differs {
                vec!["fixture".into()]
            } else {
                vec![]
            };
            assert_eq!(
                content_diffs(&selected),
                expect,
                "{selection:?}: {selected:?}"
            );
            let known = selected
                .unknowns
                .iter()
                .filter(|u| u.field == Field::Content)
                .count();
            assert_eq!(known, 0, "{selection:?}: {selected:?}");
        }
    }
}
