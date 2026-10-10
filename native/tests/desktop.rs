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
            assert_eq!(dialogs.confirm_disclosure(&s), answer);
            assert_eq!(shown.lock().unwrap().len(), 1);
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
    tauri::test::get_ipc_response(
        webview,
        tauri::webview::InvokeRequest {
            cmd: cmd.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: webview.url().unwrap(),
            body: tauri::ipc::InvokeBody::Json(body),
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
        let accepted = message_box(DISCLOSURE_TITLE, &text);
        let default = clicker.join().unwrap().expect("the box never appeared");
        assert_eq!(default, IDCANCEL, "Enter must decline");
        assert_eq!(accepted, want, "button {button}");
    }
}
