//! docs/47 lanes C4 and C5: personal sessions reach no copy effect; every copy command answers them
//! `capability_unavailable` (`copy_effect_gated`), and a personal approval over HTTP is refused by its
//! own rule (`copy_personal_approval_http`, G6). Synthetic sessions are served on registered lab roots
//! (C5). Session effect capabilities stay all false: the contract cannot say "synthetic only".
use super::*;
use std::path::PathBuf;

struct NoDialogs;
impl NativeDialogs for NoDialogs {
    fn pick_folder(&self) -> Option<PathBuf> {
        panic!("a copy command opened the folder picker")
    }
    fn confirm_disclosure(&self, _: &DisclosureSummary) -> Result<bool, ErrorBody> {
        panic!("a copy command opened the disclosure dialog")
    }
}

fn example(command: Command) -> Value {
    let path = format!(
        "{}/../../contracts/v3/examples/commands/{}.request.json",
        env!("CARGO_MANIFEST_DIR"),
        command.as_str()
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn personal_copy_is_gated_on_every_adapter_and_synthetic_is_served() {
    let copy: Vec<_> = Command::ALL
        .iter()
        .copied()
        .filter(|c| c.as_str().starts_with("copy."))
        .collect();
    assert_eq!(copy.len(), 6);
    for dataset in [DatasetClass::Synthetic, DatasetClass::Personal] {
        let tmp = temp_root();
        let svc = open(tmp.path(), dataset);
        let tauri = CallContext::tauri(Arc::new(NoDialogs));
        for ctx in [CallContext::http(), tauri] {
            let (hello, _) = ok(
                "session.hello",
                svc.call(request("session.hello", json!({})), &ctx),
            );
            let effects = hello["capabilities"]["effects"].as_object().unwrap();
            assert!(effects.values().all(|v| v == &json!(false)), "{effects:?}");
            for &c in &copy {
                let e = err(svc.call(request(c.as_str(), example(c)), &ctx));
                if dataset == DatasetClass::Synthetic {
                    // Served: the example node and operation IDs are simply unknown here.
                    if cfg!(windows) {
                        assert_ne!(reason(&e), Some("copy_effect_gated"), "{}", c.as_str());
                        assert_ne!(e.code, ErrorCode::CapabilityUnavailable, "{}", c.as_str());
                    }
                    continue;
                }
                assert_eq!(e.code, ErrorCode::CapabilityUnavailable, "{}", c.as_str());
                assert!(!e.retryable);
                let personal_http =
                    c == Command::CopyApprove && ctx.adapter == loomward_protocol::Adapter::Http;
                let want = if personal_http {
                    "copy_personal_approval_http"
                } else {
                    "copy_effect_gated"
                };
                assert_eq!(reason(&e), Some(want), "{} {dataset:?}", c.as_str());
            }
        }
    }
}

#[cfg(windows)]
mod lab_copy {
    use super::*;

    fn digest(v: &Payload) -> String {
        v["digest"].as_str().unwrap().to_owned()
    }

    fn settle_copy(svc: &Service, op: &str) -> Payload {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let (s, _) = ok(
                "copy.status",
                call(svc, "copy.status", json!({"operation_id": op})),
            );
            if matches!(
                s["state"].as_str().unwrap(),
                "committed" | "cancelled" | "needs_attention" | "blocked" | "refused"
            ) {
                return s;
            }
            assert!(Instant::now() < deadline, "{s:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Scan the lab root and return the lab with the nodes of its folders `a` and `b`.
    fn scanned() -> Option<(Lab, String, String)> {
        let lab = lab()?;
        let job = start(&lab.svc, &lab.root, "full");
        assert_eq!(settle(&lab.svc, &job)["state"], "completed");
        let slice = lab_slice(&lab.svc, &lab.root);
        let a = child_named(&slice, "a").unwrap();
        let b = child_named(&slice, "b").unwrap();
        Some((lab, a, b))
    }

    fn preview(svc: &Service, a: &str, b: &str) -> Payload {
        ok(
            "copy.preview",
            call(
                svc,
                "copy.preview",
                json!({"sources": [a], "destination_node_id": b}),
            ),
        )
        .0
    }

    /// Relative names and bytes under `p`. The C1 verifier's agreement is proven in the windows lab
    /// suite; the content reader stays engine-only here (`content_reader_is_feature_gated_and_engine_only`).
    fn contents(p: &Path) -> std::collections::BTreeMap<PathBuf, Option<Vec<u8>>> {
        let mut out = std::collections::BTreeMap::new();
        let mut pending = vec![p.to_path_buf()];
        while let Some(dir) = pending.pop() {
            for e in std::fs::read_dir(&dir).unwrap() {
                let path = e.unwrap().path();
                let key = path.strip_prefix(p).unwrap().to_path_buf();
                if path.is_dir() {
                    pending.push(path);
                    out.insert(key, None);
                } else {
                    out.insert(key, Some(std::fs::read(&path).unwrap()));
                }
            }
        }
        out
    }

    #[test]
    fn synthetic_lab_copy_previews_approves_over_http_and_commits() {
        let Some((lab, a, b)) = scanned() else { return };
        let svc = &lab.svc;
        let p = preview(svc, &a, &b);
        assert_eq!(p["state"], "draft", "{p:?}");
        assert_eq!(p["folder_name"], "a");
        assert_eq!(
            (p["files"].as_u64(), p["folders"].as_u64()),
            (Some(4), Some(2))
        );
        let op = p["operation_id"].as_str().unwrap().to_owned();
        let consent = json!({"operation_id": op, "digest": digest(&p)});
        // Starting without an approval is refused and starts nothing.
        let e = err(call(svc, "copy.start", consent.clone()));
        assert_eq!(reason(&e), Some("approval_required"));
        ok("copy.approve", call(svc, "copy.approve", consent.clone()));
        ok("copy.start", call(svc, "copy.start", consent.clone()));
        let done = settle_copy(svc, &op);
        assert_eq!(done["state"], "committed", "{done:?}");
        assert_eq!(done["items_verified"], done["items_total"]);
        assert_eq!(done["staging_folder"], Value::Null);
        assert_eq!(done["leftover_staging_folders"], json!([]));
        // A started operation never starts again, approved or not.
        let e = err(call(svc, "copy.start", consent));
        assert_eq!(reason(&e), Some("copy_already_started"));
        assert_eq!(
            contents(&lab.path.join("a")),
            contents(&lab.path.join("b").join("a"))
        );
        // The journal and the wire ID survive a restart; recovery leaves a committed copy alone.
        let state = lab.path.parent().unwrap().join("state");
        let Lab {
            _tmp, path, svc, ..
        } = lab;
        drop(svc);
        let svc = Service::open(Config {
            state_dir: state,
            dataset: DatasetClass::Synthetic,
            allow_personal: false,
            allow_identity_unverified: false,
            grant_roots: vec![],
        })
        .unwrap();
        let (s, _) = ok(
            "copy.status",
            call(&svc, "copy.status", json!({"operation_id": op})),
        );
        assert_eq!(s["state"], "committed");
        assert!(path.join("b").join("a").join("deep").join("x.bin").exists());
    }

    #[test]
    fn a_source_changed_after_approval_refuses_the_start() {
        let Some((lab, a, b)) = scanned() else { return };
        let svc = &lab.svc;
        let p = preview(svc, &a, &b);
        let consent = json!({"operation_id": p["operation_id"], "digest": digest(&p)});
        ok("copy.approve", call(svc, "copy.approve", consent.clone()));
        std::fs::write(lab.path.join("a").join("1.bin"), [1u8; 10]).unwrap();
        let e = err(call(svc, "copy.start", consent.clone()));
        assert_eq!(reason(&e), Some("source_changed"), "{e:?}");
        assert_eq!(std::fs::read_dir(lab.path.join("b")).unwrap().count(), 0);
        // The approval was consumed by that attempt (single use).
        let e = err(call(svc, "copy.start", consent));
        assert_eq!(reason(&e), Some("approval_required"));
    }

    #[test]
    fn a_wrong_digest_is_never_approved() {
        let Some((lab, a, b)) = scanned() else { return };
        let p = preview(&lab.svc, &a, &b);
        let e = err(call(
            &lab.svc,
            "copy.approve",
            json!({"operation_id": p["operation_id"], "digest": format!("sha256:{}", "0".repeat(64))}),
        ));
        assert_eq!(reason(&e), Some("digest_mismatch"));
    }

    /// Q1 at the service: abandoning a blocked operation stops and LEAVES the staging by default,
    /// reporting its name; with `loomward-copy-staging-removal` on, removes it by identity.
    #[test]
    fn abandon_applies_the_staging_removal_setting() {
        for removal in [false, true] {
            let Some((lab, a, b)) = scanned() else { return };
            let svc = &lab.svc;
            svc.inner
                .copies
                .removal
                .store(removal, std::sync::atomic::Ordering::SeqCst);
            let p = preview(svc, &a, &b);
            let op = p["operation_id"].as_str().unwrap().to_owned();
            let consent = json!({"operation_id": op, "digest": digest(&p)});
            ok("copy.approve", call(svc, "copy.approve", consent.clone()));
            // A writer on one source file stops the run `blocked` (source_in_use, resumable).
            let writer = std::fs::OpenOptions::new()
                .append(true)
                .open(lab.path.join("a").join("deep").join("x.bin"))
                .unwrap();
            ok("copy.start", call(svc, "copy.start", consent));
            let stopped = settle_copy(svc, &op);
            assert_eq!(stopped["state"], "blocked", "{stopped:?}");
            assert_eq!(stopped["refusal"]["code"], "source_in_use");
            drop(writer);
            let (gone, _) = ok(
                "copy.abandon",
                call(svc, "copy.abandon", json!({"operation_id": op})),
            );
            assert_eq!(gone["state"], "cancelled", "{gone:?}");
            let left: Vec<String> = std::fs::read_dir(lab.path.join("b"))
                .unwrap()
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect();
            if removal {
                assert!(left.is_empty(), "{left:?}");
                assert_eq!(gone["staging_folder"], Value::Null);
            } else {
                assert_eq!(left.len(), 1, "{left:?}");
                assert_eq!(gone["staging_folder"], json!(left[0]));
            }
        }
    }
}

/// Every command whose contract declares an effect is a mutation: a ticketed, retained, serialised
/// call, so opening the copy gate (docs/47 C5) can never route an effect onto the concurrent read path.
#[test]
fn every_effectful_command_is_a_mutation() {
    let path = format!(
        "{}/../../contracts/v3/commands.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut effectful = 0;
    for (name, row) in doc["commands"].as_object().unwrap() {
        if row["effects"] != "none" {
            effectful += 1;
            let c = Command::from_name(name).unwrap();
            assert!(
                crate::is_mutation(c),
                "{name} declares effects but is not a mutation"
            );
        }
    }
    assert_eq!(effectful, 4);
}
