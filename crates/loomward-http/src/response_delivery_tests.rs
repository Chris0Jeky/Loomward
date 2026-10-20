//! Retained root/volume replies: drive the real service, blocking dispatch and single-frame HTTP
//! response body. Synthetic rows and explicit parks make the forget interleavings deterministic.

use super::*;
use loomward_catalog::{Catalog, RootObservation, WriteCommand, WriteReply};
use loomward_protocol::{DatasetClass, ResponseDeliveryPolicy};
use loomward_service::{Config, Service};
use serde_json::{json, Value};
use std::sync::{mpsc as observed, Condvar, Mutex};

const PRIVATE_NAME: &str = "SyntheticRetainedVolumeAlpha";

fn request(command: &str, payload: Value) -> RequestEnvelope {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
    serde_json::from_value(json!({
        "protocol":"loomward/3", "request_id":format!("retained_rpc_{}", NEXT.fetch_add(1, Ordering::Relaxed)),
        "command":command, "payload":payload, "deadline_ms":60_000
    })).unwrap()
}

fn successful(response: ResponseEnvelope) -> Value {
    match response {
        ResponseEnvelope::Ok(ok) => Value::Object(ok.result),
        ResponseEnvelope::Err(error) => panic!("unexpected error: {:?}", error.error),
    }
}

fn fixture() -> (tempfile::TempDir, Arc<Service>, String, String, i64) {
    let dir = tempfile::tempdir_in(loomward_service::paths::test_roots_base()).unwrap();
    let catalogue = Catalog::open(dir.path(), "synthetic").unwrap();
    let mut root_row = 0;
    for (id, name) in [(1u8, PRIVATE_NAME), (2, "SyntheticRetainedVolumeBeta")] {
        let WriteReply::Root { root_id, .. } = catalogue
            .writer()
            .call(WriteCommand::RegisterRoot(RootObservation {
                volume_guid: None,
                volume_key: "fixture".into(),
                display_name: name.into(),
                display_path: name.into(),
                root_file_id: Some(vec![id; 16]),
                filesystem: Some("NTFS".into()),
                origin: "fixture".into(),
                granted_via: "fixture".into(),
                observed_at_ns: 0,
            }))
            .unwrap()
        else {
            panic!("register synthetic root")
        };
        if id == 1 {
            root_row = root_id;
        }
    }
    drop(catalogue);
    let service = Arc::new(
        Service::open(Config {
            state_dir: dir.path().into(),
            dataset: DatasetClass::Synthetic,
            allow_personal: false,
            allow_identity_unverified: false,
            grant_roots: vec![],
        })
        .unwrap(),
    );
    let roots = successful(service.call(request("roots.list", json!({})), &CallContext::http()));
    let root = roots["roots"][0]["root_id"].as_str().unwrap().to_owned();
    let volumes =
        successful(service.call(request("volumes.list", json!({})), &CallContext::http()));
    assert_eq!(volumes["volumes"][0]["display_name"], PRIVATE_NAME);
    let volume = volumes["volumes"][0]["volume_id"]
        .as_str()
        .unwrap()
        .to_owned();
    (dir, service, root, volume, root_row)
}

fn shared(service: Arc<dyn ViewService>) -> Shared {
    Shared {
        service,
        host: String::new(),
        self_origin: String::new(),
        allow_origins: vec![],
        token: String::new(),
        static_dir: None,
        read_timeout: Duration::from_secs(5),
        write_timeout: Duration::from_secs(5),
        connection_lifetime: Duration::from_secs(30),
        sse_send_timeout: Duration::from_secs(10),
        streams: Arc::new(Semaphore::new(MAX_STREAMS)),
        stop: Arc::new(AtomicBool::new(false)),
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

async fn pending_http(shared: &Shared, request: RequestEnvelope) -> Response<Body> {
    let bytes = serde_json::to_vec(&request).unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&bytes.len().to_string()).unwrap(),
    );
    call(shared, &headers, Drained::Complete(Bytes::from(bytes))).await
}

fn body_json(runtime: &tokio::runtime::Runtime, response: Response<Body>) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = runtime
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(5), response.into_body().collect()).await
        })
        .expect("the HTTP body stalled")
        .unwrap()
        .to_bytes();
    serde_json::from_slice(&bytes).expect("a complete existing response envelope")
}

fn revoke(service: &Service, root: &str) {
    successful(service.call(
        request(
            "roots.revoke",
            json!({"root_id":root,"purge_catalog":false}),
        ),
        &CallContext::http(),
    ));
}

fn assert_withheld(value: &Value, key: &str) {
    assert!(
        !value.to_string().contains(PRIVATE_NAME),
        "an unsent HTTP reply disclosed the forgotten label"
    );
    assert_eq!(value["request_id"], key);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "permission_denied");
    assert_eq!(value["error"]["detail"]["reason"], "forgotten_result");
    assert_eq!(value["error"]["retryable"], false);
}

#[test]
fn retained_privacy_http_checks_a_serialized_reply_when_its_body_is_polled() {
    let (_dir, service, root, volume, _) = fixture();
    let shared = shared(service.clone());
    let runtime = runtime();
    let req = request("volumes.declare_tier", json!({"volume_id":volume,"tier":2}));
    // The real mutation and response serialization have finished; the body has never been polled.
    let pending = runtime.block_on(pending_http(&shared, req.clone()));
    revoke(&service, &root);
    let forget = request("roots.forget", json!({"root_id":root}));
    let forgotten = runtime.block_on(pending_http(&shared, forget.clone()));
    let receipt = body_json(&runtime, forgotten);
    assert_eq!(
        receipt["ok"], true,
        "forget must not invalidate its own receipt"
    );
    assert_eq!(receipt["request_id"], forget.request_id.as_str());
    assert_withheld(&body_json(&runtime, pending), &req.request_id);
    let replay = runtime.block_on(pending_http(&shared, req.clone()));
    assert_withheld(&body_json(&runtime, replay), &req.request_id);
    let fresh = runtime.block_on(pending_http(&shared, request("volumes.list", json!({}))));
    let fresh = body_json(&runtime, fresh);
    assert_eq!(fresh["ok"], true);
    assert!(!fresh.to_string().contains(PRIVATE_NAME));
    assert_eq!(fresh["result"]["volumes"][0]["tier"]["declared_tier"], 2);
}

#[test]
fn retained_privacy_http_invalidates_pending_replies_after_a_post_commit_purge_error() {
    let (dir, service, root, volume, root_row) = fixture();
    let shared = shared(service.clone());
    let runtime = runtime();
    let req = request("volumes.declare_tier", json!({"volume_id":volume,"tier":2}));
    let pending = runtime.block_on(pending_http(&shared, req.clone()));
    revoke(&service, &root);
    let db = rusqlite::Connection::open(dir.path().join("catalog.db")).unwrap();
    db.execute_batch(&format!("CREATE TRIGGER refuse_rpc_forget_purge BEFORE DELETE ON root WHEN OLD.id={root_row} BEGIN SELECT RAISE(ABORT,'synthetic post-commit purge failure'); END")).unwrap();
    let failed = service.call(
        request("roots.forget", json!({"root_id":root})),
        &CallContext::http(),
    );
    let ResponseEnvelope::Err(error) = failed else {
        panic!("the purge should fail after state commit")
    };
    assert_eq!(error.error.code, ErrorCode::InternalError);
    let state = rusqlite::Connection::open(dir.path().join("state.db")).unwrap();
    assert_eq!(
        state
            .query_row("SELECT count(*) FROM root_forgotten", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM root WHERE id=?1", [root_row], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        1
    );
    assert_withheld(&body_json(&runtime, pending), &req.request_id);
}

#[derive(Default)]
struct ReplyGate {
    released: Mutex<bool>,
    changed: Condvar,
}

struct ReleaseReply(Arc<ReplyGate>);
impl Drop for ReleaseReply {
    fn drop(&mut self) {
        *self.0.released.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.0.changed.notify_all();
    }
}

struct ParkedReply {
    service: Arc<Service>,
    captured: observed::Sender<()>,
    gate: Arc<ReplyGate>,
}
impl ViewService for ParkedReply {
    fn call(&self, request: RequestEnvelope, ctx: &CallContext) -> ResponseEnvelope {
        let reply = self.service.call(request, ctx);
        assert!(serde_json::to_string(&reply)
            .unwrap()
            .contains(PRIVATE_NAME));
        self.captured.send(()).unwrap();
        let mut released = self.gate.released.lock().unwrap();
        while !*released {
            released = self.gate.changed.wait(released).unwrap();
        }
        reply
    }
    fn response_delivery_policy(&self, command: Command) -> Option<ResponseDeliveryPolicy> {
        self.service.response_delivery_policy(command)
    }
    fn subscribe(&self, resume: Option<(String, u64)>) -> Box<dyn EventStream> {
        self.service.subscribe(resume)
    }
}

#[test]
fn retained_privacy_http_stamps_before_the_blocking_call_returns_its_owned_reply() {
    let (_dir, service, root, _, _) = fixture();
    // On an assertion failure, release the parked blocking worker before Runtime::drop waits
    // for that worker. The guard must therefore be declared after the runtime.
    let runtime = runtime();
    let (captured, received) = observed::channel();
    let gate = Arc::new(ReplyGate::default());
    let release = ReleaseReply(gate.clone());
    let shared = Arc::new(shared(Arc::new(ParkedReply {
        service: service.clone(),
        captured,
        gate,
    })));
    let req = request("volumes.list", json!({}));
    let key = req.request_id.to_string();
    let task = runtime.spawn(async move { pending_http(&shared, req).await });
    received
        .recv_timeout(Duration::from_secs(5))
        .expect("the real private reply was not captured");
    revoke(&service, &root);
    successful(service.call(
        request("roots.forget", json!({"root_id":root})),
        &CallContext::http(),
    ));
    drop(release);
    let pending = runtime.block_on(task).unwrap();
    assert_withheld(&body_json(&runtime, pending), &key);
}

#[test]
fn retained_privacy_alerts_http_withholds_pending_list_and_configuration_reports() {
    let (_dir, service, root, _, _) = fixture();
    let shared = shared(service.clone());
    let runtime = runtime();
    let initial = successful(service.call(request("alerts.list", json!({})), &CallContext::http()));
    assert!(initial.to_string().contains(PRIVATE_NAME));
    let mut settings = initial["settings"].clone();
    settings["low_space_free_percent"] = json!(7);
    let list = request("alerts.list", json!({}));
    let configure = request("alerts.configure", settings);
    let listed = runtime.block_on(pending_http(&shared, list.clone()));
    let configured = runtime.block_on(pending_http(&shared, configure.clone()));
    revoke(&service, &root);
    successful(service.call(
        request("roots.forget", json!({"root_id":root})),
        &CallContext::http(),
    ));
    assert_withheld(&body_json(&runtime, listed), &list.request_id);
    assert_withheld(&body_json(&runtime, configured), &configure.request_id);
    let retry = runtime.block_on(pending_http(&shared, configure.clone()));
    assert_withheld(&body_json(&runtime, retry), &configure.request_id);
    let fresh = body_json(
        &runtime,
        runtime.block_on(pending_http(&shared, request("alerts.list", json!({})))),
    );
    assert_eq!(fresh["ok"], true);
    assert!(!fresh.to_string().contains(PRIVATE_NAME));
    assert_eq!(fresh["result"]["settings"]["low_space_free_percent"], 7);
}

fn rejected_forget_preserves_http_body(writer_failure: bool) {
    let (dir, service, root, volume, _) = fixture();
    let shared = shared(service.clone());
    let runtime = runtime();
    let req = request("volumes.declare_tier", json!({"volume_id":volume,"tier":2}));
    let pending = runtime.block_on(pending_http(&shared, req.clone()));
    revoke(&service, &root);
    let state = rusqlite::Connection::open(dir.path().join("state.db")).unwrap();
    let mut forget = request("roots.forget", json!({"root_id":root}));
    if writer_failure {
        state
            .execute_batch(
                "CREATE TRIGGER reject_response_forget BEFORE INSERT ON root_forgotten
             BEGIN SELECT RAISE(ABORT, 'synthetic pre-commit forget failure'); END",
            )
            .unwrap();
    } else {
        forget.expected_state_rev = Some(loomward_protocol::Generation(0));
    }
    let rejected = service.call(forget, &CallContext::http());
    let ResponseEnvelope::Err(error) = rejected else {
        panic!("the forget should be rejected before its marker commits")
    };
    assert_eq!(
        error.error.code,
        if writer_failure {
            ErrorCode::InternalError
        } else {
            ErrorCode::StaleGeneration
        }
    );
    assert_eq!(
        state
            .query_row("SELECT count(*) FROM root_forgotten", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let delivered = body_json(&runtime, pending);
    assert_eq!(delivered["request_id"], req.request_id.as_str());
    assert_eq!(
        delivered["ok"], true,
        "a rejected forget withdrew a successful HTTP reply"
    );
    assert!(delivered.to_string().contains(PRIVATE_NAME));
    let replay = runtime.block_on(pending_http(&shared, req));
    assert_eq!(body_json(&runtime, replay), delivered);
    assert_eq!(
        state
            .query_row("SELECT count(*) FROM tier_declaration", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn retained_privacy_rejected_forget_http_preserves_body_after_a_stale_revision() {
    rejected_forget_preserves_http_body(false);
}

#[test]
fn retained_privacy_rejected_forget_http_preserves_body_after_writer_rollback() {
    rejected_forget_preserves_http_body(true);
}

#[test]
fn retained_privacy_http_body_waits_asynchronously_for_a_rejected_forget() {
    use std::task::{Context, Poll, Waker};

    let (_dir, service, _, volume, _) = fixture();
    let shared = shared(service.clone());
    let runtime = runtime();
    let req = request("volumes.declare_tier", json!({"volume_id":volume,"tier":2}));
    let pending = runtime.block_on(pending_http(&shared, req.clone()));
    let policy = service
        .response_delivery_policy(Command::VolumesDeclareTier)
        .unwrap();
    let pause = policy.suspend();
    let mut body = std::pin::pin!(pending.into_body().collect());
    let mut cx = Context::from_waker(Waker::noop());
    assert!(body.as_mut().poll(&mut cx).is_pending());
    pause.resume();
    let Poll::Ready(delivered) = body.as_mut().poll(&mut cx) else {
        panic!("the resolved response pause did not release the HTTP body")
    };
    let bytes = delivered.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["request_id"], req.request_id.as_str());
    assert_eq!(value["ok"], true);
    assert!(value.to_string().contains(PRIVATE_NAME));
}

#[test]
fn retained_privacy_cancelled_http_body_releases_its_waiter() {
    use std::sync::atomic::AtomicUsize;
    use std::task::{Context, Wake, Waker};

    struct WakeCount(AtomicUsize);
    impl Wake for WakeCount {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let (_dir, service, _, volume, _) = fixture();
    let shared = shared(service.clone());
    let runtime = runtime();
    let req = request("volumes.declare_tier", json!({"volume_id":volume,"tier":2}));
    let pending = runtime.block_on(pending_http(&shared, req));
    let policy = service
        .response_delivery_policy(Command::VolumesDeclareTier)
        .unwrap();
    let pause = policy.suspend();
    let wakes = Arc::new(WakeCount(AtomicUsize::new(0)));
    let waker = Waker::from(wakes.clone());
    let mut body = Box::pin(pending.into_body().collect());
    let mut cx = Context::from_waker(&waker);
    assert!(body.as_mut().poll(&mut cx).is_pending());
    drop(body);
    drop(waker);
    assert_eq!(Arc::strong_count(&wakes), 1);
    pause.resume();
    assert_eq!(wakes.0.load(Ordering::SeqCst), 0);
}
