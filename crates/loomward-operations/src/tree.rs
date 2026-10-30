use std::{collections::BTreeMap, path::PathBuf};

pub type Observation<T> = Result<T, &'static str>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Kind {
    File,
    Directory,
}

/// Section 6 of docs/47. Report-only classes are retained without requiring equality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub enum MetadataClass {
    NamedStreams,
    CreationTime,
    LastWriteTime,
    PreservedAttributes,
    UnsupportedAttributes,
    ExtendedAttributes,
    ReparsePoint,
    SecurityDescriptor,
    LastAccessTime,
    ChangeTime,
    Owner,
    HardLinks,
    CaseSensitivity,
    ObjectId,
    ShortName,
}

impl MetadataClass {
    pub const ALL: [Self; 15] = [
        Self::NamedStreams,
        Self::CreationTime,
        Self::LastWriteTime,
        Self::PreservedAttributes,
        Self::UnsupportedAttributes,
        Self::ExtendedAttributes,
        Self::ReparsePoint,
        Self::SecurityDescriptor,
        Self::LastAccessTime,
        Self::ChangeTime,
        Self::Owner,
        Self::HardLinks,
        Self::CaseSensitivity,
        Self::ObjectId,
        Self::ShortName,
    ];
    pub fn policy(self) -> Policy {
        match self {
            Self::UnsupportedAttributes | Self::ExtendedAttributes | Self::ReparsePoint => {
                Policy::Refused
            }
            Self::LastAccessTime
            | Self::ChangeTime
            | Self::Owner
            | Self::HardLinks
            | Self::CaseSensitivity
            | Self::ObjectId
            | Self::ShortName => Policy::Reported,
            Self::SecurityDescriptor => Policy::Inherited,
            _ => Policy::Preserved,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    Preserved,
    Refused,
    Inherited,
    Reported,
}

/// Canonical values supplied by a reader. Empty bytes mean absent for refused classes;
/// for SecurityDescriptor they mean no explicit destination ACEs (not ACL equality).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub kind: Kind,
    pub size: Observation<u64>,
    pub content: Observation<[u8; 32]>,
    pub metadata: BTreeMap<MetadataClass, Observation<Vec<u8>>>,
    /// Only a complete directory listing establishes absence.
    pub children: Observation<()>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tree {
    /// Exact relative native names, including the root at the empty path.
    pub nodes: BTreeMap<PathBuf, Observation<Node>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Side {
    Source,
    Destination,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum Field {
    Entry,
    Listing,
    Size,
    Content,
    Metadata(MetadataClass),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum DifferenceKind {
    Missing,
    Extra,
    Kind,
    Size,
    Content,
    Metadata(MetadataClass),
    Refused { side: Side, class: MetadataClass },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Difference {
    pub path: PathBuf,
    pub kind: DifferenceKind,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Unknown {
    pub path: PathBuf,
    pub side: Side,
    pub field: Field,
    pub reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReportedMetadata {
    pub path: PathBuf,
    pub class: MetadataClass,
    pub source: Observation<Vec<u8>>,
    pub destination: Observation<Vec<u8>>,
}

#[derive(Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Report {
    pub differences: Vec<Difference>,
    pub unknowns: Vec<Unknown>,
    pub reported_metadata: Vec<ReportedMetadata>,
}
impl Report {
    pub fn equal(&self) -> bool {
        self.differences.is_empty() && self.unknowns.is_empty()
    }
}

pub fn compare(source: &Tree, destination: &Tree) -> Report {
    use std::collections::BTreeSet;
    let mut report = Report::default();
    let paths: BTreeSet<_> = source
        .nodes
        .keys()
        .chain(destination.nodes.keys())
        .cloned()
        .chain([PathBuf::new()])
        .collect();
    for path in paths {
        let a = source.nodes.get(&path);
        let b = destination.nodes.get(&path);
        for (side, value) in [(Side::Source, a), (Side::Destination, b)] {
            if let Some(Err(reason)) = value {
                report.unknowns.push(Unknown {
                    path: path.clone(),
                    side,
                    field: Field::Entry,
                    reason,
                });
            }
        }
        let mut difference = |kind| {
            report.differences.push(Difference {
                path: path.clone(),
                kind,
            })
        };
        match (a, b) {
            (Some(Ok(a)), Some(Ok(b))) => {
                if a.kind != b.kind {
                    difference(DifferenceKind::Kind);
                    continue;
                }
                if a.kind == Kind::Directory {
                    observe(&mut report, &path, Field::Listing, &a.children, &b.children);
                } else {
                    let sizes = observe(&mut report, &path, Field::Size, &a.size, &b.size);
                    let content =
                        observe(&mut report, &path, Field::Content, &a.content, &b.content);
                    if sizes == Some(false) {
                        report.differences.push(Difference {
                            path: path.clone(),
                            kind: DifferenceKind::Size,
                        });
                    } else if sizes == Some(true) && content == Some(false) {
                        report.differences.push(Difference {
                            path: path.clone(),
                            kind: DifferenceKind::Content,
                        });
                    }
                }
                for class in MetadataClass::ALL {
                    let missing = Err("metadata_not_observed");
                    let left = a.metadata.get(&class).unwrap_or(&missing);
                    let right = b.metadata.get(&class).unwrap_or(&missing);
                    // The roots' names belong to their parents, outside the compared grants.
                    // Keep the observations in reported_metadata, including unreadable evidence.
                    let equal = if path.as_os_str().is_empty() && class == MetadataClass::ShortName
                    {
                        None
                    } else {
                        observe(&mut report, &path, Field::Metadata(class), left, right)
                    };
                    match class.policy() {
                        Policy::Preserved if equal == Some(false) => {
                            report.differences.push(Difference {
                                path: path.clone(),
                                kind: DifferenceKind::Metadata(class),
                            })
                        }
                        Policy::Refused => {
                            for (side, value) in [(Side::Source, left), (Side::Destination, right)]
                            {
                                if value.as_ref().is_ok_and(|v| !v.is_empty()) {
                                    report.differences.push(Difference {
                                        path: path.clone(),
                                        kind: DifferenceKind::Refused { side, class },
                                    });
                                }
                            }
                        }
                        Policy::Inherited => {
                            if right.as_ref().is_ok_and(|v| !v.is_empty()) {
                                report.differences.push(Difference {
                                    path: path.clone(),
                                    kind: DifferenceKind::Metadata(class),
                                });
                            }
                            report.reported_metadata.push(ReportedMetadata {
                                path: path.clone(),
                                class,
                                source: left.clone(),
                                destination: right.clone(),
                            });
                        }
                        Policy::Reported => report.reported_metadata.push(ReportedMetadata {
                            path: path.clone(),
                            class,
                            source: left.clone(),
                            destination: right.clone(),
                        }),
                        _ => {}
                    }
                }
            }
            (Some(Ok(_)), None) | (None, Some(Ok(_))) => {
                let (tree, side, kind) = if a.is_some() {
                    (destination, Side::Destination, DifferenceKind::Missing)
                } else {
                    (source, Side::Source, DifferenceKind::Extra)
                };
                if absence_known(tree, &path) {
                    difference(kind);
                } else {
                    report.unknowns.push(Unknown {
                        path,
                        side,
                        field: Field::Entry,
                        reason: "absence_unproven",
                    });
                }
            }
            (None, None) => report.unknowns.push(Unknown {
                path,
                side: Side::Source,
                field: Field::Entry,
                reason: "root_not_observed",
            }),
            _ => {}
        }
    }
    report
}

/// Selected-set comparison (#1009): compares only the `selected` relative paths and everything
/// under a selected directory, so a plan that copied some files out of a folder can pass.
/// Unselected source entries are ignored; every destination entry still counts, so anything
/// unplanned in the destination is `Extra`. Strict ancestors of selected paths are containers:
/// their kind, listing and refused or inherited classes are checked, but not their preserved
/// metadata, because the source container also holds what the plan left behind. A selected path
/// the source does not hold, or an empty selection, is unknown, never equal.
pub fn compare_selected(
    source: &Tree,
    destination: &Tree,
    selected: &std::collections::BTreeSet<PathBuf>,
) -> Report {
    use std::{collections::BTreeSet, path::Path};
    if selected.is_empty() {
        let mut report = Report::default();
        report.unknowns.push(Unknown {
            path: PathBuf::new(),
            side: Side::Source,
            field: Field::Entry,
            reason: "empty_selection",
        });
        return report;
    }
    let containers: BTreeSet<&Path> = selected
        .iter()
        .flat_map(|path| path.ancestors().skip(1))
        .collect();
    let chosen = |path: &Path| {
        containers.contains(path)
            || path
                .ancestors()
                .any(|a| !a.as_os_str().is_empty() && selected.contains(a))
    };
    let kept = Tree {
        nodes: source
            .nodes
            .iter()
            .filter(|(path, _)| chosen(path))
            .map(|(path, node)| (path.clone(), node.clone()))
            .collect(),
    };
    let mut report = compare(&kept, destination);
    let container_preserved = |path: &Path, class: &MetadataClass| {
        containers.contains(path) && class.policy() == Policy::Preserved
    };
    report.differences.retain(
        |d| !matches!(&d.kind, DifferenceKind::Metadata(c) if container_preserved(&d.path, c)),
    );
    report
        .unknowns
        .retain(|u| !matches!(&u.field, Field::Metadata(c) if container_preserved(&u.path, c)));
    for path in selected.iter().filter(|p| !source.nodes.contains_key(*p)) {
        report.unknowns.push(Unknown {
            path: path.clone(),
            side: Side::Source,
            field: Field::Entry,
            reason: "selected_not_in_source",
        });
    }
    report
}

fn observe<T: PartialEq>(
    report: &mut Report,
    path: &std::path::Path,
    field: Field,
    source: &Observation<T>,
    destination: &Observation<T>,
) -> Option<bool> {
    for (side, value) in [(Side::Source, source), (Side::Destination, destination)] {
        if let Err(reason) = value {
            report.unknowns.push(Unknown {
                path: path.into(),
                side,
                field: field.clone(),
                reason,
            });
        }
    }
    match (source, destination) {
        (Ok(a), Ok(b)) => Some(a == b),
        _ => None,
    }
}

fn absence_known(tree: &Tree, path: &std::path::Path) -> bool {
    for parent in path.ancestors().skip(1) {
        match tree.nodes.get(parent) {
            Some(Ok(node)) => return node.kind == Kind::Directory && node.children.is_ok(),
            None => {}
            _ => return false,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    fn node(kind: Kind) -> Node {
        Node {
            kind,
            size: Ok(4),
            content: Ok([1; 32]),
            children: Ok(()),
            metadata: MetadataClass::ALL
                .into_iter()
                .map(|c| (c, Ok(vec![])))
                .collect(),
        }
    }
    fn tree() -> Tree {
        Tree {
            nodes: [
                (PathBuf::new(), Ok(node(Kind::Directory))),
                ("same".into(), Ok(node(Kind::File))),
            ]
            .into(),
        }
    }
    #[test]
    fn planted_differences_are_exact() {
        let mut left = tree();
        let mut right = tree();
        left.nodes.insert("missing".into(), Ok(node(Kind::File)));
        right.nodes.insert("extra".into(), Ok(node(Kind::File)));
        for (name, change) in [("size", 0), ("content", 1), ("kind", 2), ("metadata", 3)] {
            let a = node(Kind::File);
            let mut b = a.clone();
            match change {
                0 => b.size = Ok(5),
                1 => b.content = Ok([2; 32]),
                2 => b.kind = Kind::Directory,
                _ => {
                    b.metadata
                        .insert(MetadataClass::PreservedAttributes, Ok(vec![1]));
                }
            }
            left.nodes.insert(name.into(), Ok(a));
            right.nodes.insert(name.into(), Ok(b));
        }
        let report = compare(&left, &right);
        let found: Vec<_> = report
            .differences
            .iter()
            .map(|d| (d.path.clone(), d.kind.clone()))
            .collect();
        assert_eq!(
            found,
            vec![
                ("content".into(), DifferenceKind::Content),
                ("extra".into(), DifferenceKind::Extra),
                ("kind".into(), DifferenceKind::Kind),
                (
                    "metadata".into(),
                    DifferenceKind::Metadata(MetadataClass::PreservedAttributes)
                ),
                ("missing".into(), DifferenceKind::Missing),
                ("size".into(), DifferenceKind::Size),
            ]
        );
        assert!(report.unknowns.is_empty());
    }
    #[test]
    fn unknown_is_never_equal_or_missing() {
        let left = tree();
        let mut right = tree();
        right.nodes.insert("same".into(), Err("access_denied"));
        let report = compare(&left, &right);
        assert!(!report.equal());
        assert!(report.differences.is_empty());
        assert_eq!(report.unknowns.len(), 1);
        right.nodes.remove(&PathBuf::from("same"));
        right
            .nodes
            .get_mut(&PathBuf::new())
            .unwrap()
            .as_mut()
            .unwrap()
            .children = Err("partial_listing");
        let report = compare(&left, &right);
        assert!(!report.equal());
        assert!(report.differences.is_empty());
    }
    #[test]
    fn equal_control_passes() {
        assert!(compare(&tree(), &tree()).equal());
    }

    #[test]
    fn root_short_name_is_reported_outside_comparison_but_child_unknowns_still_block() {
        let left = tree();
        let mut right = tree();
        right
            .nodes
            .get_mut(Path::new(""))
            .unwrap()
            .as_mut()
            .unwrap()
            .metadata
            .insert(MetadataClass::ShortName, Err("metadata_unreadable"));
        let report = compare(&left, &right);
        assert!(report.equal(), "{report:?}");
        assert!(report
            .reported_metadata
            .iter()
            .any(|m| m.path == Path::new("")
                && m.class == MetadataClass::ShortName
                && m.source == Ok(vec![])
                && m.destination == Err("metadata_unreadable")));
        right
            .nodes
            .get_mut(Path::new("same"))
            .unwrap()
            .as_mut()
            .unwrap()
            .metadata
            .insert(MetadataClass::ShortName, Err("metadata_unreadable"));
        let report = compare(&left, &right);
        assert!(!report.equal());
        assert_eq!(report.unknowns.len(), 1);
        assert_eq!(report.unknowns[0].path, Path::new("same"));
        right
            .nodes
            .get_mut(Path::new(""))
            .unwrap()
            .as_mut()
            .unwrap()
            .metadata
            .insert(MetadataClass::CreationTime, Ok(vec![1]));
        assert!(compare(&left, &right)
            .differences
            .iter()
            .any(|d| d.path == Path::new("")
                && d.kind == DifferenceKind::Metadata(MetadataClass::CreationTime)));
    }

    #[test]
    fn every_metadata_class_obeys_section_six_policy() {
        for class in MetadataClass::ALL {
            let left = tree();
            let mut right = tree();
            right
                .nodes
                .get_mut(Path::new("same"))
                .unwrap()
                .as_mut()
                .unwrap()
                .metadata
                .insert(class, Ok(vec![1]));
            let report = compare(&left, &right);
            match class.policy() {
                Policy::Preserved | Policy::Inherited => assert_eq!(
                    report.differences,
                    vec![Difference {
                        path: "same".into(),
                        kind: DifferenceKind::Metadata(class)
                    }]
                ),
                Policy::Refused => assert_eq!(
                    report.differences,
                    vec![Difference {
                        path: "same".into(),
                        kind: DifferenceKind::Refused {
                            side: Side::Destination,
                            class
                        }
                    }]
                ),
                Policy::Reported => {
                    assert!(report.equal());
                    assert!(report
                        .reported_metadata
                        .iter()
                        .any(|m| m.class == class && m.source != m.destination));
                }
            }
            right
                .nodes
                .get_mut(Path::new("same"))
                .unwrap()
                .as_mut()
                .unwrap()
                .metadata
                .remove(&class);
            let report = compare(&left, &right);
            assert!(!report.equal(), "{class:?}");
            assert_eq!(report.unknowns.len(), 1);
            assert_eq!(report.unknowns[0].field, Field::Metadata(class));
        }
    }

    #[test]
    fn exact_names_empty_trees_and_identical_refusals_are_not_false_equal() {
        assert!(!compare(&Tree::default(), &Tree::default()).equal());
        let mut left = tree();
        let mut right = tree();
        let renamed = right.nodes.remove(Path::new("same")).unwrap();
        right.nodes.insert("Same".into(), renamed);
        assert_eq!(compare(&left, &right).differences.len(), 2);
        for tree in [&mut left, &mut right] {
            tree.nodes
                .get_mut(&PathBuf::new())
                .unwrap()
                .as_mut()
                .unwrap()
                .metadata
                .insert(MetadataClass::ExtendedAttributes, Ok(vec![1]));
        }
        assert_eq!(
            compare(&left, &right)
                .differences
                .iter()
                .filter(|d| matches!(d.kind, DifferenceKind::Refused { .. }))
                .count(),
            2
        );
    }

    fn selection(paths: &[&str]) -> std::collections::BTreeSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }
    fn stamped(kind: Kind, stamp: u8) -> Node {
        let mut node = node(kind);
        node.metadata
            .insert(MetadataClass::LastWriteTime, Ok(vec![stamp]));
        node
    }
    /// A source folder `m` holding a planned file and one the plan left behind; the destination
    /// holds only the planned file. Container timestamps differ, as they do after a partial copy.
    fn partial_copy() -> (Tree, Tree) {
        let source = Tree {
            nodes: [
                (PathBuf::new(), Ok(stamped(Kind::Directory, 1))),
                ("m".into(), Ok(stamped(Kind::Directory, 1))),
                ("m/planned".into(), Ok(node(Kind::File))),
                ("m/left".into(), Ok(node(Kind::File))),
                ("other".into(), Ok(node(Kind::File))),
            ]
            .into(),
        };
        let destination = Tree {
            nodes: [
                (PathBuf::new(), Ok(stamped(Kind::Directory, 2))),
                ("m".into(), Ok(stamped(Kind::Directory, 2))),
                ("m/planned".into(), Ok(node(Kind::File))),
            ]
            .into(),
        };
        (source, destination)
    }
    #[test]
    fn selected_partial_copy_passes_where_the_whole_tree_differs() {
        let (source, destination) = partial_copy();
        let whole = compare(&source, &destination);
        assert!(whole
            .differences
            .iter()
            .any(|d| d.kind == DifferenceKind::Missing));
        let report = compare_selected(&source, &destination, &selection(&["m/planned"]));
        assert!(report.equal(), "{report:?}");
    }
    #[test]
    fn a_planned_file_missing_beside_an_unplanned_sibling_is_still_missing() {
        // The hand rule this replaces matched top-level names only, so `m/left` hid `m/planned`.
        let (source, mut destination) = partial_copy();
        destination.nodes.remove(Path::new("m/planned"));
        let report = compare_selected(&source, &destination, &selection(&["m/planned"]));
        assert_eq!(
            report.differences,
            vec![Difference {
                path: "m/planned".into(),
                kind: DifferenceKind::Missing
            }]
        );
    }
    #[test]
    fn selected_files_keep_every_check_and_the_destination_may_hold_nothing_else() {
        let (source, mut destination) = partial_copy();
        destination
            .nodes
            .insert("m/left".into(), Ok(node(Kind::File)));
        let mut changed = node(Kind::File);
        changed.content = Ok([9; 32]);
        changed
            .metadata
            .insert(MetadataClass::CreationTime, Ok(vec![7]));
        destination.nodes.insert("m/planned".into(), Ok(changed));
        let report = compare_selected(&source, &destination, &selection(&["m/planned"]));
        let found: Vec<_> = report
            .differences
            .iter()
            .map(|d| (d.path.to_str().unwrap(), d.kind.clone()))
            .collect();
        assert_eq!(
            found,
            vec![
                ("m/left", DifferenceKind::Extra),
                ("m/planned", DifferenceKind::Content),
                (
                    "m/planned",
                    DifferenceKind::Metadata(MetadataClass::CreationTime)
                ),
            ]
        );
    }
    #[test]
    fn a_selected_directory_compares_its_whole_subtree_and_containers_keep_refusals() {
        let (mut source, mut destination) = partial_copy();
        for tree in [&mut source, &mut destination] {
            tree.nodes.insert("m/sub".into(), Ok(node(Kind::Directory)));
            tree.nodes.insert("m/sub/a".into(), Ok(node(Kind::File)));
        }
        source.nodes.insert("m/sub/b".into(), Ok(node(Kind::File)));
        destination.nodes.remove(Path::new("m/planned"));
        let report = compare_selected(&source, &destination, &selection(&["m/sub"]));
        assert_eq!(report.differences.len(), 1, "{report:?}");
        assert_eq!(report.differences[0].path, Path::new("m/sub/b"));
        let mut linked = stamped(Kind::Directory, 2);
        linked
            .metadata
            .insert(MetadataClass::ReparsePoint, Ok(vec![1]));
        destination.nodes.insert("m".into(), Ok(linked));
        source.nodes.remove(Path::new("m/sub/b"));
        let report = compare_selected(&source, &destination, &selection(&["m/sub"]));
        assert_eq!(
            report.differences,
            vec![Difference {
                path: "m".into(),
                kind: DifferenceKind::Refused {
                    side: Side::Destination,
                    class: MetadataClass::ReparsePoint
                }
            }]
        );
    }
    #[test]
    fn an_unheld_selection_or_an_empty_one_is_never_equal() {
        let (source, destination) = partial_copy();
        for paths in [&["m/planned", "m/typo"][..], &[]] {
            let report = compare_selected(&source, &destination, &selection(paths));
            assert!(!report.equal(), "{paths:?}");
            assert!(report.differences.is_empty());
            assert_eq!(report.unknowns.len(), 1);
            assert!(matches!(
                report.unknowns[0].reason,
                "selected_not_in_source" | "empty_selection"
            ));
        }
    }
}
