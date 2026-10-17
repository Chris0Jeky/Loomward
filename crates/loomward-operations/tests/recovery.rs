//! Doc 47 section 10 unit plan for lane C3: the section 5 recovery table at every crash point, buggy-writer
//! rejections, corrupt journals, and randomized properties over valid runs and perturbed disks.
use loomward_operations::*;
use std::collections::{HashMap, HashSet};

const OP: u64 = 7;
const TOKEN: &str = "lw-staging-1";
const FINAL: &str = "Loomward copy";

fn id(n: u8) -> ObjectId {
    ObjectId {
        file_id: [n; 16],
        creation_ft: 1_000 + n as i64,
    }
}
const S: u8 = 100;

/// A journal built the way a correct executor would: every row validated by replay as it is appended.
#[derive(Clone)]
struct J {
    replay: Replay,
    rows: Vec<Row>,
}

impl J {
    fn new() -> J {
        J {
            replay: Replay::new(OP),
            rows: Vec::new(),
        }
    }
    fn try_push(&mut self, epoch: u64, record: Record) -> Result<(), Invalid> {
        let row = self.replay.row_for(&Entry { epoch, record });
        let mut next = self.replay.clone();
        next.apply(&row)?;
        self.replay = next;
        self.rows.push(row);
        Ok(())
    }
    fn push(&mut self, epoch: u64, record: Record) -> &mut J {
        let r = format!("{record:?}");
        self.try_push(epoch, record)
            .unwrap_or_else(|e| panic!("{r} rejected: {e:?}"));
        self
    }
    fn state(&self) -> OpState {
        self.replay.state
    }
}

#[derive(Default, Clone, Debug)]
struct FakeDisk {
    parent: HashMap<String, ObjectId>,
    staged: HashMap<String, Observed>,
    /// Identity the staged map is reachable through (the folder's own identity).
    folder: Option<ObjectId>,
}

impl Disk for FakeDisk {
    fn in_parent(&self, name: &str) -> Option<ObjectId> {
        self.parent.get(name).copied()
    }
    fn in_staging(&self, staging: ObjectId, name: &str) -> Option<Observed> {
        if self.folder != Some(staging) {
            return None;
        }
        self.staged.get(name).copied()
    }
}

impl FakeDisk {
    fn staging(mut self) -> Self {
        self.parent.insert(TOKEN.into(), id(S));
        self.folder = Some(id(S));
        self
    }
    fn file(mut self, name: &str, n: u8) -> Self {
        self.staged.insert(
            name.into(),
            Observed {
                id: id(n),
                links: 1,
            },
        );
        self
    }
    fn published(mut self) -> Self {
        self.parent.remove(TOKEN);
        self.parent.insert(FINAL.into(), id(S));
        self
    }
}

fn items(v: &[(u32, &str)]) -> Vec<(u32, String)> {
    v.iter().map(|&(i, n)| (i, n.to_string())).collect()
}

/// The happy path of a three-item run (folder `d` holding `d/a`, plus `b`), one record per journal boundary.
/// Returns the journal after each record, so `run()[k]` is "crash right after record k+1".
fn run() -> Vec<J> {
    use Record::*;
    let mut j = J::new();
    let steps = vec![
        Started {
            digest: [9; 32],
            items: 3,
        },
        StagingIntent {
            token: TOKEN.into(),
        },
        StagingCreated { id: id(S) },
        CreateBatch {
            items: items(&[(0, "d"), (1, "d/a")]),
        },
        Identities {
            items: vec![(0, id(1)), (1, id(2))],
        },
        Verified { items: vec![0, 1] },
        CreateBatch {
            items: items(&[(2, "b")]),
        },
        Identities {
            items: vec![(2, id(3))],
        },
        Verified { items: vec![2] },
        Transition {
            to: OpState::Staged,
            code: None,
        },
        PublishIntent {
            final_name: FINAL.into(),
        },
        Committed,
    ];
    let mut out = Vec::new();
    for s in steps {
        j.push(1, s);
        out.push(j.clone());
    }
    out
}

fn decide(j: &J, disk: &FakeDisk) -> Recovery {
    recover(OP, &j.rows, disk)
}

fn full_disk() -> FakeDisk {
    FakeDisk::default()
        .staging()
        .file("d", 1)
        .file("d/a", 2)
        .file("b", 3)
}

fn si(item: u32, name: &str, n: u8) -> StagedItem {
    StagedItem {
        item,
        name: name.into(),
        id: id(n),
    }
}

// ---- Section 5 recovery table, one test per crash point ----

#[test]
fn cp01_after_approval_consumed_reconfirm() {
    let r = decide(&run()[0], &FakeDisk::default());
    assert_eq!(r.state, Some(OpState::Preparing));
    assert_eq!(r.decision, Decision::Reconfirm);
}

#[test]
fn cp02_after_staging_intent_never_adopts_the_folder() {
    // Whether or not the folder was created, it is never adopted or deleted; a new token is used.
    for disk in [FakeDisk::default(), FakeDisk::default().staging()] {
        let r = decide(&run()[1], &disk);
        assert_eq!(
            r.decision,
            Decision::Resume(Resume {
                new_staging: true,
                ..Resume::default()
            })
        );
        assert_eq!(r.leftover_staging, vec![TOKEN.to_string()]);
    }
}

#[test]
fn cp03_after_staging_identity_resume() {
    let r = decide(&run()[2], &FakeDisk::default().staging());
    assert_eq!(r.decision, Decision::Resume(Resume::default()));
    assert!(r.leftover_staging.is_empty());
}

#[test]
fn cp04_create_batch_without_identity_commit_names_leftovers() {
    let j = &run()[3];
    // Nothing was created yet: resume redoes the batch.
    let r = decide(j, &FakeDisk::default().staging());
    assert_eq!(r.decision, Decision::Resume(Resume::default()));
    // Empty files under the batch names: never adopted or deleted, named precisely.
    let disk = FakeDisk::default().staging().file("d", 1).file("d/a", 2);
    let r = decide(j, &disk);
    assert_eq!(
        r.decision,
        Decision::NeedsAttention(Attention::Unjournaled(vec!["d".into(), "d/a".into()]))
    );
}

#[test]
fn cp05_to_cp09_identity_committed_and_mid_write_reverify_by_identity() {
    // CP-05 (identity commit) and mid-write at four offsets leave the same journal; content is not journaled,
    // so every case re-verifies by ID and redoes on mismatch.
    let j = &run()[4];
    for _offset in [0u64, 1, 4096, 1 << 20] {
        let disk = FakeDisk::default().staging().file("d", 1).file("d/a", 2);
        assert_eq!(
            decide(j, &disk).decision,
            Decision::Resume(Resume {
                reverify: vec![si(0, "d", 1), si(1, "d/a", 2)],
                ..Resume::default()
            })
        );
    }
    // A different object under a journaled name, or a second link: touch nothing.
    let swapped = FakeDisk::default().staging().file("d", 1).file("d/a", 50);
    assert_eq!(
        decide(j, &swapped).decision,
        Decision::NeedsAttention(Attention::EntryMismatch(vec![1]))
    );
    let mut linked = FakeDisk::default().staging().file("d", 1).file("d/a", 2);
    linked.staged.get_mut("d/a").unwrap().links = 2;
    assert_eq!(
        decide(j, &linked).decision,
        Decision::NeedsAttention(Attention::EntryMismatch(vec![1]))
    );
    // Removed by identity before its record landed: journal the removal, then redo.
    let gone = FakeDisk::default().staging().file("d", 1);
    assert_eq!(
        decide(j, &gone).decision,
        Decision::Resume(Resume {
            reverify: vec![si(0, "d", 1)],
            gone: vec![si(1, "d/a", 2)],
            ..Resume::default()
        })
    );
}

#[test]
fn cp10_after_verified_batch_reverifies_only_unrecorded() {
    let disk = full_disk();
    assert_eq!(
        decide(
            &run()[5],
            &FakeDisk::default().staging().file("d", 1).file("d/a", 2)
        )
        .decision,
        Decision::Resume(Resume::default())
    );
    assert_eq!(
        decide(&run()[7], &disk).decision,
        Decision::Resume(Resume {
            reverify: vec![si(2, "b", 3)],
            ..Resume::default()
        })
    );
    // A verified file that vanished means journal and disk disagree.
    let mut lost = full_disk();
    lost.staged.remove("d/a");
    assert_eq!(
        decide(&run()[8], &lost).decision,
        Decision::NeedsAttention(Attention::EntryMismatch(vec![1]))
    );
}

#[test]
fn cp11_staged_resumes_to_publish() {
    assert_eq!(
        decide(&run()[9], &full_disk()).decision,
        Decision::Resume(Resume::default())
    );
}

#[test]
fn cp12_publish_intent_without_rename_retries_publish() {
    assert_eq!(
        decide(&run()[10], &full_disk()).decision,
        Decision::Resume(Resume {
            publish: true,
            ..Resume::default()
        })
    );
}

#[test]
fn cp13_renamed_without_committed_confirms_never_cleans() {
    let confirm = Decision::Confirm {
        staging: id(S),
        final_name: FINAL.into(),
    };
    assert_eq!(
        decide(&run()[10], &full_disk().published()).decision,
        confirm
    );
    // Someone else's object under the final name and staging gone: touch nothing.
    let mut other = full_disk();
    other.parent.remove(TOKEN);
    other.parent.insert(FINAL.into(), id(77));
    assert_eq!(
        decide(&run()[10], &other).decision,
        Decision::NeedsAttention(Attention::StagingMismatch)
    );
    // A revocation after the rename (or after a crash at CP-13) never cancels the published copy.
    let mut j = run()[10].clone();
    j.push(
        2,
        Record::Fenced {
            cause: FenceCause::GrantRevoked,
        },
    );
    assert_eq!(decide(&j, &full_disk().published()).decision, confirm);
    // `committed` is epoch-exempt: the confirmation can still record it under the old epoch.
    j.push(1, Record::Committed);
    assert_eq!(j.state(), OpState::Committed);
}

#[test]
fn cp14_committed_is_settled() {
    let r = decide(&run()[11], &full_disk().published());
    assert_eq!(r.state, Some(OpState::Committed));
    assert_eq!(r.decision, Decision::Settled);
}

#[test]
fn rename_step_epoch_read_after_fence_withdraws_and_cleans() {
    // Fence committed after the publish intent, before the rename step read the epoch: no rename.
    let mut j = run()[10].clone();
    j.push(
        2,
        Record::Fenced {
            cause: FenceCause::EmergencyStop,
        },
    );
    let all = vec![si(0, "d", 1), si(1, "d/a", 2), si(2, "b", 3)];
    assert_eq!(
        decide(&j, &full_disk()).decision,
        Decision::Cleanup(Cleanup {
            staging: Some(id(S)),
            withdraw_publish: true,
            remove: all.clone(),
            gone: vec![],
            remove_staging: true,
        })
    );
    // No further publish intent or resume is accepted under either epoch.
    for e in [1, 2] {
        assert!(j
            .clone()
            .try_push(
                e,
                Record::Transition {
                    to: OpState::Copying,
                    code: None
                }
            )
            .is_err());
    }
    j.push(1, Record::PublishWithdrawn);
    assert_eq!(j.state(), OpState::Cancelling);
    assert_eq!(
        decide(&j, &full_disk()).decision,
        Decision::Cleanup(Cleanup {
            staging: Some(id(S)),
            withdraw_publish: false,
            remove: all,
            gone: vec![],
            remove_staging: true,
        })
    );
}

#[test]
fn each_cleanup_removal_is_a_crash_point() {
    let mut j = run()[8].clone();
    j.push(
        1,
        Record::Transition {
            to: OpState::Cancelling,
            code: None,
        },
    );
    let mut disk = full_disk();
    for (item, name, n) in [(2u32, "b", 3u8), (1, "d/a", 2), (0, "d", 1)] {
        // Crash after the removal, before its record: reported as gone, never re-removed by name.
        disk.staged.remove(name);
        let Decision::Cleanup(c) = decide(&j, &disk).decision else {
            panic!("cleanup expected")
        };
        assert_eq!(c.gone, vec![si(item, name, n)]);
        assert!(c.remove.iter().all(|s| s.item != item));
        j.push(1, Record::ItemRemoved { item, id: id(n) });
    }
    disk.parent.remove(TOKEN);
    // Crash after the staging folder was removed but before its record: the name no longer resolves.
    assert_eq!(
        decide(&j, &disk).decision,
        Decision::NeedsAttention(Attention::StagingMismatch)
    );
    j.push(1, Record::StagingRemoved { id: id(S) });
    assert_eq!(
        decide(&j, &disk).decision,
        Decision::Cleanup(Cleanup {
            staging: Some(id(S)),
            ..Cleanup::default()
        })
    );
    j.push(
        1,
        Record::Transition {
            to: OpState::Cancelled,
            code: None,
        },
    );
    assert_eq!(decide(&j, &disk).decision, Decision::Settled);
}

#[test]
fn cleanup_refuses_when_staging_moved_or_unjournaled_entry_present() {
    let mut j = run()[4].clone();
    j.push(
        1,
        Record::Transition {
            to: OpState::Cancelling,
            code: None,
        },
    );
    let mut moved = full_disk();
    moved.parent.remove(TOKEN);
    assert_eq!(
        decide(&j, &moved).decision,
        Decision::NeedsAttention(Attention::StagingMismatch)
    );
    // `b` was intended later but never created by us; someone's file there blocks cleanup, untouched.
    let mut j2 = run()[6].clone();
    j2.push(
        1,
        Record::Transition {
            to: OpState::Cancelling,
            code: None,
        },
    );
    assert_eq!(
        decide(&j2, &full_disk()).decision,
        Decision::NeedsAttention(Attention::Unjournaled(vec!["b".into()]))
    );
}

#[test]
fn fenced_before_staging_cancels_with_nothing_to_remove() {
    let mut j = run()[1].clone();
    j.push(
        2,
        Record::Fenced {
            cause: FenceCause::GrantRevoked,
        },
    );
    assert!(j
        .clone()
        .try_push(2, Record::StagingCreated { id: id(S) })
        .is_err());
    let r = decide(&j, &FakeDisk::default().staging());
    assert_eq!(r.decision, Decision::Cleanup(Cleanup::default()));
    assert_eq!(r.leftover_staging, vec![TOKEN.to_string()]);
    j.push(
        2,
        Record::Transition {
            to: OpState::Cancelling,
            code: Some(Refusal::GrantRevoked),
        },
    );
    j.push(
        2,
        Record::Transition {
            to: OpState::Cancelled,
            code: None,
        },
    );
    assert_eq!(j.state(), OpState::Cancelled);
}

// ---- Item refusals and the single redo ----

#[test]
fn verification_mismatch_allows_one_redo_then_needs_attention() {
    let mut j = run()[4].clone();
    j.push(
        1,
        Record::ItemFailed {
            item: 1,
            code: Refusal::VerificationMismatch,
        },
    );
    assert_eq!(j.state(), OpState::Copying);
    // The redo removes the journaled identity, recreates under the same name, new identity.
    assert!(j
        .clone()
        .try_push(1, Record::ItemRemoved { item: 1, id: id(9) })
        .is_err());
    j.push(1, Record::ItemRemoved { item: 1, id: id(2) })
        .push(
            1,
            Record::CreateBatch {
                items: items(&[(1, "d/a")]),
            },
        )
        .push(
            1,
            Record::Identities {
                items: vec![(1, id(4))],
            },
        );
    assert!(j
        .clone()
        .try_push(
            1,
            Record::CreateBatch {
                items: items(&[(1, "d/other")])
            }
        )
        .is_err());
    j.push(
        1,
        Record::ItemFailed {
            item: 1,
            code: Refusal::VerificationMismatch,
        },
    );
    assert_eq!(j.state(), OpState::NeedsAttention);
    assert_eq!(
        decide(&j, &full_disk()).decision,
        Decision::NeedsAttention(Attention::Recorded)
    );
}

#[test]
fn resumable_refusal_blocks_and_resumes_others_need_attention() {
    let mut j = run()[4].clone();
    j.push(
        1,
        Record::ItemFailed {
            item: 0,
            code: Refusal::SourceInUse,
        },
    );
    assert_eq!(j.state(), OpState::Blocked);
    j.push(
        1,
        Record::Transition {
            to: OpState::Copying,
            code: None,
        },
    );
    j.push(
        1,
        Record::ItemFailed {
            item: 0,
            code: Refusal::SourceChanged,
        },
    );
    assert_eq!(j.state(), OpState::NeedsAttention);
    // Publish- and run-level codes are not item failures.
    let mut k = run()[4].clone();
    for code in [
        Refusal::PublishInUse,
        Refusal::GrantRevoked,
        Refusal::ChangedAfterVerification,
    ] {
        assert!(k.try_push(1, Record::ItemFailed { item: 0, code }).is_err());
    }
}

#[test]
fn publish_in_use_blocks_and_abandon_must_withdraw() {
    let mut j = run()[10].clone();
    j.push(
        1,
        Record::Transition {
            to: OpState::Blocked,
            code: Some(Refusal::PublishInUse),
        },
    );
    assert!(j
        .clone()
        .try_push(
            1,
            Record::Transition {
                to: OpState::Cancelling,
                code: None
            }
        )
        .is_err());
    j.push(1, Record::PublishWithdrawn);
    assert_eq!(j.state(), OpState::Cancelling);
}

// ---- Buggy writers: records that must never be accepted ----

#[test]
fn illegal_records_are_rejected_at_append_and_at_replay() {
    use Record::*;
    let tr = |to| Transition { to, code: None };
    // `at` = how many records of `run()` precede the illegal one.
    let cases: Vec<(usize, u64, Record)> = vec![
        (
            0,
            1,
            Started {
                digest: [0; 32],
                items: 0,
            },
        ),
        (0, 1, Committed),
        (0, 1, StagingCreated { id: id(S) }),
        (
            1,
            1,
            Started {
                digest: [0; 32],
                items: 1,
            },
        ),
        (1, 1, tr(OpState::Copying)),
        (
            3,
            1,
            StagingIntent {
                token: "again".into(),
            },
        ),
        (3, 1, StagingCreated { id: id(S) }),
        (
            3,
            1,
            CreateBatch {
                items: items(&[(0, "../escape")]),
            },
        ),
        (
            3,
            1,
            CreateBatch {
                items: items(&[(0, "/abs")]),
            },
        ),
        (
            3,
            1,
            CreateBatch {
                items: items(&[(0, "C:/x")]),
            },
        ),
        (
            3,
            1,
            CreateBatch {
                items: items(&[(0, "a"), (1, "a")]),
            },
        ),
        (
            3,
            1,
            CreateBatch {
                items: items(&[(0, "a"), (0, "b")]),
            },
        ),
        (
            3,
            1,
            CreateBatch {
                items: items(&[(3, "x")]),
            },
        ),
        (3, 1, CreateBatch { items: vec![] }),
        (
            3,
            1,
            Identities {
                items: vec![(0, id(1))],
            },
        ),
        (
            4,
            1,
            Identities {
                items: vec![(2, id(3))],
            },
        ),
        (
            4,
            1,
            Identities {
                items: vec![(0, id(S))],
            },
        ),
        (4, 1, Verified { items: vec![0] }),
        (5, 1, Verified { items: vec![0, 0] }),
        (5, 1, tr(OpState::Staged)),
        (5, 1, tr(OpState::Publishing)),
        (5, 1, tr(OpState::Committed)),
        (5, 1, tr(OpState::Cancelled)),
        (5, 1, tr(OpState::Blocked)),
        (5, 1, ItemRemoved { item: 0, id: id(2) }),
        (5, 2, Verified { items: vec![0] }),
        (5, 0, Verified { items: vec![0] }),
        (6, 1, ItemRemoved { item: 0, id: id(1) }),
        (6, 1, StagingRemoved { id: id(S) }),
        (
            9,
            1,
            PublishIntent {
                final_name: FINAL.into(),
            },
        ),
        (
            10,
            1,
            PublishIntent {
                final_name: TOKEN.into(),
            },
        ),
        (
            10,
            1,
            PublishIntent {
                final_name: "a/b".into(),
            },
        ),
        (10, 1, Committed),
        (11, 1, tr(OpState::Cancelling)),
        (11, 1, ItemRemoved { item: 0, id: id(1) }),
        (
            11,
            1,
            PublishIntent {
                final_name: FINAL.into(),
            },
        ),
        (
            11,
            1,
            Fenced {
                cause: FenceCause::GrantRevoked,
            },
        ),
        (12, 1, Committed),
        (
            12,
            2,
            Fenced {
                cause: FenceCause::GrantRevoked,
            },
        ),
        (12, 1, tr(OpState::Cancelling)),
    ];
    let runs = run();
    for (k, (at, epoch, record)) in cases.into_iter().enumerate() {
        let base = if at == 0 {
            J::new()
        } else {
            runs[at - 1].clone()
        };
        let entry = Entry { epoch, record };
        let mut j = base.clone();
        assert!(
            j.try_push(epoch, entry.record.clone()).is_err(),
            "case {k}: {entry:?} accepted"
        );
        if at == 0 {
            continue;
        }
        // A writer that bypassed the append check still cannot make recovery guess.
        let mut rows = base.rows.clone();
        rows.push(base.replay.row_for(&entry));
        let r = recover(OP, &rows, &full_disk());
        assert!(
            matches!(
                r.decision,
                Decision::NeedsAttention(Attention::InvalidJournal(_))
            ),
            "case {k}: {r:?}"
        );
    }
}

#[test]
fn fence_rejects_stale_and_advancing_records_but_not_exempt_ones() {
    let mut j = run()[4].clone();
    j.push(
        3,
        Record::Fenced {
            cause: FenceCause::GrantRevoked,
        },
    );
    assert!(j
        .clone()
        .try_push(
            3,
            Record::Fenced {
                cause: FenceCause::GrantRevoked
            }
        )
        .is_err());
    for e in [1, 3] {
        assert!(j
            .clone()
            .try_push(e, Record::Verified { items: vec![0] })
            .is_err());
    }
    // `revoked` (cancel under the new epoch) and cleanup records under the old one are accepted.
    j.push(
        1,
        Record::Transition {
            to: OpState::Cancelling,
            code: Some(Refusal::GrantRevoked),
        },
    );
    j.push(1, Record::ItemRemoved { item: 0, id: id(1) });
    assert!(j
        .clone()
        .try_push(4, Record::ItemRemoved { item: 1, id: id(2) })
        .is_err());
}

// ---- Corrupt, truncated, duplicated and reordered journals ----

fn invalid(rows: &[Row]) -> bool {
    matches!(
        recover(OP, rows, &full_disk()).decision,
        Decision::NeedsAttention(Attention::InvalidJournal(_))
    )
}

#[test]
fn corrupt_journals_need_attention() {
    let rows = run()[11].rows.clone();
    assert!(!invalid(&rows));
    assert!(invalid(&[]));
    for k in 0..rows.len() {
        let mut t = rows.clone();
        let cut = t[k].body.len() / 2;
        t[k].body.truncate(cut);
        assert!(invalid(&t), "truncated body {k}");
        let mut f = rows.clone();
        f[k].body = f[k].body.replacen("\"epoch\":1", "\"epoch\":2", 1);
        assert!(invalid(&f), "edited body {k}");
        let mut c = rows.clone();
        c[k].chain[0] ^= 1;
        assert!(invalid(&c), "chain bit {k}");
        let mut c = rows.clone();
        c[k].chain.pop();
        assert!(invalid(&c), "short chain {k}");
        let mut d = rows.clone();
        d.insert(k, rows[k].clone());
        assert!(invalid(&d), "duplicated {k}");
        if k + 1 < rows.len() {
            let mut s = rows.clone();
            s.swap(k, k + 1);
            assert!(invalid(&s), "swapped {k}");
            let mut g = rows.clone();
            g.remove(k);
            assert!(invalid(&g), "gap at {k}");
        }
    }
    // Rows of one operation replayed as another's.
    assert!(matches!(
        recover(OP + 1, &rows, &full_disk()).decision,
        Decision::NeedsAttention(Attention::InvalidJournal(_))
    ));
    // Garbage that happens to be chained is still unreadable.
    let base = &run()[3];
    let mut rows = base.rows.clone();
    let mut row = base.replay.row_for(&Entry {
        epoch: 1,
        record: Record::Committed,
    });
    row.body = "{\"epoch\":1,\"kind\":\"unknown\"}".into();
    let mut r2 = base.replay.clone();
    assert!(r2.apply(&row).is_err());
    rows.push(row);
    assert!(invalid(&rows));
}

// ---- Randomized properties over valid runs, crash prefixes and perturbed disks ----

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.next() % 100 < pct
    }
}

/// A random executor that only takes legal steps and keeps a true disk beside the journal.
fn random_run(rng: &mut Rng) -> Vec<(J, FakeDisk)> {
    use Record::*;
    let n = 1 + rng.below(6) as u32;
    let names: Vec<String> = (0..n).map(|i| format!("f{i}")).collect();
    let mut next_id = 1u8;
    let mut fresh = || {
        next_id += 1;
        id(next_id)
    };
    let mut j = J::new();
    let mut disk = FakeDisk::default();
    let mut epoch = 1;
    let mut out = Vec::new();
    let mut token_n = 0;
    j.push(
        epoch,
        Started {
            digest: [1; 32],
            items: n,
        },
    );
    out.push((j.clone(), disk.clone()));
    for _ in 0..80 {
        let r = &j.replay;
        if r.state.is_terminal() {
            break;
        }
        let token = r.staging_tokens.last().cloned();
        let staging = r.staging;
        let fenced = r.fenced;
        let publish = r.publish.clone();
        let renamed = staging.is_some() && disk.parent.get(FINAL).copied() == staging;
        let held: Vec<StagedItem> = r.held().collect();
        let unheld: Vec<u32> = (0..n)
            .filter(|&i| r.items[i as usize].id.is_none() && !r.items[i as usize].verified)
            .collect();
        let unverified: Vec<StagedItem> = held
            .iter()
            .filter(|h| !r.items[h.item as usize].verified)
            .cloned()
            .collect();
        let open = r.open_batch.clone();
        let state = r.state;
        if !fenced && rng.chance(4) {
            epoch += 1;
            j.push(
                epoch,
                Fenced {
                    cause: FenceCause::GrantRevoked,
                },
            );
            out.push((j.clone(), disk.clone()));
            continue;
        }
        match state {
            OpState::Preparing if fenced => {
                j.push(
                    epoch,
                    Transition {
                        to: OpState::Cancelling,
                        code: Some(Refusal::GrantRevoked),
                    },
                );
            }
            OpState::Preparing => match token {
                Some(t) if !rng.chance(20) => {
                    let s = fresh();
                    disk.parent.insert(t, s);
                    disk.folder = Some(s);
                    j.push(epoch, StagingCreated { id: s });
                }
                _ => {
                    token_n += 1;
                    let t = format!("stg-{token_n}");
                    j.push(epoch, StagingIntent { token: t.clone() });
                    if rng.chance(30) {
                        // A crash after the folder was created: an unjournaled leftover.
                        disk.parent.insert(t, fresh());
                    }
                }
            },
            OpState::Copying | OpState::Blocked | OpState::Staged | OpState::NeedsAttention
                if fenced =>
            {
                if publish.is_some() && renamed {
                    break; // published but unconfirmed: the owner's call
                } else if publish.is_some() {
                    j.push(epoch, PublishWithdrawn);
                } else {
                    j.push(
                        epoch,
                        Transition {
                            to: OpState::Cancelling,
                            code: Some(Refusal::GrantRevoked),
                        },
                    );
                }
            }
            OpState::Copying => {
                if !open.is_empty() {
                    // Identity commit for what the batch created.
                    let ids: Vec<(u32, ObjectId)> = open
                        .iter()
                        .filter_map(|&i| disk.staged.get(&names[i as usize]).map(|o| (i, o.id)))
                        .collect();
                    if ids.is_empty() {
                        j.push(
                            epoch,
                            Transition {
                                to: OpState::Cancelling,
                                code: None,
                            },
                        );
                    } else {
                        j.push(epoch, Identities { items: ids });
                    }
                } else if !unheld.is_empty() && rng.chance(60) {
                    let take = 1 + rng.below(unheld.len());
                    let batch: Vec<(u32, String)> = unheld[..take]
                        .iter()
                        .map(|&i| (i, names[i as usize].clone()))
                        .collect();
                    j.push(
                        epoch,
                        CreateBatch {
                            items: batch.clone(),
                        },
                    );
                    for (_, name) in batch {
                        if rng.chance(90) {
                            disk.staged.insert(
                                name,
                                Observed {
                                    id: fresh(),
                                    links: 1,
                                },
                            );
                        }
                    }
                } else if !unverified.is_empty() {
                    let h = &unverified[rng.below(unverified.len())];
                    match rng.below(10) {
                        0 => {
                            let mm = j.replay.items[h.item as usize].mismatches;
                            j.push(
                                epoch,
                                ItemFailed {
                                    item: h.item,
                                    code: Refusal::VerificationMismatch,
                                },
                            );
                            if mm == 0 {
                                disk.staged.remove(&h.name);
                                j.push(
                                    epoch,
                                    ItemRemoved {
                                        item: h.item,
                                        id: h.id,
                                    },
                                );
                            }
                        }
                        1 => {
                            j.push(
                                epoch,
                                ItemFailed {
                                    item: h.item,
                                    code: Refusal::AccessDenied,
                                },
                            );
                        }
                        2 => {
                            j.push(
                                epoch,
                                ItemFailed {
                                    item: h.item,
                                    code: Refusal::SourceChanged,
                                },
                            );
                        }
                        _ => {
                            j.push(
                                epoch,
                                Verified {
                                    items: vec![h.item],
                                },
                            );
                        }
                    }
                } else if unheld.is_empty() {
                    j.push(
                        epoch,
                        Transition {
                            to: OpState::Staged,
                            code: None,
                        },
                    );
                } else {
                    j.push(
                        epoch,
                        Transition {
                            to: OpState::Cancelling,
                            code: None,
                        },
                    );
                }
            }
            OpState::Blocked | OpState::NeedsAttention => {
                if publish.is_some() && !renamed {
                    j.push(epoch, PublishWithdrawn);
                } else if publish.is_some() {
                    break; // published but unconfirmed: the owner's call
                } else if rng.chance(50) {
                    j.push(
                        epoch,
                        Transition {
                            to: OpState::Copying,
                            code: None,
                        },
                    );
                } else {
                    j.push(
                        epoch,
                        Transition {
                            to: OpState::Cancelling,
                            code: None,
                        },
                    );
                }
            }
            OpState::Staged => {
                if rng.chance(85) {
                    j.push(
                        epoch,
                        PublishIntent {
                            final_name: FINAL.into(),
                        },
                    );
                } else {
                    j.push(
                        epoch,
                        Transition {
                            to: OpState::Cancelling,
                            code: None,
                        },
                    );
                }
            }
            OpState::Publishing => {
                if renamed {
                    if rng.chance(80) {
                        j.push(epoch, Committed);
                    } else {
                        j.push(
                            epoch,
                            Transition {
                                to: OpState::NeedsAttention,
                                code: Some(Refusal::ChangedAfterVerification),
                            },
                        );
                    }
                } else if fenced {
                    j.push(epoch, PublishWithdrawn);
                } else if rng.chance(70) {
                    let s = disk.parent.remove(token.as_ref().unwrap()).unwrap();
                    disk.parent.insert(FINAL.into(), s);
                } else {
                    j.push(
                        epoch,
                        Transition {
                            to: OpState::Blocked,
                            code: Some(Refusal::PublishInUse),
                        },
                    );
                }
            }
            OpState::Cancelling => {
                if let Some(h) = held.first() {
                    disk.staged.remove(&h.name);
                    if rng.chance(85) {
                        j.push(
                            epoch,
                            ItemRemoved {
                                item: h.item,
                                id: h.id,
                            },
                        );
                    }
                } else if let (Some(s), false) = (staging, j.replay.staging_removed) {
                    if !disk.staged.is_empty() {
                        break; // an unjournaled leftover keeps the folder: needs the owner
                    }
                    disk.parent.remove(token.as_ref().unwrap());
                    disk.folder = None;
                    j.push(epoch, StagingRemoved { id: s });
                } else {
                    j.push(
                        epoch,
                        Transition {
                            to: OpState::Cancelled,
                            code: None,
                        },
                    );
                }
            }
            _ => unreachable!("{state:?}"),
        }
        out.push((j.clone(), disk.clone()));
    }
    out
}

/// Every identity and staging-relative name the journal ever recorded.
fn recorded(rows: &[Row]) -> (HashSet<ObjectId>, HashSet<String>, HashSet<String>) {
    let (mut ids, mut names, mut tokens) = (HashSet::new(), HashSet::new(), HashSet::new());
    for row in rows {
        match serde_json::from_str::<Entry>(&row.body).unwrap().record {
            Record::StagingCreated { id } => {
                ids.insert(id);
            }
            Record::Identities { items } => ids.extend(items.into_iter().map(|(_, id)| id)),
            Record::CreateBatch { items } => names.extend(items.into_iter().map(|(_, n)| n)),
            Record::StagingIntent { token } => {
                tokens.insert(token);
            }
            _ => {}
        }
    }
    (ids, names, tokens)
}

fn perturb(rng: &mut Rng, disk: &FakeDisk, j: &J) -> FakeDisk {
    let mut d = disk.clone();
    let names: Vec<String> = j
        .replay
        .items
        .iter()
        .filter_map(|i| i.name.clone())
        .collect();
    match rng.below(8) {
        0 if !names.is_empty() => {
            let n = &names[rng.below(names.len())];
            d.staged.insert(
                n.clone(),
                Observed {
                    id: id(200),
                    links: 1,
                },
            );
        }
        1 if !names.is_empty() => {
            d.staged.remove(&names[rng.below(names.len())]);
        }
        2 if !d.staged.is_empty() => {
            let k = d.staged.keys().next().unwrap().clone();
            d.staged.get_mut(&k).unwrap().links = 2;
        }
        3 => {
            if let Some(t) = j.replay.staging_tokens.last() {
                if let Some(s) = d.parent.remove(t) {
                    d.parent.insert(FINAL.into(), s);
                }
            }
        }
        4 => {
            d.parent.insert(FINAL.into(), id(201));
        }
        5 => {
            if let Some(t) = j.replay.staging_tokens.last() {
                d.parent.insert(t.clone(), id(202));
            }
        }
        _ => {}
    }
    d
}

fn check_safety(j: &J, disk: &FakeDisk, rec: &Recovery) {
    let (ids, names, tokens) = recorded(&j.rows);
    let staged_ok = |s: &StagedItem| {
        assert!(ids.contains(&s.id), "unrecorded identity {s:?}");
        assert!(names.contains(&s.name), "unrecorded name {s:?}");
        assert!(!s.name.starts_with('/') && !s.name.split('/').any(|c| c == ".." || c.is_empty()));
    };
    let token = j.replay.staging_tokens.last();
    match &rec.decision {
        Decision::Cleanup(c) => {
            if let Some(s) = c.staging {
                assert!(ids.contains(&s));
                assert_eq!(Some(s), j.replay.staging);
            }
            // A published copy is never cleaned up.
            if let Some(s) = j.replay.staging {
                assert_ne!(
                    disk.in_parent(FINAL),
                    Some(s),
                    "cleanup of a published copy"
                );
            }
            if c.remove_staging || !c.remove.is_empty() {
                let s = c.staging.unwrap();
                assert_eq!(
                    disk.in_parent(token.unwrap()),
                    Some(s),
                    "cleanup outside staging"
                );
            }
            for s in &c.remove {
                staged_ok(s);
                assert_eq!(
                    disk.in_staging(c.staging.unwrap(), &s.name),
                    Some(Observed { id: s.id, links: 1 })
                );
            }
            c.gone.iter().for_each(staged_ok);
        }
        Decision::Resume(r) => {
            r.reverify.iter().chain(&r.gone).for_each(staged_ok);
            for s in &r.reverify {
                assert_eq!(
                    disk.in_staging(j.replay.staging.unwrap(), &s.name),
                    Some(Observed { id: s.id, links: 1 })
                );
            }
        }
        Decision::Confirm {
            staging,
            final_name,
        } => {
            assert!(ids.contains(staging));
            assert_eq!(disk.in_parent(final_name), Some(*staging));
        }
        Decision::NeedsAttention(Attention::InvalidJournal(e)) => {
            panic!("a valid journal prefix did not replay: {e:?}")
        }
        Decision::NeedsAttention(Attention::Unjournaled(ns)) => {
            assert!(ns.iter().all(|n| names.contains(n)));
        }
        _ => {}
    }
    assert!(rec.leftover_staging.iter().all(|t| tokens.contains(t)));
}

#[test]
fn property_recovery_never_targets_unrecorded_identities_or_paths() {
    let mut states = HashSet::new();
    let mut decisions = HashSet::new();
    for seed in 1..=1500u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        for (j, disk) in random_run(&mut rng) {
            for d in [
                disk.clone(),
                perturb(&mut rng, &disk, &j),
                perturb(&mut rng, &disk, &j),
            ] {
                let rec = recover(OP, &j.rows, &d);
                check_safety(&j, &d, &rec);
                states.insert(j.state());
                decisions.insert(std::mem::discriminant(&rec.decision));
            }
        }
    }
    // The walk must actually reach the interesting places.
    for s in [
        OpState::Preparing,
        OpState::Copying,
        OpState::Blocked,
        OpState::NeedsAttention,
        OpState::Staged,
        OpState::Publishing,
        OpState::Committed,
        OpState::Cancelling,
        OpState::Cancelled,
    ] {
        assert!(states.contains(&s), "never reached {s:?}");
    }
    assert_eq!(decisions.len(), 6, "every decision kind reached");
}

#[test]
fn property_any_mid_journal_damage_needs_attention() {
    for seed in 1..=400u64 {
        let mut rng = Rng(seed.wrapping_mul(0xD1B5_4A32_D192_ED03) | 1);
        let run = random_run(&mut rng);
        let rows = &run.last().unwrap().0.rows;
        if rows.len() < 2 {
            continue;
        }
        let k = rng.below(rows.len() - 1);
        let mut m = rows.clone();
        match rng.below(5) {
            0 => {
                let cut = rng.below(m[k].body.len());
                m[k].body.truncate(cut);
            }
            1 => {
                let at = rng.below(m[k].body.len());
                let mut b = m[k].body.clone().into_bytes();
                b[at] = if b[at] == b'0' { b'1' } else { b'0' };
                m[k].body = String::from_utf8_lossy(&b).into_owned();
            }
            2 => m.insert(k + 1, rows[k].clone()),
            3 => m.swap(k, k + 1),
            _ => {
                m.remove(k);
            }
        }
        assert!(invalid(&m), "seed {seed}: damage at {k} accepted");
    }
}

// ---- state.db storage ----

#[test]
fn store_is_append_only_full_sync_and_validates_appends() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let mut store = Store::open(&path).unwrap();
    let op = store.start(1, [5; 32], 1).unwrap();
    assert_eq!(op, 1);
    assert_eq!(store.start(1, [6; 32], 1).unwrap(), 2);
    store
        .append(
            op,
            1,
            Record::StagingIntent {
                token: TOKEN.into(),
            },
        )
        .unwrap();
    // Rejected records are never stored.
    assert!(matches!(
        store.append(op, 1, Record::Committed),
        Err(AppendError::Rejected(_))
    ));
    assert!(matches!(
        store.append(99, 1, Record::Committed),
        Err(AppendError::Rejected(_))
    ));
    assert_eq!(store.rows(op).unwrap().len(), 2);
    store
        .append(op, 1, Record::StagingCreated { id: id(S) })
        .unwrap();
    // A second writer (the revocation path) fences; the first writer's stale record is refused.
    let mut revoker = Store::open(&path).unwrap();
    revoker
        .append(
            op,
            2,
            Record::Fenced {
                cause: FenceCause::EmergencyStop,
            },
        )
        .unwrap();
    assert!(matches!(
        store.append(
            op,
            1,
            Record::CreateBatch {
                items: items(&[(0, "a")])
            }
        ),
        Err(AppendError::Rejected(_))
    ));
    store
        .append(
            op,
            2,
            Record::Transition {
                to: OpState::Cancelling,
                code: Some(Refusal::GrantRevoked),
            },
        )
        .unwrap();
    assert_eq!(store.operations().unwrap(), vec![1, 2]);
    assert_eq!(
        store.replay(op).unwrap().unwrap().state,
        OpState::Cancelling
    );
    let disk = FakeDisk::default().staging();
    assert_eq!(
        store.recover(op, &disk).unwrap().decision,
        Decision::Cleanup(Cleanup {
            staging: Some(id(S)),
            remove_staging: true,
            ..Cleanup::default()
        })
    );
    drop((store, revoker));

    let raw = rusqlite::Connection::open(&path).unwrap();
    assert!(raw
        .execute(
            "UPDATE op_journal SET body='{}' WHERE op_id=1 AND seq=2",
            []
        )
        .is_err());
    assert!(raw
        .execute("DELETE FROM op_journal WHERE op_id=1", [])
        .is_err());
    assert!(raw
        .execute(
            "INSERT INTO op_journal(op_id,seq,body,chain) VALUES(1,9,'x',x'00')",
            []
        )
        .is_err());
    // Tampering through a dropped trigger is still caught by the chain on load.
    raw.execute_batch(
        "DROP TRIGGER op_journal_no_update; UPDATE op_journal SET body=replace(body,'staging','stagin9') WHERE op_id=1 AND seq=2",
    )
    .unwrap();
    drop(raw);
    let store = Store::open(&path).unwrap();
    assert!(matches!(
        store.recover(op, &disk).unwrap().decision,
        Decision::NeedsAttention(Attention::InvalidJournal(Invalid { seq: 2, .. }))
    ));
    let sync: i64 = store.synchronous().unwrap();
    assert_eq!(sync, 2, "state.db journal must run synchronous=FULL");
}
