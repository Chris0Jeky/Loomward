use loomward_engine::scan::TreeBudget as Budget;
use loomward_engine::{scan::CatalogRootIdentity, scan::GrantedRoot, Engine, EngineConfig};
use loomward_operations::tree::{DifferenceKind, Report};
use loomward_protocol::{DatasetClass, RootId};
use loomward_service::{paths, Config, Service};
use std::{collections::BTreeSet, path::PathBuf, time::Duration};

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Args {
    source: PathBuf,
    destination: PathBuf,
    state_dir: PathBuf,
    max_entries: usize,
    budget_bytes: u64,
    budget_secs: u64,
    only: Option<PathBuf>,
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
        only: None,
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
            "--only" => parsed.only = Some(pair[1].clone().into()),
            _ => return Err(usage()),
        }
    }
    if !options.remainder().is_empty() || parsed.state_dir.as_os_str().is_empty() {
        return Err(usage());
    }
    Ok(parsed)
}

/// `--only` (#1009): one relative path per line, as the approved plan names them. Anything that
/// could leave the source (absolute, `..`, `.`, a drive or a root) is refused, as is an empty list,
/// which would compare nothing.
fn selection(text: &str) -> Result<BTreeSet<PathBuf>, String> {
    use std::path::Component;
    let mut paths = BTreeSet::new();
    for (number, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let path = PathBuf::from(line);
        if !path.components().all(|c| matches!(c, Component::Normal(_))) {
            return Err(format!(
                "--only: line {} is not a relative path inside the source",
                number + 1
            ));
        }
        paths.insert(path);
    }
    if paths.is_empty() {
        return Err("--only: the list names no paths".into());
    }
    Ok(paths)
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
fn root_identity(root: &GrantedRoot) -> Result<serde_json::Value, String> {
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

pub(super) fn run(args: Args) -> Result<(serde_json::Value, u8), String> {
    let only = match &args.only {
        Some(list) => Some(selection(
            &std::fs::read_to_string(list).map_err(|e| format!("--only: {e}"))?,
        )?),
        None => None,
    };
    let roots = paths::validate_roots(&[args.source, args.destination], Some(&args.state_dir))
        .map_err(|reason| {
            if reason.contains("protected_profile_or_store") {
                "whole user-profile folders and credential or browser stores are not eligible"
                    .into()
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
    let engine = Engine::open(EngineConfig::new(args.state_dir, DatasetClass::Personal))
        .map_err(|e| e.to_string())?;
    let source = granted_root(&engine, &roots[0], "rt_verify_source")?;
    let destination = granted_root(&engine, &roots[1], "rt_verify_destination")?;
    let report = engine
        .compare_selected_trees(
            &source,
            &destination,
            Budget {
                bytes: args.budget_bytes,
                time: Duration::from_secs(args.budget_secs),
            },
            args.max_entries,
            only.as_ref(),
        )
        .map_err(|e| e.to_string())?;
    let status = exit_status(&report);
    let mut json = serde_json::to_value(&report).map_err(|_| "cannot serialize tree report")?;
    json["roots"] = serde_json::json!({
        "source": root_identity(&source)?,
        "destination": root_identity(&destination)?,
    });
    if let Some(paths) = &only {
        json["selection"] = serde_json::json!({ "paths": paths.len() });
    }
    let outcome = match status {
        0 => "equal",
        1 => "differences",
        _ => "unknown or refused",
    };
    let scope = only.as_ref().map_or(String::new(), |paths| {
        format!(" over {} selected path(s)", paths.len())
    });
    eprintln!(
        "loomward-native verify-tree: {outcome}{scope}; {} difference(s), {} unknown(s)",
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
        assert_eq!(parsed.only, None);
        let parsed = parse(&args(&[
            "source",
            "destination",
            "--state-dir",
            "state",
            "--only",
            "plan.txt",
        ]))
        .unwrap();
        assert_eq!(parsed.only, Some(PathBuf::from("plan.txt")));
    }

    #[test]
    fn selection_takes_relative_plan_paths_and_refuses_anything_else() {
        let text = "\u{feff}a/model.safetensors\r\n\r\nb/c.gguf\na/model.safetensors\n";
        assert_eq!(
            selection(text).unwrap().into_iter().collect::<Vec<_>>(),
            vec![
                PathBuf::from("a/model.safetensors"),
                PathBuf::from("b/c.gguf")
            ]
        );
        for text in [
            "",
            "\n\n",
            "../escape",
            "a/../../b",
            "./a",
            "/abs",
            "a\n/abs",
        ] {
            assert!(
                selection(text).unwrap_err().starts_with("--only"),
                "{text:?}"
            );
        }
        #[cfg(windows)]
        for text in [r"C:\x", r"\\server\share\x", "C:x", r"a\..\..\b"] {
            assert!(selection(text).is_err(), "{text:?}");
        }
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
                "--only",
                "",
            ],
            vec!["source", "destination", "--state-dir", "state", "--only"],
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
