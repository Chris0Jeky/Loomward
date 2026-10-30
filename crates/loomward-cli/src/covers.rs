//! `covers A B` (#1011): is every file of A present somewhere in B by content, whatever the paths?
//! Same grants, admission and budgets as `verify-tree`; read-only on both roots.
use super::verify_tree::{admit, root_identity, Args};
use loomward_engine::scan::TreeBudget as Budget;
use loomward_operations::containment::{containment, Coverage, Item, Report};
use std::time::Duration;

/// Most entries each list carries; the totals always count everything.
const LIST_CAP: usize = 1_000;
/// Covered pairs are evidence samples, not the inventory.
const COVERED_SAMPLE: usize = 100;
const NOTE: &str = "Content coverage only: every covered A file has a B file of equal size and \
equal content digest. It is not metadata fidelity (timestamps, attributes, ACLs, streams), not an \
independent backup (B may share a failing disk) and not permission to delete anything. It holds as read: either side may change afterwards. Unknown bytes are a lower bound when size_unknown is above zero.";

pub(super) fn parse(args: &[String]) -> Result<Args, String> {
    // verify-tree may grow options (#1019 `--only`) that mean nothing here; refuse any but these.
    let known = [
        "--state-dir",
        "--max-entries",
        "--budget-bytes",
        "--budget-secs",
    ];
    if args.len() > 2
        && args[2..]
            .chunks(2)
            .any(|pair| !known.contains(&pair[0].as_str()))
    {
        return Err(super::USAGE.into());
    }
    super::verify_tree::parse(args)
}

fn exit_status(report: &Report) -> u8 {
    let totals = report.totals();
    if totals.unknown.files > 0 {
        2
    } else if totals.not_covered.files > 0 {
        1
    } else {
        0
    }
}

fn list<'a>(items: impl Iterator<Item = &'a Item>, cap: usize) -> serde_json::Value {
    let items: Vec<_> = items.collect();
    serde_json::json!({
        "items": items.iter().take(cap).collect::<Vec<_>>(),
        "truncated": items.len() > cap,
    })
}

fn render(report: &Report) -> serde_json::Value {
    let by = |f: fn(&Coverage) -> bool| report.items.iter().filter(move |i| f(&i.coverage));
    serde_json::json!({
        "totals": report.totals(),
        "not_covered": list(by(|c| matches!(c, Coverage::NotCovered)), LIST_CAP),
        "unknown": list(by(|c| matches!(c, Coverage::Unknown { .. })), LIST_CAP),
        "covered_sample": list(by(|c| matches!(c, Coverage::Covered { .. })), COVERED_SAMPLE),
        "note": NOTE,
    })
}

pub(super) fn run(args: Args) -> Result<(serde_json::Value, u8), String> {
    let (engine, a, b) = admit(&args, ["rt_covers_a", "rt_covers_b"])?;
    let (a_tree, b_tree, bytes_read) = engine
        .observe_coverage(
            &a,
            &b,
            Budget {
                bytes: args.budget_bytes,
                time: Duration::from_secs(args.budget_secs),
            },
            args.max_entries,
        )
        .map_err(|e| e.to_string())?;
    let report = containment(&a_tree, &b_tree);
    let status = exit_status(&report);
    let mut json = render(&report);
    json["bytes_read"] = bytes_read.into();
    json["roots"] = serde_json::json!({"a": root_identity(&a)?, "b": root_identity(&b)?});
    let totals = report.totals();
    eprintln!(
        "loomward-native covers: {} covered, {} not covered, {} unknown",
        totals.covered.files, totals.not_covered.files, totals.unknown.files
    );
    Ok((json, status))
}

#[cfg(test)]
mod tests {
    use super::*;
    use loomward_operations::tree::Side;

    fn item(path: &str, coverage: Coverage) -> Item {
        Item {
            path: path.into(),
            size: Some(1),
            coverage,
        }
    }

    #[test]
    fn parse_takes_verify_tree_options_and_refuses_others() {
        let words = |w: &[&str]| w.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>();
        let parsed = parse(&words(&[
            "a",
            "b",
            "--state-dir",
            "s",
            "--budget-bytes",
            "9",
        ]))
        .unwrap();
        assert_eq!(
            (parsed.state_dir.to_str(), parsed.budget_bytes),
            (Some("s"), 9)
        );
        for bad in [
            vec!["a", "b", "--state-dir", "s", "--only", "list.txt"],
            vec!["a", "b", "--state-dir", "s", "--manifest-roots"],
            vec!["a", "b", "--manifest-roots", "--state-dir", "s"],
            vec!["a", "b", "--state-dir", "s", "--unknown", "1"],
            vec!["a", "b"],
            vec!["a", "b", "--state-dir"],
        ] {
            assert!(
                parse(&words(&bad)).unwrap_err().starts_with("Usage:"),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn exit_status_is_zero_only_when_all_covered_and_unknown_wins() {
        let mut report = Report::default();
        assert_eq!(exit_status(&report), 0);
        report
            .items
            .push(item("a", Coverage::Covered { by: "b".into() }));
        assert_eq!(exit_status(&report), 0);
        report.items.push(item("c", Coverage::NotCovered));
        assert_eq!(exit_status(&report), 1);
        report.items.push(item(
            "d",
            Coverage::Unknown {
                side: Side::Destination,
                reason: "byte_budget",
            },
        ));
        assert_eq!(exit_status(&report), 2);
    }

    #[test]
    fn render_caps_lists_says_when_truncated_and_carries_the_note() {
        let mut report = Report::default();
        for n in 0..COVERED_SAMPLE + 1 {
            report
                .items
                .push(item(&format!("c{n}"), Coverage::Covered { by: "b".into() }));
        }
        report.items.push(item("gone", Coverage::NotCovered));
        let json = render(&report);
        assert_eq!(
            json["totals"]["covered"]["files"],
            COVERED_SAMPLE as u64 + 1
        );
        assert_eq!(json["covered_sample"]["truncated"], true);
        assert_eq!(
            json["covered_sample"]["items"].as_array().unwrap().len(),
            COVERED_SAMPLE
        );
        assert_eq!(json["not_covered"]["truncated"], false);
        assert_eq!(json["not_covered"]["items"][0]["path"], "gone");
        assert_eq!(json["not_covered"]["items"][0]["state"], "not_covered");
        assert_eq!(json["unknown"]["items"].as_array().unwrap().len(), 0);
        let note = json["note"].as_str().unwrap();
        assert!(note.contains("not permission to delete"));
        assert!(note.contains("not metadata fidelity"));
    }
}
