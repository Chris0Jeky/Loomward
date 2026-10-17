//! Property fuzzing of the opaque cursor table (docs/45 row 6, #414). A handle is a random token
//! and the state stays in the service, so no edit of a handle can resume another position:
//! an issued handle resumes exactly its own entry and only for its command, binding and state
//! revision; every other string (mutated handle, arbitrary text) is `Expired`; the table never
//! grows past its capacity and evicts oldest first. `LOOMWARD_FUZZ_CASES` sets the case count.
#[path = "../../../fixtures/fuzz/support.rs"]
mod support;

use super::*;
use loomward_catalog::{Basis, CursorValue, Sort};
use loomward_protocol::Cursor;
use proptest::prelude::*;
use support::{apply, config, edits};

fn position(row: i64) -> Position {
    Position::Children(ChildrenCursor {
        dir_id: 1,
        subtree_rev: 3,
        catalog_instance: "i".into(),
        sort: Sort::SizeDesc,
        basis: Basis::Logical,
        value: CursorValue::Number(5),
        kind: 1,
        row_id: row,
    })
}

fn command() -> impl Strategy<Value = Command> {
    prop::sample::select(Command::ALL.to_vec())
}

proptest! {
    #![proptest_config(config(3000))]

    #[test]
    fn an_issued_handle_resumes_only_its_own_entry(
        cmd in command(), other in command(), binding in "[ -~]{0,30}", other_binding in "[ -~]{0,30}",
        rev in proptest::option::of(any::<i64>()), other_rev in proptest::option::of(any::<i64>()), row in any::<i64>(),
    ) {
        let cursors = Cursors::new(8);
        let h = cursors.issue(cmd, binding.clone(), position(row), rev);
        // The wire type accepts what the service mints.
        prop_assert!(Cursor::new(&h).is_ok());
        prop_assert_eq!(cursors.resume(&h, cmd, &binding, rev), Ok(position(row)));
        if other != cmd || other_binding != binding {
            prop_assert_eq!(cursors.resume(&h, other, &other_binding, rev), Err(Resume::Mismatch));
        }
        if rev.is_some() && other_rev != rev {
            prop_assert_eq!(cursors.resume(&h, cmd, &binding, other_rev), Err(Resume::StateChanged));
        }
        if rev.is_none() {
            // An unbound entry ignores the caller's revision; it does not turn it into one.
            prop_assert_eq!(cursors.resume(&h, cmd, &binding, other_rev), Ok(position(row)));
        }
    }

    #[test]
    fn edited_or_arbitrary_handles_never_resume(cmd in command(), e in edits(), junk in any::<String>()) {
        let cursors = Cursors::new(4);
        let h = cursors.issue(cmd, "b".into(), position(1), None);
        let edited = String::from_utf8_lossy(&apply(h.clone().into_bytes(), &e)).into_owned();
        for candidate in [edited.as_str(), junk.as_str(), "", "cu", &h[..h.len() - 1], &h.to_uppercase(), &format!("{h} ")] {
            let got = cursors.resume(candidate, cmd, "b", None);
            if candidate == h {
                prop_assert!(got.is_ok());
            } else {
                prop_assert_eq!(got, Err(Resume::Expired), "{:?}", candidate);
            }
        }
    }

    #[test]
    fn the_table_is_bounded_and_evicts_oldest_first(capacity in 1usize..9, issued in 1usize..40) {
        let cursors = Cursors::new(capacity);
        let handles: Vec<_> = (0..issued)
            .map(|i| cursors.issue(Command::TreeChildren, "b".into(), position(i as i64), None))
            .collect();
        let mut seen = std::collections::HashSet::new();
        for (i, h) in handles.iter().enumerate() {
            prop_assert!(seen.insert(h.clone()), "a handle repeated");
            let live = i + capacity >= issued;
            let got = cursors.resume(h, Command::TreeChildren, "b", None);
            if live {
                prop_assert_eq!(got, Ok(position(i as i64)));
            } else {
                prop_assert_eq!(got, Err(Resume::Expired));
            }
        }
        prop_assert!(cursors.inner.lock().unwrap().0.len() <= capacity);
    }
}
