//! LW-067 / LW-069 canaries (docs/46 section 10, F3a): an ungranted client, one root versus a
//! multi-root index, excluded totals and fields, client-bound cursor reuse, and revocation
//! between list and explain. Synthetic names only.

use crate::tests::temp_root;
use crate::{Config, Service};
use loomward_catalog::{DirListing, Observation, RootObservation, WriteCommand, WriteReply};
use loomward_protocol::*;
use serde_json::{json, Value};

pub(crate) fn open(dir: &std::path::Path) -> Service {
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
pub(crate) fn root(svc: &Service, name: &str, id: u8, files: Option<&[(&str, u64)]>) -> RootId {
    let w = svc.inner.catalog.writer();
    let WriteReply::Root {
        grant_id,
        root_id,
        dir_id,
    } = w
        .call(WriteCommand::RegisterRoot(RootObservation {
            volume_guid: None,
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

pub(crate) const ALPHA: &[(&str, u64)] = &[
    ("report.pdf", 4000),
    ("annual report.docx", 3000),
    ("report-2024.xlsx", 2000),
    ("notes.txt", 1000),
];

#[test]
fn client_reference_stamp_binds_the_grant_epoch_and_catalogue_revision() {
    let tmp = temp_root();
    let svc = open(tmp.path());
    assert_eq!(
        svc.client_view_stamp("mcp:stamp").unwrap_err().code,
        ErrorCode::PermissionDenied
    );
    let alpha = root(&svc, "Alpha", 1, Some(ALPHA));
    svc.grant_client("mcp:stamp", std::slice::from_ref(&alpha), &[], None)
        .unwrap();
    let first = svc.client_view_stamp("mcp:stamp").unwrap();
    assert_eq!(first, svc.client_view_stamp("mcp:stamp").unwrap());
    svc.grant_client("mcp:stamp", &[alpha], &[], None).unwrap();
    let regranted = svc.client_view_stamp("mcp:stamp").unwrap();
    assert_ne!(first, regranted);
    root(&svc, "Beta", 2, Some(ALPHA));
    assert_ne!(regranted, svc.client_view_stamp("mcp:stamp").unwrap());
    svc.revoke_client("mcp:stamp").unwrap();
    assert_eq!(
        svc.client_view_stamp("mcp:stamp").unwrap_err().code,
        ErrorCode::PermissionDenied
    );
}

#[test]
fn client_host_never_accepts_a_new_root_grant() {
    let tmp = temp_root();
    let state = tmp.path().join("state");
    let error = Service::open_client(Config {
        state_dir: state.clone(),
        dataset: DatasetClass::Synthetic,
        allow_personal: false,
        allow_identity_unverified: false,
        grant_roots: vec![tmp.path().join("synthetic-lab")],
    })
    .err()
    .unwrap();
    assert_eq!(error, "a client host cannot grant roots");
    assert!(!state.exists());
}

#[cfg(windows)]
#[test]
fn client_host_never_opens_the_copy_recovery_store() {
    let tmp = temp_root();
    let regular = open(tmp.path());
    assert!(regular.inner.copies.store.is_some());
    drop(regular);
    let client = Service::open_client(Config {
        state_dir: tmp.path().into(),
        dataset: DatasetClass::Synthetic,
        allow_personal: false,
        allow_identity_unverified: false,
        grant_roots: vec![],
    })
    .unwrap();
    assert!(client.inner.copies.store.is_none());
}
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
fn as_desktop(svc: &Service, command: &str, payload: Value) -> ResponseEnvelope {
    static IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let ctx = CallContext {
        adapter: Adapter::Tauri,
        ..CallContext::http()
    };
    let mut req = request(command, payload);
    req.request_id = RequestId::new(format!(
        "r_clients_{}",
        IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
    .unwrap();
    svc.call(req, &ctx)
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

#[test]
fn owner_commands_manage_mcp_grants_and_preserve_epochs() {
    for dataset in [DatasetClass::Synthetic, DatasetClass::Personal] {
        let dir = temp_root();
        let svc = Service::open(Config {
            state_dir: dir.path().to_path_buf(),
            dataset,
            allow_personal: true,
            allow_identity_unverified: false,
            grant_roots: vec![],
        })
        .unwrap();
        let (row, _) = crate::tests::fixture(&svc, 0);
        let rt = RootId::new(svc.inner.root_id(row)).unwrap();
        svc.grant_client("ask:local", std::slice::from_ref(&rt), &["path"], None)
            .unwrap();
        let empty = ok(as_desktop(&svc, "clients.list", json!({})));
        assert_eq!(empty["grants"], json!([]));
        assert_eq!(empty["available_fields"], json!(crate::CLIENT_FIELDS));
        let payload = json!({"client": "mcp:synthetic-agent", "roots": [rt], "fields": ["size"]});
        for (command, p) in [
            ("clients.grant", payload.clone()),
            ("clients.list", json!({})),
            ("clients.revoke", json!({"client": "mcp:synthetic-agent"})),
        ] {
            assert_eq!(
                err(as_owner(&svc, command, p)).code,
                ErrorCode::CapabilityUnavailable
            );
        }
        let first = ok(as_desktop(&svc, "clients.grant", payload.clone()));
        let epoch = svc.inner.client_grant("mcp:synthetic-agent").unwrap().epoch;
        let second = ok(as_desktop(&svc, "clients.grant", payload.clone()));
        assert_ne!(first["grant_id"], second["grant_id"]);
        assert_ne!(
            epoch,
            svc.inner.client_grant("mcp:synthetic-agent").unwrap().epoch
        );
        let listed = ok(as_desktop(&svc, "clients.list", json!({})));
        let grants = listed["grants"].as_array().unwrap();
        assert_eq!(grants.len(), 2);
        assert!(grants[0]["revoked_at"].is_string());
        assert!(grants[1]["revoked_at"].is_null());
        assert_eq!(grants[1]["client"], payload["client"]);
        assert_eq!(grants[1]["roots"], payload["roots"]);
        assert_eq!(grants[1]["fields"], payload["fields"]);
        assert!(grants[1]["created_at"].is_string());
        assert!(grants[1]["expires_at"].is_null());
        for (command, p) in [
            ("clients.grant", payload.clone()),
            ("clients.list", json!({})),
            ("clients.revoke", json!({"client": "mcp:synthetic-agent"})),
        ] {
            let client = CallContext {
                adapter: Adapter::Tauri,
                ..CallContext::client("mcp:synthetic-agent")
            };
            assert_eq!(
                err(svc.call(request(command, p.clone()), &client)).code,
                ErrorCode::PermissionDenied
            );
            assert_eq!(
                err(as_client(&svc, "mcp:synthetic-agent", command, p)).code,
                ErrorCode::CapabilityUnavailable
            );
        }
        assert_eq!(
            ok(as_desktop(
                &svc,
                "clients.revoke",
                json!({"client": "mcp:synthetic-agent"})
            ))["revoked"],
            true
        );
        assert_eq!(
            err(as_client(
                &svc,
                "mcp:synthetic-agent",
                "search.query",
                search("report", 10, Value::Null)
            ))
            .code,
            ErrorCode::PermissionDenied
        );
        assert_eq!(
            ok(as_desktop(
                &svc,
                "clients.revoke",
                json!({"client": "mcp:synthetic-agent"})
            ))["revoked"],
            false
        );
        assert!(ok(as_desktop(&svc, "clients.list", json!({})))["grants"]
            .as_array()
            .unwrap()
            .iter()
            .all(|g| g["revoked_at"].is_string()));
        assert!(svc.inner.client_grant("ask:local").is_ok());
    }
}

#[test]
fn owner_commands_reject_invalid_scope_and_keep_the_live_grant() {
    let dir = temp_root();
    let svc = open(dir.path());
    let rt = root(&svc, "Synthetic scope", 92, None);
    let payload = json!({"client": "mcp:synthetic-agent", "roots": [rt], "fields": ["size"], "expires_at": "2099-01-01T00:00:00Z"});
    ok(as_desktop(&svc, "clients.grant", payload.clone()));
    let before = ok(as_desktop(&svc, "clients.list", json!({})));
    assert_eq!(
        before["grants"][0]["expires_at"],
        "2099-01-01T00:00:00.000Z"
    );
    for (key, value, code) in [
        ("client", json!("ask:local"), ErrorCode::InvalidRequest),
        ("client", json!("mcp:bad name"), ErrorCode::InvalidRequest),
        ("client", json!("mcp:"), ErrorCode::InvalidRequest),
        ("roots", json!(["rt_unknown"]), ErrorCode::NotFound),
        ("fields", json!(["secret"]), ErrorCode::InvalidRequest),
        (
            "expires_at",
            json!("9999-01-01T00:00:00Z"),
            ErrorCode::InvalidRequest,
        ),
    ] {
        let mut invalid = payload.clone();
        invalid[key] = value;
        assert_eq!(err(as_desktop(&svc, "clients.grant", invalid)).code, code);
    }
    assert_eq!(
        err(as_desktop(
            &svc,
            "clients.revoke",
            json!({"client": "ask:local"})
        ))
        .code,
        ErrorCode::InvalidRequest
    );
    assert_eq!(before, ok(as_desktop(&svc, "clients.list", json!({}))));
    let mut expired = payload;
    expired["expires_at"] = json!("2020-01-01T00:00:00Z");
    ok(as_desktop(&svc, "clients.grant", expired));
    assert_eq!(
        svc.inner
            .client_grant("mcp:synthetic-agent")
            .unwrap_err()
            .code,
        ErrorCode::PermissionDenied
    );
    assert_eq!(
        ok(as_desktop(&svc, "clients.list", json!({})))["grants"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
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
    // Owner metadata, grants and effects stay closed, even with a disclosure grant.
    svc.grant_client("mcp:granted", std::slice::from_ref(&alpha), &["path"], None)
        .unwrap();
    for (command, payload) in [
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

const ALL_FIELDS: [&str; 5] = ["path", "size", "modified", "attributes", "volumes"];

fn schema_validator(definition: &str) -> jsonschema::Validator {
    let mut schema: Value = serde_json::from_str(include_str!(
        "../../../contracts/v3/view-service.schema.json"
    ))
    .unwrap();
    schema.as_object_mut().unwrap().remove("anyOf");
    schema["$ref"] = json!(format!("#/$defs/{definition}"));
    jsonschema::draft202012::options()
        .should_validate_formats(true)
        .build(&schema)
        .unwrap()
}

fn check_client_reply_schema(command: Command) {
    let tmp = temp_root();
    let svc = open(tmp.path());
    let alpha = root(&svc, "Alpha", 1, Some(ALPHA));
    let page = ok(as_owner(
        &svc,
        "search.query",
        search("report", 10, Value::Null),
    ));
    let payload = match command {
        Command::SearchQuery => search("report", 10, Value::Null),
        Command::NodeInspect => json!({"node_id": page["hits"][0]["row"]["node_id"]}),
        _ => unreachable!(),
    };
    let owner = ok(as_owner(&svc, command.as_str(), payload.clone()));
    assert!(schema_validator(command.result_def()).is_valid(&owner));
    let commands: Value =
        serde_json::from_str(include_str!("../../../contracts/v3/commands.json")).unwrap();
    let validator = schema_validator(
        commands["commands"][command.as_str()]["client_result"]
            .as_str()
            .unwrap(),
    );
    let envelope_validator = schema_validator("ResponseEnvelope");
    for mask in 0..32 {
        let mask = if command == Command::NodeInspect {
            31 - mask
        } else {
            mask
        };
        let fields: Vec<_> = ALL_FIELDS
            .iter()
            .enumerate()
            .filter_map(|(i, field)| (mask & (1 << i) != 0).then_some(*field))
            .collect();
        svc.grant_client("ask:local", std::slice::from_ref(&alpha), &fields, None)
            .unwrap();
        let reply = as_client(&svc, "ask:local", command.as_str(), payload.clone());
        let envelope = serde_json::to_value(&reply).unwrap();
        assert!(envelope_validator.is_valid(&envelope));
        let result = ok(reply);
        let errors: Vec<_> = validator
            .iter_errors(&result)
            .map(|e| e.to_string())
            .collect();
        assert!(
            errors.is_empty(),
            "{} fields {fields:?}: {errors:?}",
            command.as_str()
        );
    }
}

#[test]
fn client_search_replies_match_the_declared_schema() {
    check_client_reply_schema(Command::SearchQuery);
}

#[test]
fn client_inspect_replies_match_the_declared_schema() {
    check_client_reply_schema(Command::NodeInspect);
}

/// Rescan the root recovered for an existing grant, without issuing a new owner grant.
fn rescan(svc: &Service, grant: i64, files: &[(&str, u64)]) -> RootId {
    let (root, dir): (i64, i64) = svc.inner.db().query_row(
        "SELECT r.id,d.id FROM main.root r JOIN main.dir d ON d.root_id=r.id WHERE r.grant_id=?1 AND d.parent_id IS NULL",
        [grant], |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    let w = svc.inner.catalog.writer();
    let WriteReply::Run(run) = w
        .call(WriteCommand::BeginRun {
            grant_id: grant,
            mode: "full".into(),
            strategy: "fixture".into(),
            started_at_ns: 0,
        })
        .unwrap()
    else {
        panic!()
    };
    w.call(WriteCommand::DirListing(DirListing {
        run_id: run,
        dir_id: dir,
        files: files.iter().map(|(n, b)| file(n, *b)).collect(),
        dirs: vec![],
        state: "complete".into(),
        skipped: 0,
        errors: 0,
    }))
    .unwrap();
    w.call(WriteCommand::EndRun {
        run_id: run,
        state: "completed".into(),
        finished_at_ns: 1,
    })
    .unwrap();
    RootId::new(svc.inner.root_id(root)).unwrap()
}

#[test]
fn client_grants_keep_their_roots_across_catalogue_recovery() {
    let tmp = temp_root();
    let svc = open(tmp.path());
    let alpha = root(&svc, "Alpha", 1, None);
    let beta = root(&svc, "Beta", 2, Some(BETA));
    root(&svc, "Gamma", 3, Some(&[("gamma secret.txt", 42)]));
    svc.grant_client("mcp:beta", &[beta], &ALL_FIELDS, None)
        .unwrap();
    ok(as_owner(
        &svc,
        "roots.revoke",
        json!({"root_id": alpha.as_ref(), "purge_catalog": false}),
    ));
    drop(svc);
    // Recovery retains state.db but rebuilds catalogue roots from only the active grants.
    std::fs::write(
        tmp.path().join("catalog.db"),
        b"synthetic corrupt catalogue",
    )
    .unwrap();
    let svc = open(tmp.path());
    let beta = rescan(&svc, 2, BETA);
    let gamma = rescan(&svc, 3, &[("gamma secret.txt", 42)]);
    assert_eq!(svc.inner.root_row(beta.as_ref()).unwrap(), 1);
    assert_eq!(svc.inner.root_row(gamma.as_ref()).unwrap(), 2);
    let query = |client| {
        ok(as_client(
            &svc,
            client,
            "search.query",
            search("secret", 100, Value::Null),
        ))
    };
    let page = query("mcp:beta");
    assert_eq!(names(&page), ["beta report secret.txt"]);
    assert_eq!(page["roots"].as_array().unwrap().len(), 1);
    assert_eq!(page["roots"][0]["root_id"], json!(beta.as_ref()));
    let node = page["hits"][0]["row"]["node_id"].clone();
    ok(as_client(
        &svc,
        "mcp:beta",
        "node.inspect",
        json!({"node_id": node}),
    ));
    let owner = ok(as_owner(
        &svc,
        "search.query",
        search("gamma secret", 10, Value::Null),
    ));
    let node = owner["hits"][0]["row"]["node_id"].clone();
    assert_eq!(
        err(as_client(
            &svc,
            "mcp:beta",
            "node.inspect",
            json!({"node_id": node})
        ))
        .code,
        ErrorCode::NotFound
    );
    // New grants also translate catalogue IDs to durable grants after the IDs have diverged.
    svc.grant_client("mcp:new", std::slice::from_ref(&beta), &ALL_FIELDS, None)
        .unwrap();
    assert_eq!(names(&query("mcp:new")), ["beta report secret.txt"]);
    ok(as_owner(
        &svc,
        "roots.revoke",
        json!({"root_id": beta.as_ref(), "purge_catalog": false}),
    ));
    assert!(names(&query("mcp:beta")).is_empty());
    assert!(names(&query("mcp:new")).is_empty());
}

#[test]
fn legacy_client_grants_are_invalidated_on_upgrade_even_after_catalogue_recovery() {
    for corrupt in [false, true] {
        let tmp = temp_root();
        let svc = open(tmp.path());
        let alpha = root(&svc, "Alpha", 1, None);
        root(&svc, "Beta", 2, Some(BETA));
        root(&svc, "Gamma", 3, Some(&[("gamma secret.txt", 42)]));
        ok(as_owner(
            &svc,
            "roots.revoke",
            json!({"root_id": alpha.as_ref(), "purge_catalog": false}),
        ));
        drop(svc);
        let db = rusqlite::Connection::open(tmp.path().join("state.db")).unwrap();
        // Recreate the pre-upgrade schema and its persisted catalogue root ID for Beta.
        let versioned: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('client_grant') WHERE name='durable_roots')", [], |r| r.get(0)).unwrap();
        if versioned {
            db.execute_batch("ALTER TABLE client_grant DROP COLUMN durable_roots")
                .unwrap();
        }
        db.execute("INSERT INTO client_grant(client,roots_json,fields_json,created_at_ns) VALUES('mcp:legacy','[2]','[]',0)", []).unwrap();
        drop(db);
        if corrupt {
            std::fs::write(
                tmp.path().join("catalog.db"),
                b"synthetic corrupt catalogue",
            )
            .unwrap();
        }
        let svc = open(tmp.path());
        rescan(&svc, 2, BETA);
        rescan(&svc, 3, &[("gamma secret.txt", 42)]);
        let refused = err(as_client(
            &svc,
            "mcp:legacy",
            "search.query",
            search("secret", 10, Value::Null),
        ));
        assert_eq!(
            refused.code,
            ErrorCode::PermissionDenied,
            "corrupt={corrupt}"
        );
        let revoked: bool = svc
            .inner
            .db()
            .query_row(
                "SELECT revoked_at_ns IS NOT NULL FROM st.client_grant WHERE client='mcp:legacy'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(revoked);
        // The invalidation and new format survive a second startup.
        drop(svc);
        let svc = open(tmp.path());
        assert_eq!(
            svc.inner.client_grant("mcp:legacy").unwrap_err().code,
            ErrorCode::PermissionDenied
        );
    }
}

#[test]
fn an_owner_revoked_root_leaves_a_multi_root_grant_and_cannot_be_granted() {
    let tmp = temp_root();
    let svc = open(tmp.path());
    let alpha = root(&svc, "Alpha", 1, Some(ALPHA));
    let beta = root(&svc, "Beta", 2, Some(BETA));
    svc.grant_client(
        "ask:local",
        &[alpha.clone(), beta.clone()],
        &ALL_FIELDS,
        None,
    )
    .unwrap();
    let query = |text: &str| {
        ok(as_client(
            &svc,
            "ask:local",
            "search.query",
            search(text, 100, Value::Null),
        ))
    };
    assert_eq!(query("report")["roots"].as_array().unwrap().len(), 2);
    let beta_node = query("secret")["hits"][0]["row"]["node_id"].clone();
    let revoked = as_owner(
        &svc,
        "roots.revoke",
        json!({"root_id": beta.as_ref(), "purge_catalog": false}),
    );
    assert!(matches!(revoked, ResponseEnvelope::Ok(_)), "{revoked:?}");
    // The client keeps its grant but sees only the still-active root, and nothing of Beta.
    let page = query("report");
    assert_eq!(
        page["roots"],
        json!([{"root_id": alpha.as_ref(), "scan_state": "complete", "coverage": "complete"}])
    );
    assert!(!names(&page).iter().any(|n| n.contains("beta")));
    for hit in page["hits"].as_array().unwrap() {
        assert_eq!(hit["root_id"], json!(alpha.as_ref()));
    }
    // The narrowing itself: the live grant lists the still-active root only. The catalogue also
    // hides a revoked root's rows, so this seam is checked where it is made.
    let live = svc.inner.client_grant("ask:local").unwrap();
    assert_eq!(live.roots, [svc.inner.root_row(alpha.as_ref()).unwrap()]);
    // Naming the revoked root, or explaining a node it held, reads as unknown.
    let mut named = search("report", 10, Value::Null);
    named["root_id"] = json!(beta.as_ref());
    let e = err(as_client(&svc, "ask:local", "search.query", named));
    assert_eq!(e.code, ErrorCode::NotFound);
    let inspect = json!({"node_id": beta_node});
    let e = err(as_client(&svc, "ask:local", "node.inspect", inspect));
    assert_eq!(e.code, ErrorCode::NotFound);
    // A revoked root, alone or among live ones, cannot be granted, and a refused grant replaces
    // nothing; an unknown root reads the same.
    let mut forged = beta.as_ref().to_owned();
    forged.push('0');
    let forged = RootId::new(forged).unwrap();
    let held = query("notes");
    for roots in [
        vec![beta.clone()],
        vec![alpha.clone(), beta.clone()],
        vec![forged],
    ] {
        let e = svc
            .grant_client("ask:local", &roots, &ALL_FIELDS, None)
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::NotFound);
        assert_eq!(e.message.as_str(), "unknown root");
        let e = svc
            .grant_client("mcp:other", &roots, &["path"], None)
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::NotFound);
    }
    assert_eq!(unsealed(query("notes")), unsealed(held));
    let other = as_client(
        &svc,
        "mcp:other",
        "search.query",
        search("notes", 5, Value::Null),
    );
    assert_eq!(err(other).code, ErrorCode::PermissionDenied);
}

/// The explain of a size-only grant carries the row's size fields and nothing else the grant
/// does not name; with every field it is the owner's row, root and path.
#[test]
fn an_inspect_projects_the_row_to_the_granted_fields() {
    let tmp = temp_root();
    let svc = open(tmp.path());
    let alpha = root(&svc, "Alpha", 1, Some(ALPHA));
    svc.grant_client("mcp:sizes", std::slice::from_ref(&alpha), &["size"], None)
        .unwrap();
    svc.grant_client("mcp:all", std::slice::from_ref(&alpha), &ALL_FIELDS, None)
        .unwrap();
    let owner = ok(as_owner(
        &svc,
        "search.query",
        search("report.pdf", 1, Value::Null),
    ));
    let node = owner["hits"][0]["row"]["node_id"].clone();
    let inspect = |client: &str| {
        ok(as_client(
            &svc,
            client,
            "node.inspect",
            json!({"node_id": node}),
        ))
    };
    let owner_row = ok(as_owner(&svc, "node.inspect", json!({"node_id": node})))["row"].clone();
    for k in ["modified_at", "attributes", "flags"] {
        assert!(owner_row.get(k).is_some(), "the fixture has {k}");
    }
    let sizes = inspect("mcp:sizes");
    let row = sizes["row"].as_object().unwrap();
    for k in ["location_hint", "modified_at", "attributes", "flags"] {
        assert!(!row.contains_key(k), "{k}");
    }
    assert_eq!(row["logical_bytes"], owner_row["logical_bytes"]);
    assert!(sizes.get("display_path").is_none());
    let all = inspect("mcp:all");
    assert_eq!(all["row"], owner_row);
    assert!(all["display_path"]["text"].is_string());
}

/// A directory resolves to its root for the scope check, as a file does.
#[test]
fn an_inspect_of_a_directory_is_scoped_by_its_root() {
    let tmp = temp_root();
    let svc = open(tmp.path());
    let alpha = root(&svc, "Alpha", 1, Some(ALPHA));
    let beta = root(&svc, "Beta", 2, Some(BETA));
    svc.grant_client("ask:local", std::slice::from_ref(&alpha), &ALL_FIELDS, None)
        .unwrap();
    let mut dirs = search("report drafts", 10, Value::Null);
    dirs["kind"] = json!("dir");
    let owner = ok(as_owner(&svc, "search.query", dirs));
    let dir_of = |root: &RootId| {
        let hit = owner["hits"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["root_id"] == json!(root.as_ref()))
            .unwrap();
        assert_eq!(hit["row"]["kind"], json!("dir"));
        hit["row"]["node_id"].clone()
    };
    let (mine, theirs) = (dir_of(&alpha), dir_of(&beta));
    let inspect = |node: &Value, generation: Option<u64>| {
        let mut r = request("node.inspect", json!({"node_id": node}));
        r.expected_generation = generation.map(Generation);
        svc.call(r, &CallContext::client("ask:local"))
    };
    let seen = ok(inspect(&mine, None));
    assert_eq!(seen["root_id"], json!(alpha.as_ref()));
    assert_eq!(seen["row"]["kind"], json!("dir"));
    // Another root's directory is an unknown node, before a generation check could tell it apart.
    let forged = {
        let mut s = theirs.as_str().unwrap().to_owned();
        s.replace_range(s.len() - 2.., "00");
        json!(s)
    };
    for generation in [None, Some(999)] {
        let (other, unknown) = (
            err(inspect(&theirs, generation)),
            err(inspect(&forged, generation)),
        );
        assert_eq!(other.code, ErrorCode::NotFound, "{generation:?}");
        assert_eq!(
            (other.message, other.detail),
            (unknown.message, unknown.detail)
        );
    }
}
fn listing(text: &str) -> Value {
    let mut p = search(text, 10, Value::Null);
    p["extension"] = json!("pdf");
    p
}

/// A text that cleans to nothing is the same extension-only, largest-first listing as "": the
/// size guard must read it the way the catalogue does (trimmed, control characters dropped).
#[test]
fn a_text_that_cleans_to_nothing_is_still_a_listing_that_needs_size() {
    let tmp = temp_root();
    let svc = open(tmp.path());
    let alpha = root(&svc, "Alpha", 1, Some(ALPHA));
    svc.grant_client("mcp:names", std::slice::from_ref(&alpha), &["path"], None)
        .unwrap();
    svc.grant_client("mcp:sizes", std::slice::from_ref(&alpha), &["size"], None)
        .unwrap();
    let want = names(&ok(as_owner(&svc, "search.query", listing(""))));
    assert_eq!(want, ["report.pdf"]);
    for text in [
        "   ",
        "\t\n",
        "\u{1}",
        " \u{1} ",
        "\u{7}\u{7f}",
        "\u{85}\u{1f}",
    ] {
        let e = err(as_client(&svc, "mcp:names", "search.query", listing(text)));
        assert_eq!(e.code, ErrorCode::InvalidRequest, "{text:?}");
        // With `size` the same text is the owner's listing: the catalogue agrees it is empty.
        let page = ok(as_client(&svc, "mcp:sizes", "search.query", listing(text)));
        assert_eq!(names(&page), want, "{text:?}");
        let owner = ok(as_owner(&svc, "search.query", listing(text)));
        assert_eq!(names(&owner), want, "{text:?}");
    }
    // A zero-width space survives the catalogue cleaning, so it is a text search, never a
    // largest-first listing (the second half of the Muse claim).
    let page = ok(as_client(
        &svc,
        "mcp:names",
        "search.query",
        listing("\u{200b}"),
    ));
    assert_eq!(names(&page), Vec::<String>::new());
}

fn root_node(svc: &Service, root: &RootId) -> Value {
    let slice = ok(as_owner(
        svc,
        "tree.slice",
        json!({
            "anchor": {"kind": "root", "root_id": root.as_ref()},
            "depth": 1, "max_nodes": 20, "min_share": 0, "basis": "logical", "include_files": true
        }),
    ));
    slice["anchor_node_id"].clone()
}

fn space_reads(root: &RootId, node: &Value) -> [(&'static str, Value); 5] {
    [
        ("volumes.list", json!({})),
        ("roots.list", json!({})),
        (
            "tree.children",
            json!({"node_id": node, "sort": "size_desc", "basis": "logical", "limit": 200, "cursor": null}),
        ),
        (
            "stats.breakdown",
            json!({"node_id": node, "by": "extension", "basis": "logical", "limit": 64}),
        ),
        (
            "clutter.developer",
            json!({"root_id": root.as_ref(), "limit": 200}),
        ),
    ]
}

#[test]
fn client_space_reads_require_live_grants_and_their_specific_fields() {
    let tmp = temp_root();
    let svc = open(tmp.path());
    let alpha = root(&svc, "Alpha", 1, Some(ALPHA));
    let node = root_node(&svc, &alpha);
    for (command, payload) in space_reads(&alpha, &node) {
        let validator = schema_validator(
            Command::from_name(command)
                .unwrap()
                .client_result_def()
                .unwrap(),
        );
        let needed = match command {
            "volumes.list" => Some("volumes"),
            "roots.list" => None,
            _ => Some("size"),
        };
        for client in ["mcp:space", "ask:local"] {
            let e = err(as_client(&svc, client, command, payload.clone()));
            assert_eq!(e.code, ErrorCode::PermissionDenied, "{command}");
            assert_eq!(e.message.as_str(), "this client has no disclosure grant");
            // Every other field, including size, cannot stand in for volumes consent.
            let other: Vec<_> = ["path", "size", "modified", "attributes", "volumes"]
                .into_iter()
                .filter(|f| Some(*f) != needed)
                .collect();
            svc.grant_client(client, std::slice::from_ref(&alpha), &other, None)
                .unwrap();
            if let Some(field) = needed {
                let e = err(as_client(&svc, client, command, payload.clone()));
                assert_eq!(e.code, ErrorCode::PermissionDenied, "{command}");
                assert!(
                    e.message
                        .as_str()
                        .contains(if field == "size" { "sizes" } else { field }),
                    "{e:?}"
                );
            }
            let fields: Vec<_> = needed.into_iter().collect();
            svc.grant_client(client, std::slice::from_ref(&alpha), &fields, None)
                .unwrap();
            let result = ok(as_client(&svc, client, command, payload.clone()));
            let errors: Vec<_> = validator
                .iter_errors(&result)
                .map(|e| e.to_string())
                .collect();
            assert!(errors.is_empty(), "{command}: {errors:?}");
            assert!(svc.revoke_client(client).unwrap());
            let e = err(as_client(&svc, client, command, payload.clone()));
            assert_eq!(e.code, ErrorCode::PermissionDenied, "{command}");
            assert_eq!(e.message.as_str(), "this client has no disclosure grant");
        }
    }
}

#[test]
fn client_space_nodes_and_roots_outside_the_grant_are_unknown_before_generation() {
    let tmp = temp_root();
    let svc = open(tmp.path());
    let alpha = root(&svc, "Alpha", 1, Some(ALPHA));
    let beta = root(&svc, "Beta", 2, Some(BETA));
    svc.grant_client("mcp:space", &[alpha], &["size"], None)
        .unwrap();
    let node = root_node(&svc, &beta);
    for (command, payload) in space_reads(&beta, &node).into_iter().skip(2) {
        for generation in [None, Some(999)] {
            if command == "clutter.developer" && generation.is_some() {
                continue; // This command does not accept expected_generation in its envelope.
            }
            let mut req = request(command, payload.clone());
            req.expected_generation = generation.map(Generation);
            let e = err(svc.call(req, &CallContext::client("mcp:space")));
            assert_eq!(e.code, ErrorCode::NotFound, "{command}: {e:?}");
            assert_eq!(
                e.message.as_str(),
                if command == "clutter.developer" {
                    "unknown root"
                } else {
                    "unknown node"
                }
            );
            assert!(e.detail.is_none());
        }
    }
}

#[test]
fn client_roots_are_absent_outside_scope_and_paths_start_at_the_root_name() {
    let tmp = temp_root();
    let svc = open(tmp.path());
    let alpha = root(&svc, "Alpha", 1, Some(ALPHA));
    root(&svc, "Beta", 2, Some(BETA));
    for fields in [&[][..], &["path"][..], &["size"][..]] {
        svc.grant_client("mcp:space", std::slice::from_ref(&alpha), fields, None)
            .unwrap();
        let page = ok(as_client(&svc, "mcp:space", "roots.list", json!({})));
        let roots = page["roots"].as_array().unwrap();
        assert_eq!(roots.len(), 1);
        let row = &roots[0];
        assert_eq!(row["root_id"], alpha.as_ref());
        for key in [
            "origin",
            "dataset_class",
            "volume_id",
            "granted_at",
            "granted_via",
            "grant_state",
            "totals",
        ] {
            assert!(row.get(key).is_none(), "{key}: {row}");
        }
        assert!(row["scan"].get("last_job_id").is_none());
        assert_eq!(row["scan"]["coverage"], "complete");
        if fields.contains(&"path") {
            assert_eq!(
                row["display_path"],
                json!({"text": "Alpha", "truncated": false})
            );
        } else {
            assert!(row.get("display_path").is_none());
        }
        assert!(!page.to_string().contains("Synthetic Owner"));
        assert!(!page.to_string().contains("Beta"));
    }
}

#[test]
fn client_children_and_breakdowns_count_only_the_granted_subtree_and_project_rows() {
    let tmp = temp_root();
    let svc = open(tmp.path());
    let alpha = root(&svc, "Alpha", 1, Some(ALPHA));
    root(&svc, "Beta", 2, Some(BETA));
    let node = root_node(&svc, &alpha);
    svc.grant_client("mcp:space", std::slice::from_ref(&alpha), &["size"], None)
        .unwrap();
    for (command, payload) in space_reads(&alpha, &node).into_iter().skip(2).take(2) {
        let owner = ok(as_owner(&svc, command, payload.clone()));
        let page = ok(as_client(&svc, "mcp:space", command, payload));
        if command == "tree.children" {
            assert_eq!(page["total"], owner["total"]);
            let items = page["items"].as_array().unwrap();
            assert_eq!(items.len(), 5);
            let total: u64 = items
                .iter()
                .map(|r| r["logical_bytes"].as_str().unwrap().parse::<u64>().unwrap())
                .sum();
            assert_eq!(total, 10_007);
            for row in items {
                for key in ["location_hint", "modified_at", "attributes", "flags"] {
                    assert!(row.get(key).is_none(), "{key}: {row}");
                }
            }
        } else {
            assert_eq!(page, owner);
            let total: u64 = page["buckets"]
                .as_array()
                .unwrap()
                .iter()
                .map(|b| b["bytes"].as_str().unwrap().parse::<u64>().unwrap())
                .sum();
            assert_eq!(total, 10_007);
        }
    }
    svc.grant_client(
        "mcp:space",
        std::slice::from_ref(&alpha),
        &["volumes"],
        None,
    )
    .unwrap();
    assert_eq!(
        ok(as_client(&svc, "mcp:space", "volumes.list", json!({}))),
        ok(as_owner(&svc, "volumes.list", json!({})))
    );
}

fn clutter_root(svc: &Service, name: &str, id: u8, bytes: u64) -> RootId {
    let root = root(svc, name, id, None);
    let (grant, dir): (i64, i64) = svc.inner.db().query_row(
        "SELECT r.grant_id,d.id FROM main.root r JOIN main.dir d ON d.root_id=r.id WHERE r.id=?1 AND d.parent_id IS NULL",
        [svc.inner.root_row(root.as_ref()).unwrap()], |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    let w = svc.inner.catalog.writer();
    let WriteReply::Run(run) = w
        .call(WriteCommand::BeginRun {
            grant_id: grant,
            mode: "full".into(),
            strategy: "fixture".into(),
            started_at_ns: 0,
        })
        .unwrap()
    else {
        panic!()
    };
    w.call(WriteCommand::DirListing(DirListing {
        run_id: run,
        dir_id: dir,
        files: vec![file("Cargo.toml", 10)],
        dirs: vec![file("target", 0)],
        state: "complete".into(),
        skipped: 0,
        errors: 0,
    }))
    .unwrap();
    let sub: i64 = svc.inner.db().query_row(
        "SELECT id FROM main.bulk_dir WHERE parent_id=?1 UNION ALL SELECT id FROM main.dir WHERE parent_id=?1",
        [dir], |r| r.get(0),
    ).unwrap();
    w.call(WriteCommand::DirListing(DirListing {
        run_id: run,
        dir_id: sub,
        files: vec![file("output.bin", bytes)],
        dirs: vec![],
        state: "complete".into(),
        skipped: 0,
        errors: 0,
    }))
    .unwrap();
    w.call(WriteCommand::EndRun {
        run_id: run,
        state: "completed".into(),
        finished_at_ns: 1,
    })
    .unwrap();
    root
}

#[test]
fn client_clutter_projects_only_candidates_under_the_granted_root() {
    let tmp = temp_root();
    let svc = open(tmp.path());
    let alpha = clutter_root(&svc, "Alpha", 1, 123);
    clutter_root(&svc, "Beta", 2, 999);
    for fields in [&["size"][..], &["size", "path"][..]] {
        svc.grant_client("mcp:space", std::slice::from_ref(&alpha), fields, None)
            .unwrap();
        let payload = json!({"root_id": alpha.as_ref(), "limit": 200});
        let page = ok(as_client(
            &svc,
            "mcp:space",
            "clutter.developer",
            payload.clone(),
        ));
        assert_eq!(page, ok(as_owner(&svc, "clutter.developer", payload)));
        let candidates = page["candidates"].as_array().unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0]["logical_bytes"], "123");
        // The current clutter DTO has names and references, but no path fields to disclose.
        for key in ["path", "display_path", "location_hint"] {
            assert!(candidates[0].get(key).is_none());
        }
        assert!(!page.to_string().contains("Beta"));
    }
}

#[test]
fn client_children_cursors_bind_the_client_and_grant_epoch() {
    let tmp = temp_root();
    let svc = open(tmp.path());
    let alpha = root(&svc, "Alpha", 1, Some(ALPHA));
    let node = root_node(&svc, &alpha);
    for client in ["mcp:space", "ask:local"] {
        svc.grant_client(client, std::slice::from_ref(&alpha), &["size"], None)
            .unwrap();
    }
    let mut payload = space_reads(&alpha, &node)[2].1.clone();
    payload["limit"] = json!(1);
    let first = ok(as_client(
        &svc,
        "mcp:space",
        "tree.children",
        payload.clone(),
    ));
    assert!(first["next_cursor"].is_string());
    payload["cursor"] = first["next_cursor"].clone();
    let next = ok(as_client(
        &svc,
        "mcp:space",
        "tree.children",
        payload.clone(),
    ));
    assert_ne!(first["items"][0]["node_id"], next["items"][0]["node_id"]);
    assert_eq!(
        err(as_client(
            &svc,
            "ask:local",
            "tree.children",
            payload.clone()
        ))
        .code,
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        err(as_owner(&svc, "tree.children", payload.clone())).code,
        ErrorCode::InvalidRequest
    );
    svc.grant_client("mcp:space", &[alpha], &["size"], None)
        .unwrap();
    assert_eq!(
        err(as_client(&svc, "mcp:space", "tree.children", payload)).code,
        ErrorCode::InvalidRequest
    );
}

#[test]
fn client_age_band_breakdowns_need_modified_times() {
    let tmp = temp_root();
    let svc = open(tmp.path());
    let alpha = root(&svc, "Alpha", 1, Some(ALPHA));
    let node = root_node(&svc, &alpha);
    let payload = json!({"node_id": node, "by": "age_band", "basis": "logical", "limit": 64});
    svc.grant_client("mcp:space", std::slice::from_ref(&alpha), &["size"], None)
        .unwrap();
    let e = err(as_client(
        &svc,
        "mcp:space",
        "stats.breakdown",
        payload.clone(),
    ));
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    assert!(e.message.as_str().contains("modified"), "{e:?}");
    svc.grant_client(
        "mcp:space",
        std::slice::from_ref(&alpha),
        &["size", "modified"],
        None,
    )
    .unwrap();
    ok(as_client(&svc, "mcp:space", "stats.breakdown", payload));
}

#[test]
fn client_children_sorted_by_modified_time_need_modified_times() {
    let tmp = temp_root();
    let svc = open(tmp.path());
    let alpha = root(&svc, "Alpha", 1, Some(ALPHA));
    let node = root_node(&svc, &alpha);
    let payload = json!({"node_id": node, "sort": "modified_desc", "basis": "logical", "limit": 200, "cursor": null});
    svc.grant_client("mcp:space", std::slice::from_ref(&alpha), &["size"], None)
        .unwrap();
    let e = err(as_client(
        &svc,
        "mcp:space",
        "tree.children",
        payload.clone(),
    ));
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    assert!(e.message.as_str().contains("modified"), "{e:?}");
    svc.grant_client(
        "mcp:space",
        std::slice::from_ref(&alpha),
        &["size", "modified"],
        None,
    )
    .unwrap();
    let page = ok(as_client(&svc, "mcp:space", "tree.children", payload));
    // With the field granted, rows keep it (a projector that always stripped would fail here).
    assert!(
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row.get("modified_at").is_some()),
        "{page}"
    );
}

#[test]
fn client_volume_mount_points_need_path() {
    let mut map = serde_json::Map::new();
    map.insert(
        "volumes".into(),
        json!([{"volume_id": "vo_1", "mount_points": ["X:/", "X:/Mounted/Folder"]}]),
    );
    let grant = |fields: &[&str]| crate::clients::ClientGrant {
        client: "mcp:space".into(),
        epoch: 1,
        roots: vec![],
        fields: fields.iter().map(|f| f.to_string()).collect(),
    };
    let mut without = map.clone();
    crate::clients::project(Command::VolumesList, &grant(&["volumes"]), &mut without);
    assert_eq!(without["volumes"][0]["mount_points"], json!([]));
    let mut with = map.clone();
    crate::clients::project(
        Command::VolumesList,
        &grant(&["volumes", "path"]),
        &mut with,
    );
    assert_eq!(
        with["volumes"][0]["mount_points"].as_array().unwrap().len(),
        2
    );
}

#[cfg(windows)]
#[test]
fn verification_open_grants_roots_without_background_work() {
    if loomward_windows::enumerate::skip_elevated_test() {
        return;
    }
    let temp = tempfile::tempdir_in(crate::paths::test_roots_base()).unwrap();
    let root = temp.path().join("synthetic-fixture");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("fixture"), b"synthetic").unwrap();
    let svc = Service::open_for_verification(Config {
        state_dir: temp.path().join("state"),
        dataset: DatasetClass::Personal,
        allow_personal: true,
        allow_identity_unverified: false,
        grant_roots: vec![root],
    })
    .unwrap();
    assert!(svc.inner.copies.store.is_none());
    assert!(!svc.inner.freshness.is_started());
    assert!(svc.inner.freshness.automatic_jobs().is_empty());
    let conn = svc.inner.db();
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM st.root_grant WHERE state='active'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM main.file", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

/// #903: `copies.store.is_none()` holds for any Personal open, so the claims are pinned against
/// `Service::open` instead. A Synthetic dataset opens the copy journal (and runs recovery) only
/// when not client-only; the telemetry pump and the backup-scrub retry each keep one `Weak` to the
/// service state for the life of their thread, and the automatic-scan machinery keeps more.
#[cfg(windows)]
#[test]
fn verification_open_starts_no_telemetry_copy_recovery_or_backup_threads() {
    let config = |dir: &std::path::Path| Config {
        state_dir: dir.to_path_buf(),
        dataset: DatasetClass::Synthetic,
        allow_personal: false,
        allow_identity_unverified: false,
        grant_roots: vec![],
    };
    let temp = tempfile::tempdir_in(crate::paths::test_roots_base()).unwrap();
    let open = Service::open(config(&temp.path().join("open"))).unwrap();
    let verify = Service::open_for_verification(config(&temp.path().join("verify"))).unwrap();
    let client = Service::open_client(config(&temp.path().join("client"))).unwrap();
    // The control: a normal open does run copy recovery for a Synthetic dataset.
    assert!(open.inner.copies.store.is_some());
    assert!(verify.inner.copies.store.is_none());
    let weak = |svc: &Service| std::sync::Arc::weak_count(&svc.inner);
    // Telemetry pump and backup-scrub retry: two threads, each holding one Weak.
    assert!(
        weak(&open) >= weak(&verify) + 2,
        "open {} verify {}",
        weak(&open),
        weak(&verify)
    );
    assert_eq!(weak(&verify), weak(&client), "same footprint as a client host");
}

#[test]
fn verification_open_names_no_flag_verify_tree_does_not_have() {
    use crate::{grant_refusal, Refused};
    let flagged = grant_refusal(1, &Refused::IdentityUnverified, true);
    assert!(flagged.contains("--allow-identity-unverified"), "{flagged}");
    let plain = grant_refusal(2, &Refused::IdentityUnverified, false);
    assert!(
        plain.starts_with("grant root #2 refused: identity_unverified"),
        "{plain}"
    );
    assert!(!plain.contains("--allow-identity-unverified"), "{plain}");
    assert!(plain.contains("reads no content"), "{plain}");
}

#[test]
fn clients_never_scanned_root_does_not_claim_an_empty_known_path() {
    let tmp = temp_root();
    let svc = open(tmp.path());
    let alpha = root(&svc, "Unscanned", 71, None);
    svc.grant_client("mcp:unscanned", &[alpha], &["path"], None)
        .unwrap();
    let page = ok(as_client(&svc, "mcp:unscanned", "roots.list", json!({})));
    let root = &page["roots"][0];
    assert!(
        root.get("display_path")
            .is_none_or(|p| p["text"].as_str().is_some_and(|s| !s.is_empty())),
        "{root}"
    );
}

#[test]
fn clients_held_references_reopen_reseal_and_recheck_scope_and_epoch() {
    let tmp = temp_root();
    let svc = open(tmp.path());
    let alpha = root(&svc, "Alpha", 81, Some(ALPHA));
    let beta = root(&svc, "Beta", 82, Some(BETA));
    svc.grant_client(
        "mcp:held",
        std::slice::from_ref(&alpha),
        &["size", "volumes"],
        None,
    )
    .unwrap();
    let node = root_node(&svc, &alpha);
    let foreign = root_node(&svc, &beta);
    assert_eq!(
        svc.client_reference("mcp:held", foreign.as_str().unwrap())
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        svc.client_reference("mcp:held", beta.as_ref())
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    let held = svc
        .client_reference("mcp:held", node.as_str().unwrap())
        .unwrap();
    let root_held = svc.client_reference("mcp:held", alpha.as_ref()).unwrap();
    drop(svc);
    let reopened = Service::open_client(Config {
        state_dir: tmp.path().into(),
        dataset: DatasetClass::Synthetic,
        allow_personal: false,
        allow_identity_unverified: false,
        grant_roots: vec![],
    })
    .unwrap();
    let held = reopened.resume_client_reference("mcp:held", &held).unwrap();
    let root_held = reopened
        .resume_client_reference("mcp:held", &root_held)
        .unwrap();
    let ids = reopened
        .resolve_client_reference("mcp:held", &held)
        .unwrap();
    assert_ne!(ids.node_id.as_deref(), node.as_str());
    let described = ok(as_client(
        &reopened,
        "mcp:held",
        "node.inspect",
        json!({"node_id":ids.node_id}),
    ));
    assert_eq!(described["row"]["name"], "Alpha");
    assert!(reopened
        .resolve_client_reference("mcp:held", &root_held)
        .unwrap()
        .node_id
        .is_some());
    reopened.revoke_client("mcp:held").unwrap();
    assert!(
        matches!(reopened.resolve_client_reference("mcp:held",&held),Err(e) if e.code == ErrorCode::PermissionDenied)
    );
}

#[test]
fn clients_empty_root_path_is_absent_and_archive_fields_are_service_refusals() {
    let grant = crate::clients::ClientGrant {
        client: "mcp:projection".into(),
        epoch: 1,
        roots: vec![1],
        fields: vec!["path".into()],
    };
    let mut value = json!({"roots":[{"root_id":"rt_1","display_path":{"text":"","truncated":false},"scan":{"state":"never_scanned"}}]}).as_object().unwrap().clone();
    crate::clients::project(Command::RootsList, &grant, &mut value);
    assert!(value["roots"][0].get("display_path").is_none());
    let tmp = temp_root();
    let svc = open(tmp.path());
    let alpha = root(&svc, "Alpha", 83, Some(ALPHA));
    for (fields, message) in [
        (&[][..], "this grant does not disclose sizes"),
        (&["size"][..], "this grant does not disclose volumes"),
        (&["size", "volumes"][..], ""),
    ] {
        svc.grant_client("mcp:fields", std::slice::from_ref(&alpha), fields, None)
            .unwrap();
        let result = svc.client_archive_fields("mcp:fields");
        if message.is_empty() {
            result.unwrap();
        } else {
            assert_eq!(&*result.unwrap_err().message, message);
        }
    }
}
