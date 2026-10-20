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

/// #738 wire record contract. Parsing the proposed row makes this test compile on the old reader;
/// that reader rejects the new kind, rather than turning a missing variant into a compile failure.
fn after_publish(cause: &str) -> Record {
    serde_json::from_value(serde_json::json!({
        "kind": "revoked_after_publish",
        "cause": cause,
    }))
    .expect("the journal must recognize the metadata-only terminal annotation")
}

#[test]
fn terminal_annotation_preserves_the_committed_receipt_and_effect_refusals() {
    let mut j = run().pop().unwrap();
    let before = j.replay.clone();
    j.push(1, after_publish("grant_revoked"));
    let after = Replay::from_rows(OP, &j.rows).unwrap();
    assert_eq!(after.state, before.state);
    assert_eq!(after.epoch, before.epoch);
    assert_eq!(after.fenced, before.fenced);
    assert_eq!(after.items, before.items);
    assert_eq!(after.staging, before.staging);
    assert_eq!(after.publish, before.publish);
    assert_eq!(after.seq, before.seq + 1);
    assert_eq!(
        decide(&j, &full_disk().published()).decision,
        Decision::Settled
    );
    for record in [
        Record::Fenced {
            cause: FenceCause::EmergencyStop,
        },
        Record::Committed,
        Record::Transition {
            to: OpState::Copying,
            code: None,
        },
        Record::Transition {
            to: OpState::Cancelling,
            code: None,
        },
    ] {
        assert!(j.try_push(2, record).is_err());
    }
}

#[test]
fn terminal_annotation_is_illegal_before_commit_and_at_a_stale_epoch() {
    for mut j in run().into_iter().take(11) {
        assert!(j.try_push(1, after_publish("grant_revoked")).is_err());
    }
    let mut committed = run().pop().unwrap();
    assert!(committed
        .try_push(0, after_publish("grant_revoked"))
        .is_err());
    assert!(committed
        .try_push(2, after_publish("grant_revoked"))
        .is_err());
}

#[test]
fn terminal_annotation_keeps_distinct_causes_and_rejects_a_duplicate() {
    let mut j = run().pop().unwrap();
    j.push(1, after_publish("grant_revoked"));
    j.push(1, after_publish("emergency_stop"));
    assert!(j.try_push(1, after_publish("grant_revoked")).is_err());
    assert!(j.try_push(1, after_publish("emergency_stop")).is_err());
    assert_eq!(j.state(), OpState::Committed);
}

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
        planned_ft: None,
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

// ---- Cancel is sticky; batches close exactly (review round 1 on #474) ----

fn tr(to: OpState) -> Record {
    Record::Transition { to, code: None }
}

/// Bypassing the append check must not help either: recovery refuses the journal.
fn refused_everywhere(base: &J, epoch: u64, record: Record) {
    let entry = Entry { epoch, record };
    assert!(
        base.clone().try_push(epoch, entry.record.clone()).is_err(),
        "{entry:?} accepted"
    );
    let mut rows = base.rows.clone();
    rows.push(base.replay.row_for(&entry));
    assert!(
        matches!(
            recover(OP, &rows, &full_disk()).decision,
            Decision::NeedsAttention(Attention::InvalidJournal(_))
        ),
        "{entry:?} replayed"
    );
}

#[test]
fn cancel_is_sticky_abandon_is_the_only_exit() {
    // The reviewer's sequence: cancelling -> needs_attention -> copying -> staged -> publish.
    let mut j = run()[8].clone();
    j.push(1, tr(OpState::Cancelling))
        .push(1, tr(OpState::NeedsAttention));
    refused_everywhere(&j, 1, tr(OpState::Copying));
    // Abandon again is the exit.
    let mut k = j.clone();
    k.push(1, tr(OpState::Cancelling));
    assert_eq!(k.state(), OpState::Cancelling);
    assert_eq!(
        decide(&j, &full_disk()).decision,
        Decision::NeedsAttention(Attention::Recorded)
    );

    // Staging removed first, then anything that would recreate or publish against it.
    let mut j = run()[8].clone();
    j.push(1, tr(OpState::Cancelling));
    for (item, n) in [(0u32, 1u8), (1, 2), (2, 3)] {
        j.push(1, Record::ItemRemoved { item, id: id(n) });
    }
    j.push(1, Record::StagingRemoved { id: id(S) })
        .push(1, tr(OpState::NeedsAttention));
    refused_everywhere(&j, 1, tr(OpState::Copying));
    refused_everywhere(
        &j,
        1,
        Record::CreateBatch {
            items: items(&[(0, "d")]),
        },
    );
    refused_everywhere(
        &j,
        1,
        Record::PublishIntent {
            final_name: FINAL.into(),
        },
    );
    // Even without the detour through needs_attention.
    let mut c = run()[8].clone();
    c.push(1, tr(OpState::Cancelling));
    refused_everywhere(&c, 1, Record::Verified { items: vec![0] });
    refused_everywhere(
        &c,
        1,
        Record::ItemFailed {
            item: 0,
            code: Refusal::AccessDenied,
        },
    );
}

#[test]
fn batch_retry_and_create_failure_inside_a_batch() {
    // A crash before the identity commit: the same batch may be intended again, nothing else.
    let mut j = run()[3].clone();
    j.push(
        1,
        Record::CreateBatch {
            items: items(&[(1, "d/a"), (0, "d")]),
        },
    );
    // A create that fails inside the batch leaves it; the identity commit covers exactly the rest,
    // even though the failure already moved the run to needs_attention.
    j.push(
        1,
        Record::ItemFailed {
            item: 1,
            code: Refusal::StagingNameTaken,
        },
    );
    assert_eq!(j.state(), OpState::NeedsAttention);
    refused_everywhere(
        &j,
        1,
        Record::Identities {
            items: vec![(0, id(1)), (1, id(2))],
        },
    );
    j.push(
        1,
        Record::Identities {
            items: vec![(0, id(1))],
        },
    );
    assert!(j.replay.open_batch.is_empty());
    // The failed item's name is reported if something sits there, never adopted.
    let disk = FakeDisk::default().staging().file("d", 1).file("d/a", 77);
    assert_eq!(
        decide(&j, &disk).decision,
        Decision::NeedsAttention(Attention::Recorded)
    );
    j.push(1, tr(OpState::Copying));
    assert_eq!(
        decide(&j, &disk).decision,
        Decision::NeedsAttention(Attention::Unjournaled(vec!["d/a".into()]))
    );
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
        // Batch tracking: the identity commit covers the open batch exactly; no second batch while open.
        (
            4,
            1,
            Identities {
                items: vec![(0, id(1))],
            },
        ),
        (
            4,
            1,
            Identities {
                items: vec![(0, id(1)), (1, id(2)), (2, id(3))],
            },
        ),
        (
            4,
            1,
            CreateBatch {
                items: items(&[(2, "b")]),
            },
        ),
        (
            4,
            1,
            CreateBatch {
                items: items(&[(0, "d")]),
            },
        ),
        (
            4,
            1,
            CreateBatch {
                items: items(&[(0, "d"), (1, "d/a"), (2, "b")]),
            },
        ),
        (
            7,
            1,
            Identities {
                items: vec![(2, id(3)), (2, id(4))],
            },
        ),
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
        let cancelled = r.cancelled;
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
            OpState::Copying | OpState::Blocked | OpState::NeedsAttention
                if !open.is_empty() && !cancelled =>
            {
                // Close the open batch: a create that did not happen fails its item (leaving the batch),
                // then one identity commit covers exactly what remains.
                let missing = open
                    .iter()
                    .copied()
                    .find(|&i| !disk.staged.contains_key(&names[i as usize]));
                if let (Some(item), OpState::Copying) = (missing, state) {
                    j.push(
                        epoch,
                        ItemFailed {
                            item,
                            code: Refusal::AccessDenied,
                        },
                    );
                } else if missing.is_some() {
                    break; // a create failed outside copying: the owner's call
                } else {
                    let ids = open
                        .iter()
                        .map(|&i| (i, disk.staged[&names[i as usize]].id))
                        .collect();
                    j.push(epoch, Identities { items: ids });
                }
            }
            OpState::Copying => {
                if !unheld.is_empty() && rng.chance(60) {
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
                if cancelled {
                    // Once cancel started, Abandon is the only exit.
                    j.push(
                        epoch,
                        Transition {
                            to: OpState::Cancelling,
                            code: None,
                        },
                    );
                } else if publish.is_some() && !renamed {
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
                if rng.chance(8) {
                    // Cleanup refused: waits for the owner, who can only abandon again.
                    j.push(
                        epoch,
                        Transition {
                            to: OpState::NeedsAttention,
                            code: None,
                        },
                    );
                } else if let Some(h) = held.first() {
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
                if j.replay.cancelled {
                    assert!(
                        matches!(
                            j.state(),
                            OpState::Cancelling | OpState::NeedsAttention | OpState::Cancelled
                        ),
                        "cancel did not stick: {:?}",
                        j.state()
                    );
                }
                states.insert((j.state(), j.replay.cancelled));
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
        assert!(
            states.contains(&(s, false)) || states.contains(&(s, true)),
            "never reached {s:?}"
        );
    }
    assert!(
        states.contains(&(OpState::NeedsAttention, true)),
        "never reached needs_attention from cancelling"
    );
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

fn fenced_store(path: &std::path::Path) -> (Store, u64) {
    let mut store = Store::open(path).unwrap();
    let op = store.start(1, admitted(5)).unwrap();
    store
        .append(
            op,
            1,
            Record::StagingIntent {
                token: TOKEN.into(),
            },
        )
        .unwrap();
    store
        .append(op, 1, Record::StagingCreated { id: id(S) })
        .unwrap();
    store
        .append(
            op,
            2,
            Record::Fenced {
                cause: FenceCause::EmergencyStop,
            },
        )
        .unwrap();
    (store, op)
}

#[test]
fn store_tail_truncation_needs_attention_even_after_reopen() {
    for lost in 1..=4 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let (mut store, op) = fenced_store(&path);
        let disk = FakeDisk::default().staging();
        assert!(matches!(
            store.recover(op, &disk).unwrap().decision,
            Decision::Cleanup(_)
        ));
        let raw = rusqlite::Connection::open(&path).unwrap();
        raw.execute_batch("DROP TRIGGER op_journal_no_delete")
            .unwrap();
        raw.execute(
            "DELETE FROM op_journal WHERE op_id=?1 AND seq>?2",
            rusqlite::params![op as i64, 4 - lost],
        )
        .unwrap();
        assert!(store.replay(op).unwrap().is_err(), "lost {lost} rows");
        assert!(matches!(
            store.append(
                op,
                2,
                Record::Fenced {
                    cause: FenceCause::EmergencyStop
                }
            ),
            Err(AppendError::Rejected(_))
        ));
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert_eq!(store.operations().unwrap(), vec![op]);
        assert!(
            matches!(
                store.recover(op, &disk).unwrap().decision,
                Decision::NeedsAttention(Attention::InvalidJournal(_))
            ),
            "lost {lost} rows"
        );
        assert_eq!(store.start(1, admitted(6)).unwrap(), op + 1);
    }
}

#[test]
fn store_insert_or_replace_cannot_replace_a_fence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let (mut store, op) = fenced_store(&path);
    let rows = store.rows(op).unwrap();
    let prefix = Replay::from_rows(op, &rows[..3]).unwrap();
    let replacement = prefix.row_for(&Entry {
        epoch: 1,
        record: Record::Transition {
            to: OpState::Blocked,
            code: Some(Refusal::AccessDenied),
        },
    });
    let raw = rusqlite::Connection::open(&path).unwrap();
    let recursive: i64 = raw
        .query_row("PRAGMA recursive_triggers", [], |r| r.get(0))
        .unwrap();
    assert_eq!(recursive, 0);
    assert!(raw
        .execute(
            "INSERT OR REPLACE INTO op_journal(op_id,seq,body,chain) VALUES(?1,?2,?3,?4)",
            rusqlite::params![
                op as i64,
                replacement.seq as i64,
                replacement.body,
                replacement.chain
            ]
        )
        .is_err());
    assert_eq!(store.rows(op).unwrap(), rows);
    assert!(store.replay(op).unwrap().unwrap().fenced);

    // Simulate damage that bypasses the trigger: an otherwise valid replacement with the same seq.
    raw.execute_batch("DROP TRIGGER op_journal_no_replace")
        .unwrap();
    raw.execute(
        "INSERT OR REPLACE INTO op_journal(op_id,seq,body,chain) VALUES(?1,?2,?3,?4)",
        rusqlite::params![
            op as i64,
            replacement.seq as i64,
            replacement.body,
            replacement.chain
        ],
    )
    .unwrap();
    assert!(
        !Replay::from_rows(op, &store.rows(op).unwrap())
            .unwrap()
            .fenced
    );
    assert!(store.replay(op).unwrap().is_err());
    assert!(matches!(
        store
            .recover(op, &FakeDisk::default().staging())
            .unwrap()
            .decision,
        Decision::NeedsAttention(Attention::InvalidJournal(_))
    ));
    assert!(matches!(
        store.append(
            op,
            2,
            Record::Fenced {
                cause: FenceCause::EmergencyStop
            }
        ),
        Err(AppendError::Rejected(_))
    ));
    drop(store);
    assert!(Store::open(&path).unwrap().replay(op).unwrap().is_err());
}

#[test]
fn store_anchors_legacy_tails_once_and_never_repairs_a_missing_head() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let (store, op) = fenced_store(&path);
    let original = store.rows(op).unwrap();
    drop(store);
    let raw = rusqlite::Connection::open(&path).unwrap();
    // The legacy schema has no head table, but already contains C5 journals.
    raw.execute_batch("DROP TABLE IF EXISTS op_journal_head")
        .unwrap();
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.rows(op).unwrap(), original);
    assert!(store.replay(op).unwrap().unwrap().fenced);
    store
        .append(
            op,
            2,
            Record::Transition {
                to: OpState::Cancelling,
                code: None,
            },
        )
        .unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.replay(op).unwrap().unwrap().seq, 5);
    raw.execute("DELETE FROM op_journal_head WHERE op_id=?1", [op as i64])
        .unwrap();
    drop(store);
    let mut store = Store::open(&path).unwrap();
    assert!(store.replay(op).unwrap().is_err());
    assert!(matches!(
        store.recover(op, &FakeDisk::default()).unwrap().decision,
        Decision::NeedsAttention(Attention::InvalidJournal(_))
    ));
    assert!(matches!(
        store.append(
            op,
            2,
            Record::Transition {
                to: OpState::Cancelled,
                code: None
            }
        ),
        Err(AppendError::Rejected(_))
    ));
}

#[test]
fn store_rolls_back_row_when_head_write_fails() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let mut store = Store::open(&path).unwrap();
    let raw = rusqlite::Connection::open(&path).unwrap();
    // Fail precisely after the journal insert, for both start and append.
    raw.execute_batch(
        "CREATE TRIGGER fail_head_insert BEFORE INSERT ON op_journal_head
        WHEN EXISTS(SELECT 1 FROM op_journal WHERE op_id=NEW.op_id AND seq=NEW.seq)
        BEGIN SELECT RAISE(FAIL, 'head write interrupted'); END;",
    )
    .unwrap();
    assert!(matches!(
        store.start(1, admitted(5)),
        Err(AppendError::Sql(_))
    ));
    assert!(store.rows(1).unwrap().is_empty());
    assert!(store.operations().unwrap().is_empty());
    raw.execute_batch("DROP TRIGGER fail_head_insert").unwrap();
    let op = store.start(1, admitted(5)).unwrap();
    let before = store.rows(op).unwrap();
    raw.execute_batch(
        "CREATE TRIGGER fail_head_update BEFORE UPDATE ON op_journal_head
        WHEN EXISTS(SELECT 1 FROM op_journal WHERE op_id=NEW.op_id AND seq=NEW.seq)
        BEGIN SELECT RAISE(FAIL, 'head write interrupted'); END;",
    )
    .unwrap();
    assert!(matches!(
        store.append(
            op,
            1,
            Record::StagingIntent {
                token: TOKEN.into()
            }
        ),
        Err(AppendError::Sql(_))
    ));
    assert_eq!(store.rows(op).unwrap(), before);
    drop(store);
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.replay(op).unwrap().unwrap().seq, 1);
    assert_eq!(
        store.recover(op, &FakeDisk::default()).unwrap().decision,
        Decision::Reconfirm
    );
    raw.execute_batch("DROP TRIGGER fail_head_update").unwrap();
    store
        .append(
            op,
            1,
            Record::StagingIntent {
                token: TOKEN.into(),
            },
        )
        .unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.replay(op).unwrap().unwrap().seq, 2);
    assert!(matches!(
        store.recover(op, &FakeDisk::default()).unwrap().decision,
        Decision::Resume(_)
    ));
}

/// A consumed approval through the public consent path: the only way to start a journal.
fn admitted(n: u8) -> Admitted {
    let id = |b: u8| Identity {
        volume: 1,
        file_id: [b; 16],
    };
    let dest = DestinationFacts {
        grant: 1,
        parent: id(200),
        ancestors: vec![id(200)],
        verified_volume: true,
        cloud_synced: false,
        unsupported_volume: false,
        protected: false,
        case_sensitive: false,
        existing_names: vec![],
        parent_path_units: 10,
        available_bytes: 10 << 30,
        volume_bytes: 100 << 30,
        cluster_bytes: 4096,
    };
    let ctx = Context {
        elevated: false,
        grants_active: true,
        named_streams: false,
        date: "2026-10-17".into(),
    };
    let entry = SourceEntry {
        name: "a.bin".into(),
        kind: loomward_operations::tree::Kind::File,
        identity: id(n),
        size: 1,
        creation_ft: 1,
        last_write_ft: 1,
        change_ft: 1,
        attributes: 0x20,
        reparse_tag: None,
        ea_size: 0,
        streams: vec![],
        link_count: 1,
        explicit_aces: false,
        verified_volume: true,
    };
    let p = preview(vec![entry], &dest, &ctx);
    approve(&p, p.manifest.digest(), 0, 0)
        .unwrap()
        .admit(&p, 0, 1)
        .unwrap()
}

#[test]
fn store_is_append_only_full_sync_and_validates_appends() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let mut store = Store::open(&path).unwrap();
    let op = store.start(1, admitted(5)).unwrap();
    assert_eq!(op, 1);
    assert_eq!(store.start(1, admitted(6)).unwrap(), 2);
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
}

#[test]
fn store_forces_full_sync_and_wal_on_a_weaker_connection() {
    // SQLite's default is already FULL, so start from a connection deliberately set weaker: the store's
    // own pragmas are the only thing that can bring it back.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=OFF")
        .unwrap();
    let probe = |c: &rusqlite::Connection| -> i64 {
        c.query_row("PRAGMA synchronous", [], |r| r.get(0)).unwrap()
    };
    assert_eq!(probe(&conn), 0);
    let store = Store::new(conn).unwrap();
    assert_eq!(
        store.synchronous().unwrap(),
        2,
        "state.db journal must run synchronous=FULL"
    );
    let mode: String = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
}
