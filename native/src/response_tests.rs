use super::*;
use loomward_protocol::{DisclosureSummary, EventStream, RequestId, ResponseDeliveryPolicy};

pub(super) const PRIVATE_LABEL: &str = "SyntheticRetainedNativeReply";

pub(super) fn private_reply(id: RequestId) -> ResponseEnvelope {
    let mut result: Value = serde_json::from_str(include_str!(
        "../../contracts/v3/examples/commands/roots.list.result.json"
    ))
    .unwrap();
    result["roots"][0]["display_path"]["text"] = json!(PRIVATE_LABEL);
    let meta: ResponseMeta = serde_json::from_value(json!({
        "served_at": "2026-10-19T00:00:00Z", "elapsed_ms": 0,
        "dataset_class": "synthetic", "budget_hit": false,
        "catalog_rev": "1", "state_rev": "1"
    }))
    .unwrap();
    let reply = ResponseEnvelope::ok_object(id, result.as_object().unwrap().clone(), meta);
    let ResponseEnvelope::Ok(ok) = &reply else {
        unreachable!()
    };
    ok.validate_for(Command::RootsList).unwrap();
    reply
}

pub(super) fn assert_withheld(reply: ResponseEnvelope) {
    assert!(
        !serde_json::to_string(&reply)
            .unwrap()
            .contains(PRIVATE_LABEL),
        "a pending native reply disclosed the forgotten label"
    );
    let ResponseEnvelope::Err(error) = reply else {
        panic!("an invalidated native reply must be a privacy refusal")
    };
    assert_eq!(error.request_id.unwrap().as_str(), "native-retained-reply");
    assert_eq!(error.error.code, ErrorCode::PermissionDenied);
    assert!(!error.error.retryable);
    assert_eq!(error.error.detail.unwrap()["reason"], "forgotten_result");
    assert!(error.error.message.as_str().contains("may have completed"));
}

struct NoDialogs;
impl NativeDialogs for NoDialogs {
    fn pick_folder(&self) -> Option<std::path::PathBuf> {
        panic!("a read-only reply fixture must never open a folder picker")
    }

    fn confirm_disclosure(&self, _: &DisclosureSummary) -> Result<bool, ErrorBody> {
        panic!("a read-only reply fixture must never open a disclosure dialog")
    }
}

struct PrivateService {
    policy: ResponseDeliveryPolicy,
    forget_during_call: bool,
}

impl ViewService for PrivateService {
    fn call(&self, request: RequestEnvelope, _: &CallContext) -> ResponseEnvelope {
        assert_eq!(request.command.as_str(), "roots.list");
        let reply = private_reply(request.request_id);
        if self.forget_during_call {
            self.policy.invalidate();
        }
        reply
    }

    fn response_delivery_policy(&self, command: Command) -> Option<ResponseDeliveryPolicy> {
        (command == Command::RootsList).then(|| self.policy.clone())
    }

    fn subscribe(&self, _: Option<(String, u64)>) -> Box<dyn EventStream> {
        unreachable!("this fixture does not open an event stream")
    }
}

fn request_bytes() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "protocol": "loomward/3", "request_id": "native-retained-reply",
        "command": "roots.list", "payload": {}
    }))
    .unwrap()
}

#[test]
fn native_dispatch_refuses_a_reply_invalidated_while_the_service_is_running() {
    let service = Arc::new(PrivateService {
        policy: ResponseDeliveryPolicy::default(),
        forget_during_call: true,
    });
    let desktop = Desktop::new(service, DatasetClass::Synthetic, Arc::new(NoDialogs));
    assert_withheld(desktop.dispatch_bytes(&request_bytes()).unwrap());
}

#[test]
fn native_reply_is_still_pending_across_the_worker_await() {
    use std::future::{poll_fn, Future};
    use std::task::{Context, Poll, Waker};

    let policy = ResponseDeliveryPolicy::default();
    let service = Arc::new(PrivateService {
        policy: policy.clone(),
        forget_during_call: false,
    });
    let desktop = Desktop::new(service, DatasetClass::Synthetic, Arc::new(NoDialogs));
    let mut pending = Some(desktop.dispatch_bytes_pending(&request_bytes()).unwrap());
    let mut yielded = false;
    let worker = poll_fn(move |cx| {
        if !yielded {
            yielded = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        Poll::Ready(Ok::<_, ()>(Ok(pending.take().unwrap())))
    });
    let mut response = std::pin::pin!(crate::response::admit_after_work(worker));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(response.as_mut().poll(&mut cx).is_pending());
    policy.invalidate();
    let Poll::Ready(reply) = response.as_mut().poll(&mut cx) else {
        panic!("the completed worker did not return its reply")
    };
    assert_withheld(reply.unwrap());
}

#[test]
fn native_admission_preserves_current_replies_and_adapter_refusals() {
    let service = Arc::new(PrivateService {
        policy: ResponseDeliveryPolicy::default(),
        forget_during_call: false,
    });
    let desktop = Desktop::new(service, DatasetClass::Synthetic, Arc::new(NoDialogs));
    let current = desktop
        .dispatch_bytes_pending(&request_bytes())
        .unwrap()
        .admit();
    assert_eq!(
        current,
        private_reply(RequestId::new("native-retained-reply").unwrap())
    );
    let refused = desktop
        .dispatch_bytes_pending(
            &serde_json::to_vec(&json!({
                "protocol": "loomward/3", "request_id": "native-retained-reply",
                "command": "roots.request_grant", "payload": {"purpose":"metadata_scan"}
            }))
            .unwrap(),
        )
        .unwrap()
        .admit();
    let ResponseEnvelope::Ok(ok) = refused else {
        panic!("the adapter's synthetic picker refusal changed shape")
    };
    ok.validate_for(Command::RootsRequestGrant).unwrap();
    assert_eq!(ok.result["refusal"], "synthetic_session_requires_lab_root");
    assert_eq!(ok.result["root"], Value::Null);
}

#[test]
fn native_worker_and_dispatch_faults_keep_their_transport_meaning() {
    use std::future::{ready, Future};
    use std::task::{Context, Poll, Waker};

    let mut cx = Context::from_waker(Waker::noop());
    let mut worker_fault = std::pin::pin!(crate::response::admit_after_work(ready(Err::<
        Result<PendingResponse, String>,
        (),
    >(()))));
    assert_eq!(
        worker_fault.as_mut().poll(&mut cx),
        Poll::Ready(Err("the service failed".into()))
    );
    let mut dispatch_fault = std::pin::pin!(crate::response::admit_after_work(ready(Ok::<_, ()>(
        Err("the request exceeds 64 KiB".into())
    ))));
    assert_eq!(
        dispatch_fault.as_mut().poll(&mut cx),
        Poll::Ready(Err("the request exceeds 64 KiB".into()))
    );
}

#[test]
fn native_response_waits_for_a_rejected_forget_after_the_worker_returns() {
    use std::future::{ready, Future};
    use std::task::{Context, Poll, Waker};

    let policy = ResponseDeliveryPolicy::default();
    let service = Arc::new(PrivateService {
        policy: policy.clone(),
        forget_during_call: false,
    });
    let desktop = Desktop::new(service, DatasetClass::Synthetic, Arc::new(NoDialogs));
    let pending = desktop.dispatch_bytes_pending(&request_bytes()).unwrap();
    let pause = policy.suspend();
    let mut response = std::pin::pin!(crate::response::admit_after_work(ready(Ok::<_, ()>(Ok(
        pending
    )))));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(response.as_mut().poll(&mut cx).is_pending());
    pause.resume();
    let Poll::Ready(delivered) = response.as_mut().poll(&mut cx) else {
        panic!("a rejected forget left the native reply paused")
    };
    assert_eq!(
        delivered.unwrap(),
        private_reply(RequestId::new("native-retained-reply").unwrap())
    );
}

#[test]
fn native_response_withholds_an_unclassified_forget_after_the_worker_returns() {
    use std::future::{ready, Future};
    use std::task::{Context, Poll, Waker};

    let policy = ResponseDeliveryPolicy::default();
    let service = Arc::new(PrivateService {
        policy: policy.clone(),
        forget_during_call: false,
    });
    let desktop = Desktop::new(service, DatasetClass::Synthetic, Arc::new(NoDialogs));
    let pending = desktop.dispatch_bytes_pending(&request_bytes()).unwrap();
    let pause = policy.suspend();
    let mut response = std::pin::pin!(crate::response::admit_after_work(ready(Ok::<_, ()>(Ok(
        pending
    )))));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(response.as_mut().poll(&mut cx).is_pending());
    drop(pause);
    let Poll::Ready(delivered) = response.as_mut().poll(&mut cx) else {
        panic!("an unclassified forget left the native reply paused")
    };
    assert_withheld(delivered.unwrap());
}
