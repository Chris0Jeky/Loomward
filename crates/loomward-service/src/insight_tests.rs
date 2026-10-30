//! LW-138 (#877) `insight.folder`: synthetic trees only. Axes and fact-only lines with traces that
//! resolve into the evidence packet; partial roots abstain on coverage; unknown stays unknown; only the
//! owner asks, and a read changes nothing.
use super::*;
use loomward_catalog::NodeKey;
use loomward_engine::learning::packet_builder::{build, PacketIds};

/// 2020-01-15T00:00:00Z as a FILETIME.
const JAN_2020: i64 = 116_444_736_000_000_000 + 1_579_046_400 * 10_000_000;

fn root_node(svc: &Service, root: &str) -> String {
    let (slice, _) = ok(
        "tree.slice",
        call(
            svc,
            "tree.slice",
            json!({"anchor": {"kind": "root", "root_id": root}, "depth": 1, "max_nodes": 16,
                   "min_share": 0.0, "basis": "logical", "include_files": false}),
        ),
    );
    slice["anchor_node_id"].as_str().unwrap().to_owned()
}

/// The directory (or file) named `name` directly under `parent`.
fn child(svc: &Service, parent: &str, name: &str) -> String {
    let (page, _) = ok(
        "tree.children",
        call(
            svc,
            "tree.children",
            json!({"node_id": parent, "sort": "name_asc", "basis": "logical", "limit": 50, "cursor": null}),
        ),
    );
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == name)
        .unwrap_or_else(|| panic!("no child {name}"))["node_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn insight(svc: &Service, root: &str, node: &str) -> Value {
    json!(
        ok(
            "insight.folder",
            call(
                svc,
                "insight.folder",
                json!({"root_id": root, "node_id": node})
            )
        )
        .0
    )
}

fn old_files(svc: &Service) {
    svc.inner
        .db()
        .execute("UPDATE main.file SET modified_ft=?1", [JAN_2020])
        .unwrap();
}

/// The packet `insight.folder` reads for `node`, rebuilt from the same catalogue facts.
fn packet_of(svc: &Service, node: &str, observed_at: u64) -> Value {
    let mut reader = svc.inner.catalog.reader().unwrap();
    let NodeKey::Dir(dir) = svc.inner.node(node, &mut reader).unwrap() else {
        panic!("not a directory")
    };
    let facts = reader.evidence_facts(dir).unwrap();
    build(
        &facts,
        PacketIds {
            packet_id: "insight-folder".into(),
            group_id: "insight-folder".into(),
            observed_at,
            synthetic: true,
        },
    )
    .unwrap()
}

fn axis<'a>(r: &'a Value, name: &str) -> &'a Value {
    r["axes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["axis"] == name)
        .unwrap()
}

fn voted(a: &Value) -> Vec<(&str, &str)> {
    a["votes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["function"].as_str().unwrap(),
                v["label"].as_str().unwrap(),
            )
        })
        .collect()
}

fn text(r: &Value) -> String {
    r["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["text"].as_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn insight_folder_labels_a_known_project_and_every_trace_resolves() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = scanned(&svc, 1, &projects());
    old_files(&svc);
    let app = child(&svc, &root_node(&svc, &root), "app");
    let before = svc.inner.revisions().unwrap();

    let r = insight(&svc, &root, &app);
    assert_eq!(
        svc.inner.revisions().unwrap(),
        before,
        "a read must not write"
    );
    assert_eq!(
        (r["root_id"].as_str(), r["node_id"].as_str()),
        (Some(root.as_str()), Some(app.as_str()))
    );
    assert_eq!(
        (r["coverage"].as_str(), r["partial"].as_bool()),
        (Some("complete"), Some(false))
    );
    assert!(r["generation"].is_string());

    let order: Vec<_> = r["axes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["axis"].as_str().unwrap())
        .collect();
    assert_eq!(order, ["meaning", "lifecycle", "recreatability"]);
    let meaning = axis(&r, "meaning");
    assert_eq!(
        (meaning["outcome"].as_str(), meaning["label"].as_str()),
        (Some("label"), Some("source_project"))
    );
    assert!(meaning["abstain_reason"].is_null());
    assert_eq!(voted(meaning), [("lf_source_manifest", "source_project")]);
    assert_eq!(axis(&r, "lifecycle")["label"], "historical");
    assert_eq!(
        voted(axis(&r, "lifecycle")),
        [("lf_quiet_years", "historical")]
    );
    assert_eq!(axis(&r, "recreatability")["label"], "mixed");
    assert_eq!(
        voted(axis(&r, "recreatability")),
        [("lf_project_with_outputs", "mixed")]
    );

    // Facts first, wording from the explainer: counts from the catalogue, coldness as absence of evidence.
    let all = text(&r);
    assert!(all.contains("Coverage is complete."), "{all}");
    assert!(all.contains("4 files"), "{all}");
    assert!(
        all.contains("No activity evidence seen since January 2020"),
        "{all}"
    );
    assert!(
        all.contains("an inference from the facts above, not a recorded fact"),
        "{all}"
    );
    for banned in ["unused", "safe to", "delete", "approve", "probab"] {
        assert!(!all.to_lowercase().contains(banned), "{banned} in {all}");
    }

    // Every line names the packet fields it rests on, and each one resolves in the packet.
    let observed = chrono_free_unix(r["observed_at"].as_str().unwrap());
    let packet = packet_of(&svc, &app, observed);
    for line in r["lines"].as_array().unwrap() {
        let trace = line["trace"].as_array().unwrap();
        assert!(!trace.is_empty(), "{line}");
        for pointer in trace {
            assert!(
                packet.pointer(pointer.as_str().unwrap()).is_some(),
                "{pointer} in {line}"
            );
        }
    }
    serde_json::from_value::<InsightFolder>(r).unwrap();
}

/// `YYYY-MM-DDTHH:MM:SS...Z` to Unix seconds (days from civil; the clock is after 1970).
fn chrono_free_unix(ts: &str) -> u64 {
    let n = |a: usize, b: usize| ts[a..b].parse::<i64>().unwrap();
    let (mut y, m, d) = (n(0, 4), n(5, 7), n(8, 10));
    y -= i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    (days * 86_400 + n(11, 13) * 3600 + n(14, 16) * 60 + n(17, 19)) as u64
}

#[test]
fn insight_folder_abstains_with_a_reason_and_never_invents_a_label() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = scanned(
        &svc,
        1,
        &dir(
            "",
            &[],
            vec![
                dir("plain", &[("notes.bin", 10)], vec![]),
                // A manifest and a camera layout beside mostly images: two Meaning rules disagree.
                dir(
                    "mixed",
                    &[("Cargo.toml", 1), ("a.cr3", 5), ("b.cr3", 5), ("c.cr3", 5)],
                    vec![dir("DCIM", &[], vec![])],
                ),
            ],
        ),
    );
    old_files(&svc);
    svc.inner
        .db()
        .execute_batch(
            "INSERT OR IGNORE INTO ext(ext,family) VALUES('cr3','image');
             UPDATE main.file SET ext_id=(SELECT id FROM ext WHERE ext='cr3') WHERE name LIKE '%.cr3';",
        )
        .unwrap();
    let top = root_node(&svc, &root);

    let plain = insight(&svc, &root, &child(&svc, &top, "plain"));
    let meaning = axis(&plain, "meaning");
    assert_eq!(
        (
            meaning["outcome"].as_str(),
            meaning["abstain_reason"].as_str()
        ),
        (Some("abstain"), Some("no_vote"))
    );
    assert!(meaning["label"].is_null() && meaning["votes"].as_array().unwrap().is_empty());
    assert_eq!(axis(&plain, "recreatability")["abstain_reason"], "no_vote");
    assert!(text(&plain).contains("Meaning: no rule had enough evidence, so no label is given."));

    let mixed = insight(&svc, &root, &child(&svc, &top, "mixed"));
    let meaning = axis(&mixed, "meaning");
    assert_eq!(
        (
            meaning["outcome"].as_str(),
            meaning["abstain_reason"].as_str()
        ),
        (Some("abstain"), Some("conflict"))
    );
    assert!(meaning["label"].is_null());
    // Votes keep the labelling order (axis, then function name); both disagreeing rules are shown.
    assert_eq!(
        voted(meaning),
        [
            ("lf_photo_shoot", "photo_shoot"),
            ("lf_source_manifest", "source_project")
        ]
    );
    assert!(text(&mixed).contains("Meaning: rules disagree"));
}

#[test]
fn insight_folder_on_an_unscanned_root_reports_unknown_not_zero() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let (row, _) = register(&svc, 1);
    let root = svc.inner.root_id(row);
    let node = root_node(&svc, &root);
    let r = insight(&svc, &root, &node);
    assert_ne!(r["coverage"], "complete");
    assert_eq!(r["partial"], true);
    for a in r["axes"].as_array().unwrap() {
        assert_eq!(
            (a["outcome"].as_str(), a["abstain_reason"].as_str()),
            (Some("abstain"), Some("coverage")),
            "{a}"
        );
        assert!(a["label"].is_null());
    }
    let all = text(&r);
    assert!(all.contains("files unknown"), "{all}");
    assert!(!all.contains("0 files"), "{all}");
}

#[cfg(windows)]
#[test]
fn insight_folder_on_a_root_with_unresolved_changes_says_stale_and_abstains() {
    use loomward_windows::watch::Change;
    // A real scanned lab root, as the clutter stale test: a watcher hint makes its figures an earlier state.
    let Some(lab) = lab() else { return };
    std::fs::write(lab.path.join("Cargo.toml"), b"synthetic manifest").unwrap();
    std::fs::create_dir(lab.path.join("target")).unwrap();
    std::fs::write(lab.path.join("target").join("synthetic.bin"), vec![0u8; 64]).unwrap();
    settle(&lab.svc, &start(&lab.svc, &lab.root, "full"));
    f3_wait_live(&lab.svc);
    lab.svc.inner.freshness.pause_pump();
    let node = root_node(&lab.svc, &lab.root);
    let fresh = insight(&lab.svc, &lab.root, &node);
    assert_eq!(fresh["coverage"], "complete");
    assert_eq!(axis(&fresh, "meaning")["label"], "source_project");
    let row = lab.svc.inner.root_row(&lab.root).unwrap();
    lab.svc.inner.freshness.record(row, vec![Change::RootDirty]);
    assert!(lab.svc.inner.freshness.stale(row));
    let r = insight(&lab.svc, &lab.root, &node);
    assert_eq!(
        (r["coverage"].as_str(), r["partial"].as_bool()),
        (Some("stale"), Some(true))
    );
    for a in r["axes"].as_array().unwrap() {
        assert_eq!(a["abstain_reason"], "coverage", "{a}");
        assert!(a["label"].is_null());
    }
    assert!(text(&r).contains("Coverage is stale: the figures describe an earlier scan."));
}

#[test]
fn insight_folder_refuses_unknown_wrong_root_file_and_malformed_requests() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = scanned(&svc, 1, &projects());
    let other = scanned(&svc, 2, &projects());
    let top = root_node(&svc, &root);
    let app = child(&svc, &top, "app");
    let file = child(&svc, &app, "Cargo.toml");
    let refuse = |payload: Value, code: ErrorCode, what: &str| {
        assert_eq!(
            err(call(&svc, "insight.folder", payload)).code,
            code,
            "{what}"
        );
    };
    refuse(
        json!({"root_id": "rt_AAAAAAAAAAAAAAAA", "node_id": app}),
        ErrorCode::NotFound,
        "unknown root",
    );
    refuse(
        json!({"root_id": root, "node_id": "nd_AAAAAAAAAAAAAAAA"}),
        ErrorCode::NotFound,
        "unknown node",
    );
    refuse(
        json!({"root_id": other, "node_id": app}),
        ErrorCode::NotFound,
        "node in another root",
    );
    refuse(
        json!({"root_id": root, "node_id": file}),
        ErrorCode::InvalidRequest,
        "file node",
    );
    refuse(
        json!({"root_id": root}),
        ErrorCode::InvalidRequest,
        "missing node",
    );
    refuse(
        json!({"root_id": root, "node_id": app, "limit": 5}),
        ErrorCode::InvalidRequest,
        "extra field",
    );
    refuse(
        json!({"root_id": "x", "node_id": app}),
        ErrorCode::InvalidRequest,
        "malformed root",
    );
    ok(
        "roots.revoke",
        call(
            &svc,
            "roots.revoke",
            json!({"root_id": root, "purge_catalog": false}),
        ),
    );
    refuse(
        json!({"root_id": root, "node_id": app}),
        ErrorCode::NotFound,
        "revoked root",
    );
}

#[test]
fn only_the_owner_asks_insight_folder() {
    let tmp = temp_root();
    let svc = open(tmp.path(), DatasetClass::Synthetic);
    let root = scanned(&svc, 1, &projects());
    let app = child(&svc, &root_node(&svc, &root), "app");
    for client in ["ask:local", "mcp:reader"] {
        let e = err(svc.call(
            request("insight.folder", json!({"root_id": root, "node_id": app})),
            &CallContext::client(client),
        ));
        assert_eq!(e.code, ErrorCode::PermissionDenied, "{client}");
    }
    assert!(!crate::clients::open_to_clients(Command::InsightFolder));
}
