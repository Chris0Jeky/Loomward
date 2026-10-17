//! LW-067 / LW-069 canaries (docs/46 section 10, F3a): an ungranted client, one root versus a
//! multi-root index, excluded totals and fields, client-bound cursor reuse, and revocation
//! between list and explain. Synthetic names only.

use crate::tests::temp_root;
use crate::{Config, Service};
use loomward_catalog::{DirListing, Observation, RootObservation, WriteCommand, WriteReply};
use loomward_protocol::*;
use serde_json::{json, Value};

fn open(dir: &std::path::Path) -> Service {
    Service::open(Config {
        state_dir: dir.to_path_buf(),
        dataset: DatasetClass::Synthetic,
        allow_personal: false,
        allow_identity_unverified: false,
        grant_roots: vec![],
    })
    .unwrap()
}

fn file(name: &str, bytes: u64) -> Observation {
    let mut o = Observation::file(name, bytes, Some(bytes));
    o.extension = name.rsplit_once('.').map(|(_, e)| e.into());
    o.family = "document".into();
    o.modified_ft = Some(133_000_000_000_000_000 + bytes as i64);
    o
}

/// A fixture root. With `files`, one complete run lists them and a `report drafts` folder holding
/// one more; without, the root stays unscanned.
fn root(svc: &Service, name: &str, id: u8, files: Option<&[(&str, u64)]>) -> RootId {
    let w = svc.inner.catalog.writer();
    let WriteReply::Root {
        grant_id,
        root_id,
        dir_id,
    } = w
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_key: "fixture".into(),
            display_name: name.into(),
            display_path: format!(r"Synthetic Parent\Synthetic Owner\{name}"),
            root_file_id: Some(vec![id; 16]),
            filesystem: Some("NTFS".into()),
            origin: "fixture".into(),
            granted_via: "fixture".into(),
            observed_at_ns: 0,
        }))
        .unwrap()
    else {
        panic!()
    };
    if let Some(files) = files {
        let WriteReply::Run(run) = w
            .call(WriteCommand::BeginRun {
                grant_id,
                mode: "full".into(),
                strategy: "fixture".into(),
                started_at_ns: 0,
            })
            .unwrap()
        else {
            panic!()
        };
        let mut sub = Observation::file("report drafts", 0, Some(0));
        sub.family = "none".into();
        let listing = |dir, files: Vec<Observation>, dirs| {
            w.call(WriteCommand::DirListing(DirListing {
                run_id: run,
                dir_id: dir,
                files,
                dirs,
                state: "complete".into(),
                skipped: 0,
                errors: 0,
            }))
            .unwrap();
        };
        listing(
            dir_id,
            files.iter().map(|(n, b)| file(n, *b)).collect(),
            vec![sub],
        );
        let sub_id: i64 = svc
            .inner
            .db()
            .query_row(
                "SELECT id FROM main.dir WHERE parent_id=?1 UNION ALL SELECT id FROM main.bulk_dir WHERE parent_id=?1",
                [dir_id],
                |r| r.get(0),
            )
            .unwrap();
        listing(sub_id, vec![file("report outline.txt", 7)], vec![]);
        w.call(WriteCommand::EndRun {
            run_id: run,
            state: "completed".into(),
            finished_at_ns: 1,
        })
        .unwrap();
    }
    RootId::new(svc.inner.root_id(root_id)).unwrap()
}

const ALPHA: &[(&str, u64)] = &[
    ("report.pdf", 4000),
    ("annual report.docx", 3000),
    ("report-2024.xlsx", 2000),
    ("notes.txt", 1000),
];
const BETA: &[(&str, u64)] = &[
    ("report.pdf", 9000),
    ("beta report secret.txt", 8000),
    ("areport.bin", 6000),
];

fn request(command: &str, payload: Value) -> RequestEnvelope {
    serde_json::from_value(json!({
        "protocol": "loomward/3", "request_id": "r1", "command": command, "payload": payload,
        "deadline_ms": 60_000
    }))
    .unwrap()
}
fn as_owner(svc: &Service, command: &str, payload: Value) -> ResponseEnvelope {
    svc.call(request(command, payload), &CallContext::http())
}
fn as_client(svc: &Service, client: &str, command: &str, payload: Value) -> ResponseEnvelope {
    svc.call(request(command, payload), &CallContext::client(client))
}
fn search(text: &str, limit: u32, cursor: Value) -> Value {
    json!({"root_id": null, "text": text, "extension": null, "min_bytes": null, "max_bytes": null,
           "modified_after": null, "modified_before": null, "kind": "any", "limit": limit, "cursor": cursor})
}
fn ok(r: ResponseEnvelope) -> Value {
    match r {
        ResponseEnvelope::Ok(o) => Value::Object(o.result),
        ResponseEnvelope::Err(e) => panic!("{:?}", e.error),
    }
}
fn err(r: ResponseEnvelope) -> ErrorBody {
    match r {
        ResponseEnvelope::Err(e) => e.error,
        ResponseEnvelope::Ok(o) => panic!("expected an error, got {:?}", o.result),
    }
}
/// Every page of a query, each with its cursor handle removed (handles are random).
fn pages(call: impl Fn(Value) -> ResponseEnvelope) -> Vec<Value> {
    let mut out = vec![];
    let mut cursor = Value::Null;
    loop {
        let mut page = ok(call(cursor));
        let next = page["next_cursor"].take();
        out.push(page);
        if next.is_null() || out.len() > 20 {
            break out;
        }
        cursor = next;
    }
}
/// A page with the per-instance sealed IDs blanked, so two services' pages compare.
fn unsealed(mut v: Value) -> Value {
    fn walk(v: &mut Value) {
        match v {
            Value::Object(m) => {
                for (k, x) in m.iter_mut() {
                    if k == "node_id" || k == "root_id" {
                        *x = json!("id");
                    } else {
                        walk(x);
                    }
                }
            }
            Value::Array(a) => a.iter_mut().for_each(walk),
            _ => {}
        }
    }
    walk(&mut v);
    v
}
fn names(page: &Value) -> Vec<String> {
    page["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["row"]["name"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn an_ungranted_client_reaches_nothing_and_learns_nothing() {
    let tmp = temp_root();
    let svc = open(tmp.path());
    let alpha = root(&svc, "Alpha", 1, Some(ALPHA));
    let refused = |client: &str| {
        let mut errors = vec![
            err(as_client(
                &svc,
                client,
                "search.query",
                search("report", 10, Value::Null),
            )),
            err(as_client(
                &svc,
                client,
                "search.query",
                json!({"root_id": alpha.as_ref(), "text": "report", "extension": null, "min_bytes": null,
                       "max_bytes": null, "modified_after": null, "modified_before": null, "kind": "any",
                       "limit": 10, "cursor": null}),
            )),
        ];
        let owner = ok(as_owner(
            &svc,
            "search.query",
            search("report", 1, Value::Null),
        ));
        let node = owner["hits"][0]["row"]["node_id"].clone();
        errors.push(err(as_client(
            &svc,
            client,
            "node.inspect",
            json!({"node_id": node}),
        )));
        errors
    };
    let no_grant = refused("mcp:probe");
    for e in &no_grant {
        assert_eq!(e.code, ErrorCode::PermissionDenied);
        assert!(e.detail.is_none());
        assert!(!e.message.contains("Alpha"));
    }
    // Every other command is closed to a client, granted or not: no roots, grants or effects.
    svc.grant_client("mcp:granted", std::slice::from_ref(&alpha), &["path"], None)
        .unwrap();
    for (command, payload) in [
        ("roots.list", json!({})),
        ("grants.list", json!({})),
        ("session.hello", json!({})),
        ("collections.list", json!({})),
        ("health.get", json!({})),
        (
            "roots.revoke",
            json!({"root_id": alpha.as_ref(), "purge_catalog": false}),
        ),
    ] {
        for client in ["mcp:probe", "mcp:granted"] {
            let e = err(as_client(&svc, client, command, payload.clone()));
            assert_eq!(e.code, ErrorCode::PermissionDenied, "{command} {client}");
        }
    }
    let page = ok(as_owner(
        &svc,
        "search.query",
        search("report", 1, Value::Null),
    ));
    assert_eq!(
        page["roots"][0]["scan_state"],
        json!("complete"),
        "a client never revokes"
    );
    // An expired grant and a revoked one read exactly as no grant.
    svc.grant_client(
        "mcp:expired",
        std::slice::from_ref(&alpha),
        &["path"],
        Some(1),
    )
    .unwrap();
    assert_eq!(refused("mcp:expired"), no_grant);
    assert!(svc.revoke_client("mcp:granted").unwrap());
    assert!(!svc.revoke_client("mcp:granted").unwrap());
    assert_eq!(refused("mcp:granted"), no_grant);
    // A client's refused mutation claimed nothing: the owner's request with the same ID runs.
    let revoke = json!({"root_id": alpha.as_ref(), "purge_catalog": false});
    assert!(matches!(
        as_owner(&svc, "roots.revoke", revoke),
        ResponseEnvelope::Ok(_)
    ));
    // Grants are owner-issued for well-formed principals over roots the owner granted.
    for (client, roots, fields) in [
        ("owner", vec![alpha.clone()], vec![]),
        ("mcp:", vec![alpha.clone()], vec![]),
        ("mcp:a b", vec![alpha.clone()], vec![]),
        ("mcp:x", vec![], vec![]),
        ("mcp:x", vec![alpha.clone()], vec!["contents"]),
    ] {
        let e = svc.grant_client(client, &roots, &fields, None).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidRequest, "{client}");
    }
}

#[test]
fn a_one_root_grant_reads_a_multi_root_index_as_a_one_root_index() {
    let (t1, t2) = (temp_root(), temp_root());
    let (alone, shared) = (open(t1.path()), open(t2.path()));
    let a1 = root(&alone, "Alpha", 1, Some(ALPHA));
    let a2 = root(&shared, "Alpha", 1, Some(ALPHA));
    let beta = root(&shared, "Beta", 2, Some(BETA));
    let fields = ["path", "size", "modified", "attributes"];
    alone
        .grant_client("ask:local", &[a1], &fields, None)
        .unwrap();
    shared
        .grant_client("ask:local", std::slice::from_ref(&a2), &fields, None)
        .unwrap();
    for (text, limit) in [
        ("report", 2),
        ("report", 100),
        ("re", 3),
        ("secret", 5),
        ("", 2),
    ] {
        let want = pages(|c| as_owner(&alone, "search.query", search(text, limit, c)));
        let got =
            pages(|c| as_client(&shared, "ask:local", "search.query", search(text, limit, c)));
        let unsealed_pages = |p: &[Value]| p.iter().cloned().map(unsealed).collect::<Vec<_>>();
        assert_eq!(
            unsealed_pages(&got),
            unsealed_pages(&want),
            "{text:?}/{limit}"
        );
        // The fixture is a real test: the owner of the shared index sees Beta.
        let owner = pages(|c| as_owner(&shared, "search.query", search(text, limit, c)));
        assert_ne!(
            unsealed_pages(&owner),
            unsealed_pages(&want),
            "{text:?}/{limit}"
        );
        for page in &got {
            assert_eq!(
                page["roots"],
                json!([{"root_id": a2.as_ref(), "scan_state": "complete", "coverage": "complete"}])
            );
            for hit in page["hits"].as_array().unwrap() {
                assert_eq!(hit["root_id"], json!(a2.as_ref()));
            }
        }
    }
    // Naming the other root reads exactly as naming one that never existed.
    let mut forged = beta.as_ref().to_owned();
    forged.push('0');
    let named = |id: &str| {
        let mut p = search("report", 10, Value::Null);
        p["root_id"] = json!(id);
        err(as_client(&shared, "ask:local", "search.query", p))
    };
    let (other, unknown) = (named(beta.as_ref()), named(&forged));
    assert_eq!(other.code, ErrorCode::NotFound);
    assert_eq!(
        (other.message, other.detail),
        (unknown.message, unknown.detail)
    );
}

#[test]
fn excluded_fields_are_absent_and_excluded_roots_do_not_reach_the_totals() {
    let tmp = temp_root();
    let svc = open(tmp.path());
    let alpha = root(&svc, "Alpha", 1, Some(ALPHA));
    // Beta is granted to the owner but never scanned: the owner's page is partial.
    let _beta = root(&svc, "Beta", 2, None);
    svc.grant_client("mcp:names", std::slice::from_ref(&alpha), &[], None)
        .unwrap();
    svc.grant_client(
        "mcp:sizes",
        std::slice::from_ref(&alpha),
        &["path", "size"],
        None,
    )
    .unwrap();
    let owner = ok(as_owner(
        &svc,
        "search.query",
        search("report", 100, Value::Null),
    ));
    let names_only = ok(as_client(
        &svc,
        "mcp:names",
        "search.query",
        search("report", 100, Value::Null),
    ));
    let sizes = ok(as_client(
        &svc,
        "mcp:sizes",
        "search.query",
        search("report", 100, Value::Null),
    ));
    assert_eq!(owner["partial"], json!(true));
    assert_eq!(owner["roots"].as_array().unwrap().len(), 2);
    for page in [&names_only, &sizes] {
        assert_eq!(page["partial"], json!(false));
        assert_eq!(page["index_state"], json!("current"));
        assert_eq!(page["roots"].as_array().unwrap().len(), 1);
        assert_eq!(names(page), names(&owner));
    }
    let withheld = [
        "location_hint",
        "logical_bytes",
        "allocated_bytes",
        "files",
        "dirs",
        "modified_at",
        "attributes",
        "flags",
    ];
    for (i, hit) in names_only["hits"].as_array().unwrap().iter().enumerate() {
        assert!(hit.get("path").is_none(), "absent, not null");
        let row = hit["row"].as_object().unwrap();
        for k in withheld {
            assert!(!row.contains_key(k), "{k}");
        }
        assert_eq!(hit["row"]["name"], owner["hits"][i]["row"]["name"]);
        assert_eq!(hit["match_kind"], owner["hits"][i]["match_kind"]);
    }
    // Granted fields pass through unchanged: the folder's total is the owner's own figure.
    for (i, hit) in sizes["hits"].as_array().unwrap().iter().enumerate() {
        let want = &owner["hits"][i];
        assert_eq!(hit["path"], want["path"]);
        assert_eq!(hit["row"]["logical_bytes"], want["row"]["logical_bytes"]);
        assert_eq!(hit["row"]["location_hint"], want["row"]["location_hint"]);
        assert!(hit["row"].get("modified_at").is_none());
    }
    // A filter on a withheld field would disclose it by bisection, as would a largest-first listing.
    for (client, key, value) in [
        ("mcp:names", "min_bytes", json!("1000")),
        ("mcp:names", "max_bytes", json!("1000")),
        ("mcp:sizes", "modified_after", json!("2020-01-01T00:00:00Z")),
        (
            "mcp:sizes",
            "modified_before",
            json!("2020-01-01T00:00:00Z"),
        ),
    ] {
        let mut p = search("report", 10, Value::Null);
        p[key] = value;
        let e = err(as_client(&svc, client, "search.query", p));
        assert_eq!(e.code, ErrorCode::InvalidRequest, "{client} {key}");
    }
    let mut listing = search("", 10, Value::Null);
    listing["extension"] = json!("pdf");
    let e = err(as_client(
        &svc,
        "mcp:names",
        "search.query",
        listing.clone(),
    ));
    assert_eq!(e.code, ErrorCode::InvalidRequest);
    let listed = ok(as_client(&svc, "mcp:sizes", "search.query", listing));
    assert_eq!(names(&listed), ["report.pdf"]);
}

#[test]
fn cursors_are_bound_to_their_client_and_its_grant_epoch() {
    let tmp = temp_root();
    let svc = open(tmp.path());
    let alpha = root(&svc, "Alpha", 1, Some(ALPHA));
    for client in ["mcp:one", "mcp:two"] {
        svc.grant_client(client, std::slice::from_ref(&alpha), &["path"], None)
            .unwrap();
    }
    let first = ok(as_client(
        &svc,
        "mcp:one",
        "search.query",
        search("report", 1, Value::Null),
    ));
    let cursor = first["next_cursor"].clone();
    assert!(cursor.is_string());
    let resume = |client: Option<&str>| {
        let p = search("report", 1, cursor.clone());
        match client {
            Some(c) => as_client(&svc, c, "search.query", p),
            None => as_owner(&svc, "search.query", p),
        }
    };
    let foreign = |r: ResponseEnvelope| {
        let e = err(r);
        assert_eq!(
            (e.code, e.message.as_str()),
            (
                ErrorCode::InvalidRequest,
                "the cursor belongs to another request"
            )
        );
    };
    foreign(resume(Some("mcp:two")));
    foreign(resume(None));
    assert_ne!(names(&ok(resume(Some("mcp:one")))), names(&first));
    // The owner's cursor is no client's either.
    let owner = ok(as_owner(
        &svc,
        "search.query",
        search("report", 1, Value::Null),
    ));
    foreign(as_client(
        &svc,
        "mcp:one",
        "search.query",
        search("report", 1, owner["next_cursor"].clone()),
    ));
    // A re-grant is a new epoch, even with the same roots and fields: the old cursor is dead.
    svc.grant_client("mcp:one", &[alpha], &["path"], None)
        .unwrap();
    foreign(resume(Some("mcp:one")));
}

#[test]
fn a_revocation_between_list_and_explain_refuses_the_explain_and_the_next_page() {
    let tmp = temp_root();
    let svc = open(tmp.path());
    let alpha = root(&svc, "Alpha", 1, Some(ALPHA));
    let _beta = root(&svc, "Beta", 2, Some(BETA));
    svc.grant_client("ask:local", std::slice::from_ref(&alpha), &["size"], None)
        .unwrap();
    let list = ok(as_client(
        &svc,
        "ask:local",
        "search.query",
        search("report", 1, Value::Null),
    ));
    let node = list["hits"][0]["row"]["node_id"].clone();
    let explain = || as_client(&svc, "ask:local", "node.inspect", json!({"node_id": node}));
    let detail = ok(explain());
    // The explain is the row and its root: no path without `path`, none of the owner's records.
    let keys: Vec<&str> = detail
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["root_id", "row"]);
    assert_eq!(detail["root_id"], json!(alpha.as_ref()));
    assert!(detail["row"].get("location_hint").is_none());
    assert!(detail["row"].get("logical_bytes").is_some());
    // A node of an ungranted root explains as a node that does not exist.
    let owner = ok(as_owner(
        &svc,
        "search.query",
        search("secret", 1, Value::Null),
    ));
    let theirs = owner["hits"][0]["row"]["node_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut tampered = theirs.clone();
    tampered.replace_range(tampered.len() - 2.., "00");
    let explain_of = |id: &str| {
        err(as_client(
            &svc,
            "ask:local",
            "node.inspect",
            json!({"node_id": id}),
        ))
    };
    let (other, unknown) = (explain_of(&theirs), explain_of(&tampered));
    assert_eq!(other.code, ErrorCode::NotFound);
    assert_eq!(
        (other.message, other.detail),
        (unknown.message, unknown.detail)
    );
    // ...even with a wrong expected generation, which would otherwise answer stale for a real node.
    let stale_explain = |id: &str| {
        let mut r = request("node.inspect", json!({"node_id": id}));
        r.expected_generation = Some(Generation(999));
        err(svc.call(r, &CallContext::client("ask:local")))
    };
    let (other, unknown) = (stale_explain(&theirs), stale_explain(&tampered));
    assert_eq!(other.code, ErrorCode::NotFound);
    assert_eq!(
        (other.message, other.detail),
        (unknown.message, unknown.detail)
    );
    // Revoke between the list and the explain: both the explain and the next page are refused.
    assert!(svc.revoke_client("ask:local").unwrap());
    assert_eq!(err(explain()).code, ErrorCode::PermissionDenied);
    let next = search("report", 1, list["next_cursor"].clone());
    assert_eq!(
        err(as_client(&svc, "ask:local", "search.query", next.clone())).code,
        ErrorCode::PermissionDenied
    );
    // Granting again opens a new epoch; the pre-revocation cursor stays dead.
    svc.grant_client("ask:local", std::slice::from_ref(&alpha), &["size"], None)
        .unwrap();
    assert_eq!(
        err(as_client(&svc, "ask:local", "search.query", next)).code,
        ErrorCode::InvalidRequest
    );
    assert!(ok(explain())["row"].is_object());
    // The owner revoking the root itself empties the client's view without an error.
    let e = as_owner(
        &svc,
        "roots.revoke",
        json!({"root_id": alpha.as_ref(), "purge_catalog": false}),
    );
    assert!(matches!(e, ResponseEnvelope::Ok(_)), "{e:?}");
    let page = ok(as_client(
        &svc,
        "ask:local",
        "search.query",
        search("report", 10, Value::Null),
    ));
    assert_eq!(page["hits"], json!([]));
    assert_eq!(page["roots"], json!([]));
    assert_eq!(err(explain()).code, ErrorCode::NotFound);
}

/// A client's paths start at the granted root's own name: never the owner's absolute path above it.
#[test]
fn a_client_path_starts_at_the_granted_root_and_the_owner_keeps_the_absolute_one() {
    let tmp = temp_root();
    let svc = open(tmp.path());
    let alpha = root(&svc, "Alpha", 1, Some(ALPHA));
    svc.grant_client("ask:local", &[alpha], &["path"], None)
        .unwrap();
    let list = ok(as_client(
        &svc,
        "ask:local",
        "search.query",
        search("outline", 1, Value::Null),
    ));
    assert_eq!(
        list["hits"][0]["path"]["text"],
        json!(r"report drafts\report outline.txt")
    );
    let node = list["hits"][0]["row"]["node_id"].clone();
    let client = ok(as_client(
        &svc,
        "ask:local",
        "node.inspect",
        json!({"node_id": node}),
    ));
    assert_eq!(
        client["display_path"]["text"],
        json!(r"Alpha\report drafts\report outline.txt")
    );
    for page in [&list, &client] {
        let text = page.to_string();
        assert!(
            !text.contains("Synthetic Parent") && !text.contains("Synthetic Owner"),
            "{text}"
        );
    }
    let owner = ok(as_owner(&svc, "node.inspect", json!({"node_id": node})));
    assert_eq!(
        owner["display_path"]["text"],
        json!(r"Synthetic Parent\Synthetic Owner\Alpha\report drafts\report outline.txt")
    );
}
