//! Test gaps the Muse wave-4 retro review found in this crate (issue #521). Each test pins one behaviour
//! through the public API only. Each was shown to fail when the line it covers is mutated (see the PR body).
use loomward_operations::manifest::{FOLDER_BYTES, MIN_RESERVE};
use loomward_operations::tree::{
    compare, Difference, DifferenceKind, Field, Kind, MetadataClass, Node, Policy, Side, Tree,
    Unknown,
};
use loomward_operations::*;
use std::path::{Path, PathBuf};

// ---- tree: refused metadata on the source side, and size before content ----

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
    let mut t = Tree::default();
    t.nodes.insert(PathBuf::new(), Ok(node(Kind::Directory)));
    t.nodes.insert(PathBuf::from("same"), Ok(node(Kind::File)));
    t
}

fn file_mut(t: &mut Tree) -> &mut Node {
    t.nodes
        .get_mut(Path::new("same"))
        .unwrap()
        .as_mut()
        .unwrap()
}

#[test]
fn refused_metadata_present_only_on_the_source_side_is_reported_as_source() {
    let refused: Vec<_> = MetadataClass::ALL
        .into_iter()
        .filter(|c| c.policy() == Policy::Refused)
        .collect();
    assert!(!refused.is_empty());
    for class in refused {
        let mut source = tree();
        file_mut(&mut source).metadata.insert(class, Ok(vec![1]));
        let report = compare(&source, &tree());
        assert_eq!(
            report.differences,
            vec![Difference {
                path: "same".into(),
                kind: DifferenceKind::Refused {
                    side: Side::Source,
                    class
                }
            }],
            "{class:?}"
        );
        assert!(report.unknowns.is_empty(), "{class:?}");
    }
}

#[test]
fn size_mismatch_reports_size_alone_even_when_content_also_differs() {
    let mut dest = tree();
    let file = file_mut(&mut dest);
    file.size = Ok(5);
    file.content = Ok([2; 32]);
    let report = compare(&tree(), &dest);
    assert_eq!(
        report.differences,
        vec![Difference {
            path: "same".into(),
            kind: DifferenceKind::Size
        }]
    );
    assert!(report.unknowns.is_empty());
}

#[test]
fn unobserved_size_never_turns_a_content_mismatch_into_a_content_difference() {
    let mut dest = tree();
    let file = file_mut(&mut dest);
    file.size = Err("access_denied");
    file.content = Ok([2; 32]);
    let report = compare(&tree(), &dest);
    assert!(report.differences.is_empty());
    assert_eq!(
        report.unknowns,
        vec![Unknown {
            path: "same".into(),
            side: Side::Destination,
            field: Field::Size,
            reason: "access_denied",
        }]
    );
}

// ---- manifest: preview counters and the space reserve floor ----

fn ident(volume: u64, n: u8) -> Identity {
    Identity {
        volume,
        file_id: [n; 16],
    }
}

/// A plain source object on `volume`: 10 bytes, one link, no explicit ACEs, verified.
fn source(name: &str, kind: Kind, volume: u64, n: u8) -> SourceEntry {
    SourceEntry {
        name: name.into(),
        kind,
        identity: ident(volume, n),
        size: 10,
        creation_ft: 1,
        last_write_ft: 1,
        change_ft: 1,
        attributes: 0,
        reparse_tag: None,
        ea_size: 0,
        streams: vec![],
        link_count: 1,
        explicit_aces: false,
        verified_volume: true,
    }
}

/// The destination parent lives on volume 1; the sources default to volume 2.
fn facts(volume_bytes: u64, available_bytes: u64) -> DestinationFacts {
    DestinationFacts {
        grant: 1,
        parent: ident(1, 9),
        ancestors: vec![ident(1, 9)],
        verified_volume: true,
        cloud_synced: false,
        unsupported_volume: false,
        protected: false,
        case_sensitive: false,
        existing_names: vec![],
        parent_path_units: 10,
        available_bytes,
        volume_bytes,
        cluster_bytes: 4096,
    }
}

fn context() -> Context {
    Context {
        elevated: false,
        grants_active: true,
        named_streams: true,
        date: "2026-10-17".into(),
    }
}

#[test]
fn preview_counts_explicit_acls_hard_link_names_and_same_volume_items() {
    let mut a = source("a", Kind::File, 2, 1);
    a.link_count = 2;
    a.explicit_aces = true;
    // The only object on the destination volume, listed between two remote ones: `same_volume` must
    // stay true once any item shares the volume.
    let b = source("b", Kind::File, 1, 2);
    let mut c = source("c", Kind::Directory, 2, 3);
    c.link_count = 2; // a directory's link count is not a hard-linked file name
    c.explicit_aces = true;
    let mut d = source("d", Kind::File, 2, 4);
    d.link_count = 3;
    let p = preview(
        vec![a, b, c, d],
        &facts(200 << 30, u64::MAX / 2),
        &context(),
    );
    assert!(p.refusals.is_empty(), "{:?}", p.refusals);
    assert!(p.same_volume);
    assert_eq!(p.explicit_acl_items, 2);
    assert_eq!(p.hard_link_names, 2);

    let remote = preview(
        vec![source("x", Kind::File, 2, 5)],
        &facts(200 << 30, u64::MAX / 2),
        &context(),
    );
    assert!(!remote.same_volume);
    assert_eq!(remote.explicit_acl_items, 0);
    assert_eq!(remote.hard_link_names, 0);
}

#[test]
fn space_reserve_is_the_one_gib_floor_on_small_volumes_and_one_percent_on_large_ones() {
    let one_file = vec![source("f", Kind::File, 2, 1)];
    // 10 GiB: one per cent is about 107 MB, so the 1 GiB floor dominates.
    let small = preview(one_file.clone(), &facts(10 << 30, u64::MAX / 2), &context());
    assert_eq!(small.space_reserve, MIN_RESERVE);
    // 1 TiB: one per cent is about 10 GiB, which dominates the floor.
    let large = preview(one_file.clone(), &facts(1 << 40, u64::MAX / 2), &context());
    assert_eq!(large.space_reserve, (1u64 << 40) / 100);

    // The reserve is what the free-space check adds to the need: exactly enough passes, one byte short fails.
    let need = small.space_needed;
    let exact = preview(
        one_file.clone(),
        &facts(10 << 30, need + MIN_RESERVE),
        &context(),
    );
    assert!(!exact
        .refusals
        .iter()
        .any(|r| r.code == Code::InsufficientSpace));
    let short = preview(
        one_file,
        &facts(10 << 30, need + MIN_RESERVE - 1),
        &context(),
    );
    assert!(short
        .refusals
        .iter()
        .any(|r| r.code == Code::InsufficientSpace));
    assert!(need >= FOLDER_BYTES);
}

// ---- journal: an item count above MAX_ITEMS never begins an operation ----

#[test]
fn started_above_max_items_is_rejected_and_the_limit_itself_is_accepted() {
    let started = |items| Entry {
        epoch: 1,
        record: Record::Started {
            digest: [9; 32],
            items,
        },
    };
    let mut over = Replay::new(7);
    let row = over.row_for(&started(MAX_ITEMS + 1));
    let err = over
        .apply(&row)
        .expect_err("more items than MAX_ITEMS must not begin an operation");
    assert_eq!(err.why, "item count out of range");

    let mut zero = Replay::new(7);
    let row = zero.row_for(&started(0));
    assert!(zero.apply(&row).is_err());

    let mut at_limit = Replay::new(7);
    let row = at_limit.row_for(&started(MAX_ITEMS));
    at_limit
        .apply(&row)
        .expect("exactly MAX_ITEMS items is accepted");
}

// ---- recovery: a renamed final name outside Publishing is recorded, not confirmed ----

const OP: u64 = 7;
const TOKEN: &str = "lw-staging-1";
const FINAL: &str = "Loomward copy";
const STAGING: u8 = 100;

fn oid(n: u8) -> ObjectId {
    ObjectId {
        file_id: [n; 16],
        creation_ft: 1_000 + n as i64,
    }
}

#[derive(Default)]
struct FakeDisk {
    parent: std::collections::HashMap<String, ObjectId>,
}

impl Disk for FakeDisk {
    fn in_parent(&self, name: &str) -> Option<ObjectId> {
        self.parent.get(name).copied()
    }
    fn in_staging(&self, _: ObjectId, _: &str) -> Option<Observed> {
        None
    }
}

/// The disk after the rename: the final name carries the staging identity, the token is gone.
fn renamed() -> FakeDisk {
    let mut d = FakeDisk::default();
    d.parent.insert(FINAL.into(), oid(STAGING));
    d
}

/// The journal of a three-item run up to and including the publish intent (the same steps as the
/// recovery suite's happy path).
fn publish_intent_steps() -> Vec<Record> {
    use Record::*;
    let items = |v: &[(u32, &str)]| -> Vec<(u32, String)> {
        v.iter().map(|&(i, n)| (i, n.to_string())).collect()
    };
    vec![
        Started {
            digest: [9; 32],
            items: 3,
        },
        StagingIntent {
            token: TOKEN.into(),
        },
        StagingCreated { id: oid(STAGING) },
        CreateBatch {
            items: items(&[(0, "d"), (1, "d/a")]),
        },
        Identities {
            items: vec![(0, oid(1)), (1, oid(2))],
        },
        Verified { items: vec![0, 1] },
        CreateBatch {
            items: items(&[(2, "b")]),
        },
        Identities {
            items: vec![(2, oid(3))],
        },
        Verified { items: vec![2] },
        Transition {
            to: OpState::Staged,
            code: None,
        },
        PublishIntent {
            final_name: FINAL.into(),
        },
    ]
}

fn rows_of(steps: Vec<Record>) -> Vec<Row> {
    let mut replay = Replay::new(OP);
    let mut rows = Vec::new();
    for record in steps {
        let row = replay.row_for(&Entry { epoch: 1, record });
        replay
            .apply(&row)
            .expect("each recovery-test step validates");
        rows.push(row);
    }
    rows
}

#[test]
fn renamed_publish_confirms_only_while_publishing_and_is_recorded_otherwise() {
    let confirm = Decision::Confirm {
        staging: oid(STAGING),
        final_name: FINAL.into(),
    };
    // Control: still Publishing, the rename is confirmed.
    let publishing = recover(OP, &rows_of(publish_intent_steps()), &renamed());
    assert_eq!(publishing.state, Some(OpState::Publishing));
    assert_eq!(publishing.decision, confirm);

    // The same rename after the executor recorded NeedsAttention: a human looks, nothing is confirmed or cleaned.
    let mut steps = publish_intent_steps();
    steps.push(Record::Transition {
        to: OpState::NeedsAttention,
        code: None,
    });
    let recorded = recover(OP, &rows_of(steps), &renamed());
    assert_eq!(recorded.state, Some(OpState::NeedsAttention));
    assert_eq!(
        recorded.decision,
        Decision::NeedsAttention(Attention::Recorded)
    );
}
