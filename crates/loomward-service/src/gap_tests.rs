//! Muse test-gap pins (w2): the grant-id grammar, backup-copy holds, root scan words, the
//! state-changing command set, default deadlines and the retry table. Each test names the
//! production line a mutation of which it must fail.
use super::*;
use crate::handlers::{grant_number, holds, scan_view};
use crate::{default_deadline, fail, is_mutation, retry_flag};

/// `grant_number` accepts exactly the canonical `gr_N` with N > 0 that fits in an i64.
#[test]
fn grant_ids_accept_only_the_canonical_positive_form() {
    assert_eq!(grant_number("gr_1").unwrap(), 1);
    assert_eq!(grant_number("gr_42").unwrap(), 42);
    assert_eq!(grant_number(&format!("gr_{}", i64::MAX)).unwrap(), i64::MAX);
    for id in [
        "gr_0",
        "gr_-1",
        "gr_+1",
        "gr_01",
        "gr_x",
        "gr_",
        "gr_1 ",
        "gr_1.0",
        "gr_9223372036854775808",
        "1",
        "GR_1",
        "Gr_1",
        "grant_1",
        "",
    ] {
        assert_eq!(
            grant_number(id).unwrap_err().code,
            ErrorCode::NotFound,
            "{id:?}"
        );
    }
}

/// Through `grants.revoke`: a non-canonical spelling of a live grant's id is refused and the grant
/// stays active; only the canonical id revokes it. A `+` sign is outside the wire `GrantId`
/// pattern (`^gr_[A-Za-z0-9_-]{1,64}$`) and is refused as `invalid_request`; the others pass the
/// pattern and reach `grant_number`, which answers `not_found`.
#[test]
fn grant_ids_that_are_not_canonical_never_revoke_a_live_grant() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, grant) = register(&svc, 9);
    for (id, code) in [
        (format!("gr_0{grant}"), ErrorCode::NotFound),
        (format!("gr_00{grant}"), ErrorCode::NotFound),
        (format!("gr_-{grant}"), ErrorCode::NotFound),
        ("gr_0".to_string(), ErrorCode::NotFound),
        ("gr_x".to_string(), ErrorCode::NotFound),
        (format!("gr_+{grant}"), ErrorCode::InvalidRequest),
    ] {
        assert_eq!(
            err(call(&svc, "grants.revoke", json!({"grant_id": id}))).code,
            code,
            "{id}"
        );
        assert_eq!(
            crate::db::roots(&svc.inner.db(), Some(root)).unwrap()[0].grant_state,
            "active",
            "{id}"
        );
    }
    ok(
        "grants.revoke",
        call(
            &svc,
            "grants.revoke",
            json!({"grant_id": format!("gr_{grant}")}),
        ),
    );
}

/// `holds` matches the name as UTF-8 or as UTF-16LE bytes; a copy that cannot be read holds it
/// (so the copy is removed), and an empty name holds nothing.
#[test]
fn backup_copies_hold_a_name_as_utf8_or_utf16le_and_fail_closed() {
    let dir = temp_root();
    let file = dir.path().join("state.before-v1.db");
    let name = "Synthetic Holds Name";
    let utf16: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let around = |middle: &[u8]| {
        let mut bytes = vec![0u8, 1, 2];
        bytes.extend_from_slice(middle);
        bytes.extend_from_slice(&[0xff, 0xfe]);
        bytes
    };
    std::fs::write(&file, around(name.as_bytes())).unwrap();
    assert!(holds(&file, name), "UTF-8 name");
    std::fs::write(&file, around(&utf16)).unwrap();
    assert!(holds(&file, name), "UTF-16LE name");
    std::fs::write(&file, around(b"Synthetic Other Name")).unwrap();
    assert!(!holds(&file, name), "another name");
    let unreadable = dir.path().join("state.before-v2.db");
    std::fs::create_dir(&unreadable).unwrap();
    assert!(holds(&unreadable, name), "an unreadable copy");
    assert!(!holds(&file, ""), "an empty name");
}

fn scan_row(grant_state: &str, state: &str, subtree_complete: bool) -> crate::db::RootRow {
    crate::db::RootRow {
        root: 1,
        grant: 1,
        volume: None,
        generation: None,
        state: state.into(),
        display_path: "Synthetic root".into(),
        origin: "fixture".into(),
        granted_via: "fixture".into(),
        grant_state: grant_state.into(),
        granted_at_ns: 0,
        revoked_at_ns: None,
        dir: None,
        finished_at_ns: None,
        subtree_complete,
    }
}

/// The run state picks the scan word; `complete` needs a complete witness; a root whose grant is
/// not active is stale whatever its run state; an unrecognised run state reads failed/unknown.
#[test]
fn scan_views_map_grant_and_run_state_to_the_roots_list_words() {
    let cases = [
        (
            ("active", "never_scanned", false),
            ("never_scanned", "unscanned"),
        ),
        (("active", "scanning", false), ("scanning", "partial")),
        (("active", "complete", true), ("complete", "complete")),
        (("active", "complete", false), ("complete", "partial")),
        (("active", "partial", true), ("partial", "partial")),
        (("active", "repairing", true), ("partial", "partial")),
        (("active", "stale", true), ("stale", "stale")),
        (("active", "failed", false), ("failed", "unknown")),
        (("active", "unrecognised", false), ("failed", "unknown")),
        (("revoked", "complete", true), ("stale", "stale")),
        (("identity_changed", "scanning", false), ("stale", "stale")),
        (("revoked", "failed", false), ("stale", "stale")),
    ];
    for ((grant, run, subtree), want) in cases {
        assert_eq!(
            scan_view(&scan_row(grant, run, subtree)),
            want,
            "{grant}/{run}/{subtree}"
        );
    }
}

/// The same words through `roots.list`: a `failed` run state reads failed/unknown, and a revoked
/// grant reads stale/stale even when its root row still says `complete`.
#[test]
fn roots_list_shows_failed_and_revoked_roots_with_the_scan_words() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (root, grant) = register(&svc, 5);
    let scan = |svc: &Service| {
        let (roots, _) = ok("roots.list", call(svc, "roots.list", json!({})));
        roots["roots"][0]["scan"].clone()
    };
    svc.inner
        .db()
        .execute("UPDATE main.root SET state='failed' WHERE id=?1", [root])
        .unwrap();
    let s = scan(&svc);
    assert_eq!(
        (s["state"].as_str(), s["coverage"].as_str()),
        (Some("failed"), Some("unknown"))
    );
    svc.inner
        .db()
        .execute("UPDATE main.root SET state='complete' WHERE id=?1", [root])
        .unwrap();
    assert_eq!(scan(&svc)["state"], "complete");
    ok(
        "grants.revoke",
        call(
            &svc,
            "grants.revoke",
            json!({"grant_id": format!("gr_{grant}")}),
        ),
    );
    svc.inner
        .db()
        .execute("UPDATE main.root SET state='complete' WHERE id=?1", [root])
        .unwrap();
    let s = scan(&svc);
    assert_eq!(
        (s["state"].as_str(), s["coverage"].as_str()),
        (Some("stale"), Some("stale"))
    );
}

/// `is_mutation` is exactly the state-changing set (the mutation FIFO, the mutation deadline
/// default and the mutation retry rule all key on it).
#[test]
fn only_the_listed_commands_change_state() {
    use Command::*;
    let state_changing = [
        RootsRequestGrant,
        RootsRevoke,
        RootsForget,
        VolumesDeclareTier,
        ScanStart,
        ScanCancel,
        JobsCancel,
        DuplicatesInspect,
        DuplicatesCancel,
        FeedbackRecord,
        LearningRefit,
        CollectionsCreate,
        CollectionsUpdateMembers,
        TeacherPreview,
        GrantsCreateDisclosure,
        GrantsRevoke,
        TeacherRun,
        PlacementSimulate,
        TelemetrySubscribe,
        TelemetryUnsubscribe,
        BudgetsSet,
        BackupsImport,
        BackupsForget,
        DiagnosticsSave,
        AlertsConfigure,
        // Gated today (copy_effect_gated), mutations so the gate can never open onto the read path (#508).
        CopyApprove,
        CopyStart,
        CopyCancel,
        CopyAbandon,
        AskConfigure,
        AskGrant,
        ClientsGrant,
        ClientsRevoke,
        AskRevoke,
        AskTurn,
    ];
    for c in Command::ALL {
        assert_eq!(
            is_mutation(*c),
            state_changing.contains(c),
            "{}",
            c.as_str()
        );
    }
}

/// `semantics.md` section 2 defaults: reads 2,000 ms, `tree.slice` 5,000 ms, `search.query`
/// 3,000 ms, mutations 5,000 ms, and `placement.simulate` 10,000 ms.
#[test]
fn default_deadlines_follow_the_contract_per_command_class() {
    use Command::*;
    assert_eq!(default_deadline(TreeSlice), 5_000);
    assert_eq!(default_deadline(SearchQuery), 3_000);
    assert_eq!(default_deadline(PlacementSimulate), 10_000);
    assert_eq!(default_deadline(BudgetsSet), 5_000);
    assert_eq!(default_deadline(TeacherRun), 5_000);
    assert_eq!(default_deadline(RootsRevoke), 5_000);
    assert_eq!(default_deadline(RootsList), 2_000);
}

/// `retryable` by code, per `semantics.md` section 8 and #144, across four command classes: a
/// read, a mutation, a placement simulation (a mutation id that is a read unless saved) and
/// `teacher.run`.
#[test]
fn retry_flags_follow_the_section_8_table_for_every_code() {
    use ErrorCode::*;
    let commands = [
        Command::RootsList,
        Command::BudgetsSet,
        Command::PlacementSimulate,
        Command::TeacherRun,
    ];
    let table = [
        (InvalidRequest, [false; 4]),
        (UnsupportedProtocol, [false; 4]),
        (UnknownCommand, [false; 4]),
        (NotFound, [false; 4]),
        (PermissionDenied, [false; 4]),
        (CapabilityUnavailable, [false; 4]),
        (Cancelled, [false; 4]),
        (StaleGeneration, [true; 4]),
        (DeviceOffline, [true; 4]),
        (PartialCoverage, [true; 4]),
        (ResourceBudget, [true; 4]),
        (Busy, [true; 4]),
        // A timed-out mutation has an unknown outcome and is never blindly resent.
        (DeadlineExceeded, [true, false, true, false]),
        // Retryable (at most once in the contract), except for teacher.run.
        (InternalError, [true, true, true, false]),
    ];
    for (code, want) in table {
        for (command, expected) in commands.into_iter().zip(want) {
            assert_eq!(
                retry_flag(command, fail(code, "synthetic")).retryable,
                expected,
                "{code:?} on {}",
                command.as_str()
            );
        }
    }
}
