//! Command-layer tests: the self test, the refusals that guard both native dialogs, the
//! disclosure dialog's text, and Tauri's own IPC + ACL (mock runtime) proving the webview can
//! invoke `lw_call`, `lw_events` and `lw_update` and nothing else.

use loomward_desktop::dialogs::{disclosure_text, visible, DesktopDialogs, DISCLOSURE_TITLE};
use loomward_desktop::selftest::{self, envelope, fixture_desktop, wait_until};
use loomward_desktop::updates::{self, Channel, Updates};
use loomward_desktop::{shell, Desktop};
use loomward_http::fixture::EXAMPLES_DIR;
use loomward_protocol::{
    CallContext, Command, DatasetClass, DisclosureSummary, EventStream, NativeDialogs,
    RequestEnvelope, ResponseEnvelope, ViewService,
};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[test]
fn self_test_passes() {
    let mut lines = Vec::new();
    let ok = selftest::run(&mut |l| lines.push(l.to_string()));
    assert!(ok, "{lines:#?}");
}

/// A service that would grant anything, and counts how often it is asked.
#[derive(Default)]
struct Granting(AtomicUsize);

impl ViewService for Granting {
    fn call(&self, request: RequestEnvelope, _: &CallContext) -> ResponseEnvelope {
        self.0.fetch_add(1, Ordering::SeqCst);
        let meta = json!({ "served_at": "2026-10-10T00:00:00.000Z", "elapsed_ms": 0,
            "dataset_class": "personal", "budget_hit": false, "catalog_rev": null, "state_rev": null });
        let Value::Object(result) = json!({ "outcome": "granted" }) else {
            unreachable!()
        };
        ResponseEnvelope::ok_object(
            request.request_id,
            result,
            serde_json::from_value(meta).unwrap(),
        )
    }
    fn subscribe(&self, _: Option<(String, u64)>) -> Box<dyn EventStream> {
        unreachable!("not used")
    }
}

type Shown = Arc<Mutex<Vec<String>>>;

fn recording(dataset: DatasetClass, answer: bool) -> (DesktopDialogs, Shown) {
    let shown: Shown = Arc::default();
    let log = shown.clone();
    let dialogs = DesktopDialogs::with_prompt(dataset, move |title, text| {
        assert_eq!(title, DISCLOSURE_TITLE);
        log.lock().unwrap().push(text.to_string());
        answer
    });
    (dialogs, shown)
}

#[test]
fn personal_disclosure_is_refused_with_confinement_not_enforced_before_any_dialog() {
    let service = Arc::new(Granting::default());
    let (dialogs, shown) = recording(DatasetClass::Personal, true);
    let desktop = Desktop::new(service.clone(), DatasetClass::Personal, Arc::new(dialogs));
    let payload = selftest::example_payload(Command::GrantsCreateDisclosure);
    let r = desktop
        .call(&envelope("grants.create_disclosure", payload))
        .unwrap();
    let ResponseEnvelope::Ok(ok) = &r else {
        panic!("{r:?}")
    };
    ok.validate_for(Command::GrantsCreateDisclosure).unwrap();
    assert_eq!(ok.result["outcome"], json!("refused"));
    assert_eq!(ok.result["refusal"], json!("confinement_not_enforced"));
    assert_eq!(ok.result["grant"], Value::Null);
    assert_eq!(service.0.load(Ordering::SeqCst), 0, "the service was asked");
    assert!(shown.lock().unwrap().is_empty(), "a dialog was shown");
    // A personal session does reach the service for the picker command.
    desktop
        .call(&envelope(
            "roots.request_grant",
            json!({ "purpose": "metadata_scan" }),
        ))
        .unwrap();
    assert_eq!(service.0.load(Ordering::SeqCst), 1);
}

#[test]
fn synthetic_request_grant_never_reaches_the_service() {
    let service = Arc::new(Granting::default());
    let (dialogs, _) = recording(DatasetClass::Synthetic, true);
    let desktop = Desktop::new(service.clone(), DatasetClass::Synthetic, Arc::new(dialogs));
    let r = desktop
        .call(&envelope(
            "roots.request_grant",
            json!({ "purpose": "metadata_scan" }),
        ))
        .unwrap();
    let ResponseEnvelope::Ok(ok) = &r else {
        panic!("{r:?}")
    };
    ok.validate_for(Command::RootsRequestGrant).unwrap();
    assert_eq!(
        ok.result["refusal"],
        json!("synthetic_session_requires_lab_root")
    );
    assert_eq!(service.0.load(Ordering::SeqCst), 0);
}

#[test]
fn the_picker_never_opens_in_a_synthetic_session() {
    // Would block on a real dialog if it opened.
    assert_eq!(
        DesktopDialogs::new(DatasetClass::Synthetic).pick_folder(),
        None
    );
}

fn summary(dataset: &str, items: Option<Vec<Value>>) -> DisclosureSummary {
    let file = Path::new(EXAMPLES_DIR)
        .join("dtos")
        .join("disclosure-summary.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    let mut v = doc["value"].clone();
    v["dataset_class"] = json!(dataset);
    if let Some(items) = items {
        v["items"] = Value::Array(items);
    }
    serde_json::from_value(v).unwrap()
}

#[test]
fn disclosure_pages_ask_for_every_item_and_cancel_the_whole_summary() {
    for count in [5usize, 25] {
        let items = (0..count)
            .map(|i| {
                json!({"handle":format!("i{i:02}"),
            "name":format!("item-{i}"),"extension":"txt","context":"Lab","size_bucket":0})
            })
            .collect();
        let s = summary("synthetic", Some(items));
        let (dialogs, shown) = recording(DatasetClass::Synthetic, true);
        assert!(dialogs.confirm_disclosure(&s).unwrap());
        let shown = shown.lock().unwrap();
        assert_eq!(shown.len(), count.div_ceil(4));
        let all = shown.join("\n");
        for i in 0..count {
            assert_eq!(all.matches(&format!("item-{i}  ")).count(), 1);
        }
        for page in shown.iter() {
            assert!(page.chars().count() <= loomward_desktop::dialogs::MAX_DISCLOSURE_CHARS);
            assert!(
                page.lines()
                    .map(|line| line.chars().count().div_ceil(80).max(1))
                    .sum::<usize>()
                    <= loomward_desktop::dialogs::MAX_DISCLOSURE_LINES
            );
            assert!(page.matches("size bucket").count() <= 4);
            assert!(page.contains(s.payload_digest.as_str()));
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let dialogs = DesktopDialogs::with_prompt(DatasetClass::Synthetic, move |_, _| {
            seen.fetch_add(1, Ordering::SeqCst) == 0
        });
        assert!(!dialogs.confirm_disclosure(&s).unwrap());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}

#[test]
fn disclosure_over_display_budget_returns_an_explicit_error() {
    for count in [5, 25] {
        let items = (0..count)
            .map(|i| {
                json!({"handle":format!("i{i:02}"),
            "name":"n".repeat(256),"extension":"txt","context":"c".repeat(512),"size_bucket":0})
            })
            .collect();
        let (dialogs, shown) = recording(DatasetClass::Synthetic, true);
        let error = dialogs
            .confirm_disclosure(&summary("synthetic", Some(items)))
            .unwrap_err();
        assert_eq!(error.code, loomward_protocol::ErrorCode::ResourceBudget);
        assert!(error.message.as_str().contains("no items were sent"));
        assert!(shown.lock().unwrap().is_empty());
    }
}

#[test]
fn disclosure_dialog_accept_and_decline_paths() {
    for (dataset, wire, other) in [
        (DatasetClass::Synthetic, "synthetic", "personal"),
        (DatasetClass::Personal, "personal", "synthetic"),
    ] {
        let s = summary(wire, None);
        for answer in [true, false] {
            let (dialogs, shown) = recording(dataset, answer);
            assert_eq!(
                dialogs.confirm_disclosure(&s).unwrap(),
                answer && dataset == DatasetClass::Synthetic
            );
            assert_eq!(
                shown.lock().unwrap().len(),
                usize::from(dataset == DatasetClass::Synthetic)
            );
        }
        // A summary of the other class is declined without asking.
        let (dialogs, shown) = recording(dataset, true);
        assert!(!dialogs.confirm_disclosure(&summary(other, None)).unwrap());
        assert!(shown.lock().unwrap().is_empty());
    }
}

#[test]
fn disclosure_text_renders_every_item_with_hidden_characters_escaped() {
    let names = [
        "invoice\u{202E}txt.exe",
        "budget\u{200B}\u{200D}final.xlsx",
        "<img src=x onerror=alert(1)>.png",
        "line\nbreak.txt",
    ];
    let items: Vec<Value> = (0..25)
        .map(|i| {
            json!({ "handle": format!("i{i:02}"), "name": format!("{}-{i}", names[i % names.len()]),
                    "extension": "txt", "context": format!("Lab\\S1\\dir{i}"), "size_bucket": i % 17 })
        })
        .collect();
    let s = summary("personal", Some(items.clone()));
    let text = disclosure_text(&s);
    for (i, item) in items.iter().enumerate() {
        let name = visible(item["name"].as_str().unwrap());
        assert!(
            text.contains(&format!("{:>2}. {name}", i + 1)),
            "item {i} missing: {name}"
        );
        assert!(text.contains(&visible(item["context"].as_str().unwrap())));
    }
    assert!(text.contains("Items (25):"));
    for hidden in ['\u{202E}', '\u{200B}', '\u{200D}'] {
        assert!(!text.contains(hidden), "raw {hidden:?} in the dialog text");
    }
    assert!(text.contains("invoice\\u{202E}txt.exe"));
    assert!(text.contains("line\\u{000A}break.txt"));
    assert!(text.contains(s.payload_digest.as_str()));
    assert!(text.contains("Cancel (the default) sends nothing"));
}

fn invoke(
    webview: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    cmd: &str,
    body: Value,
) -> Result<tauri::ipc::InvokeResponseBody, Value> {
    let body = if cmd == "lw_call" {
        tauri::ipc::InvokeBody::Raw(serde_json::to_vec(&body["request"]).unwrap())
    } else {
        tauri::ipc::InvokeBody::Json(body)
    };
    invoke_body(webview, cmd, body, webview.url().unwrap())
}

fn invoke_body(
    webview: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    cmd: &str,
    body: tauri::ipc::InvokeBody,
    url: tauri::Url,
) -> Result<tauri::ipc::InvokeResponseBody, Value> {
    tauri::test::get_ipc_response(
        webview,
        tauri::webview::InvokeRequest {
            cmd: cmd.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url,
            body,
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
    )
}

#[test]
fn the_webview_may_invoke_exactly_lw_call_lw_events_and_lw_update() {
    let (service, desktop) = fixture_desktop(Duration::from_millis(300)).unwrap();
    let desktop = Arc::new(desktop);
    let prefs = tempfile::tempdir().unwrap();
    // The real context (tauri.conf.json and the compiled capability ACL), minus the configured
    // window, which the test creates itself under the same label.
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    // The checked-in config has no updater public key yet: the build must say so, not fail.
    let configured = updates::configured(context.config());
    // The manifest's version must match the version inside the signed artifact (no downgrade by
    // an edited latest.json pairing a new number with an older signed installer).
    assert_eq!(
        context.config().plugins.0["updater"]["requireSignedVersion"],
        json!(true)
    );
    let current = context.package_info().version.to_string();
    let updates = Arc::new(Updates::with_prompt(
        Some(prefs.path().into()),
        configured,
        &current,
        |_, _, _| panic!("no install confirmation in this test"),
    ));
    let app = shell::configure(tauri::test::mock_builder().manage(updates), desktop.clone())
        .build(context)
        .unwrap();
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    let r = invoke(
        &webview,
        "lw_call",
        json!({ "request": envelope("session.hello", json!({})) }),
    )
    .expect("lw_call is allowed");
    let r: Value = r.deserialize().unwrap();
    assert_eq!(r["ok"], json!(true), "{r}");
    assert_eq!(r["result"]["adapter"], json!("tauri"));

    // Over IPC a `Channel` argument is a callback id; the mock delivers to nowhere, which is
    // enough to open the stream and see it registered for this webview.
    let opened = invoke(
        &webview,
        "lw_events",
        json!({ "channel": "__CHANNEL__:7", "lastEpoch": null, "lastSeq": null }),
    );
    assert!(opened.is_ok(), "lw_events is allowed: {opened:?}");
    assert_eq!(desktop.open_streams("main"), 1);

    let update = |request: Value| {
        invoke(&webview, "lw_update", json!({ "request": request }))
            .map(|r| r.deserialize::<Value>().unwrap())
    };
    let status = update(json!({ "action": "status" })).expect("lw_update is allowed");
    assert_eq!(
        status["prefs"],
        json!({ "check_on_start": false, "channel": "preview" })
    );
    assert_eq!(status["offer"], Value::Null);
    if !configured {
        assert_eq!(
            status["unavailable"],
            json!("updates are not configured in this build")
        );
        // Refused before any request is built.
        let checked = update(json!({ "action": "check", "previous": false }));
        assert_eq!(
            checked.unwrap_err(),
            json!("updates are not configured in this build")
        );
    }
    let set =
        update(json!({ "action": "set_prefs", "check_on_start": true, "channel": "nightly" }))
            .unwrap();
    assert_eq!(
        set["prefs"],
        json!({ "check_on_start": true, "channel": "nightly" })
    );
    assert_eq!(updates::load_prefs(prefs.path()).channel, Channel::Nightly);
    for bad in [
        json!({ "action": "check", "previous": false, "endpoint": "https://example.invalid" }),
        json!({ "action": "set_prefs", "check_on_start": true, "channel": "beta" }),
        json!({ "action": "install" }),
    ] {
        assert!(update(bad.clone()).is_err(), "{bad} was accepted");
    }
    assert!(
        update(json!({ "action": "install", "version": "9.9.9" })).is_err(),
        "install without a checked offer"
    );

    for (cmd, body) in [
        ("plugin:updater|check", json!({})),
        ("plugin:updater|download_and_install", json!({ "rid": 1 })),
        (
            "plugin:event|listen",
            json!({ "event": "x", "target": { "kind": "Any" }, "handler": 3 }),
        ),
        ("plugin:webview|create_webview_window", json!({})),
        ("plugin:window|close", json!({})),
        ("plugin:app|version", json!({})),
        ("plugin:path|resolve_directory", json!({ "directory": 1 })),
        ("plugin:dialog|open", json!({})),
        (
            "plugin:fs|read_file",
            json!({ "path": "C:\\Windows\\win.ini" }),
        ),
        ("plugin:shell|execute", json!({})),
        ("lw_delete", json!({})),
    ] {
        assert!(invoke(&webview, cmd, body).is_err(), "{cmd} was allowed");
    }

    // The mock runtime emits no window lifecycle events, so the hooks in `shell::configure` are
    // exercised on the real WebView2 build (evidence/v3/native); here the teardown they call.
    assert_eq!(desktop.close_webview("main"), 1);
    assert!(wait_until(Duration::from_secs(3), || {
        desktop.open_streams("main") == 0 && service.open_streams() == 0
    }));
}

#[test]
fn disclosure_length_budget_declines_without_truncating() {
    use loomward_desktop::dialogs::MAX_DISCLOSURE_CHARS;
    let items = (0..25)
        .map(|i| {
            json!({ "handle": format!("i{i:02}"), "name": "n".repeat(256),
        "extension": "txt", "context": "c".repeat(512), "size_bucket": 0 })
        })
        .collect();
    let long = summary("synthetic", Some(items));
    assert!(disclosure_text(&long).len() > MAX_DISCLOSURE_CHARS);
    let (dialogs, shown) = recording(DatasetClass::Synthetic, true);
    assert!(dialogs.confirm_disclosure(&long).is_err());
    assert!(shown.lock().unwrap().is_empty());
    assert!(dialogs
        .confirm_disclosure(&summary("synthetic", None))
        .unwrap());
    let mut items: Vec<_> = (0..3)
        .map(|i| {
            json!({ "handle": format!("i{i:02}"),
        "name": "n", "extension": "txt", "context": "c", "size_bucket": 0 })
        })
        .collect();
    let mut extra =
        MAX_DISCLOSURE_CHARS - disclosure_text(&summary("synthetic", Some(items.clone()))).len();
    for item in &mut items {
        let add = extra.min(511);
        item["context"] = json!("c".repeat(add + 1));
        extra -= add;
    }
    assert_eq!(extra, 0);
    let exact = summary("synthetic", Some(items.clone()));
    assert_eq!(disclosure_text(&exact).len(), MAX_DISCLOSURE_CHARS);
    assert!(
        dialogs.confirm_disclosure(&exact).is_err(),
        "wrapping also consumes the height budget"
    );
    let last = items.last_mut().unwrap();
    last["context"] = json!(format!("{}c", last["context"].as_str().unwrap()));
    assert!(dialogs
        .confirm_disclosure(&summary("synthetic", Some(items)))
        .is_err());
}

#[test]
fn concurrent_disclosure_is_declined() {
    use std::sync::mpsc;
    let (started, rx) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let wait = Mutex::new(wait);
    let count = AtomicUsize::new(0);
    let dialogs = Arc::new(DesktopDialogs::with_prompt(
        DatasetClass::Synthetic,
        move |_, _| {
            if count.fetch_add(1, Ordering::SeqCst) == 0 {
                started.send(()).unwrap();
                wait.lock().unwrap().recv().unwrap();
            }
            true
        },
    ));
    let d = dialogs.clone();
    let first =
        std::thread::spawn(move || d.confirm_disclosure(&summary("synthetic", None)).unwrap());
    rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let second = dialogs
        .confirm_disclosure(&summary("synthetic", None))
        .unwrap();
    release.send(()).unwrap();
    assert!(first.join().unwrap());
    assert!(!second, "a second dialog was admitted");
}

#[test]
fn visible_escapes_all_non_ascii_including_every_reported_invisible() {
    assert_eq!(visible(r"\u{202E}"), r"\\u{202E}");
    assert_ne!(visible(r"\u{202E}"), visible("\u{202E}"));
    for c in [
        '\u{2800}',
        '\u{3164}',
        '\u{115F}',
        '\u{1160}',
        '\u{FFA0}',
        '\u{034F}',
        '\u{FE00}',
        '\u{FE0F}',
        '\u{E0000}',
        '\u{E007F}',
        '\u{2028}',
        '\u{2029}',
        '\u{17B4}',
        '\u{17B5}',
    ] {
        assert_eq!(visible(&c.to_string()), format!("\\u{{{:04X}}}", c as u32));
    }
    assert_eq!(visible("Lab file.txt"), "Lab file.txt");
}

#[test]
fn disclosure_height_budget_declines_wrapped_lines() {
    let items = (0..3)
        .map(|i| {
            json!({ "handle": format!("i{i:02}"),
            "name": "n", "extension": "txt", "context": "c".repeat(400), "size_bucket": 0 })
        })
        .collect();
    let s = summary("synthetic", Some(items));
    assert!(disclosure_text(&s).len() < loomward_desktop::dialogs::MAX_DISCLOSURE_CHARS);
    let (dialogs, shown) = recording(DatasetClass::Synthetic, true);
    assert!(dialogs.confirm_disclosure(&s).is_err());
    assert!(shown.lock().unwrap().is_empty());
    assert!(dialogs
        .confirm_disclosure(&summary("synthetic", None))
        .unwrap());
}

#[test]
fn call_saturation_is_busy_through_tauri_ipc_and_recovers_after_release() {
    let (_, desktop) = fixture_desktop(Duration::from_millis(30)).unwrap();
    let desktop = Arc::new(desktop);
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    let app = shell::configure(tauri::test::mock_builder(), desktop.clone())
        .build(context)
        .unwrap();
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let mut permits: Vec<_> = (0..loomward_desktop::MAX_CALLS)
        .map(|_| desktop.begin_call().unwrap())
        .collect();
    let body = json!({ "request": envelope("session.hello", json!({})) });
    assert!(invoke(&webview, "lw_call", body.clone())
        .unwrap_err()
        .as_str()
        .unwrap()
        .starts_with("busy:"));
    permits.pop();
    assert_eq!(
        invoke(&webview, "lw_call", body)
            .unwrap()
            .deserialize::<Value>()
            .unwrap()["ok"],
        true
    );
}

/// Hosted CI runs elevated, where the volume open refuses every root's identity: the picked
/// folder is then refused for that reason alone, and the profile rules cannot be observed.
fn identity_refused(root: &std::path::Path) -> bool {
    use loomward_protocol::RootGrantResultRefusal as Refusal;
    loomward_service::paths::check_root(root, None, &[], true) == Err(Refusal::IdentityUnavailable)
}

#[test]
fn nonstandard_profile_parent_does_not_fence_unrelated_siblings() {
    use loomward_desktop::dialogs::validate_picked_folder;
    // The system temp directory is inside AppData, which the shared grant validator refuses.
    let tmp = tempfile::tempdir_in(loomward_service::paths::test_roots_base()).unwrap();
    let profile = tmp.path().join("fixture-home");
    let sibling = tmp.path().join("fixture-dataset");
    std::fs::create_dir(&profile).unwrap();
    std::fs::create_dir(&sibling).unwrap();
    if identity_refused(&sibling) {
        assert!(validate_picked_folder(&sibling, &profile).is_err());
        return;
    }
    assert_eq!(
        validate_picked_folder(&sibling, &profile).unwrap(),
        sibling.canonicalize().unwrap()
    );
    assert!(validate_picked_folder(&profile, &profile).is_err());
}

#[test]
fn picker_fences_additional_credential_stores_case_insensitively() {
    use loomward_desktop::dialogs::validate_picked_folder;
    let tmp = tempfile::tempdir_in(loomward_service::paths::test_roots_base()).unwrap();
    let profile = tmp.path().join("profiles/fixture-user");
    for store in [".docker", ".CARGO", ".npm", ".terraform.d", ".gemini"] {
        let path = profile.join(store).join("nested");
        std::fs::create_dir_all(&path).unwrap();
        assert!(validate_picked_folder(&path, &profile).is_err(), "{store}");
    }
}

#[test]
fn calls_are_capped_and_unwinding_releases_the_permit() {
    let (_, desktop) = fixture_desktop(Duration::from_millis(30)).unwrap();
    let mut permits: Vec<_> = (0..loomward_desktop::MAX_CALLS)
        .map(|_| desktop.begin_call().unwrap())
        .collect();
    assert!(desktop.begin_call().is_err());
    let permit = permits.pop().unwrap();
    assert!(std::panic::catch_unwind(move || {
        let _permit = permit;
        panic!("synthetic service panic");
    })
    .is_err());
    assert!(desktop.begin_call().is_ok());
}

#[test]
fn stream_bursts_never_replace_a_pump_before_it_releases_its_slot() {
    use std::sync::mpsc;
    let (service, desktop) = fixture_desktop(Duration::from_millis(30)).unwrap();
    let (entered_tx, entered_rx) = mpsc::channel();
    let mut releases = Vec::new();
    for _ in 0..loomward_desktop::MAX_STREAMS_PER_WEBVIEW {
        let (release_tx, release_rx) = mpsc::channel();
        releases.push(release_tx);
        let entered_tx = entered_tx.clone();
        let mut first = true;
        desktop
            .open_stream("main", None, None, move |_| {
                if first {
                    first = false;
                } else {
                    entered_tx.send(()).unwrap();
                    // Hold the live pump even after its stop flag is set.
                    let _ = release_rx.recv();
                }
                true
            })
            .unwrap();
    }
    for _ in 0..4 {
        entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    }
    for i in 0..32 {
        if i == 16 {
            desktop.close_webview("main");
        }
        let opened = desktop.open_stream("main", None, None, |_| true);
        assert_eq!(
            service.open_streams(),
            4,
            "burst {i}: a live subscription exceeded the cap"
        );
        assert_eq!(
            desktop.open_streams("main"),
            4,
            "burst {i}: a live pump lost its slot"
        );
        assert!(opened.unwrap_err().contains("busy"));
    }
    releases.remove(0).send(()).unwrap();
    assert!(wait_until(Duration::from_secs(3), || desktop
        .open_streams("main")
        == 3
        && service.open_streams() == 3));
    desktop.open_stream("main", None, None, |_| true).unwrap();
    assert_eq!(desktop.open_streams("main"), 4);
    assert_eq!(service.open_streams(), 4);
    desktop.close_webview("main");
    drop(releases);
    assert!(wait_until(Duration::from_secs(3), || desktop
        .open_streams("main")
        == 0
        && service.open_streams() == 0));
}

#[test]
fn fifth_stream_evicts_only_the_oldest() {
    use std::sync::mpsc;
    let (service, desktop) = fixture_desktop(Duration::from_millis(30)).unwrap();
    let mut receivers = Vec::new();
    for _ in 0..4 {
        let (tx, rx) = mpsc::channel();
        desktop
            .open_stream("main", None, None, move |e| tx.send(e).is_ok())
            .unwrap();
        rx.recv_timeout(Duration::from_secs(1)).unwrap();
        receivers.push(rx);
    }
    assert!(desktop
        .open_stream("main", None, None, |_| true)
        .unwrap_err()
        .contains("busy"));
    assert!(wait_until(Duration::from_secs(3), || desktop
        .open_streams("main")
        == 3
        && service.open_streams() == 3));
    let (tx, rx) = mpsc::channel();
    desktop
        .open_stream("main", None, None, move |e| tx.send(e).is_ok())
        .unwrap();
    rx.recv_timeout(Duration::from_secs(1)).unwrap();
    receivers.push(rx);
    assert!(wait_until(Duration::from_secs(3), || service
        .open_streams()
        == 4));
    assert_eq!(desktop.open_streams("main"), 4);
    assert!(wait_until(Duration::from_secs(3), || {
        matches!(
            receivers[0].try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        )
    }));
    assert!(receivers[1..]
        .iter()
        .all(|rx| !matches!(rx.try_recv(), Err(mpsc::TryRecvError::Disconnected))));
    desktop.close_webview("main");
}

#[test]
fn pump_panic_releases_both_adapter_and_service_slots() {
    let (service, desktop) = fixture_desktop(Duration::from_millis(30)).unwrap();
    let mut first = true;
    desktop
        .open_stream("main", None, None, move |_| {
            if first {
                first = false;
                true
            } else {
                panic!("synthetic sink panic")
            }
        })
        .unwrap();
    assert!(wait_until(Duration::from_secs(3), || desktop
        .open_streams("main")
        == 0
        && service.open_streams() == 0));
    desktop.open_stream("main", None, None, |_| true).unwrap();
    assert_eq!(desktop.open_streams("main"), 1);
    desktop.close_webview("main");
}

#[test]
fn pump_receive_panic_releases_both_slots_and_allows_reopen() {
    struct PanicService(Arc<loomward_http::fixture::FixtureService>);
    struct PanicStream {
        inner: Box<dyn EventStream>,
        first: bool,
    }
    impl EventStream for PanicStream {
        fn epoch(&self) -> &str {
            self.inner.epoch()
        }
        fn recv_timeout(&mut self, timeout: Duration) -> loomward_protocol::RecvOutcome {
            if !self.first {
                panic!("synthetic pump receive panic");
            }
            self.first = false;
            self.inner.recv_timeout(timeout)
        }
        fn close(&mut self) {
            self.inner.close();
        }
    }
    impl ViewService for PanicService {
        fn call(&self, request: RequestEnvelope, ctx: &CallContext) -> ResponseEnvelope {
            self.0.call(request, ctx)
        }
        fn subscribe(&self, resume: Option<(String, u64)>) -> Box<dyn EventStream> {
            Box::new(PanicStream {
                inner: self.0.subscribe(resume),
                first: true,
            })
        }
    }
    let (service, _) = fixture_desktop(Duration::from_millis(30)).unwrap();
    let desktop = Desktop::new(
        Arc::new(PanicService(service.clone())),
        DatasetClass::Synthetic,
        Arc::new(DesktopDialogs::with_prompt(
            DatasetClass::Synthetic,
            |_, _| false,
        )),
    );
    for _ in 0..5 {
        desktop.open_stream("main", None, None, |_| true).unwrap();
        assert!(wait_until(Duration::from_secs(3), || desktop
            .open_streams("main")
            == 0
            && service.open_streams() == 0));
    }
}

#[test]
fn navigation_accepts_only_the_embedded_origin() {
    for url in [
        "http://tauri.localhost/index.html#/atlas",
        "tauri://localhost/index.html",
    ] {
        assert!(shell::local_navigation(&url.parse().unwrap()));
    }
    for url in [
        "https://remote.invalid/",
        "http://tauri.localhost.evil.invalid/",
        "http://tauri.localhost:81/",
        "http://user@tauri.localhost/",
        "file:///fixture",
        "data:text/html,fixture",
    ] {
        assert!(!shell::local_navigation(&url.parse().unwrap()), "{url}");
    }
}

#[test]
fn raw_ipc_enforces_encoded_size_duplicates_and_remote_origin_denial() {
    use tauri::ipc::InvokeBody;
    let (_, desktop) = fixture_desktop(Duration::from_millis(30)).unwrap();
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    let app = shell::configure(tauri::test::mock_builder(), Arc::new(desktop))
        .build(context)
        .unwrap();
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let url = webview.url().unwrap();
    let mut bytes = serde_json::to_vec(&envelope("session.hello", json!({}))).unwrap();
    bytes.resize(65536, b' ');
    let response = invoke_body(
        &webview,
        "lw_call",
        InvokeBody::Raw(bytes.clone()),
        url.clone(),
    )
    .unwrap();
    assert_eq!(response.deserialize::<Value>().unwrap()["ok"], true);
    bytes.push(b' ');
    assert!(invoke_body(&webview, "lw_call", InvokeBody::Raw(bytes), url.clone()).is_err());
    for payload in [
        r#"{"protocol":"loomward/3","request_id":"r_dup","command":"session.hello","command":"roots.list","payload":{}}"#,
        r#"{"protocol":"loomward/3","request_id":"r_dup","command":"session.hello","payload":{"x":1,"x":2}}"#,
    ] {
        let r = invoke_body(
            &webview,
            "lw_call",
            InvokeBody::Raw(payload.as_bytes().to_vec()),
            url.clone(),
        )
        .unwrap();
        let r: Value = r.deserialize().unwrap();
        assert_eq!(r["ok"], false, "{r}");
        assert!(
            r["error"]["message"]
                .as_str()
                .unwrap()
                .contains("duplicate"),
            "{r}"
        );
    }
    let request = envelope("session.hello", json!({}));
    assert!(invoke_body(
        &webview,
        "lw_call",
        InvokeBody::Json(json!({"request": request})),
        url
    )
    .is_err());
    assert!(invoke_body(
        &webview,
        "lw_call",
        InvokeBody::Raw(serde_json::to_vec(&request).unwrap()),
        "https://remote.invalid/".parse().unwrap()
    )
    .is_err());
}

#[test]
fn picked_folder_uses_grant_rules_and_fences_profiles_and_stores() {
    use loomward_desktop::dialogs::validate_picked_folder;
    let tmp = tempfile::tempdir_in(loomward_service::paths::test_roots_base()).unwrap();
    let profile = tmp.path().join("profiles").join("fixture-user");
    let allowed = profile.join("Documents").join("lab");
    for path in [
        &allowed,
        &profile.join(".ssh"),
        &profile.join("AppData").join("browser"),
        &profile.join(".aws"),
        &tmp.path().join("profiles").join("other-fixture"),
    ] {
        std::fs::create_dir_all(path).unwrap();
    }
    if identity_refused(&allowed) {
        assert!(validate_picked_folder(&allowed, &profile).is_err());
        return;
    }
    assert_eq!(
        validate_picked_folder(&allowed, &profile).unwrap(),
        allowed.canonicalize().unwrap()
    );
    for path in [
        &profile,
        &profile.parent().unwrap().to_path_buf(),
        &profile.join(".ssh"),
        &profile.join("AppData").join("browser"),
        &profile.join(".aws"),
        &tmp.path().join("profiles").join("other-fixture"),
        &tmp.path().join("missing"),
    ] {
        assert!(validate_picked_folder(path, &profile).is_err());
    }
    for path in [
        r"\\fixture\share",
        r"\\?\UNC\fixture\share",
        r"\\.\fixture",
        r"\\?\GLOBALROOT\fixture",
    ] {
        assert!(validate_picked_folder(Path::new(path), &profile).is_err());
    }
    let volume = allowed.ancestors().last().unwrap();
    assert!(
        validate_picked_folder(volume, &profile).is_ok(),
        "native volume selection is allowed"
    );
    assert!(
        loomward_service::paths::check_root(volume, None, &[], false).is_err(),
        "CLI volume grant is refused"
    );
}

/// The real Win32 box: Enter's default is Cancel, OK accepts, Cancel declines. A helper thread
/// finds the box by title and presses a button, so no human is needed, but it does need an
/// interactive desktop session.
#[cfg(windows)]
#[test]
#[ignore = "opens a real MessageBox; run with --ignored on an interactive desktop"]
fn native_disclosure_box_defaults_to_cancel_and_maps_both_buttons() {
    use loomward_desktop::dialogs::message_box;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, PostMessageW, SendMessageW, DM_GETDEFID, IDCANCEL, IDOK, WM_COMMAND,
    };
    let text = disclosure_text(&summary("personal", None));
    for (button, want) in [(IDOK, true), (IDCANCEL, false)] {
        let clicker = std::thread::spawn(move || {
            let title: Vec<u16> = DISCLOSURE_TITLE.encode_utf16().chain(Some(0)).collect();
            for _ in 0..200 {
                // SAFETY: NUL-terminated title; the handle is used only while the box is open.
                let hwnd = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
                if !hwnd.is_null() {
                    std::thread::sleep(Duration::from_millis(300));
                    // SAFETY: plain messages to a live dialog window.
                    let default = unsafe { SendMessageW(hwnd, DM_GETDEFID, 0, 0) } & 0xFFFF;
                    unsafe { PostMessageW(hwnd, WM_COMMAND, button as usize, 0) };
                    return Some(default as i32);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            None
        });
        // SAFETY: the built-in STATIC class creates a synthetic owner for the ignored UI test.
        let class: Vec<u16> = "STATIC".encode_utf16().chain(Some(0)).collect();
        let owner = unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                0,
                0,
                0,
                100,
                100,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        assert!(!owner.is_null());
        let accepted = message_box(owner as isize, DISCLOSURE_TITLE, &text);
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(owner);
        }
        let default = clicker.join().unwrap().expect("the box never appeared");
        assert_eq!(default, IDCANCEL, "Enter must decline");
        assert_eq!(accepted, want, "button {button}");
    }
}

// ----------------------------------------------------------------------------------------------
// The engine service the window hosts by default (launch.rs).
// ----------------------------------------------------------------------------------------------

use loomward_desktop::launch::{self, bundled_examples_dir, default_state_dir, Launch};

fn launch_args(args: &[&str]) -> Result<Launch, String> {
    Launch::parse(args.iter().map(|s| s.to_string()))
}

/// A picker stand-in that always returns one disposable folder.
struct Picker(std::path::PathBuf);
impl NativeDialogs for Picker {
    fn pick_folder(&self) -> Option<std::path::PathBuf> {
        Some(self.0.clone())
    }
    fn confirm_disclosure(
        &self,
        _: &DisclosureSummary,
    ) -> Result<bool, loomward_protocol::ErrorBody> {
        Ok(false)
    }
}

/// A fresh `request_id` per call: the engine retains mutation outcomes by id.
fn request(command: &str, payload: Value) -> Value {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let mut r = envelope(command, payload);
    r["request_id"] = json!(format!("r_engine_{}", NEXT.fetch_add(1, Ordering::SeqCst)));
    r
}

fn engine_desktop(launch: &Launch, base: &Path, picked: &Path) -> Desktop {
    let service = launch::open(launch, Some(base.as_os_str().to_owned())).unwrap();
    Desktop::new(
        service,
        launch.dataset,
        Arc::new(Picker(picked.to_path_buf())),
    )
}

fn ok(desktop: &Desktop, command: &str, payload: Value) -> serde_json::Map<String, Value> {
    match desktop.call(&request(command, payload)).unwrap() {
        ResponseEnvelope::Ok(ok) => {
            ok.validate_for(Command::from_name(command).unwrap())
                .unwrap();
            ok.result
        }
        ResponseEnvelope::Err(e) => panic!("{command}: {:?}", e.error),
    }
}

#[test]
fn bundled_examples_are_found_only_when_installed_beside_the_exe() {
    let exe_dir = tempfile::tempdir().expect("temp dir");
    assert_eq!(bundled_examples_dir(exe_dir.path()), None);
    let commands = exe_dir.path().join("contracts/v3/examples/commands");
    std::fs::create_dir_all(&commands).expect("create");
    assert_eq!(
        bundled_examples_dir(exe_dir.path()),
        Some(exe_dir.path().join("contracts/v3/examples"))
    );
}

#[test]
fn launch_defaults_to_a_personal_engine_session_and_takes_no_root_argument() {
    let default = launch_args(&[]).unwrap();
    assert_eq!(default.dataset, DatasetClass::Personal);
    assert!(!default.fixtures && !default.self_test && default.state_dir.is_none());
    let synthetic = launch_args(&["--dataset", "synthetic", "--state-dir", "s"]).unwrap();
    assert_eq!(synthetic.dataset, DatasetClass::Synthetic);
    assert_eq!(synthetic.state_dir, Some("s".into()));
    assert_eq!(
        launch_args(&["--fixtures"]).unwrap().dataset,
        DatasetClass::Synthetic
    );
    for (args, want) in [
        (
            &["--fixtures", "--dataset", "personal"][..],
            "--fixtures serves synthetic contract examples only",
        ),
        (
            &["--dataset", "personal", "--fixtures"][..],
            "--fixtures serves synthetic contract examples only",
        ),
        (
            &["--fixtures", "--state-dir", "s"][..],
            "--fixtures keeps no state; drop --state-dir",
        ),
        (
            &["--dataset", "Personal"][..],
            "--dataset is synthetic or personal",
        ),
        (&["--state-dir"][..], "--state-dir needs a value"),
        // Roots come only from the owner's native picker.
        (
            &["--grant-root", "C:\\"][..],
            "unknown argument --grant-root",
        ),
        (
            &["--allow-personal"][..],
            "unknown argument --allow-personal",
        ),
    ] {
        assert_eq!(launch_args(args).unwrap_err(), want, "{args:?}");
    }
}

#[test]
fn default_state_dir_is_per_user_and_per_dataset_class() {
    let base = std::env::temp_dir().join("fixture-local-app-data");
    let personal = default_state_dir(Some(base.clone().into()), DatasetClass::Personal).unwrap();
    let synthetic = default_state_dir(Some(base.clone().into()), DatasetClass::Synthetic).unwrap();
    // Beside the per-user install directory (`Loomward`), never inside it (#373).
    assert_eq!(
        personal,
        base.join("Loomward Data").join("v3").join("personal")
    );
    assert_eq!(
        synthetic,
        base.join("Loomward Data").join("v3").join("synthetic")
    );
    assert!(!personal.starts_with(&synthetic) && !synthetic.starts_with(&personal));
    for missing in [None, Some("relative\\appdata".into()), Some("".into())] {
        assert!(default_state_dir(missing, DatasetClass::Personal).is_err());
    }
}

#[test]
fn the_default_launch_opens_the_engine_service_in_the_injected_state_dir() {
    let base = tempfile::tempdir().unwrap();
    let launch = launch_args(&[]).unwrap();
    let desktop = engine_desktop(&launch, base.path(), base.path());
    let hello = ok(&desktop, "session.hello", json!({}));
    assert!(
        hello["engine_version"]
            .as_str()
            .unwrap()
            .starts_with("loomward-service/"),
        "{hello:?}"
    );
    assert_eq!(hello["adapter"], "tauri");
    assert_eq!(hello["dataset_class"], "personal");
    assert_eq!(hello["features"]["grant_picker"], true);
    assert_eq!(hello["features"]["disclosure_dialog"], false);
    let state = base.path().join("Loomward Data/v3/personal");
    assert!(state.join("state.db").is_file() && state.join("catalog.db").is_file());
    // The adapter's wall holds in front of the engine too: no disclosure dialog in a personal session.
    let refused = ok(
        &desktop,
        "grants.create_disclosure",
        selftest::example_payload(Command::GrantsCreateDisclosure),
    );
    assert_eq!(refused["refusal"], "confinement_not_enforced");
}

#[test]
fn a_second_instance_on_one_state_directory_is_refused_until_the_first_closes() {
    let scratch = tempfile::tempdir().unwrap();
    for args in [&["--dataset", "synthetic"][..], &[][..]] {
        let state = scratch.path().join(args.len().to_string());
        let launch = Launch {
            state_dir: Some(state.clone()),
            ..launch_args(args).unwrap()
        };
        let first = launch::open(&launch, None).unwrap();
        let second = launch::open(&launch, None)
            .err()
            .expect("a second instance opened the same state directory");
        assert!(
            second.starts_with("busy: Loomward is already open"),
            "{second}"
        );
        assert!(state.join("lock").is_file());
        // An OS lock on a handle, not a marker file: closing the first frees the directory.
        drop(first);
        launch::open(&launch, None).expect("reopen after the first closed");
    }
}

#[test]
fn state_kept_inside_the_install_directory_moves_out_once() {
    let base = tempfile::tempdir().unwrap();
    let install = base.path().join("Loomward");
    let old = install.join("v3/personal");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("fixture-marker"), b"synthetic").unwrap();
    std::fs::write(install.join("uninstall.exe"), b"stand-in").unwrap();
    let launch = launch_args(&[]).unwrap();
    drop(launch::open(&launch, Some(base.path().as_os_str().to_owned())).unwrap());
    let new = base.path().join("Loomward Data/v3/personal");
    assert!(new.join("fixture-marker").is_file() && new.join("state.db").is_file());
    assert!(
        !install.join("v3").exists(),
        "the old state was copied, not moved"
    );
    assert!(install.join("uninstall.exe").is_file());
    // Once moved, an old directory that reappears is left alone, never merged.
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("later"), b"synthetic").unwrap();
    let notice = launch::legacy_state_notice(&launch, Some(base.path().as_os_str().to_owned()))
        .expect("coexisting state homes must be disclosed");
    assert!(notice.contains("left untouched") && notice.contains("Loomward Data"));
    drop(launch::open(&launch, Some(base.path().as_os_str().to_owned())).unwrap());
    assert!(old.join("later").is_file() && !new.join("later").exists());
    // An explicit --state-dir never migrates.
    let other = tempfile::tempdir().unwrap();
    let legacy = other.path().join("Loomward/v3");
    std::fs::create_dir_all(&legacy).unwrap();
    let scratch = Launch {
        state_dir: Some(other.path().join("scratch")),
        ..launch.clone()
    };
    drop(launch::open(&scratch, Some(other.path().as_os_str().to_owned())).unwrap());
    assert!(legacy.is_dir() && !other.path().join("Loomward Data").exists());
    assert!(
        launch::legacy_state_notice(&scratch, Some(other.path().as_os_str().to_owned())).is_none()
    );
}

#[test]
fn an_unavailable_folder_picker_is_not_an_owner_cancel() {
    let base = tempfile::tempdir().unwrap();
    let launch = launch_args(&[]).unwrap();
    let desktop = Desktop::new(
        launch::open(&launch, Some(base.path().as_os_str().to_owned())).unwrap(),
        launch.dataset,
        Arc::new(DesktopDialogs::new(launch.dataset)),
    );
    let reply = desktop
        .call(&request(
            "roots.request_grant",
            json!({"purpose": "metadata_scan"}),
        ))
        .unwrap();
    let ResponseEnvelope::Err(error) = reply else {
        panic!("an unopened picker was reported as a cancellation: {reply:?}");
    };
    assert_eq!(
        error.error.code,
        loomward_protocol::ErrorCode::CapabilityUnavailable
    );
    assert!(error.error.message.contains("folder picker is unavailable"));
    assert_eq!(ok(&desktop, "roots.list", json!({}))["roots"], json!([]));
}

#[test]
fn a_synthetic_session_cannot_read_personal_state() {
    let base = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir_in(loomward_service::paths::test_roots_base()).unwrap();
    std::fs::write(folder.path().join("fixture.txt"), b"synthetic").unwrap();
    let personal = engine_desktop(&launch_args(&[]).unwrap(), base.path(), folder.path());
    let granted = ok(
        &personal,
        "roots.request_grant",
        json!({ "purpose": "metadata_scan" }),
    );
    if identity_refused(folder.path()) {
        assert_eq!(granted["refusal"], "identity_unavailable");
    } else {
        assert_eq!(granted["outcome"], "granted", "{granted:?}");
        let roots = ok(&personal, "roots.list", json!({}));
        assert_eq!(roots["roots"].as_array().unwrap().len(), 1);
    }
    // The default synthetic session opens its own store beside it and sees no personal root.
    let synthetic = engine_desktop(
        &launch_args(&["--dataset", "synthetic"]).unwrap(),
        base.path(),
        folder.path(),
    );
    assert_eq!(ok(&synthetic, "roots.list", json!({}))["roots"], json!([]));
    let r = ok(
        &synthetic,
        "roots.request_grant",
        json!({ "purpose": "metadata_scan" }),
    );
    assert_eq!(r["refusal"], "synthetic_session_requires_lab_root");
    // Pointed at the personal store, a synthetic session refuses to open at all (once the
    // personal window has closed; while it is open the state lock refuses first).
    drop(personal);
    let wrong = Launch {
        state_dir: Some(base.path().join("Loomward Data/v3/personal")),
        ..launch_args(&["--dataset", "synthetic"]).unwrap()
    };
    let refused = launch::open(&wrong, None)
        .err()
        .expect("synthetic opened personal state");
    assert!(refused.starts_with("state store:"), "{refused}");
}

#[test]
fn engine_events_revocation_cancellation_and_budgets_flow_through_the_adapter() {
    use loomward_protocol::{EventEnvelope, EventName};
    let base = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir_in(loomward_service::paths::test_roots_base()).unwrap();
    for i in 0..3 {
        std::fs::write(folder.path().join(format!("fixture-{i}.txt")), b"synthetic").unwrap();
    }
    let desktop = engine_desktop(&launch_args(&[]).unwrap(), base.path(), folder.path());
    let (tx, rx) = selftest::sink();
    desktop.open_stream("main", None, None, tx).unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap().event,
        EventName::StreamHello
    );
    let next = |want: &dyn Fn(&EventEnvelope) -> bool| loop {
        let e = rx.recv_timeout(Duration::from_secs(10)).expect("event");
        if want(&e) {
            return e;
        }
    };
    // Budgets travel as ordinary calls.
    let budgets = ok(
        &desktop,
        "budgets.set",
        json!({ "pool": "scan_enumerate", "max_workers": 2 }),
    );
    assert!(
        budgets["pools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["pool"] == "scan_enumerate"
                && p["current_workers"].as_u64().is_some_and(|n| n <= 2)),
        "{budgets:?}"
    );
    let granted = ok(
        &desktop,
        "roots.request_grant",
        json!({ "purpose": "metadata_scan" }),
    );
    if identity_refused(folder.path()) {
        // Hosted CI runs elevated: native identity opens are refused, so no root, no scan.
        assert_eq!(granted["refusal"], "identity_unavailable");
        desktop.close_webview("main");
        return;
    }
    assert_eq!(granted["outcome"], "granted", "{granted:?}");
    assert_eq!(granted["root"]["granted_via"], "desktop_picker");
    let root_id = granted["root"]["root_id"].clone();
    next(&|e| e.event == EventName::RootsChanged);

    let job = ok(
        &desktop,
        "scan.start",
        json!({ "root_id": root_id, "mode": "full", "budget": { "max_entries": 1000, "threads": 1 } }),
    );
    let job_id = job["job"]["job_id"].clone();
    let done = next(&|e| {
        e.event == EventName::JobState
            && e.data["job"]["job_id"] == job_id
            && e.data["job"]["state"] == "completed"
    });
    assert_eq!(done.data["job"]["root_id"], root_id);
    let status = ok(&desktop, "jobs.get", json!({ "job_id": job_id }));
    assert!(
        status["job"]["progress"]["indexed_files"].as_u64().unwrap() >= 3,
        "{status:?}"
    );
    // Cancelling a finished scan is answered through the same adapter and changes nothing.
    for command in ["scan.cancel", "jobs.cancel"] {
        let r = desktop
            .call(&request(command, json!({ "job_id": job_id })))
            .unwrap();
        if let ResponseEnvelope::Ok(ok) = &r {
            ok.validate_for(Command::from_name(command).unwrap())
                .unwrap();
            assert_eq!(ok.result["job"]["state"], "completed", "{command}");
        }
    }

    ok(
        &desktop,
        "roots.revoke",
        json!({ "root_id": root_id, "purge_catalog": false }),
    );
    let revoked = next(&|e| e.event == EventName::RootsChanged);
    assert_eq!(
        revoked.data["roots"][0]["grant_state"], "revoked",
        "{:?}",
        revoked.data
    );
    let r = desktop
        .call(&request(
            "scan.start",
            json!({ "root_id": root_id, "mode": "full" }),
        ))
        .unwrap();
    assert!(
        matches!(&r, ResponseEnvelope::Err(_)),
        "a revoked root scanned: {r:?}"
    );
    desktop.close_webview("main");
}

// ----------------------------------------------------------------------------------------------
// Backups stage 1: the open-file dialog mirrors the folder picker's walls.
// ----------------------------------------------------------------------------------------------

fn import_payload() -> Value {
    selftest::example_payload(Command::BackupsImport)
}

#[test]
fn synthetic_backup_import_never_reaches_the_service() {
    let service = Arc::new(Granting::default());
    let (dialogs, _) = recording(DatasetClass::Synthetic, true);
    let desktop = Desktop::new(service.clone(), DatasetClass::Synthetic, Arc::new(dialogs));
    let r = desktop
        .call(&envelope("backups.import", import_payload()))
        .unwrap();
    let ResponseEnvelope::Ok(ok) = &r else {
        panic!("{r:?}")
    };
    ok.validate_for(Command::BackupsImport).unwrap();
    assert_eq!(ok.result["refusal"], json!("synthetic_session"));
    assert_eq!(ok.result["evidence"], Value::Null);
    assert_eq!(service.0.load(Ordering::SeqCst), 0);
}

#[test]
fn the_file_picker_never_opens_in_a_synthetic_session() {
    // Would block on a real dialog if it opened.
    assert_eq!(
        DesktopDialogs::new(DatasetClass::Synthetic).pick_file("Loomward: synthetic"),
        None
    );
}

/// A file-picker stand-in that answers with one synthetic receipt and records each title.
struct FilePicker(std::path::PathBuf, Mutex<Vec<String>>);
impl NativeDialogs for FilePicker {
    fn pick_folder(&self) -> Option<std::path::PathBuf> {
        panic!("backups.import opened the folder picker")
    }
    fn pick_file(&self, title: &str) -> Option<std::path::PathBuf> {
        self.1.lock().unwrap().push(title.to_owned());
        Some(self.0.clone())
    }
    fn confirm_disclosure(
        &self,
        _: &DisclosureSummary,
    ) -> Result<bool, loomward_protocol::ErrorBody> {
        Ok(false)
    }
}

/// The picked path is used by the service in-process and never crosses IPC: neither the import
/// answer nor the list carries the path, the file name or anything the receipt held beyond
/// normalised evidence. The webview has no dialog permission of its own (`plugin:dialog|open`
/// is refused in `the_webview_may_invoke_exactly_lw_call_lw_events_and_lw_update`).
#[test]
fn the_picked_receipt_path_stays_in_rust() {
    let base = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir_in(loomward_service::paths::test_roots_base()).unwrap();
    let receipt = folder.path().join("synthetic-receipt-marker.json");
    let snapshot = import_payload()["snapshot_id"].clone();
    std::fs::write(
        &receipt,
        json!([{ "id": snapshot, "time": "2026-10-09T21:00:00Z", "paths": ["/synthetic-root-marker"],
            "hostname": "synthetic-host-marker" }])
        .to_string(),
    )
    .unwrap();
    let launch = launch_args(&[]).unwrap();
    let service = launch::open(&launch, Some(base.path().as_os_str().to_owned())).unwrap();
    let picker = Arc::new(FilePicker(receipt.clone(), Mutex::default()));
    let desktop = Desktop::new(service, launch.dataset, picker.clone());
    let mut payload = import_payload();
    payload["source_volume_id"] = Value::Null;
    payload["target_volume_id"] = Value::Null;
    payload["integrity_check"] = json!(false);
    let mut call = request("backups.import", payload);
    call["deadline_ms"] = json!(60_000);
    let r = desktop.call(&call).unwrap();
    let ResponseEnvelope::Ok(answer) = &r else {
        panic!("{r:?}")
    };
    answer.validate_for(Command::BackupsImport).unwrap();
    assert_eq!(answer.result["outcome"], "imported", "{r:?}");
    assert_eq!(picker.1.lock().unwrap().len(), 1, "one dialog");
    let listed = ok(&desktop, "backups.list", json!({}));
    assert_eq!(listed["evidence"].as_array().unwrap().len(), 1);
    let wire = format!(
        "{}{}",
        serde_json::to_string(&r).unwrap(),
        Value::Object(listed)
    );
    let folder_text = folder.path().canonicalize().unwrap();
    for leak in [
        "synthetic-receipt-marker",
        "synthetic-root-marker",
        "synthetic-host-marker",
        &folder.path().file_name().unwrap().to_string_lossy(),
        &folder_text.to_string_lossy().replace('\\', "\\\\"),
    ] {
        assert!(!wire.contains(leak), "{leak} crossed IPC");
    }
}
