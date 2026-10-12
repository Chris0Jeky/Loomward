//! Redacted diagnostic bundles (LW-055, owner decision q-15, docs/42 ADR-V3-24): Loomward's first
//! file write. `diagnostics.preview` builds an allowlist bundle from typed fields only and keeps
//! its exact bytes in one in-memory slot; `diagnostics.save` writes exactly those bytes, and only
//! when the request names their digest, to a file the owner picks in a Rust-side save dialog. The
//! path never leaves Rust, and nothing is ever uploaded.
use crate::handlers::scan_view;
use crate::{db, fail, now, paths, Inner, Out};
use loomward_protocol::*;
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

type Handled = Result<Out, ErrorBody>;
use DiagnosticsSaveRefusal as Why;

/// A bundle is a few KiB (at most 64 roots); past this the preview is refused, never cut.
pub(crate) const MAX_BUNDLE_BYTES: usize = 256 * 1024;
/// The newest jobs the engine lists, counted by kind and state.
const JOB_WINDOW: u64 = 50;
pub(crate) const SAVE_TITLE: &str =
    "Loomward: save the diagnostic bundle (exactly the bytes you previewed)";

/// The last preview: its digest and the exact bytes `diagnostics.save` may write.
pub(crate) type Held = Option<(String, Arc<Vec<u8>>)>;

fn out(result: Value) -> Out {
    Out {
        result,
        catalog_rev: None,
        state_rev: None,
    }
}

fn wire<T: serde::Serialize>(v: T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// A wire enum from the service's own snake_case string.
fn variant<T: serde::de::DeserializeOwned>(s: &str) -> Option<T> {
    serde_json::from_value(json!(s)).ok()
}

fn known<T>(v: Option<T>, reason: &'static str) -> Known<T> {
    v.map_or_else(|| Known::unknown(reason), Known::Value)
}

fn count(v: Option<i64>) -> Option<Count> {
    v.and_then(|n| Count::new(u64::try_from(n).ok()?).ok())
}

fn filesystem(name: Option<&str>) -> Known<DiagnosticsFilesystem> {
    use DiagnosticsFilesystem as F;
    let Some(name) = name else {
        return Known::unknown("volume_not_observed");
    };
    Known::Value(match name.to_ascii_lowercase().as_str() {
        "ntfs" => F::Ntfs,
        "refs" => F::Refs,
        "fat" => F::Fat,
        "fat32" => F::Fat32,
        "exfat" => F::Exfat,
        _ => F::Other,
    })
}

/// The exact bytes the wire value serialises to, pretty-printed for the owner to read. The bundle
/// holds no floating-point number, so the app's `JSON.stringify(bundle, null, 2)` of the received
/// value reproduces these bytes and can check them against the digest.
pub(crate) fn canonical(bundle: &DiagnosticsBundle) -> Result<Vec<u8>, ErrorBody> {
    let value =
        serde_json::to_value(bundle).map_err(|_| fail(ErrorCode::InternalError, "encode"))?;
    serde_json::to_vec_pretty(&value).map_err(|_| fail(ErrorCode::InternalError, "encode"))
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    let hash = Sha256::digest(bytes);
    format!(
        "sha256:{}",
        hash.iter().map(|b| format!("{b:02x}")).collect::<String>()
    )
}

/// `loomward-diagnostics-<UTC yyyymmdd-hhmmss>.json`.
pub(crate) fn default_name(at: &Timestamp) -> String {
    let digits: String = at
        .as_str()
        .chars()
        .take(19)
        .filter(char::is_ascii_digit)
        .collect();
    format!(
        "loomward-diagnostics-{}-{}.json",
        &digits[..8],
        &digits[8..]
    )
}

/// Writes `bytes` to an owner-picked `target`, never through a link and never half-written.
///
/// Refuses before touching anything: a relative path, a network, device or verbatim namespace, a
/// Windows device name, a stream name, a parent that is not a directory, and an existing target
/// that is not a regular file or is a link, reparse point or cloud placeholder. The bytes go to a
/// new temporary file in the same directory, are flushed, and replace the target by rename: a
/// rename replaces the directory entry, so it never follows a link the target became meanwhile,
/// and a failure leaves the target as it was and removes the temporary file. An existing regular
/// target is replaced only because the owner confirmed that in the native dialog.
pub(crate) fn write_export(target: &Path, bytes: &[u8]) -> Result<(), Why> {
    let name = target.file_name().map(|n| n.to_string_lossy());
    if !target.is_absolute()
        || paths::network_path(target)
        || paths::reserved_device(target)
        || name.as_deref().is_none_or(|n| n.contains(':'))
    {
        return Err(Why::UnsupportedTarget);
    }
    let dir = target.parent().ok_or(Why::UnsupportedTarget)?;
    // The folder as resolved: a junction or link among its ancestors that leads to a share is refused.
    let resolved = std::fs::canonicalize(dir).map_err(|_| Why::UnsupportedTarget)?;
    if paths::network_path(&resolved) || !resolved.is_dir() {
        return Err(Why::UnsupportedTarget);
    }
    match std::fs::symlink_metadata(target) {
        Ok(meta) if !meta.is_file() || paths::unsafe_attributes(&meta) => {
            return Err(Why::UnsupportedTarget)
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(Why::UnsupportedTarget),
    }
    let mut nonce = [0u8; 8];
    getrandom::fill(&mut nonce).map_err(|_| Why::WriteFailed)?;
    let temporary = dir.join(format!(
        ".loomward-diagnostics-{}.partial",
        nonce.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ));
    let written = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, target)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
        return Err(Why::WriteFailed);
    }
    Ok(())
}

impl Inner {
    /// Every field is read into a typed slot; anything unreadable is an explicit unknown.
    fn bundle(&self, ctx: &CallContext) -> Result<DiagnosticsBundle, ErrorBody> {
        let decode = |v: Value| serde_json::from_value::<SessionInfo>(v).ok();
        let info = decode(self.hello(ctx)?.result)
            .ok_or_else(|| fail(ErrorCode::InternalError, "session info unreadable"))?;
        let host = loomward_telemetry::host();
        let health = self
            .health()
            .ok()
            .and_then(|o| serde_json::from_value::<Health>(o.result).ok())
            .map(|h| {
                let bytes = |b: Option<Bytes>, why| known(b, why);
                Known::Value(DiagnosticsHealth {
                    engine_private_commit_bytes: bytes(
                        h.engine.private_commit_bytes,
                        "own_usage_unavailable",
                    ),
                    engine_cpu_ms: known(
                        h.engine
                            .cpu_seconds
                            .and_then(|s| Count::new((s.get() * 1000.0) as u64).ok()),
                        "own_usage_unavailable",
                    ),
                    catalog_db_bytes: bytes(h.catalog.db_bytes, "not_present"),
                    catalog_wal_bytes: bytes(h.catalog.wal_bytes, "not_present"),
                    writer_queue_depth: h.catalog.writer_queue_depth,
                    writer_queue_capacity: h.catalog.writer_queue_capacity,
                    jobs_running: h.jobs_running,
                    warning_codes: BoundedVec::new(h.warnings.iter().map(|w| w.code).collect())
                        .unwrap_or_default(),
                })
            })
            .unwrap_or_else(|| Known::unknown("health_unavailable"));
        let conn = self.db();
        let pragma = |schema: &str| {
            conn.pragma_query_value(Some(schema), "user_version", |r| r.get::<_, i64>(0))
                .ok()
        };
        let (files, dirs) = db::counts(&conn).ok().unzip();
        let stores = DiagnosticsStores {
            catalog_schema_version: known(count(pragma("main")), "store_unreadable"),
            state_schema_version: known(count(pragma("st")), "store_unreadable"),
            catalog_files: known(count(files), "store_unreadable"),
            catalog_dirs: known(count(dirs), "store_unreadable"),
        };
        let counts = conn
            .query_row(
                "SELECT
                   (SELECT count(*) FROM st.root_grant WHERE state='active'),
                   (SELECT count(*) FROM st.root_grant WHERE state='revoked' AND id NOT IN (SELECT grant_id FROM st.root_forgotten)),
                   (SELECT count(*) FROM st.root_grant WHERE state='identity_changed' AND id NOT IN (SELECT grant_id FROM st.root_forgotten)),
                   (SELECT count(*) FROM st.root_forgotten),
                   (SELECT count(*) FROM st.collection),
                   (SELECT count(*) FROM st.human_feedback),
                   (SELECT count(*) FROM st.teacher_label),
                   (SELECT count(*) FROM st.backup_evidence)",
                [],
                |r| {
                    let c = |i| r.get::<_, i64>(i).map(|n| count(Some(n)).unwrap_or_default());
                    Ok(DiagnosticsCounts {
                        roots_active: c(0)?,
                        roots_revoked: c(1)?,
                        roots_identity_changed: c(2)?,
                        roots_forgotten: c(3)?,
                        collections: c(4)?,
                        human_feedback_records: c(5)?,
                        teacher_label_records: c(6)?,
                        backup_evidence: c(7)?,
                    })
                },
            )
            .ok();
        let roots = self.diagnostic_roots(&conn);
        drop(conn);
        let jobs = self
            .engine
            .job_list(&decode_exact(json!({ "limit": JOB_WINDOW }))?)
            .ok()
            .map(|list| {
                let mut tally: Vec<DiagnosticsJobCount> = Vec::new();
                for job in list.jobs.iter() {
                    match tally
                        .iter_mut()
                        .find(|t| t.kind == job.kind && t.state == job.state)
                    {
                        Some(t) => t.count = Count::new(t.count.get() + 1).unwrap_or(t.count),
                        None => tally.push(DiagnosticsJobCount {
                            kind: job.kind,
                            state: job.state,
                            count: Count::new(1).unwrap_or_default(),
                        }),
                    }
                }
                DiagnosticsJobs {
                    window: Count::new(JOB_WINDOW).unwrap_or_default(),
                    by_kind_state: BoundedVec::new(tally).unwrap_or_default(),
                }
            });
        Ok(DiagnosticsBundle {
            format: DiagnosticsFormat,
            generated_at: now(),
            loomward: DiagnosticsBuild {
                engine_version: info.engine_version,
                protocol: info.protocol,
                adapter: info.adapter,
                dataset_class: info.dataset_class,
                enumeration_strategy: info.enumeration_strategy,
            },
            host: DiagnosticsHost {
                os_family: if cfg!(windows) {
                    DiagnosticsOsFamily::Windows
                } else {
                    DiagnosticsOsFamily::Other
                },
                os_build: known(
                    host.os_build.and_then(|b| Count::new(b.into()).ok()),
                    "os_version_unavailable",
                ),
                logical_processors: known(
                    host.logical_processors
                        .and_then(|n| Count::new(n.into()).ok()),
                    "processor_count_unavailable",
                ),
                memory_total_bytes: known(
                    host.memory_total_bytes.map(Bytes),
                    "physical_memory_unavailable",
                ),
            },
            session: DiagnosticsSession {
                capabilities: info.capabilities,
                features: info.features,
            },
            health,
            stores,
            counts: known(counts, "store_unreadable"),
            jobs: known(jobs, "job_list_unavailable"),
            roots: known(roots, "store_unreadable"),
            // The service keeps no error history; a bundle never invents one.
            recent_errors: Known::unknown("not_retained"),
        })
    }

    /// Opaque ID, origin, grant and scan state, coverage, filesystem family and identity mode:
    /// never the display path, the volume's name or label, or any name under the root.
    fn diagnostic_roots(
        &self,
        conn: &rusqlite::Connection,
    ) -> Option<BoundedVec<DiagnosticsRoot, 0, 64>> {
        let rows = db::roots(conn, None).ok()?;
        let mut stmt = conn
            .prepare(
                "SELECT g.identity_mode,v.filesystem FROM main.root r JOIN st.root_grant g ON g.id=r.grant_id
                 LEFT JOIN main.volume v ON v.id=r.volume_id WHERE r.id=?1",
            )
            .ok()?;
        let mut roots = Vec::new();
        for r in &rows {
            let (mode, fs): (String, Option<String>) = stmt
                .query_row([r.root], |q| Ok((q.get(0)?, q.get(1)?)))
                .ok()?;
            let (state, coverage) = scan_view(r);
            roots.push(DiagnosticsRoot {
                root_id: RootId::new(self.root_id(r.root)).ok()?,
                origin: variant(&r.origin)?,
                grant_state: variant(&r.grant_state)?,
                scan_state: variant(state)?,
                coverage: variant(coverage)?,
                filesystem: filesystem(fs.as_deref()),
                identity_mode: variant(&mode)?,
            });
        }
        BoundedVec::new(roots).ok()
    }

    /// Read-only: builds the bundle, keeps its exact bytes for `diagnostics.save`, writes nothing.
    pub(crate) fn diagnostics_preview(&self, ctx: &CallContext) -> Handled {
        let bundle = self.bundle(ctx)?;
        let bytes = canonical(&bundle)?;
        if bytes.len() > MAX_BUNDLE_BYTES {
            return Err(fail(
                ErrorCode::ResourceBudget,
                "the diagnostic bundle exceeds its size budget",
            ));
        }
        let digest = digest(&bytes);
        let size = bytes.len().to_string();
        *self.preview.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((digest.clone(), Arc::new(bytes)));
        Ok(out(
            json!({"bundle": wire(&bundle), "bytes": size, "digest": digest}),
        ))
    }

    /// Desktop only, any dataset class (the bundle is redacted). The digest is checked before any
    /// dialog opens; the bytes written are the ones held under that digest, whatever is previewed
    /// while the dialog is open. One dialog at a time, sharing the pickers' lock and cooldown.
    pub(crate) fn diagnostics_save(&self, req: &RequestEnvelope, ctx: &CallContext) -> Handled {
        let r: DiagnosticsSaveRequest = req.decode_payload()?;
        let answer = |outcome: DiagnosticsSaveOutcome, written: Option<usize>, why: Option<Why>| {
            Ok(out(json!({
                "outcome": wire(outcome),
                "bytes_written": written.map(|n| n.to_string()),
                "refusal": why.map(wire),
            })))
        };
        let held = self
            .preview
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .filter(|(d, _)| d == r.digest.as_str())
            .map(|(_, bytes)| bytes.clone());
        let Some(bytes) = held else {
            return answer(
                DiagnosticsSaveOutcome::Refused,
                None,
                Some(Why::PreviewChanged),
            );
        };
        let dialogs = ctx.dialogs.as_ref().ok_or_else(|| {
            fail(
                ErrorCode::CapabilityUnavailable,
                "no native dialogs on this adapter",
            )
        })?;
        let _dialog = self.dialog_turn()?;
        let Some(target) = dialogs.save_file(SAVE_TITLE, &default_name(&now())) else {
            return answer(DiagnosticsSaveOutcome::CancelledByUser, None, None);
        };
        match write_export(&target, &bytes) {
            Ok(()) => answer(DiagnosticsSaveOutcome::Saved, Some(bytes.len()), None),
            Err(why) => answer(DiagnosticsSaveOutcome::Refused, None, Some(why)),
        }
    }
}
