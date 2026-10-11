//! Command-layer tests: the self test, the refusals that guard both native dialogs, the
//! disclosure dialog's text, and Tauri's own IPC + ACL (mock runtime) proving the webview can
//! invoke `lw_call` and `lw_events` and nothing else.

use loomward_desktop::dialogs::{disclosure_text, visible, DesktopDialogs, DISCLOSURE_TITLE};
use loomward_desktop::selftest::{self, envelope, fixture_desktop, wait_until};
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
fn disclosure_dialog_accept_and_decline_paths() {
    for (dataset, wire, other) in [
        (DatasetClass::Synthetic, "synthetic", "personal"),
        (DatasetClass::Personal, "personal", "synthetic"),
    ] {
        let s = summary(wire, None);
        for answer in [true, false] {
            let (dialogs, shown) = recording(dataset, answer);
            assert_eq!(
                dialogs.confirm_disclosure(&s),
                answer && dataset == DatasetClass::Synthetic
            );
            assert_eq!(
                shown.lock().unwrap().len(),
                usize::from(dataset == DatasetClass::Synthetic)
            );
        }
        // A summary of the other class is declined without asking.
        let (dialogs, shown) = recording(dataset, true);
        assert!(!dialogs.confirm_disclosure(&summary(other, None)));
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
fn the_webview_may_invoke_exactly_lw_call_and_lw_events() {
    let (service, desktop) = fixture_desktop(Duration::from_millis(300)).unwrap();
    let desktop = Arc::new(desktop);
    // The real context (tauri.conf.json and the compiled capability ACL), minus the configured
    // window, which the test creates itself under the same label.
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    let app = shell::configure(tauri::test::mock_builder(), desktop.clone())
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

    for (cmd, body) in [
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
    assert!(!dialogs.confirm_disclosure(&long));
    assert!(shown.lock().unwrap().is_empty());
    assert!(dialogs.confirm_disclosure(&summary("synthetic", None)));
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
        !dialogs.confirm_disclosure(&exact),
        "wrapping also consumes the height budget"
    );
    let last = items.last_mut().unwrap();
    last["context"] = json!(format!("{}c", last["context"].as_str().unwrap()));
    assert!(!dialogs.confirm_disclosure(&summary("synthetic", Some(items))));
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
    let first = std::thread::spawn(move || d.confirm_disclosure(&summary("synthetic", None)));
    rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let second = dialogs.confirm_disclosure(&summary("synthetic", None));
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
fn disclosure_height_budget_declines_short_many_items_and_wrapped_lines() {
    for (count, context) in [(25, "c".to_string()), (3, "c".repeat(400))] {
        let items = (0..count)
            .map(|i| {
                json!({ "handle": format!("i{i:02}"),
            "name": "n", "extension": "txt", "context": context, "size_bucket": 0 })
            })
            .collect();
        let s = summary("synthetic", Some(items));
        assert!(disclosure_text(&s).len() < loomward_desktop::dialogs::MAX_DISCLOSURE_CHARS);
        let (dialogs, shown) = recording(DatasetClass::Synthetic, true);
        assert!(!dialogs.confirm_disclosure(&s));
        assert!(shown.lock().unwrap().is_empty());
        assert!(dialogs.confirm_disclosure(&summary("synthetic", None)));
    }
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
    let tmp = tempfile::tempdir_in(".").unwrap();
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
    let tmp = tempfile::tempdir_in(".").unwrap();
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
    let tmp = tempfile::tempdir_in(".").unwrap();
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
