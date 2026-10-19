//! #769: drive the production SSE body and catalogue-backed service, with the body deliberately
//! unpolled after hello. A wrapper observes the next receive only after the previous pump send
//! finished, so the test proves the sensitive event is in the adapter queue before forgetting it.

use super::*;
use loomward_catalog::{Catalog, RootObservation, WriteCommand, WriteReply};
use loomward_protocol::{DatasetClass, EventName};
use loomward_service::{Config, Service};
use serde_json::{json, Value};
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc as observed;
use std::sync::{Condvar, Mutex};

const FORGOTTEN_PATH: &str = "SyntheticQueuedForgetAlpha";
const KEPT_PATH: &str = "SyntheticQueuedKeepBeta";

struct ObservedService {
    service: Arc<Service>,
    received: observed::Sender<u64>,
    sent: observed::Sender<u64>,
    receive_gate: Option<Arc<ReceiveGate>>,
}

struct ObservedStream {
    stream: Box<dyn EventStream>,
    received: observed::Sender<u64>,
    sent: observed::Sender<u64>,
    previous: Option<u64>,
    receive_gate: Option<Arc<ReceiveGate>>,
}

#[derive(Default)]
struct ReceiveGate {
    released: Mutex<bool>,
    changed: Condvar,
}

struct ReleaseReceive(Option<Arc<ReceiveGate>>);
impl Drop for ReleaseReceive {
    fn drop(&mut self) {
        if let Some(gate) = &self.0 {
            *gate.released.lock().unwrap() = true;
            gate.changed.notify_all();
        }
    }
}

enum PendingPlace {
    Queue(usize),
    Receiving,
}

impl ViewService for ObservedService {
    fn call(&self, request: RequestEnvelope, context: &CallContext) -> ResponseEnvelope {
        self.service.call(request, context)
    }

    fn subscribe(&self, resume: Option<(String, u64)>) -> Box<dyn EventStream> {
        Box::new(ObservedStream {
            stream: self.service.subscribe(resume),
            received: self.received.clone(),
            sent: self.sent.clone(),
            previous: None,
            receive_gate: self.receive_gate.clone(),
        })
    }
}

impl EventStream for ObservedStream {
    fn epoch(&self) -> &str {
        self.stream.epoch()
    }

    fn delivery_policy(&self) -> loomward_protocol::EventDeliveryPolicy {
        self.stream.delivery_policy()
    }

    fn recv_timeout(&mut self, timeout: Duration) -> RecvOutcome {
        // The real pump reaches this call only after send_timeout accepted the previous frame.
        if let Some(seq) = self.previous.take() {
            let _ = self.sent.send(seq);
        }
        let event = self.stream.recv_timeout(timeout);
        if let RecvOutcome::Event(e) = &event {
            if e.event == EventName::RootsChanged {
                self.previous = Some(e.seq.get());
                let _ = self.received.send(e.seq.get());
                // Hold a real owned Bus clone inside recv_timeout. A stamp captured only after
                // this method returns would wrongly bless that old clone after the forget.
                if let Some(gate) = self.receive_gate.take() {
                    let mut released = gate.released.lock().unwrap();
                    while !*released {
                        released = gate.changed.wait(released).unwrap();
                    }
                }
            }
        }
        event
    }

    fn close(&mut self) {
        self.stream.close();
    }
}

fn command(service: &Service, command: &str, payload: Value) -> ResponseEnvelope {
    static NEXT: AtomicUsize = AtomicUsize::new(1);
    let request = serde_json::from_value(json!({
        "protocol": "loomward/3",
        "request_id": format!("r_forget_queue_{}", NEXT.fetch_add(1, Ordering::Relaxed)),
        "command": command,
        "payload": payload,
    }))
    .unwrap();
    service.call(request, &CallContext::http())
}

fn successful(response: ResponseEnvelope) -> Value {
    match response {
        ResponseEnvelope::Ok(ok) => Value::Object(ok.result),
        ResponseEnvelope::Err(error) => panic!("unexpected service error: {:?}", error.error),
    }
}

fn body_frame(runtime: &tokio::runtime::Runtime, body: &mut Body) -> Option<Bytes> {
    runtime
        .block_on(async { tokio::time::timeout(Duration::from_secs(5), body.frame()).await })
        .expect("the production response body stalled")
        .map(|frame| frame.unwrap().into_data().expect("SSE data frame"))
}

fn pending_forget(place: PendingPlace, fail_purge: bool) {
    let (queued, hold_receive) = match place {
        PendingPlace::Queue(count) => (count, false),
        PendingPlace::Receiving => (1, true),
    };
    let tmp = tempfile::tempdir_in(loomward_service::paths::test_roots_base()).unwrap();
    let catalogue = Catalog::open(tmp.path(), "synthetic").unwrap();
    let mut forgotten_grant = 0;
    let mut forgotten_root = 0;
    for id in 0..=queued {
        let path = if id == 0 {
            FORGOTTEN_PATH.to_owned()
        } else if id == queued {
            KEPT_PATH.to_owned()
        } else {
            format!("SyntheticOtherQueuedRoot{id}")
        };
        let WriteReply::Root {
            root_id, grant_id, ..
        } = catalogue
            .writer()
            .call(WriteCommand::RegisterRoot(RootObservation {
                volume_guid: None,
                volume_key: "fixture".into(),
                display_name: path.clone(),
                display_path: path,
                root_file_id: Some(vec![id as u8 + 1; 16]),
                filesystem: Some("NTFS".into()),
                origin: "fixture".into(),
                granted_via: "fixture".into(),
                observed_at_ns: 0,
            }))
            .unwrap()
        else {
            panic!("register synthetic root");
        };
        if id == 0 {
            forgotten_grant = grant_id;
            forgotten_root = root_id;
        }
    }
    drop(catalogue);
    let service = Arc::new(
        Service::open(Config {
            state_dir: tmp.path().to_path_buf(),
            dataset: DatasetClass::Synthetic,
            allow_personal: false,
            allow_identity_unverified: false,
            grant_roots: vec![],
        })
        .unwrap(),
    );
    let roots = successful(command(&service, "roots.list", json!({})));
    let roots = roots["roots"].as_array().unwrap();
    assert_eq!(roots[0]["display_path"]["text"], FORGOTTEN_PATH);
    let forgotten_id = roots[0]["root_id"].clone();
    let (received_tx, received) = observed::channel();
    let (sent_tx, sent) = observed::channel();
    let receive_gate = hold_receive.then(|| Arc::new(ReceiveGate::default()));
    // Release even if an assertion unwinds while the real pump owns its event clone.
    let release_receive = ReleaseReceive(receive_gate.clone());
    let shared = Shared {
        service: Arc::new(ObservedService {
            service: service.clone(),
            received: received_tx,
            sent: sent_tx,
            receive_gate,
        }),
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
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let response = runtime.block_on(events(&shared, &HeaderMap::new()));
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    let first = body_frame(&runtime, &mut body).unwrap();
    let first = std::str::from_utf8(&first).unwrap();
    let resume = first
        .lines()
        .find_map(|line| line.strip_prefix("id: "))
        .unwrap()
        .to_string();

    for root in roots.iter().take(queued) {
        successful(command(
            &service,
            "roots.revoke",
            json!({"root_id": root["root_id"], "purge_catalog": false}),
        ));
    }
    if !hold_receive {
        for _ in 0..queued.min(32) {
            sent.recv_timeout(Duration::from_secs(5))
                .expect("the root event was not accepted into the actual SSE queue");
        }
    }
    for _ in 0..queued {
        received
            .recv_timeout(Duration::from_secs(5))
            .expect("the pump did not receive the expected event");
    }
    if queued > 32 {
        assert!(
            sent.try_recv().is_err(),
            "the 33rd send must remain blocked"
        );
    }

    let connection =
        rusqlite::Connection::open(loomward_catalog::sqlite_dir(tmp.path()).join("catalog.db"))
            .unwrap();
    if fail_purge {
        connection
            .execute_batch(&format!(
                "CREATE TRIGGER refuse_forget_purge BEFORE DELETE ON root WHEN OLD.id={forgotten_root} BEGIN SELECT RAISE(ABORT, 'synthetic post-commit purge failure'); END"
            ))
            .unwrap();
    }
    let forgotten = command(&service, "roots.forget", json!({"root_id": forgotten_id}));
    if fail_purge {
        assert!(matches!(forgotten, ResponseEnvelope::Err(_)));
        let state =
            rusqlite::Connection::open(loomward_catalog::sqlite_dir(tmp.path()).join("state.db"))
                .unwrap();
        assert!(state
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM root_forgotten WHERE grant_id=?1)",
                [forgotten_grant],
                |row| row.get::<_, bool>(0),
            )
            .unwrap());
    } else {
        successful(forgotten);
    }
    drop(release_receive);

    // These bytes have never been admitted to the response body: the test left it unpolled.
    // Closing an invalidated stream is acceptable; delivering its old sensitive frame is not.
    if let Some(frame) = body_frame(&runtime, &mut body) {
        assert!(
            !std::str::from_utf8(&frame)
                .unwrap()
                .contains(FORGOTTEN_PATH),
            "an unsent HTTP frame named the forgotten root after roots.forget settled"
        );
    }
    drop(body);

    // A reconnect must still preserve the other roots in replay, including after a failed purge.
    let mut headers = HeaderMap::new();
    headers.insert("last-event-id", resume.parse().unwrap());
    let response = runtime.block_on(events(&shared, &headers));
    assert_eq!(response.status(), StatusCode::OK);
    let mut replay = response.into_body();
    body_frame(&runtime, &mut replay).expect("replay hello");
    let retained = body_frame(&runtime, &mut replay).expect("retained roots.changed");
    let retained = std::str::from_utf8(&retained).unwrap();
    assert!(!retained.contains(FORGOTTEN_PATH));
    assert!(retained.contains(KEPT_PATH));
    drop(replay);
    shared.stop.store(true, Ordering::SeqCst);
}

#[test]
fn forget_invalidates_a_root_event_already_queued_by_the_real_http_pump() {
    pending_forget(PendingPlace::Queue(1), false);
}

#[test]
fn forget_invalidates_the_full_http_queue_and_its_blocked_extra_send() {
    pending_forget(PendingPlace::Queue(33), false);
}

#[test]
fn a_post_commit_forget_purge_error_invalidates_queued_and_replayed_http_events() {
    pending_forget(PendingPlace::Queue(1), true);
}

#[test]
fn forget_invalidates_an_owned_event_before_the_receive_call_returns() {
    pending_forget(PendingPlace::Receiving, false);
}
