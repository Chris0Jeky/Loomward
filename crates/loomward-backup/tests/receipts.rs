use loomward_backup::*;
use serde_json::{json, Value};

fn file(path: &str) -> FileVersion {
    FileVersion {
        path: path.into(),
        version: "a".repeat(64),
    }
}

fn observation() -> Observation {
    Observation {
        repository_id: "synthetic-repository".into(),
        snapshot_id: "b".repeat(64),
        observed_at: 990,
        exit_code: 0,
    }
}

fn source() -> BackupSource {
    let files = vec![file("fixture/document.txt"), file("fixture/notes.txt")];
    BackupSource {
        repository_id: "synthetic-repository".into(),
        snapshot_id: "b".repeat(64),
        observed_at: 990,
        now: 1000,
        max_age_seconds: 60,
        topology: Topology::IndependentTarget,
        credentials_valid_until: Some(2000),
        required: files.clone(),
        snapshot_files: files.clone(),
        required_metadata: vec![MetadataClass::Acl],
        integrity: Some(IntegrityObservation {
            observation: observation(),
            raw: check(),
            read_all_data: true,
        }),
        restore: Some(RestoreObservation {
            observation: observation(),
            complete: true,
            files: files
                .into_iter()
                .map(|file| RestoredFile {
                    file,
                    compared_metadata: vec![MetadataClass::Acl],
                })
                .collect(),
        }),
    }
}

fn snapshots() -> Vec<u8> {
    serde_json::to_vec(&json!([{"id":"b".repeat(64),"time":"2026-01-01T00:00:00Z","paths":["/synthetic-fixture"],
        "tree":"c".repeat(64),"hostname":"synthetic-host","username":"synthetic-owner","uid":0,"gid":0,
        "summary":{"total_files_processed":2,"total_bytes_processed":32}}])).unwrap()
}

fn check() -> Vec<u8> {
    br#"{"message_type":"summary","num_errors":0,"broken_packs":[],"suggest_repair_index":false,"suggest_prune":false}"#.to_vec()
}

fn evidence(source: &BackupSource) -> RecoveryEvidence {
    normalise_backup_receipt(&snapshots(), source).unwrap()
}

fn refused(source: &BackupSource, reason: Refusal) {
    let e = evidence(source);
    assert!(!e.reported_recovery_evidence, "{e:?}");
    assert!(e.refusals.contains(&reason), "{e:?}");
}

#[test]
fn complete_restore_is_scoped_and_proposal_is_only_data() {
    let s = source();
    let e = evidence(&s);
    assert!(
        e.snapshot_taken
            && e.integrity_checked
            && e.restore_drill_succeeded
            && e.reported_recovery_evidence
    );
    assert!(e.integrity_read_all_data);
    assert_eq!(e.snapshot_time.as_deref(), Some("2026-01-01T00:00:00Z"));
    assert_eq!(e.observation_age_seconds, Some(10));
    assert_eq!(e.integrity_age_seconds, Some(10));
    assert_eq!(e.restore_age_seconds, Some(10));
    assert_eq!(e.restore_coverage, s.required);
    assert_eq!(e.restored_versions, s.required);
    assert_eq!(e.required_versions, s.required);
    assert_eq!(e.proposed_drill.files, s.required);
    assert_eq!(e.proposed_drill.target, "new-disposable-target");
    assert_eq!(e.proposed_drill.compare_metadata, s.required_metadata);
    assert!(e
        .proposed_drill
        .proofs
        .contains(&"independent-content-sha256"));
    assert!(e.refusals.is_empty());
}

#[test]
fn snapshot_and_integrity_are_distinct_from_restore() {
    let mut s = source();
    s.integrity = None;
    s.restore = None;
    let e = evidence(&s);
    assert!(e.snapshot_taken);
    assert!(!e.integrity_checked && !e.restore_drill_succeeded && !e.reported_recovery_evidence);
    assert!(e.restored_versions.is_empty() && e.restore_coverage.is_empty());
    s.integrity = source().integrity;
    let e = evidence(&s);
    assert!(e.integrity_checked && !e.restore_drill_succeeded && !e.reported_recovery_evidence);
    s.integrity = None;
    s.restore = source().restore;
    refused(&s, Refusal::IntegrityNotChecked);
}

#[test]
fn green_job_without_matching_version_is_not_recovery() {
    let mut s = source();
    s.snapshot_files[0].version = "d".repeat(64);
    refused(&s, Refusal::VersionNotCovered);
    s = source();
    s.restore.as_mut().unwrap().files[0].file.version = "d".repeat(64);
    refused(&s, Refusal::VersionNotCovered);
    assert_eq!(evidence(&s).restore_coverage.len(), 1);
    s.snapshot_files.clear();
    refused(&s, Refusal::VersionNotCovered);
}

#[test]
fn same_volume_is_not_recovery() {
    let mut s = source();
    s.topology = Topology::SameVolume;
    refused(&s, Refusal::TargetNotIndependent);
    assert_eq!(evidence(&s).topology, Topology::SameVolume);
}

#[test]
fn same_device_and_unknown_topology_are_not_recovery() {
    for topology in [Topology::SamePhysicalDevice, Topology::Unknown] {
        let mut s = source();
        s.topology = topology;
        refused(&s, Refusal::TargetNotIndependent);
        assert_eq!(evidence(&s).topology, topology);
    }
}

#[test]
fn expired_or_missing_credentials_are_not_recovery() {
    for validity in [None, Some(999), Some(1000)] {
        let mut s = source();
        s.credentials_valid_until = validity;
        refused(&s, Refusal::CredentialsUnavailable);
    }
}

#[test]
fn partial_restore_is_not_recovery() {
    let mut s = source();
    s.restore.as_mut().unwrap().complete = false;
    refused(&s, Refusal::PartialRestore);
    s = source();
    s.restore.as_mut().unwrap().files.pop();
    refused(&s, Refusal::PartialRestore);
    assert_eq!(evidence(&s).restore_coverage.len(), 1);
}

#[test]
fn unsupported_metadata_is_not_recovery() {
    let mut s = source();
    s.required_metadata.push(MetadataClass::AlternateStreams);
    refused(&s, Refusal::UnsupportedMetadata);
    s = source();
    s.restore.as_mut().unwrap().files[0]
        .compared_metadata
        .clear();
    refused(&s, Refusal::UnsupportedMetadata);
}

#[test]
fn stale_and_future_observations_are_not_recovery() {
    for which in 0..3 {
        for timestamp in [939, 1001] {
            let mut s = source();
            match which {
                0 => s.observed_at = timestamp,
                1 => s.integrity.as_mut().unwrap().observation.observed_at = timestamp,
                _ => s.restore.as_mut().unwrap().observation.observed_at = timestamp,
            }
            refused(
                &s,
                if timestamp > s.now {
                    Refusal::InvalidObservationTime
                } else {
                    Refusal::StaleObservation
                },
            );
        }
    }
    let mut s = source();
    s.observed_at = 940;
    assert!(
        evidence(&s).reported_recovery_evidence,
        "expiry is inclusive at the configured age"
    );
}

#[test]
fn missing_snapshot_and_unbound_or_failed_observations_are_not_recovery() {
    let e = normalise_backup_receipt(b"[]", &source()).unwrap();
    assert!(!e.snapshot_taken && !e.reported_recovery_evidence);
    assert!(e.refusals.contains(&Refusal::SnapshotMissing));
    for which in 0..2 {
        for failure in 0..3 {
            let mut s = source();
            let o = if which == 0 {
                &mut s.integrity.as_mut().unwrap().observation
            } else {
                &mut s.restore.as_mut().unwrap().observation
            };
            match failure {
                0 => o.repository_id = "other-repository".into(),
                1 => o.snapshot_id = "e".repeat(64),
                _ => o.exit_code = 12,
            }
            refused(
                &s,
                if which == 0 {
                    Refusal::IntegrityNotChecked
                } else {
                    Refusal::RestoreNotSucceeded
                },
            );
        }
    }
}

#[test]
fn malformed_wrong_types_huge_numbers_and_duplicate_keys_are_rejected() {
    for raw in [
        b"{".as_slice(),
        b"[] []",
        br#"[{"id":"a","id":"b"}]"#,
        br#"[{"extra":{"x":0,"\u0078":1}}]"#,
        br#"[{"n":18446744073709551616}]"#,
        br#"[{"n":1e999}]"#,
    ] {
        assert_eq!(
            normalise_backup_receipt(raw, &source()),
            Err(ReceiptError::InvalidJson)
        );
    }
    for raw in [b"null".as_slice(), b"{}", b"[1]", b"[true]"] {
        assert_eq!(
            normalise_backup_receipt(raw, &source()),
            Err(ReceiptError::InvalidReceipt)
        );
    }
    for (field, value) in [
        ("id", json!(true)),
        ("paths", json!(1)),
        ("paths", json!([1])),
        ("time", json!(1000)),
        ("id", json!("short-id")),
        ("time", json!("")),
    ] {
        let mut raw: Value = serde_json::from_slice(&snapshots()).unwrap();
        raw[0][field] = value;
        assert_eq!(
            normalise_backup_receipt(&serde_json::to_vec(&raw).unwrap(), &source()),
            Err(ReceiptError::InvalidReceipt)
        );
    }
}

#[test]
fn byte_and_depth_budgets_are_enforced() {
    assert_eq!(
        normalise_backup_receipt(&vec![b' '; MAX_INPUT_BYTES + 1], &source()),
        Err(ReceiptError::InputTooLarge)
    );
    let deep = format!("{}0{}", "[".repeat(129), "]".repeat(129));
    assert_eq!(
        normalise_backup_receipt(deep.as_bytes(), &source()),
        Err(ReceiptError::InvalidJson)
    );
    let snapshots: Value = serde_json::from_slice(&snapshots()).unwrap();
    let many = serde_json::to_vec(&vec![snapshots[0].clone(); MAX_RECORDS + 1]).unwrap();
    assert_eq!(
        normalise_backup_receipt(&many, &source()),
        Err(ReceiptError::InvalidReceipt)
    );
}

#[test]
fn invalid_utf8_is_rejected_without_lossy_conversion() {
    let mut raw = snapshots();
    let offset = raw.iter().position(|c| *c == b'b').unwrap();
    raw[offset] = 0xff;
    assert_eq!(
        normalise_backup_receipt(&raw, &source()),
        Err(ReceiptError::InvalidJson)
    );
}

#[test]
fn utf16_byte_order_marks_are_named_not_reported_as_generic_json() {
    // Windows PowerShell 5.1 `>` redirection writes UTF-16 with a byte-order mark (#349). The
    // refusal names that cause instead of the generic "not valid JSON".
    let units: Vec<u16> = "[]".encode_utf16().collect();
    let little: Vec<u8> = [0xff, 0xfe]
        .into_iter()
        .chain(units.iter().flat_map(|u| u.to_le_bytes()))
        .collect();
    let big: Vec<u8> = [0xfe, 0xff]
        .into_iter()
        .chain(units.iter().flat_map(|u| u.to_be_bytes()))
        .collect();
    for raw in [little, big] {
        assert_eq!(
            normalise_backup_receipt(&raw, &source()),
            Err(ReceiptError::Utf16Bom)
        );
    }
}

#[test]
fn traversal_and_windows_ambiguous_names_are_refused() {
    for path in [
        "../escape",
        "fixture/../escape",
        "fixture\\..\\escape",
        "fixture/./file",
        "fixture//file",
        "fixture/file:stream",
        "fixture/file.",
        "fixture/CON",
        "fixture/NUL.txt",
        "fixture/CON .txt",
        "fixture/com1  .log",
        "fixture/file\0",
        "/absolute",
        "Q:/absolute",
    ] {
        let mut s = source();
        s.required[0].path = path.into();
        assert_eq!(
            normalise_backup_receipt(&snapshots(), &s),
            Err(ReceiptError::InvalidSource),
            "{path}"
        );
    }
    let mut raw: Value = serde_json::from_slice(&snapshots()).unwrap();
    raw[0]["paths"] = json!(["/synthetic-fixture/../escape"]);
    assert_eq!(
        normalise_backup_receipt(&serde_json::to_vec(&raw).unwrap(), &source()),
        Err(ReceiptError::InvalidReceipt)
    );
}

#[test]
fn source_limits_empty_scope_and_duplicate_paths_are_refused() {
    for which in 0..7 {
        let mut s = source();
        match which {
            0 => s.required.clear(),
            1 => s.required.push(s.required[0].clone()),
            2 => s.snapshot_files.push(s.snapshot_files[0].clone()),
            3 => {
                let r = s.restore.as_mut().unwrap();
                r.files.push(r.files[0].clone());
            }
            4 => s.required = vec![file("fixture/file"); MAX_RECORDS + 1],
            5 => s.required.push(file("Fixture/Document.TXT")),
            _ => s.now = u64::MAX,
        }
        assert_eq!(
            normalise_backup_receipt(&snapshots(), &s),
            Err(ReceiptError::InvalidSource)
        );
    }
}

#[test]
fn integrity_summary_errors_and_structure_only_check_do_not_verify_recovery() {
    let mut s = source();
    s.integrity.as_mut().unwrap().read_all_data = false;
    let e = evidence(&s);
    assert!(e.integrity_checked && !e.integrity_read_all_data && !e.reported_recovery_evidence);
    // restic prints `"broken_packs":null` on a clean check (no omitempty), or may omit it.
    for raw in [
        br#"{"message_type":"summary","num_errors":0,"broken_packs":null,"suggest_repair_index":false,"suggest_prune":false}"#.as_slice(),
        br#"{"message_type":"summary","num_errors":0,"suggest_repair_index":false,"suggest_prune":false}"#.as_slice(),
    ] {
        let mut s = source();
        s.integrity.as_mut().unwrap().raw = raw.to_vec();
        assert!(evidence(&s).reported_recovery_evidence, "a clean summary verifies integrity");
    }
    for (field, value) in [
        ("num_errors", json!(1)),
        ("broken_packs", json!(["synthetic-pack"])),
        ("suggest_repair_index", json!(true)),
        ("suggest_prune", json!(true)),
    ] {
        let mut s = source();
        let mut raw: Value = serde_json::from_slice(&check()).unwrap();
        raw[field] = value;
        s.integrity.as_mut().unwrap().raw = serde_json::to_vec(&raw).unwrap();
        refused(&s, Refusal::IntegrityNotChecked);
    }
}

/// #349: an empty, plain-text or non-summary check document is a named refusal that keeps the
/// snapshot evidence, never an error that loses it.
#[test]
fn unreadable_integrity_documents_refuse_without_losing_the_snapshot() {
    for raw in [
        b"".as_slice(),
        b"Fatal: unable to open repository",
        b"{}",
        br#"{"message_type":"summary","num_errors":false}"#,
        br#"{"message_type":"summary","num_errors":-1}"#,
        br#"{"message_type":"status","num_errors":0}"#,
        br#"[{"message_type":"summary","num_errors":0}]"#,
    ] {
        let mut s = source();
        s.integrity.as_mut().unwrap().raw = raw.to_vec();
        let e = evidence(&s);
        assert!(e.snapshot_taken && !e.integrity_checked, "{raw:?}");
        assert!(e.refusals.contains(&Refusal::IntegrityNotChecked));
    }
}

/// #349: unrelated snapshots need only a unique digest ID, so more than 256 entries or one odd
/// root elsewhere cannot hide the bound snapshot; the bound one stays strict.
#[test]
fn only_the_bound_snapshot_is_validated_strictly() {
    let bound: Value = serde_json::from_slice(&snapshots()).unwrap();
    let mut list: Vec<Value> = (0..300)
        .map(|i| json!({"id": format!("{i:064x}"), "time": "\u{7}", "paths": ["../odd"]}))
        .collect();
    list.push(bound[0].clone());
    let e = normalise_backup_receipt(&serde_json::to_vec(&list).unwrap(), &source()).unwrap();
    assert!(e.snapshot_taken && e.reported_recovery_evidence, "{e:?}");
    list.push(json!({"id": "c".repeat(64), "time": 1}));
    list.push(json!({"id": "c".repeat(64)}));
    assert_eq!(
        normalise_backup_receipt(&serde_json::to_vec(&list).unwrap(), &source()),
        Err(ReceiptError::InvalidReceipt),
        "duplicate unrelated IDs are still refused"
    );
    list.pop();
    list.pop();
    list.push(json!({"time": "2026"}));
    assert_eq!(
        normalise_backup_receipt(&serde_json::to_vec(&list).unwrap(), &source()),
        Err(ReceiptError::InvalidReceipt),
        "an entry without a digest ID is refused"
    );
}

/// #349: the repository label and the provider's time text are restricted charsets, so a URL,
/// path or credential can never be echoed into evidence.
#[test]
fn labels_and_snapshot_time_are_restricted_charsets() {
    for label in [
        "s3:https://host/bucket",
        r"C:\backups",
        "user:secret@host",
        "",
        &"x".repeat(65),
        "label with space",
    ] {
        let mut s = source();
        s.repository_id = label.into();
        assert_eq!(
            normalise_backup_receipt(&snapshots(), &s),
            Err(ReceiptError::InvalidSource),
            "{label}"
        );
    }
    let mut s = source();
    s.repository_id = "Synthetic_repo.v2-a".into();
    s.integrity = None;
    s.restore = None;
    assert!(evidence(&s).snapshot_taken);
    for time in [
        "2026-01-01 00:00:00",
        "C:/x",
        "https://host",
        "time\u{0}",
        &"1".repeat(65),
    ] {
        let mut raw: Value = serde_json::from_slice(&snapshots()).unwrap();
        raw[0]["time"] = json!(time);
        assert_eq!(
            normalise_backup_receipt(&serde_json::to_vec(&raw).unwrap(), &source()),
            Err(ReceiptError::InvalidReceipt),
            "{time}"
        );
    }
    let mut raw: Value = serde_json::from_slice(&snapshots()).unwrap();
    raw[0]["time"] = json!("2026-01-01T00:00:00.123456789+01:00");
    let e = normalise_backup_receipt(&serde_json::to_vec(&raw).unwrap(), &source()).unwrap();
    assert_eq!(
        e.snapshot_time.as_deref(),
        Some("2026-01-01T00:00:00.123456789+01:00")
    );
}

/// Validation folds case for duplicate names, so coverage must use the same Windows-name identity:
/// a case-variant restore or manifest entry names the same required file, while another version does not.
#[test]
fn case_variant_paths_name_the_same_required_file() {
    let mut s = source();
    for r in s.restore.as_mut().unwrap().files.iter_mut() {
        r.file.path = r.file.path.to_uppercase();
    }
    let e = evidence(&s);
    assert!(
        e.reported_recovery_evidence && e.refusals.is_empty(),
        "{e:?}"
    );
    assert_eq!(e.restore_coverage, source().required);

    let mut s = source();
    for f in s.snapshot_files.iter_mut() {
        f.path = f.path.to_uppercase();
    }
    let e = evidence(&s);
    assert!(
        e.reported_recovery_evidence && e.refusals.is_empty(),
        "{e:?}"
    );
    assert_eq!(e.restore_coverage, source().required);

    let mut s = source();
    for r in s.restore.as_mut().unwrap().files.iter_mut() {
        r.file.path = r.file.path.to_uppercase();
    }
    s.restore.as_mut().unwrap().files[0].file.version = "d".repeat(64);
    refused(&s, Refusal::VersionNotCovered);
}
