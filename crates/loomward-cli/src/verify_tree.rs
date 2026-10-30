use loomward_engine::scan::TreeBudget as Budget;
use loomward_engine::{scan::CatalogRootIdentity, scan::GrantedRoot, Engine, EngineConfig};
use loomward_operations::tree::{DifferenceKind, Report};
use loomward_protocol::{DatasetClass, RootId};
use loomward_service::{paths, Config, Service};
use std::{path::PathBuf, time::Duration};

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Args {
    pub(super) source: PathBuf,
    pub(super) destination: PathBuf,
    pub(super) state_dir: PathBuf,
    pub(super) max_entries: usize,
    pub(super) budget_bytes: u64,
    pub(super) budget_secs: u64,
}

pub(super) fn parse(args: &[String]) -> Result<Args, String> {
    let usage = || super::USAGE.to_owned();
    if args.len() < 4
        || args[..2]
            .iter()
            .any(|arg| arg.is_empty() || arg.starts_with("--"))
    {
        return Err(usage());
    }
    let mut parsed = Args {
        source: args[0].clone().into(),
        destination: args[1].clone().into(),
        state_dir: PathBuf::new(),
        max_entries: 50_000,
        budget_bytes: 1024 * 1024 * 1024,
        budget_secs: 60,
    };
    let mut seen = std::collections::HashSet::new();
    let mut options = args[2..].chunks_exact(2);
    for pair in &mut options {
        if !seen.insert(pair[0].as_str()) || pair[1].is_empty() {
            return Err(usage());
        }
        match pair[0].as_str() {
            "--state-dir" => parsed.state_dir = pair[1].clone().into(),
            "--max-entries" => parsed.max_entries = pair[1].parse().map_err(|_| usage())?,
            "--budget-bytes" => parsed.budget_bytes = pair[1].parse().map_err(|_| usage())?,
            "--budget-secs" => parsed.budget_secs = pair[1].parse().map_err(|_| usage())?,
            _ => return Err(usage()),
        }
    }
    if !options.remainder().is_empty() || parsed.state_dir.as_os_str().is_empty() {
        return Err(usage());
    }
    Ok(parsed)
}

// Resolve the CLI's explicit path to an existing durable grant, never a caller-supplied identity.
fn granted_root(
    engine: &Engine,
    path: &std::path::Path,
    name: &str,
) -> Result<GrantedRoot, String> {
    let dir = loomward_catalog::sqlite_dir(&engine.config().state_dir);
    let open = |name| {
        rusqlite::Connection::open_with_flags(
            dir.join(name),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|_| "grant store unavailable".to_owned())
    };
    let text = path.to_str().ok_or("invalid_path")?;
    let display_path = text.strip_prefix(r"\\?\").unwrap_or(text);
    let conn = open("state.db")?;
    let mut statement = conn
        .prepare("SELECT id FROM root_grant WHERE display_path=?1 AND state='active'")
        .map_err(|_| "grant store unavailable")?;
    let grants = statement
        .query_map([display_path], |r| r.get::<_, i64>(0))
        .map_err(|_| "grant store unavailable")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| "grant store unavailable")?;
    let [grant] = grants.as_slice() else {
        return Err("grant selection is ambiguous or stale; use a fresh --state-dir".into());
    };
    let (row, instance) = open("catalog.db")?
        .query_row("SELECT id,(SELECT value FROM meta WHERE key='instance_id') FROM root WHERE grant_id=?1", [grant], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|_| "grant_root_mismatch")?;
    engine
        .granted_root(
            RootId::new(name).map_err(|e| e.to_string())?,
            *grant,
            &CatalogRootIdentity { row, instance },
        )
        .map_err(|e| e.to_string())
}

// These are the admitted identities compare_trees bound its pinned opens to, never new path probes.
pub(super) fn root_identity(root: &GrantedRoot) -> Result<serde_json::Value, String> {
    use loomward_engine::scan::source::FileIdObs;
    let identity = root
        .expected_identity()
        .ok_or("root identity unavailable")?;
    let volume = identity.volume_guid.ok_or("root volume GUID unavailable")?;
    let Some(FileIdObs::Id128(id)) = identity.id else {
        return Err("root file ID unavailable".into());
    };
    let file_id: String = id.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(serde_json::json!({"volume_guid": format!("{volume:032x}"), "root_file_id": file_id}))
}

fn exit_status(report: &Report) -> u8 {
    if !report.unknowns.is_empty()
        || report
            .differences
            .iter()
            .any(|d| matches!(d.kind, DifferenceKind::Refused { .. }))
    {
        2
    } else if report.equal() {
        0
    } else {
        1
    }
}

/// Grants exactly the two explicit folders into the scratch `--state-dir` and resolves each to its
/// durable grant (#1011 shares this with `covers`). No server or command transport starts.
pub(super) fn admit(
    args: &Args,
    names: [&str; 2],
) -> Result<(Engine, GrantedRoot, GrantedRoot), String> {
    let roots = paths::validate_roots(
        &[args.source.clone(), args.destination.clone()],
        Some(&args.state_dir),
    )
    .map_err(|reason| {
        if reason.contains("protected_profile_or_store") {
            "whole user-profile folders and credential or browser stores are not eligible".into()
        } else {
            reason
        }
    })?;
    // The shared Windows fence uses OS known folders; also honour the explicit USERPROFILE folder.
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        let profile = PathBuf::from(profile)
            .canonicalize()
            .map_err(|_| "cannot resolve user profile")?;
        if roots.contains(&profile) {
            return Err(
                "the whole user-profile folder is not eligible (q-4); choose a child folder".into(),
            );
        }
    }
    // As with --grant-root, consent is durable in state.db. No server or command transport starts.
    let service = Service::open_for_verification(Config {
        state_dir: args.state_dir.clone(),
        dataset: DatasetClass::Personal,
        allow_personal: true,
        grant_roots: roots.clone(),
        allow_identity_unverified: false,
    })?;
    drop(service);
    let engine = Engine::open(EngineConfig::new(
        args.state_dir.clone(),
        DatasetClass::Personal,
    ))
    .map_err(|e| e.to_string())?;
    let source = granted_root(&engine, &roots[0], names[0])?;
    let destination = granted_root(&engine, &roots[1], names[1])?;
    Ok((engine, source, destination))
}

pub(super) fn run(args: Args) -> Result<(serde_json::Value, u8), String> {
    let (engine, source, destination) =
        admit(&args, ["rt_verify_source", "rt_verify_destination"])?;
    let report = engine
        .compare_trees(
            &source,
            &destination,
            Budget {
                bytes: args.budget_bytes,
                time: Duration::from_secs(args.budget_secs),
            },
            args.max_entries,
        )
        .map_err(|e| e.to_string())?;
    let status = exit_status(&report);
    let mut json = serde_json::to_value(&report).map_err(|_| "cannot serialize tree report")?;
    json["roots"] = serde_json::json!({
        "source": root_identity(&source)?,
        "destination": root_identity(&destination)?,
    });
    let outcome = match status {
        0 => "equal",
        1 => "differences",
        _ => "unknown or refused",
    };
    eprintln!(
        "loomward-native verify-tree: {outcome}; {} difference(s), {} unknown(s)",
        report.differences.len(),
        report.unknowns.len()
    );
    Ok((json, status))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).into()).collect()
    }

    #[test]
    fn parse_defaults_and_explicit_budgets() {
        let parsed = parse(&args(&["source", "destination", "--state-dir", "state"])).unwrap();
        assert_eq!(parsed.source, PathBuf::from("source"));
        assert_eq!(parsed.destination, PathBuf::from("destination"));
        assert_eq!(parsed.state_dir, PathBuf::from("state"));
        assert_eq!(
            (parsed.max_entries, parsed.budget_bytes, parsed.budget_secs),
            (50_000, 1024 * 1024 * 1024, 60)
        );
        let parsed = parse(&args(&[
            "source",
            "destination",
            "--budget-secs",
            "0",
            "--max-entries",
            "12",
            "--state-dir",
            "state",
            "--budget-bytes",
            "34",
        ]))
        .unwrap();
        assert_eq!(
            (parsed.max_entries, parsed.budget_bytes, parsed.budget_secs),
            (12, 34, 0)
        );
    }

    #[test]
    fn parse_refuses_missing_unknown_duplicate_and_invalid_arguments() {
        for words in [
            vec![],
            vec!["source", "destination"],
            vec!["", "destination", "--state-dir", "state"],
            vec!["source", "", "--state-dir", "state"],
            vec!["source", "destination", "--state-dir"],
            vec!["source", "destination", "--state-dir", ""],
            vec!["source", "destination", "--state-dir", "state", "extra"],
            vec![
                "source",
                "destination",
                "--state-dir",
                "state",
                "--unknown",
                "1",
            ],
            vec![
                "source",
                "destination",
                "--state-dir",
                "state",
                "--state-dir",
                "again",
            ],
            vec![
                "source",
                "destination",
                "--state-dir",
                "state",
                "--budget-bytes",
                "-1",
            ],
            vec![
                "source",
                "destination",
                "--state-dir",
                "state",
                "--budget-secs",
                "1.5",
            ],
            vec![
                "source",
                "destination",
                "--state-dir",
                "state",
                "--max-entries",
                "18446744073709551616",
            ],
        ] {
            assert!(
                parse(&args(&words)).unwrap_err().starts_with("Usage:"),
                "{words:?}"
            );
        }
    }

    #[test]
    fn unknowns_and_refused_classes_take_precedence_over_differences() {
        use loomward_operations::tree::*;
        let mut report = Report::default();
        assert_eq!(exit_status(&report), 0);
        report.differences.push(Difference {
            path: "fixture".into(),
            kind: DifferenceKind::Content,
        });
        assert_eq!(exit_status(&report), 1);
        report.unknowns.push(Unknown {
            path: "fixture".into(),
            side: Side::Source,
            field: Field::Content,
            reason: "byte_budget",
        });
        assert_eq!(exit_status(&report), 2);
        report.unknowns.clear();
        report.differences.push(Difference {
            path: "fixture".into(),
            kind: DifferenceKind::Refused {
                side: Side::Destination,
                class: MetadataClass::ReparsePoint,
            },
        });
        assert_eq!(exit_status(&report), 2);
        let json = serde_json::to_value(report).unwrap();
        assert_eq!(
            json["differences"][1]["kind"]["Refused"]["class"],
            "ReparsePoint"
        );
    }
}
