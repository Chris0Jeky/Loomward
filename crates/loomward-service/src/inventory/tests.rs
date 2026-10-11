use super::*;
use crate::{Config, Service};
use loomward_protocol::{CallContext, ResponseEnvelope, ViewService};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Condvar, Mutex,
    },
    time::Duration,
};

fn open(
    dir: &std::path::Path,
    dataset: DatasetClass,
    provider: Option<Arc<dyn Provider>>,
) -> Service {
    Service::open_stores(
        Config {
            state_dir: dir.into(),
            dataset,
            allow_personal: true,
            allow_identity_unverified: false,
            grant_roots: vec![],
        },
        crate::idem::TTL,
        true,
        None,
        provider,
    )
    .unwrap()
}
fn call(s: &Service, cmd: &str, payload: Value, deadline: u64) -> Value {
    let request: RequestEnvelope = serde_json::from_value(json!({"protocol": "loomward/3", "request_id": "metadata_test", "command": cmd, "payload": payload, "deadline_ms": deadline})).unwrap();
    let response = s.call(request, &CallContext::http());
    serde_json::to_value(response).unwrap()
}
struct Probe {
    calls: AtomicUsize,
    delay: Duration,
    dataset: DatasetClass,
    /// `read` waits until this is true: a test holds a provider mid-read without a wall-clock delay.
    released: (Mutex<bool>, Condvar),
}
impl Probe {
    fn new(delay: Duration, dataset: DatasetClass) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            delay,
            dataset,
            released: (Mutex::new(true), Condvar::new()),
        }
    }
    fn hold(&self) {
        *self.released.0.lock().unwrap() = false;
    }
    fn release(&self) {
        *self.released.0.lock().unwrap() = true;
        self.released.1.notify_all();
    }
}
impl Provider for Probe {
    fn dataset(&self) -> DatasetClass {
        self.dataset
    }
    fn read(&self, cmd: Command, deadline: Instant) -> Result<Value, ErrorBody> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        let mut open = self.released.0.lock().unwrap();
        while !*open {
            open = self.released.1.wait(open).unwrap();
        }
        drop(open);
        Fixtures.read(cmd, deadline)
    }
}

#[test]
fn injected_synthetic_pages_bind_session_command_and_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let probe = Arc::new(Probe::new(Duration::ZERO, DatasetClass::Synthetic));
    let s = open(tmp.path(), DatasetClass::Synthetic, Some(probe.clone()));
    let a = call(&s, "apps.list", json!({"limit": 1, "cursor": null}), 2000);
    assert_eq!(a["ok"], true);
    assert_eq!(a["meta"]["dataset_class"], "synthetic");
    assert_eq!(
        a["result"]["applications"][0]["estimated_size_bytes"],
        "1048576"
    );
    assert_eq!(a["result"]["providers"][2]["state"], "denied");
    let cursor = a["result"]["next_cursor"].clone();
    let b = call(
        &s,
        "apps.list",
        json!({"limit": 200, "cursor": cursor}),
        2000,
    );
    assert_eq!(b["result"]["applications"].as_array().unwrap().len(), 1);
    assert!(b["result"]["applications"][0]["estimated_size_bytes"].is_null());
    assert_eq!(a["result"]["observed_at"], b["result"]["observed_at"]);
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        call(
            &s,
            "desktop.contexts",
            json!({"limit": 1, "cursor": cursor}),
            2000
        )["error"]["code"],
        "invalid_request"
    );
    let foreign = tempfile::tempdir().unwrap();
    let other = open(foreign.path(), DatasetClass::Synthetic, None);
    assert_eq!(
        call(
            &other,
            "apps.list",
            json!({"limit": 1, "cursor": cursor}),
            2000
        )["error"]["detail"]["reason"],
        "cursor_expired"
    );
    let desktop = call(
        &s,
        "desktop.contexts",
        json!({"limit": 200, "cursor": null}),
        2000,
    );
    assert_eq!(desktop["ok"], true);
    assert_eq!(desktop["result"]["contexts"][0]["redirected"], true);
    assert_eq!(desktop["result"]["shortcuts"][0]["health"], "missing");
    assert_eq!(
        desktop["result"]["shortcuts"][1]["health"],
        "volume_unavailable"
    );
    assert_eq!(desktop["result"]["contexts"][1]["ownership"], "public");
    assert_eq!(
        call(&s, "apps.list", json!({"limit": 201, "cursor": null}), 2000)["error"]["code"],
        "invalid_request"
    );
}

#[test]
fn wrong_dataset_and_timed_out_providers_never_publish_fallback() {
    let tmp = tempfile::tempdir().unwrap();
    let mismatch = Arc::new(Probe::new(Duration::ZERO, DatasetClass::Personal));
    let s = open(tmp.path(), DatasetClass::Synthetic, Some(mismatch.clone()));
    assert_eq!(
        call(&s, "apps.list", json!({"limit": 200, "cursor": null}), 2000)["error"]["detail"]
            ["reason"],
        "metadata_dataset_mismatch"
    );
    assert_eq!(mismatch.calls.load(Ordering::SeqCst), 0);
    drop(s);
    let slow = Arc::new(Probe::new(Duration::ZERO, DatasetClass::Synthetic));
    slow.hold();
    let s = open(tmp.path(), DatasetClass::Synthetic, Some(slow.clone()));
    // The provider is parked mid-read, so the call can only come back by its deadline.
    assert_eq!(
        call(
            &s,
            "desktop.contexts",
            json!({"limit": 200, "cursor": null}),
            20
        )["error"]["code"],
        "deadline_exceeded"
    );
    assert_eq!(slow.calls.load(Ordering::SeqCst), 1);
    assert_eq!(*s.inner.reads.lock().unwrap(), 1);
    slow.release();
    let until = Instant::now() + Duration::from_secs(60);
    while *s.inner.reads.lock().unwrap() != 0 {
        assert!(Instant::now() < until, "the read slot was never released");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn native_personal_contexts_refuse_shortcut_reach_and_effects() {
    let tmp = tempfile::tempdir().unwrap();
    let s = open(tmp.path(), DatasetClass::Personal, None);
    let result = call(
        &s,
        "desktop.contexts",
        json!({"limit": 200, "cursor": null}),
        2000,
    );
    if personal_admission().is_err() {
        assert_eq!(result["error"]["code"], "capability_unavailable");
        assert!(result["error"]["detail"]["reason"]
            .as_str()
            .unwrap()
            .starts_with("personal_metadata_"));
        let applications = call(&s, "apps.list", json!({"limit": 200, "cursor": null}), 2000);
        assert_eq!(applications["error"]["code"], "capability_unavailable");
        return;
    }
    assert_eq!(result["ok"], true);
    assert_eq!(result["result"]["contexts"].as_array().unwrap().len(), 5);
    assert_eq!(result["result"]["shortcuts"], json!([]));
    for p in result["result"]["providers"].as_array().unwrap() {
        assert_eq!(p["state"], "unavailable");
        assert!(p["reason"]
            .as_str()
            .unwrap()
            .contains("desktop_enumeration_unavailable"));
    }
    let _typed: ResponseEnvelope = serde_json::from_value(result).unwrap();
    let applications = call(&s, "apps.list", json!({"limit": 200, "cursor": null}), 2000);
    assert_eq!(applications["ok"], true);
    assert_eq!(
        applications["result"]["providers"]
            .as_array()
            .unwrap()
            .len(),
        5
    );
    assert!(
        applications["result"]["applications"]
            .as_array()
            .unwrap()
            .len()
            <= 200
    );
}

#[test]
fn projections_keep_registry_evidence_and_folder_unknowns() {
    use loomward_apps::inventory::*;
    struct Registry;
    impl RegistryReader for Registry {
        fn read(&self, _: RegistryView) -> RegistryRead {
            RegistryRead {
                state: ProviderState::Available,
                reason: None,
                entries: vec![RegistryEntry {
                    key: "Synthetic".into(),
                    values: std::collections::BTreeMap::from([
                        (
                            "DisplayName".into(),
                            RegistryValue::Bytes {
                                kind: 2,
                                data: "%SYNTHETIC%"
                                    .encode_utf16()
                                    .chain([0])
                                    .flat_map(u16::to_le_bytes)
                                    .collect(),
                            },
                        ),
                        (
                            "EstimatedSize".into(),
                            RegistryValue::Bytes {
                                kind: 4,
                                data: 7u32.to_le_bytes().to_vec(),
                            },
                        ),
                    ]),
                }],
            }
        }
    }
    let result = crate::apps::project(
        inventory(&Registry),
        Instant::now() + Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(result["applications"][0]["name"]["value"], "%SYNTHETIC%");
    assert_eq!(result["applications"][0]["name"]["expandable"], true);
    assert_eq!(result["applications"][0]["estimated_size_bytes"], "7168");
    assert_eq!(result["providers"][4]["state"], "unavailable");
    struct Folders;
    impl loomward_shell::known_folders::FolderProvider for Folders {
        fn location(
            &self,
            _: loomward_shell::known_folders::Folder,
        ) -> loomward_shell::known_folders::Location {
            loomward_shell::known_folders::Location::FileSystem {
                current: "C:/Synthetic".into(),
                default: None,
            }
        }
    }
    let result =
        crate::desktop::project(&Folders, Instant::now() + Duration::from_secs(1)).unwrap();
    assert!(result["contexts"][0]["redirected"].is_null());
    assert!(result["contexts"][0]["default_path"].is_null());
}

#[test]
fn metadata_snapshots_and_cursors_have_finite_budgets() {
    struct Oversized;
    impl Provider for Oversized {
        fn dataset(&self) -> DatasetClass {
            DatasetClass::Synthetic
        }
        fn read(&self, _: Command, _: Instant) -> Result<Value, ErrorBody> {
            Ok(json!({"applications": [], "synthetic_oversized": "x".repeat(4 * 1024 * 1024)}))
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    let s = open(
        tmp.path(),
        DatasetClass::Synthetic,
        Some(Arc::new(Oversized)),
    );
    assert_eq!(
        call(&s, "apps.list", json!({"limit": 200, "cursor": null}), 2000)["error"]["detail"]
            ["reason"],
        "metadata_snapshot_budget"
    );
    drop(s);
    let s = open(tmp.path(), DatasetClass::Synthetic, None);
    let first = call(&s, "apps.list", json!({"limit": 1, "cursor": null}), 2000)["result"]
        ["next_cursor"]
        .clone();
    for _ in 0..16 {
        assert_eq!(
            call(&s, "apps.list", json!({"limit": 1, "cursor": null}), 2000)["ok"],
            true
        );
    }
    assert_eq!(
        call(&s, "apps.list", json!({"limit": 1, "cursor": first}), 2000)["error"]["detail"]
            ["reason"],
        "cursor_expired"
    );
}

#[test]
fn metadata_entry_bounds_refuse_the_whole_page_and_issue_no_cursor() {
    struct Rows {
        key: &'static str,
        count: usize,
    }
    impl Provider for Rows {
        fn dataset(&self) -> DatasetClass {
            DatasetClass::Synthetic
        }
        fn read(&self, command: Command, deadline: Instant) -> Result<Value, ErrorBody> {
            let mut value = Fixtures.read(command, deadline)?;
            let row = value[self.key][0].clone();
            value[self.key] = Value::Array(vec![row; self.count]);
            Ok(value)
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    let at_apps_bound = open(
        tmp.path(),
        DatasetClass::Synthetic,
        Some(Arc::new(Rows {
            key: "applications",
            count: 6144,
        })),
    );
    let page = call(
        &at_apps_bound,
        "apps.list",
        json!({"limit": 1, "cursor": null}),
        2000,
    );
    assert_eq!(page["ok"], true, "{page}");
    assert!(!page["result"]["next_cursor"].is_null());
    drop(at_apps_bound);

    let over_apps_bound = open(
        tmp.path(),
        DatasetClass::Synthetic,
        Some(Arc::new(Rows {
            key: "applications",
            count: 6145,
        })),
    );
    let refused = call(
        &over_apps_bound,
        "apps.list",
        json!({"limit": 1, "cursor": null}),
        2000,
    );
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["error"]["code"], "resource_budget");
    assert!(refused["error"]["detail"].is_null());
    assert!(refused["result"].is_null());
    drop(over_apps_bound);

    let at_shortcut_bound = open(
        tmp.path(),
        DatasetClass::Synthetic,
        Some(Arc::new(Rows {
            key: "shortcuts",
            count: 400,
        })),
    );
    assert_eq!(
        call(
            &at_shortcut_bound,
            "desktop.contexts",
            json!({"limit": 1, "cursor": null}),
            2000
        )["ok"],
        true
    );
    drop(at_shortcut_bound);

    let over_shortcut_bound = open(
        tmp.path(),
        DatasetClass::Synthetic,
        Some(Arc::new(Rows {
            key: "shortcuts",
            count: 401,
        })),
    );
    let refused = call(
        &over_shortcut_bound,
        "desktop.contexts",
        json!({"limit": 1, "cursor": null}),
        2000,
    );
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["error"]["code"], "resource_budget");
    assert!(refused["error"]["detail"].is_null());
    assert!(refused["result"].is_null());
}

#[test]
fn timed_out_metadata_page_does_not_evict_a_received_cursor() {
    let tmp = tempfile::tempdir().unwrap();
    let s = Arc::new(open(tmp.path(), DatasetClass::Synthetic, None));
    let first = call(&s, "apps.list", json!({"limit": 1, "cursor": null}), 2000)["result"]
        ["next_cursor"]
        .clone();
    for _ in 0..15 {
        assert_eq!(
            call(&s, "apps.list", json!({"limit": 1, "cursor": null}), 2000)["ok"],
            true
        );
    }
    s.inner.hooks.arrived.lock().unwrap().clear();
    s.inner
        .hooks
        .closed
        .lock()
        .unwrap()
        .push((Command::AppsList, "after"));
    let caller = s.clone();
    let worker = std::thread::spawn(move || {
        call(
            &caller,
            "apps.list",
            json!({"limit": 1, "cursor": null}),
            500,
        )
    });
    s.inner.hooks.until("metadata response held", |h| {
        h.arrived
            .lock()
            .unwrap()
            .contains(&(Command::AppsList, "after"))
    });
    let timeout = worker.join().unwrap();
    s.inner.hooks.open(Command::AppsList, "after");
    s.inner.hooks.until("metadata worker finished", |_| {
        *s.inner.reads.lock().unwrap() == 0
    });
    assert_eq!(timeout["error"]["code"], "deadline_exceeded");
    let resumed = call(
        &s,
        "apps.list",
        json!({"limit": 200, "cursor": first}),
        2000,
    );
    assert_eq!(resumed["ok"], true, "{resumed}");
    assert_eq!(
        resumed["result"]["applications"].as_array().unwrap().len(),
        1
    );
}

#[test]
fn metadata_encoding_stops_before_serializing_an_over_budget_tail() {
    struct Oversized;
    impl serde::Serialize for Oversized {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            use serde::ser::SerializeSeq;
            let mut sequence = serializer.serialize_seq(Some(2))?;
            sequence.serialize_element(&"x".repeat(SNAPSHOT_BYTES))?;
            panic!("serializer reached the tail after exceeding its budget");
        }
    }
    assert_eq!(
        encoded_size(&json!({"synthetic": "😀\n\""})).unwrap(),
        serde_json::to_vec(&json!({"synthetic": "😀\n\""}))
            .unwrap()
            .len()
    );
    assert_eq!(
        encoded_size(&Oversized).unwrap_err().code,
        ErrorCode::ResourceBudget
    );
}

#[test]
fn application_projection_refuses_cumulative_encoded_bytes_before_full_snapshot() {
    use loomward_apps::inventory::*;
    struct Registry;
    impl RegistryReader for Registry {
        fn read(&self, _: RegistryView) -> RegistryRead {
            RegistryRead {
                state: ProviderState::Available,
                reason: None,
                entries: vec![RegistryEntry {
                    key: "synthetic".into(),
                    values: std::collections::BTreeMap::new(),
                }],
            }
        }
    }
    let mut providers = inventory(&Registry);
    let mut app = providers[0].applications[0].clone();
    app.display_name = Evidence::Known(Text {
        value: "\"".repeat(1024),
        expandable: false,
    });
    providers[0].applications = vec![app; 2048];
    let result = crate::apps::project(providers, Instant::now() + Duration::from_secs(10));
    assert_eq!(
        result.unwrap_err().detail.unwrap()["reason"],
        "metadata_snapshot_budget"
    );
}

#[test]
fn metadata_modules_expose_no_effect_or_unbounded_filesystem_api() {
    for source in [
        include_str!("../apps.rs"),
        include_str!("../desktop.rs"),
        include_str!("../inventory.rs"),
    ] {
        for forbidden in [
            "std::process",
            "RegSet",
            "RegCreate",
            "std::fs",
            "shortcuts::read",
            "read_dir",
            "ShellExecute",
            "LoadLibrary",
            "CoCreateInstance",
        ] {
            assert!(
                !source.contains(forbidden),
                "forbidden effect or discovery API: {forbidden}"
            );
        }
    }
    let registry = include_str!("../../../loomward-apps/src/inventory/windows.rs");
    assert!(registry.contains("KEY_READ | KEY_WOW64_32KEY"));
    assert!(registry.contains("KEY_READ | KEY_WOW64_64KEY"));
    assert!(registry.contains("\"EstimatedSize\""));
    assert!(!registry.contains("KEY_WRITE"));
}
