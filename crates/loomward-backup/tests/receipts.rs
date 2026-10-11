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
    assert!(!e.verified_recovery, "{e:?}");
    assert!(e.refusals.contains(&reason), "{e:?}");
}

#[test]
fn complete_restore_is_scoped_and_proposal_is_only_data() {
    let s = source();
    let e = evidence(&s);
    assert!(
        e.snapshot_taken && e.integrity_checked && e.restore_drill_succeeded && e.verified_recovery
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
    assert!(!e.integrity_checked && !e.restore_drill_succeeded && !e.verified_recovery);
    assert!(e.restored_versions.is_empty() && e.restore_coverage.is_empty());
    s.integrity = source().integrity;
    let e = evidence(&s);
    assert!(e.integrity_checked && !e.restore_drill_succeeded && !e.verified_recovery);
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
        evidence(&s).verified_recovery,
        "expiry is inclusive at the configured age"
    );
}

#[test]
fn missing_snapshot_and_unbound_or_failed_observations_are_not_recovery() {
    let e = normalise_backup_receipt(b"[]", &source()).unwrap();
    assert!(!e.snapshot_taken && !e.verified_recovery);
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
    assert!(e.integrity_checked && !e.integrity_read_all_data && !e.verified_recovery);
    // restic prints `"broken_packs":null` on a clean check (no omitempty), or may omit it.
    for raw in [
        br#"{"message_type":"summary","num_errors":0,"broken_packs":null,"suggest_repair_index":false,"suggest_prune":false}"#.as_slice(),
        br#"{"message_type":"summary","num_errors":0,"suggest_repair_index":false,"suggest_prune":false}"#.as_slice(),
    ] {
        let mut s = source();
        s.integrity.as_mut().unwrap().raw = raw.to_vec();
        assert!(evidence(&s).verified_recovery, "a clean summary verifies integrity");
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
    for (raw, expected) in [
        (b"{}".as_slice(), ReceiptError::InvalidReceipt),
        (
            br#"{"message_type":"summary","num_errors":false}"#.as_slice(),
            ReceiptError::InvalidReceipt,
        ),
        (b"green job".as_slice(), ReceiptError::InvalidJson),
    ] {
        let mut s = source();
        s.integrity.as_mut().unwrap().raw = raw.to_vec();
        assert_eq!(normalise_backup_receipt(&snapshots(), &s), Err(expected));
    }
}
