//! #1144: count actual final identity-query returns, never infer stopped work from a final error.
use super::*;
use crate::duplicate_reader::directory_recheck_fixture::{self, Action, Point};
use loomward_operations::containment::{containment, recheck_source, Coverage};
use std::sync::Mutex;

fn fixture() -> (tempfile::TempDir, OpenedIdentity) {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("a/nested")).unwrap();
    std::fs::create_dir(temp.path().join("b")).unwrap();
    for name in ["root-file", "a/file", "a/nested/file", "b/file"] {
        std::fs::write(temp.path().join(name), b"same").unwrap();
    }
    let (_, expected) = NativeSource::default().open_root(temp.path()).unwrap();
    (temp, expected)
}

fn read(root: &Path, expected: &OpenedIdentity, flags: &[Arc<AtomicBool>]) -> Tree {
    crate::duplicate_reader::read_tree_wanting_with_cancels(
        root,
        expected,
        Budget {
            bytes: 1024 * 1024,
            time: Duration::from_secs(30),
        },
        100,
        flags[0].clone(),
        &flags[1..],
        &|_, _| true,
        &|_, _| false,
    )
    .unwrap()
    .0
}

fn stopped(after_queries: usize, cancelled: Option<usize>, expire: bool) -> (Tree, Tree, usize) {
    let (temp, expected) = fixture();
    let flags: Vec<_> = (0..3).map(|_| Arc::new(AtomicBool::new(false))).collect();
    let complete = read(temp.path(), &expected, &flags);
    let queried = Arc::new(Mutex::new(Vec::new()));
    let seen = queried.clone();
    let cancel = cancelled.map(|index| flags[index].clone());
    let _hook = directory_recheck_fixture::install(temp.path(), move |point, path| {
        let stop = match point {
            Point::BeforeLoop => after_queries == 0,
            Point::QueryReturned => {
                let mut seen = seen.lock().unwrap();
                seen.push(path.to_path_buf());
                seen.len() == after_queries
            }
        };
        if stop {
            if let Some(cancel) = &cancel {
                cancel.store(true, Ordering::Release);
            }
            if expire {
                return Action::ExpireBudget;
            }
        }
        Action::Continue
    });
    let stopped = read(temp.path(), &expected, &flags);
    let queries = queried.lock().unwrap().len();
    (stopped, complete, queries)
}

fn assert_unknown(stopped: &Tree, complete: &Tree, after_queries: usize, reason: &'static str) {
    // Root is the first query. A stop after its return still invalidates that query; after the
    // second return, only root's validation has finished. Child enumeration order is irrelevant.
    if after_queries <= 1 {
        assert_eq!(stopped.nodes[Path::new("")].as_ref().unwrap_err(), &reason);
    } else {
        assert!(stopped.nodes[Path::new("")].is_ok());
    }
    for dir in ["a", "a/nested", "b"] {
        assert_eq!(stopped.nodes[Path::new(dir)].as_ref().unwrap_err(), &reason);
    }
    let report = containment(stopped, complete);
    let mut rechecked = complete.clone();
    recheck_source(&mut rechecked, stopped);
    let final_report = containment(&rechecked, complete);
    for path in ["root-file", "a/file", "a/nested/file", "b/file"] {
        let node = stopped.nodes[Path::new(path)].as_ref().unwrap();
        let rechecked_node = rechecked.nodes[Path::new(path)].as_ref().unwrap();
        assert_eq!(node.size, Ok(4), "observed size is retained: {path}");
        assert_eq!(
            rechecked_node.size,
            Ok(4),
            "recheck retains observed size: {path}"
        );
        if path == "root-file" && after_queries == 2 {
            assert!(node.content.is_ok());
            assert!(rechecked_node.content.is_ok());
            continue;
        }
        assert_eq!(node.content, Err(reason), "unvalidated ancestor: {path}");
        assert_eq!(
            rechecked_node.content,
            Err(reason),
            "final A recheck: {path}"
        );
        for report in [&report, &final_report] {
            let item = report
                .items
                .iter()
                .find(|item| item.path == Path::new(path))
                .unwrap();
            assert!(
                matches!(item.coverage, Coverage::Unknown { reason: why, .. } if why == reason)
            );
        }
    }
}

#[test]
fn directory_recheck_stops_for_each_flag_before_or_during_the_loop() {
    if crate::enumerate::skip_elevated_test() {
        return;
    }
    for cancelled in 0..3 {
        for after_queries in [0, 1, 2] {
            let (tree, complete, queried) = stopped(after_queries, Some(cancelled), false);
            assert_eq!(queried, after_queries, "flag {cancelled}");
            assert_unknown(&tree, &complete, after_queries, "cancelled");
        }
    }
}

#[test]
fn directory_recheck_time_budget_keeps_unvalidated_descendants_unknown() {
    if crate::enumerate::skip_elevated_test() {
        return;
    }
    for after_queries in [0, 1, 2] {
        let (tree, complete, queried) = stopped(after_queries, None, true);
        assert_eq!(queried, after_queries);
        assert_unknown(&tree, &complete, after_queries, "time_budget");
    }
}

#[test]
fn directory_recheck_cancellation_takes_precedence_over_an_expired_budget() {
    if crate::enumerate::skip_elevated_test() {
        return;
    }
    for cancelled in 0..3 {
        let (tree, complete, queried) = stopped(1, Some(cancelled), true);
        assert_eq!(queried, 1);
        assert_unknown(&tree, &complete, 1, "cancelled");
    }
}

#[test]
fn directory_recheck_still_detects_identity_changes_without_cancellation() {
    if crate::enumerate::skip_elevated_test() {
        return;
    }
    let (temp, expected) = fixture();
    let changed = temp.path().join("a");
    let queries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = queries.clone();
    let _hook = directory_recheck_fixture::install(temp.path(), move |point, _| {
        match point {
            Point::BeforeLoop => {
                std::fs::OpenOptions::new()
                    .access_mode(FILE_WRITE_ATTRIBUTES)
                    .share_mode(SHARE)
                    .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                    .open(&changed)
                    .unwrap()
                    .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_000_000_000))
                    .unwrap();
            }
            Point::QueryReturned => {
                counted.fetch_add(1, Ordering::AcqRel);
            }
        }
        Action::Continue
    });
    let flags: Vec<_> = (0..3).map(|_| Arc::new(AtomicBool::new(false))).collect();
    let tree = read(temp.path(), &expected, &flags);
    assert_eq!(queries.load(Ordering::Acquire), 4);
    assert_eq!(
        tree.nodes[Path::new("a")].as_ref().unwrap_err(),
        &"changed_during_read"
    );
    assert!(tree.nodes[Path::new("b")].is_ok());
}
