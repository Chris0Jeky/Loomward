//! Path-independent containment (#1011): is every file of A present in B by content?
//!
//! Pure over two observed `Tree`s. Coverage is equal size and equal content digest on both sides,
//! never a name, size or timestamp alone. It is not metadata fidelity, not an independent backup
//! and never permission to delete.
use crate::tree::{Kind, Node, Side, Tree};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Coverage {
    /// One B file with the same size and content digest.
    Covered {
        by: PathBuf,
    },
    /// No B file of that size, or none of that content, and B's listing was complete.
    NotCovered,
    Unknown {
        side: Side,
        reason: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Item {
    pub path: PathBuf,
    pub size: Option<u64>,
    #[serde(flatten)]
    pub coverage: Coverage,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Total {
    /// Items: files, plus unknown A entries whose kind or listing could not be read.
    pub files: u64,
    /// A lower bound whenever `size_unknown` is above zero.
    pub bytes: u64,
    pub size_unknown: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Totals {
    pub covered: Total,
    pub not_covered: Total,
    pub unknown: Total,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// One per A file, plus one per A entry whose kind or listing is unknown.
    pub items: Vec<Item>,
}

impl Report {
    pub fn totals(&self) -> Totals {
        let mut totals = Totals::default();
        for item in &self.items {
            let total = match item.coverage {
                Coverage::Covered { .. } => &mut totals.covered,
                Coverage::NotCovered => &mut totals.not_covered,
                Coverage::Unknown { .. } => &mut totals.unknown,
            };
            total.files += 1;
            match item.size {
                Some(size) => total.bytes += size,
                None => total.size_unknown += 1,
            }
        }
        totals
    }
}

/// B listing that could hide a file of any size: a missing or unreadable entry, an incomplete
/// directory listing or a file of unknown size.
fn incomplete(tree: &Tree) -> bool {
    !matches!(tree.nodes.get(Path::new("")), Some(Ok(_)))
        || tree.nodes.values().any(|node| match node {
            Err(_) => true,
            Ok(node) => match node.kind {
                Kind::Directory => node.children.is_err(),
                Kind::File => node.size.is_err(),
            },
        })
}

/// A file whose size, creation, last-write or change time differs between two reads of the same
/// root (#1037): the digest taken in between no longer describes it.
pub const CHANGED_DURING_COMPARE: &str = "changed_during_compare";
/// The recheck could not say either way (its own listing was cut short, or times were not
/// observed): the file is not known to have changed (#1037).
pub const RECHECK_INCOMPLETE: &str = "recheck_incomplete";

enum Recheck {
    Same,
    Changed,
    /// Could not re-verify, with the reason the recheck gives.
    Unverified(&'static str),
}

fn recheck_file(path: &Path, node: &Node, again: &Tree) -> Recheck {
    use crate::tree::MetadataClass::{ChangeTime, CreationTime, LastWriteTime};
    match again.nodes.get(path) {
        Some(Err(reason)) => Recheck::Unverified(reason),
        Some(Ok(now)) => {
            if now.kind != Kind::File || now.size != node.size {
                return Recheck::Changed;
            }
            let mut seen = true;
            for class in [CreationTime, LastWriteTime, ChangeTime] {
                match (node.metadata.get(&class), now.metadata.get(&class)) {
                    (Some(Ok(before)), Some(Ok(after))) if before != after => {
                        return Recheck::Changed
                    }
                    (Some(Ok(_)), Some(Ok(_))) => {}
                    _ => seen = false,
                }
            }
            if seen {
                Recheck::Same
            } else {
                Recheck::Unverified(RECHECK_INCOMPLETE)
            }
        }
        // Absent: gone only if its folder was listed completely; otherwise say why not.
        None => match path.parent().and_then(|dir| again.nodes.get(dir)) {
            Some(Ok(dir)) => match dir.children {
                Ok(()) => Recheck::Changed,
                Err(reason) => Recheck::Unverified(reason),
            },
            Some(Err(reason)) => Recheck::Unverified(reason),
            None => Recheck::Unverified(RECHECK_INCOMPLETE),
        },
    }
}

/// Re-verifies A after every hashing read (#1037). `again` is a listing-only read of the same root.
/// A file that is gone from a completely listed folder, or whose size or creation, last-write or
/// change time moved, loses its content with `changed_during_compare`, replacing whatever it held:
/// the report must not judge bytes that may be gone. A file the recheck could not reach (budget,
/// cancel, a cut-short listing) is not known to have changed: a hashed file becomes unknown with
/// the recheck's own reason, and any content verdict already `Err` is kept. Files that appeared
/// since are outside "A as read" and are not reported.
pub fn recheck_source(a: &mut Tree, again: &Tree) {
    for (path, node) in &mut a.nodes {
        let Ok(node) = node else { continue };
        if node.kind != Kind::File {
            continue;
        }
        match recheck_file(path, node, again) {
            Recheck::Same => {}
            Recheck::Changed => node.content = Err(CHANGED_DURING_COMPARE),
            Recheck::Unverified(reason) if node.content.is_ok() => node.content = Err(reason),
            Recheck::Unverified(_) => {}
        }
    }
}

pub fn containment(a: &Tree, b: &Tree) -> Report {
    // Index B once (#1011 review): the first trusted path per (size, digest); per size, why some
    // same-size B file cannot vouch (unread, or under a directory that changed or failed during
    // the read, so its digest may describe a file that is gone); and every size B listed.
    let mut matches = BTreeMap::<(u64, [u8; 32]), &PathBuf>::new();
    let mut unread = BTreeMap::<u64, &'static str>::new();
    let mut sizes = BTreeSet::<u64>::new();
    for (path, node) in &b.nodes {
        let Ok(node) = node else { continue };
        let (Kind::File, Ok(size)) = (node.kind, node.size) else {
            continue;
        };
        sizes.insert(size);
        let ancestor = path.ancestors().skip(1).find_map(|a| match b.nodes.get(a) {
            Some(Ok(_)) => None,
            Some(Err(reason)) => Some(*reason),
            None => Some("ancestor_not_observed"),
        });
        match (ancestor, node.content) {
            (None, Ok(hash)) => {
                matches.entry((size, hash)).or_insert(path);
            }
            (Some(reason), _) | (None, Err(reason)) => {
                unread.entry(size).or_insert(reason);
            }
        }
    }
    let b_incomplete = incomplete(b);
    let unknown = |side, reason| Coverage::Unknown { side, reason };
    let mut report = Report::default();
    if !a.nodes.contains_key(Path::new("")) {
        report.items.push(Item {
            path: PathBuf::new(),
            size: None,
            coverage: unknown(Side::Source, "root_not_observed"),
        });
    }
    for (path, node) in &a.nodes {
        let node = match node {
            Err(reason) => {
                report.items.push(Item {
                    path: path.clone(),
                    size: None,
                    coverage: unknown(Side::Source, *reason),
                });
                continue;
            }
            Ok(node) => node,
        };
        if node.kind == Kind::Directory {
            // Directories are not items, but an incomplete listing hides unknown A files.
            if let Err(reason) = node.children {
                report.items.push(Item {
                    path: path.clone(),
                    size: None,
                    coverage: unknown(Side::Source, reason),
                });
            }
            continue;
        }
        let size = match node.size {
            Ok(size) => size,
            Err(reason) => {
                report.items.push(Item {
                    path: path.clone(),
                    size: None,
                    coverage: unknown(Side::Source, reason),
                });
                continue;
            }
        };
        let coverage = match node.content {
            Ok(hash) => {
                if let Some(by) = matches.get(&(size, hash)) {
                    Coverage::Covered { by: (*by).clone() }
                } else if let Some(reason) = unread.get(&size) {
                    unknown(Side::Destination, reason)
                } else if b_incomplete {
                    unknown(Side::Destination, "absence_unproven")
                } else {
                    Coverage::NotCovered
                }
            }
            // A was not read because B had no file of its size: only a complete B still lacking
            // that size proves it. Any other unread A file stays unknown.
            Err("not_requested") if !sizes.contains(&size) && !b_incomplete => Coverage::NotCovered,
            // B gained a file of this size after its listing, so A was never asked for: the cause
            // is drift in B, and only a fresh run can say (#1037).
            Err("not_requested") if sizes.contains(&size) => {
                unknown(Side::Destination, CHANGED_DURING_COMPARE)
            }
            Err(reason) => unknown(Side::Source, reason),
        };
        report.items.push(Item {
            path: path.clone(),
            size: Some(size),
            coverage,
        });
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{MetadataClass, Node, Observation};

    fn dir() -> Observation<Node> {
        Ok(Node {
            kind: Kind::Directory,
            size: Ok(0),
            content: Err("content_not_read"),
            metadata: Default::default(),
            children: Ok(()),
        })
    }
    fn file(size: u64, content: u8) -> Observation<Node> {
        Ok(Node {
            kind: Kind::File,
            size: Ok(size),
            content: Ok([content; 32]),
            metadata: Default::default(),
            children: Err("not_a_directory"),
        })
    }
    fn tree(entries: &[(&str, Observation<Node>)]) -> Tree {
        let mut tree = Tree::default();
        tree.nodes.insert(PathBuf::new(), dir());
        for (path, node) in entries {
            tree.nodes.insert(PathBuf::from(path), node.clone());
        }
        tree
    }
    fn coverage(report: &Report) -> Vec<(String, Coverage)> {
        report
            .items
            .iter()
            .map(|item| {
                (
                    item.path.to_string_lossy().replace('\\', "/"),
                    item.coverage.clone(),
                )
            })
            .collect()
    }
    fn covered(by: &str) -> Coverage {
        Coverage::Covered { by: by.into() }
    }

    #[test]
    fn renamed_moved_and_duplicated_files_are_covered_by_content() {
        let a = tree(&[
            ("photos", dir()),
            ("photos/one.jpg", file(10, 1)),
            ("two.txt", file(20, 2)),
        ]);
        let b = tree(&[
            ("archive", dir()),
            ("archive/renamed.jpg", file(10, 1)),
            ("archive/deep", dir()),
            ("archive/deep/two-copy.txt", file(20, 2)),
            ("z-two-again.txt", file(20, 2)),
            ("unrelated.bin", file(10, 9)),
        ]);
        let report = containment(&a, &b);
        assert_eq!(
            coverage(&report),
            vec![
                ("photos/one.jpg".into(), covered("archive/renamed.jpg")),
                ("two.txt".into(), covered("archive/deep/two-copy.txt")),
            ]
        );
        let totals = report.totals();
        assert_eq!(
            totals.covered,
            Total {
                files: 2,
                bytes: 30,
                size_unknown: 0
            }
        );
        assert_eq!(totals.not_covered, Total::default());
        assert_eq!(totals.unknown, Total::default());
    }

    #[test]
    fn same_size_different_content_and_missing_sizes_are_not_covered() {
        let a = tree(&[("same-size", file(10, 1)), ("missing", file(30, 3))]);
        let b = tree(&[("other", file(10, 2))]);
        let report = containment(&a, &b);
        assert_eq!(
            coverage(&report),
            vec![
                ("missing".into(), Coverage::NotCovered),
                ("same-size".into(), Coverage::NotCovered),
            ]
        );
        assert_eq!(
            report.totals().not_covered,
            Total {
                files: 2,
                bytes: 40,
                size_unknown: 0
            }
        );
        // A was never read for a size B lacks: still not covered while B is complete.
        let mut unread = file(30, 0).unwrap();
        unread.content = Err("not_requested");
        let a = tree(&[("missing", Ok(unread))]);
        assert_eq!(
            coverage(&containment(&a, &b)),
            vec![("missing".into(), Coverage::NotCovered)]
        );
    }

    #[test]
    fn incomplete_b_listing_makes_absence_unknown_never_not_covered() {
        let a = tree(&[("missing", file(30, 3)), ("present", file(10, 1))]);
        let mut unread = file(30, 0).unwrap();
        unread.content = Err("not_requested");
        let a_unread = tree(&[("missing", Ok(unread))]);
        let absent = Coverage::Unknown {
            side: Side::Destination,
            reason: "absence_unproven",
        };
        let mut partial_root = tree(&[("present", file(10, 1))]);
        partial_root
            .nodes
            .get_mut(Path::new(""))
            .unwrap()
            .as_mut()
            .unwrap()
            .children = Err("entry_budget");
        let unreadable_entry = tree(&[("present", file(10, 1)), ("locked", Err("access_denied"))]);
        let mut no_root = tree(&[("present", file(10, 1))]);
        no_root.nodes.remove(Path::new(""));
        // A file under an unobserved root cannot vouch for its digest (#1011 review).
        let unvouched = Coverage::Unknown {
            side: Side::Destination,
            reason: "ancestor_not_observed",
        };
        for (b, present) in [
            (partial_root, covered("present")),
            (unreadable_entry, covered("present")),
            (no_root, unvouched),
        ] {
            assert_eq!(
                coverage(&containment(&a, &b)),
                vec![
                    ("missing".into(), absent.clone()),
                    ("present".into(), present),
                ]
            );
            assert_eq!(
                coverage(&containment(&a_unread, &b)),
                vec![(
                    "missing".into(),
                    Coverage::Unknown {
                        side: Side::Source,
                        reason: "not_requested"
                    }
                )]
            );
        }
    }

    #[test]
    fn unreadable_a_and_unread_b_candidates_are_unknown() {
        let mut denied = file(10, 1).unwrap();
        denied.content = Err("access_denied");
        let a = tree(&[
            ("denied-content", Ok(denied)),
            ("locked", Err("sharing_violation")),
            ("sub", dir()),
            ("needs-b", file(20, 2)),
        ]);
        let a = {
            let mut a = a;
            a.nodes
                .get_mut(Path::new("sub"))
                .unwrap()
                .as_mut()
                .unwrap()
                .children = Err("incomplete_listing");
            a
        };
        let mut budget = file(20, 0).unwrap();
        budget.content = Err("byte_budget");
        let b = tree(&[("copy", file(10, 1)), ("b-unread", Ok(budget))]);
        let report = containment(&a, &b);
        let source = |reason| Coverage::Unknown {
            side: Side::Source,
            reason,
        };
        assert_eq!(
            coverage(&report),
            vec![
                ("denied-content".into(), source("access_denied")),
                ("locked".into(), source("sharing_violation")),
                (
                    "needs-b".into(),
                    Coverage::Unknown {
                        side: Side::Destination,
                        reason: "byte_budget"
                    }
                ),
                ("sub".into(), source("incomplete_listing")),
            ]
        );
        assert_eq!(
            report.totals().unknown,
            Total {
                files: 4,
                bytes: 30,
                size_unknown: 2
            }
        );
        assert_eq!(report.totals().covered, Total::default());
    }

    #[test]
    fn empty_a_has_no_items_and_an_unobserved_a_root_is_unknown() {
        let b = tree(&[("x", file(1, 1))]);
        let report = containment(&tree(&[]), &b);
        assert!(report.items.is_empty());
        assert_eq!(report.totals(), Totals::default());
        assert_eq!(
            coverage(&containment(&Tree::default(), &b)),
            vec![(
                String::new(),
                Coverage::Unknown {
                    side: Side::Source,
                    reason: "root_not_observed"
                }
            )]
        );
    }

    #[test]
    fn a_digest_under_a_directory_that_changed_during_the_read_never_covers() {
        // Sol review of #1011: the copy was hashed, then deleted before the folder's final check.
        let a = tree(&[("keep", file(4, 7))]);
        let mut b = tree(&[("d/copy", file(4, 7)), ("d", dir())]);
        b.nodes.insert("d".into(), Err("changed_during_read"));
        assert_eq!(
            coverage(&containment(&a, &b)),
            vec![(
                "keep".into(),
                Coverage::Unknown {
                    side: Side::Destination,
                    reason: "changed_during_read"
                }
            )]
        );
        // A trusted copy elsewhere still covers it.
        b.nodes.insert("elsewhere".into(), file(4, 7));
        assert_eq!(
            coverage(&containment(&a, &b)),
            vec![("keep".into(), covered("elsewhere"))]
        );
    }

    #[test]
    fn unknown_sizes_are_counted_so_unknown_bytes_read_as_a_lower_bound() {
        let mut a = tree(&[("big", file(9, 1))]);
        if let Ok(node) = a.nodes.get_mut(Path::new("big")).unwrap() {
            node.size = Err("metadata_unreadable");
        }
        let totals = containment(&a, &tree(&[])).totals();
        assert_eq!(
            (
                totals.unknown.files,
                totals.unknown.bytes,
                totals.unknown.size_unknown
            ),
            (1, 0, 1)
        );
    }

    #[test]
    fn a_size_b_gained_after_its_listing_is_destination_drift_not_a_source_gap() {
        // A was never hashed because B's listing had no file of its size; B's final read does.
        let mut unread = file(30, 0).unwrap();
        unread.content = Err("not_requested");
        let a = tree(&[("late", Ok(unread))]);
        let b = tree(&[("arrived", file(30, 4))]);
        assert_eq!(
            coverage(&containment(&a, &b)),
            vec![(
                "late".into(),
                Coverage::Unknown {
                    side: Side::Destination,
                    reason: CHANGED_DURING_COMPARE
                }
            )]
        );
    }

    fn stamped(size: u64, content: u8, write: &[u8]) -> Observation<Node> {
        let mut node = file(size, content).unwrap();
        for (class, value) in [
            (MetadataClass::CreationTime, &b"c"[..]),
            (MetadataClass::LastWriteTime, write),
            (MetadataClass::ChangeTime, &b"x"[..]),
        ] {
            node.metadata.insert(class, Ok(value.to_vec()));
        }
        Ok(node)
    }

    #[test]
    fn a_changed_or_vanished_source_file_loses_its_verdict_after_the_final_recheck() {
        let read = tree(&[
            ("same", stamped(4, 1, b"w1")),
            ("rewritten", stamped(4, 2, b"w1")),
            ("grown", stamped(4, 3, b"w1")),
            ("gone", stamped(4, 4, b"w1")),
            ("dir", dir()),
        ]);
        let again = tree(&[
            ("same", stamped(4, 0, b"w1")),
            ("rewritten", stamped(4, 0, b"w2")),
            ("grown", stamped(5, 0, b"w1")),
            ("dir", dir()),
            ("new", stamped(4, 0, b"w1")),
        ]);
        let mut a = read.clone();
        recheck_source(&mut a, &again);
        let b = tree(&[
            ("copy", file(4, 1)),
            ("c2", file(4, 2)),
            ("c3", file(4, 3)),
            ("c4", file(4, 4)),
        ]);
        let drift = Coverage::Unknown {
            side: Side::Source,
            reason: CHANGED_DURING_COMPARE,
        };
        // Before the recheck every file is covered; after it only the untouched one stays so.
        assert!(containment(&read, &b)
            .items
            .iter()
            .all(|i| matches!(i.coverage, Coverage::Covered { .. })));
        assert_eq!(
            coverage(&containment(&a, &b)),
            vec![
                ("gone".into(), drift.clone()),
                ("grown".into(), drift.clone()),
                ("rewritten".into(), drift),
                ("same".into(), covered("copy")),
            ]
        );
        // A file whose times were never observed cannot be re-verified, but is not known changed.
        let mut blind = tree(&[("same", file(4, 1))]);
        recheck_source(&mut blind, &again);
        assert_eq!(content_of(&blind, "same"), Err(RECHECK_INCOMPLETE));
    }

    fn content_of(tree: &Tree, path: &str) -> Observation<[u8; 32]> {
        tree.nodes[Path::new(path)].as_ref().unwrap().content
    }

    #[test]
    fn a_recheck_that_could_not_look_never_blames_a_change() {
        let mut read = tree(&[
            ("ok", stamped(4, 1, b"w1")),
            ("timed", stamped(4, 2, b"w1")),
            ("sub", dir()),
            ("sub/under-err", stamped(4, 3, b"w1")),
            ("cut", dir()),
            ("cut/short", stamped(4, 4, b"w1")),
            ("lost", dir()),
            ("lost/deep", stamped(4, 5, b"w1")),
            ("gone", dir()),
            ("gone/file", stamped(4, 6, b"w1")),
            ("sub/budget-hit", stamped(4, 7, b"w1")),
        ]);
        // Verdicts already held stay: only a hashed file loses its digest.
        read.nodes
            .get_mut(Path::new("sub/budget-hit"))
            .unwrap()
            .as_mut()
            .unwrap()
            .content = Err("byte_budget");
        let mut again = tree(&[
            ("ok", stamped(4, 0, b"w1")),
            ("timed", Err("time_budget")),
            ("sub", Err("cancelled")),
            ("cut", dir()),
            ("gone", dir()),
        ]);
        again
            .nodes
            .get_mut(Path::new("cut"))
            .unwrap()
            .as_mut()
            .unwrap()
            .children = Err("entry_budget");
        // `lost` is not in `again` at all, so nothing under it was observed.
        recheck_source(&mut read, &again);
        for (name, expect) in [
            ("ok", Ok([1; 32])),
            ("timed", Err("time_budget")),
            ("sub/under-err", Err("cancelled")),
            ("sub/budget-hit", Err("byte_budget")),
            ("cut/short", Err("entry_budget")),
            ("lost/deep", Err(RECHECK_INCOMPLETE)),
            // The folder was listed completely and the file is not in it: really gone.
            ("gone/file", Err(CHANGED_DURING_COMPARE)),
        ] {
            assert_eq!(content_of(&read, name), expect, "{name}");
        }
        // A real change replaces a held verdict, and an unchanged unread file keeps its own.
        let mut unread = stamped(4, 0, b"w1").unwrap();
        unread.content = Err("not_requested");
        let mut moved = unread.clone();
        moved
            .metadata
            .insert(MetadataClass::LastWriteTime, Ok(b"w2".to_vec()));
        let mut a = tree(&[("still", Ok(unread)), ("moved", Ok(moved))]);
        let again = tree(&[
            ("still", stamped(4, 0, b"w1")),
            ("moved", stamped(4, 0, b"w1")),
        ]);
        recheck_source(&mut a, &again);
        assert_eq!(content_of(&a, "still"), Err("not_requested"));
        assert_eq!(content_of(&a, "moved"), Err(CHANGED_DURING_COMPARE));
    }

    #[test]
    fn report_items_serialize_flat() {
        let item = Item {
            path: "a".into(),
            size: Some(3),
            coverage: covered("b"),
        };
        assert_eq!(
            serde_json::to_value(item).unwrap(),
            serde_json::json!({"path": "a", "size": 3, "state": "covered", "by": "b"})
        );
    }
}
