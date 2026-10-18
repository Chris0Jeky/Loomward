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
