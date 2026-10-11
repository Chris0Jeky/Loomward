//! Backups stage 1 (docs/42 ADR-V3-23): the owner picks a restic receipt in a Rust-side open-file
//! dialog; the service reads at most 256 KiB of it read-only, normalises it with `loomward-backup`
//! and stores only the normalised evidence in `state.db`. It never runs a backup tool, never reads a
//! backup repository or its configuration, and never stores receipt bytes, a path or a file name.
use crate::{db, fail, now_ns, paths, sql_error, ts_ns, Inner, Out};
use loomward_backup as lb;
use loomward_protocol::*;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};
use std::io::Read;
use std::path::Path;
use std::time::UNIX_EPOCH;

/// Freshness window for snapshot and integrity observations (stage 1 constant).
pub(crate) const MAX_AGE_SECONDS: u64 = 7 * 24 * 60 * 60;
const MAX_REPOSITORIES: i64 = 64;
pub(crate) const SNAPSHOTS_TITLE: &str =
    "Loomward: choose a restic snapshots --json file (read only, at most 256 KiB)";
pub(crate) const CHECK_TITLE: &str =
    "Loomward: choose a restic check --json output (read only, at most 256 KiB)";
/// Metadata a Windows restore drill must compare, proposed until the owner can choose (stage 3).
const COMPARE: [lb::MetadataClass; 2] = [lb::MetadataClass::Acl, lb::MetadataClass::Timestamps];
const TIME_REFUSALS: [&str; 2] = ["stale_observation", "invalid_observation_time"];

type Handled = Result<Out, ErrorBody>;
use BackupImportRefusal as Why;

fn out(result: Value, revs: Option<(i64, i64)>) -> Out {
    Out {
        result,
        catalog_rev: revs.map(|r| r.0),
        state_rev: revs.map(|r| r.1),
    }
}

/// At most [`lb::MAX_INPUT_BYTES`] of an owner-picked regular local file, read-only. Refuses
/// network, device and verbatim namespaces, reserved device names, links, reparse points and cloud
/// placeholders before opening anything (opening a placeholder would download it). The handle is
/// dropped before this returns, so nothing holds the file while parsing. Returns the bytes and the
/// file's modification time in Unix seconds (when the provider wrote it, not when Loomward read it).
pub(crate) fn read_receipt(path: &Path) -> Result<(Vec<u8>, u64), Why> {
    if paths::network_path(path) || paths::reserved_device(path) {
        return Err(Why::NotARegularFile);
    }
    let meta = std::fs::symlink_metadata(path).map_err(|_| Why::Unreadable)?;
    if !meta.is_file() || paths::unsafe_attributes(&meta) {
        return Err(Why::NotARegularFile);
    }
    let file = std::fs::File::open(path).map_err(|_| Why::Unreadable)?;
    let meta = file.metadata().map_err(|_| Why::Unreadable)?;
    if !meta.is_file() || paths::unsafe_attributes(&meta) {
        return Err(Why::NotARegularFile);
    }
    if meta.len() > lb::MAX_INPUT_BYTES as u64 {
        return Err(Why::TooLarge);
    }
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .ok_or(Why::Unreadable)?;
    let mut bytes = Vec::new();
    // A file that grows after the length check still stops one byte past the budget.
    file.take(lb::MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Why::Unreadable)?;
    if bytes.len() > lb::MAX_INPUT_BYTES {
        return Err(Why::TooLarge);
    }
    Ok((bytes, modified))
}

/// `check --json` may print status or error lines before the summary (#349): a document that is
/// not one JSON value yields its last non-empty line, which the normaliser parses strictly.
fn summary_document(raw: &[u8]) -> &[u8] {
    if serde_json::from_slice::<serde::de::IgnoredAny>(raw).is_ok() {
        return raw;
    }
    raw.split(|b| *b == b'\n')
        .map(<[u8]>::trim_ascii)
        .rfind(|line| !line.is_empty())
        .unwrap_or_default()
}

fn wire<T: serde::Serialize>(v: T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

fn evidence_row(id: &str) -> Result<i64, ErrorBody> {
    id.strip_prefix("be_")
        .and_then(|n| n.parse::<i64>().ok())
        .filter(|n| *n > 0 && format!("be_{n}") == id)
        .ok_or_else(|| fail(ErrorCode::NotFound, "unknown backup evidence"))
}

/// Stored evidence plus the time-derived fields, recomputed against `now` on every read: ages,
/// the staleness and future-time refusals, and the recovery predicate that depends on them.
fn project(
    id: i64,
    stored: &str,
    snapshot_at: i64,
    integrity_at: Option<i64>,
    imported_at: i64,
    now: u64,
) -> Result<Value, ErrorBody> {
    let mut v: Value = serde_json::from_str(stored)
        .map_err(|_| fail(ErrorCode::InternalError, "stored evidence unreadable"))?;
    let secs = |ns: i64| u64::try_from(ns / 1_000_000_000).unwrap_or(0);
    let snapshot = secs(snapshot_at);
    let integrity = integrity_at.map(secs);
    let snapshot_age = now.checked_sub(snapshot);
    let integrity_age = integrity.and_then(|t| now.checked_sub(t));
    let mut refusals: Vec<Value> = v["refusals"].as_array().cloned().unwrap_or_default();
    refusals.retain(|r| !TIME_REFUSALS.contains(&r.as_str().unwrap_or("")));
    if [snapshot_age, integrity_age]
        .iter()
        .flatten()
        .any(|age| *age > MAX_AGE_SECONDS)
    {
        refusals.push(json!("stale_observation"));
    }
    if snapshot > now || integrity.is_some_and(|t| t > now) {
        refusals.push(json!("invalid_observation_time"));
    }
    v["evidence_id"] = json!(format!("be_{id}"));
    v["imported_at"] = wire(ts_ns(imported_at));
    v["snapshot_observed_at"] = wire(ts_ns(snapshot_at));
    v["integrity_observed_at"] = wire(integrity_at.map(ts_ns));
    v["snapshot_age_seconds"] = json!(snapshot_age);
    v["integrity_age_seconds"] = json!(integrity_age);
    v["max_age_seconds"] = json!(MAX_AGE_SECONDS);
    v["reported_recovery_evidence"] = json!(refusals.is_empty());
    v["refusals"] = Value::Array(refusals);
    Ok(v)
}

fn rows(conn: &Connection, only: Option<i64>, now: u64) -> Result<Vec<Value>, ErrorBody> {
    let mut stmt = conn
        .prepare(
            "SELECT id,evidence_json,snapshot_observed_ns,integrity_observed_ns,imported_at_ns
             FROM st.backup_evidence WHERE ?1 IS NULL OR id=?1 ORDER BY repository_label",
        )
        .map_err(sql_error)?;
    let found = stmt
        .query_map([only], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })
        .map_err(sql_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(sql_error)?;
    found
        .iter()
        .map(|(id, json, snap, integrity, imported)| {
            project(*id, json, *snap, *integrity, *imported, now)
        })
        .collect()
}

fn now_secs() -> u64 {
    u64::try_from(now_ns() / 1_000_000_000).unwrap_or(0)
}

impl Inner {
    pub(crate) fn backups_list(&self) -> Handled {
        let conn = self.db();
        let revs = db::revisions(&conn).map_err(sql_error)?;
        let evidence = rows(&conn, None, now_secs())?;
        Ok(out(json!({ "evidence": evidence }), Some(revs)))
    }

    pub(crate) fn backups_forget(&self, req: &RequestEnvelope) -> Handled {
        let r: BackupForgetRequest = req.decode_payload()?;
        let id = evidence_row(&r.evidence_id)?;
        let _permit = self.mutation.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = self.db();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        if tx
            .execute("DELETE FROM st.backup_evidence WHERE id=?1", [id])
            .map_err(sql_error)?
            == 0
        {
            return Err(fail(ErrorCode::NotFound, "unknown backup evidence"));
        }
        tx.execute(
            "UPDATE st.revision SET state_rev=state_rev+1 WHERE id=1",
            [],
        )
        .map_err(sql_error)?;
        let revs = db::revisions(&tx).map_err(sql_error)?;
        tx.commit().map_err(sql_error)?;
        Ok(out(
            json!({"evidence_id": r.evidence_id, "forgotten_at": crate::now()}),
            Some(revs),
        ))
    }

    /// Volume identity, never text: the same volume row is `same_volume`; different volumes stay
    /// `unknown` because no physical device identity exists yet; anything undeclared is `unknown`.
    fn backup_topology(
        &self,
        source: Option<&VolumeId>,
        target: Option<&VolumeId>,
    ) -> Result<(lb::Topology, BackupTopologyBasis), ErrorBody> {
        let key = |id: &VolumeId| -> Result<String, ErrorBody> {
            let row = self.volume_row(id)?;
            self.db()
                .query_row(
                    "SELECT volume_key FROM main.volume WHERE id=?1",
                    [row],
                    |r| r.get(0),
                )
                .optional()
                .map_err(sql_error)?
                .ok_or_else(|| fail(ErrorCode::NotFound, "unknown volume"))
        };
        let source = source.map(key).transpose()?;
        let target = target.map(key).transpose()?;
        Ok(match (source, target) {
            (Some(a), Some(b)) if a == b => (
                lb::Topology::SameVolume,
                BackupTopologyBasis::SameVolumeIdentity,
            ),
            (Some(_), Some(_)) => (
                lb::Topology::Unknown,
                BackupTopologyBasis::DifferentVolumesDeviceUnknown,
            ),
            _ => (lb::Topology::Unknown, BackupTopologyBasis::Undeclared),
        })
    }

    /// Desktop only, personal sessions only (a synthetic session refuses before any dialog). One
    /// dialog at a time, sharing the folder picker's lock and cooldown.
    pub(crate) fn backups_import(&self, req: &RequestEnvelope, ctx: &CallContext) -> Handled {
        let r: BackupImportRequest = req.decode_payload()?;
        let refused = |why: Why| {
            Ok(out(
                json!({"outcome": "refused", "evidence": null, "refusal": wire(why)}),
                None,
            ))
        };
        let cancelled = || {
            Ok(out(
                json!({"outcome": "cancelled_by_user", "evidence": null, "refusal": null}),
                None,
            ))
        };
        if self.dataset != DatasetClass::Personal {
            return refused(Why::SyntheticSession);
        }
        // Volume IDs are checked before any dialog opens.
        let (topology, basis) =
            self.backup_topology(r.source_volume_id.as_ref(), r.target_volume_id.as_ref())?;
        let dialogs = ctx.dialogs.as_ref().ok_or_else(|| {
            fail(
                ErrorCode::CapabilityUnavailable,
                "no native dialogs on this adapter",
            )
        })?;
        let _dialog = self.dialog_turn()?;
        let Some(picked) = dialogs.pick_file(SNAPSHOTS_TITLE) else {
            return cancelled();
        };
        let (snapshots, snapshot_at) = match read_receipt(&picked) {
            Ok(read) => read,
            Err(why) => return refused(why),
        };
        let integrity = if r.integrity_check {
            let Some(picked) = dialogs.pick_file(CHECK_TITLE) else {
                return cancelled();
            };
            match read_receipt(&picked) {
                Ok(read) => Some(read),
                Err(why) => return refused(why),
            }
        } else {
            None
        };
        drop(_dialog);
        let label = r.repository_label.to_string();
        let snapshot_id = r.snapshot_id.to_string();
        let observation = |observed_at| lb::Observation {
            repository_id: label.clone(),
            snapshot_id: snapshot_id.clone(),
            observed_at,
            // Stage 1 sees no exit code; a clean, strictly parsed summary stands in for it.
            exit_code: 0,
        };
        let required: Vec<lb::FileVersion> = r
            .required
            .iter()
            .map(|f| lb::FileVersion {
                path: f.relative_name.to_string(),
                version: f.version.trim_start_matches("sha256:").to_string(),
            })
            .collect();
        let now = now_secs();
        let source = lb::BackupSource {
            repository_id: label.clone(),
            snapshot_id: snapshot_id.clone(),
            observed_at: snapshot_at,
            now,
            max_age_seconds: MAX_AGE_SECONDS,
            topology,
            credentials_valid_until: None,
            required,
            // `snapshots --json` holds no file versions; nothing else supplies a manifest yet.
            snapshot_files: Vec::new(),
            required_metadata: COMPARE.to_vec(),
            integrity: integrity
                .as_ref()
                .map(|(raw, at)| lb::IntegrityObservation {
                    observation: observation(*at),
                    raw: summary_document(raw).to_vec(),
                    // Stage 1 cannot see check arguments; the stage-2 observer records them.
                    read_all_data: false,
                }),
            restore: None,
        };
        let evidence = match lb::normalise_backup_receipt(&snapshots, &source) {
            Ok(e) => e,
            Err(e) => {
                return refused(match e {
                    lb::ReceiptError::InputTooLarge => Why::TooLarge,
                    lb::ReceiptError::InvalidJson => Why::InvalidJson,
                    lb::ReceiptError::InvalidReceipt => Why::InvalidReceipt,
                    lb::ReceiptError::InvalidSource => Why::InvalidSource,
                })
            }
        };
        drop(snapshots);
        let stored = stored_evidence(&evidence, basis);
        let _permit = self.mutation.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = self.db();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let others: i64 = tx
            .query_row(
                "SELECT count(*) FROM st.backup_evidence WHERE repository_label<>?1",
                [&label],
                |r| r.get(0),
            )
            .map_err(sql_error)?;
        if others >= MAX_REPOSITORIES {
            return refused(Why::RepositoryLimit);
        }
        let ns = |s: u64| i64::try_from(s).unwrap_or(i64::MAX / 1_000_000_000) * 1_000_000_000;
        let imported = now_ns();
        tx.execute(
            "INSERT INTO st.backup_evidence(repository_label,evidence_json,snapshot_observed_ns,integrity_observed_ns,imported_at_ns)
             VALUES(?1,?2,?3,?4,?5) ON CONFLICT(repository_label) DO UPDATE SET evidence_json=excluded.evidence_json,
             snapshot_observed_ns=excluded.snapshot_observed_ns,integrity_observed_ns=excluded.integrity_observed_ns,
             imported_at_ns=excluded.imported_at_ns",
            params![label, stored.to_string(), ns(snapshot_at), integrity.as_ref().map(|(_, at)| ns(*at)), imported],
        )
        .map_err(sql_error)?;
        let id: i64 = tx
            .query_row(
                "SELECT id FROM st.backup_evidence WHERE repository_label=?1",
                [&label],
                |r| r.get(0),
            )
            .map_err(sql_error)?;
        tx.execute(
            "UPDATE st.revision SET state_rev=state_rev+1 WHERE id=1",
            [],
        )
        .map_err(sql_error)?;
        let evidence = rows(&tx, Some(id), now)?
            .pop()
            .ok_or_else(|| fail(ErrorCode::InternalError, "evidence vanished"))?;
        let revs = db::revisions(&tx).map_err(sql_error)?;
        tx.commit().map_err(sql_error)?;
        Ok(out(
            json!({"outcome": "imported", "evidence": evidence, "refusal": null}),
            Some(revs),
        ))
    }
}

/// The wire evidence without its time-derived fields, which [`project`] recomputes on each read.
fn stored_evidence(e: &lb::RecoveryEvidence, basis: BackupTopologyBasis) -> Value {
    let versions = |files: &[lb::FileVersion]| -> Vec<Value> {
        files
            .iter()
            .map(|f| json!({"relative_name": f.path, "version": format!("sha256:{}", f.version)}))
            .collect()
    };
    let metadata = |m: &lb::MetadataClass| match m {
        lb::MetadataClass::Acl => "acl",
        lb::MetadataClass::AlternateStreams => "alternate_streams",
        lb::MetadataClass::HardLinks => "hard_links",
        lb::MetadataClass::Timestamps => "timestamps",
        lb::MetadataClass::Sparse => "sparse",
        lb::MetadataClass::Compression => "compression",
        lb::MetadataClass::Efs => "efs",
    };
    let refusal = |r: &lb::Refusal| match r {
        lb::Refusal::SnapshotMissing => "snapshot_missing",
        lb::Refusal::VersionNotCovered => "version_not_covered",
        lb::Refusal::TargetNotIndependent => "target_not_independent",
        lb::Refusal::CredentialsUnavailable => "credentials_unavailable",
        lb::Refusal::IntegrityNotChecked => "integrity_not_checked",
        lb::Refusal::RestoreNotSucceeded => "restore_not_succeeded",
        lb::Refusal::PartialRestore => "partial_restore",
        lb::Refusal::UnsupportedMetadata => "unsupported_metadata",
        lb::Refusal::StaleObservation => "stale_observation",
        lb::Refusal::InvalidObservationTime => "invalid_observation_time",
    };
    let topology = match e.topology {
        lb::Topology::IndependentTarget => "independent_target",
        lb::Topology::SameVolume => "same_volume",
        lb::Topology::SamePhysicalDevice => "same_physical_device",
        lb::Topology::Unknown => "unknown",
    };
    json!({
        "repository_label": e.repository_id,
        "snapshot_id": e.snapshot_id,
        "snapshot_time": e.snapshot_time,
        "snapshot_taken": e.snapshot_taken,
        "integrity_checked": e.integrity_checked,
        "integrity_read_all_data": e.integrity_read_all_data,
        "restore_drill_succeeded": e.restore_drill_succeeded,
        "required_versions": versions(&e.required_versions),
        "restore_coverage": versions(&e.restore_coverage),
        "topology": topology,
        "topology_basis": wire(basis),
        "refusals": e.refusals.iter().map(refusal).collect::<Vec<_>>(),
        "proposed_drill": {
            "files": versions(&e.proposed_drill.files),
            "target": "new_disposable_target",
            "compare_metadata": e.proposed_drill.compare_metadata.iter().map(metadata).collect::<Vec<_>>(),
            "proofs": e.proposed_drill.proofs.iter().map(|p| p.replace('-', "_")).collect::<Vec<_>>(),
            "executable": false,
        },
    })
}
