//! Decision and wake-up regressions for the response-only forget pause. Event admission keeps
//! its independent, irreversible suspension policy.

use super::*;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Wake;

#[derive(Default)]
struct WakeCount(AtomicUsize);

impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn retained_privacy_response_pause_resumes_original_and_during_pause_stamps() {
    let policy = ResponseDeliveryPolicy::default();
    let before = policy.stamp();
    let pause = policy.suspend();
    let during = policy.stamp();
    let wakes = Arc::new(WakeCount::default());
    let waker = Waker::from(wakes.clone());
    let mut cx = Context::from_waker(&waker);
    let mut before = std::pin::pin!(before.admit_async());
    let mut during = std::pin::pin!(during.admit_async());
    assert!(before.as_mut().poll(&mut cx).is_pending());
    assert!(before.as_mut().poll(&mut cx).is_pending());
    assert!(during.as_mut().poll(&mut cx).is_pending());
    pause.resume();
    assert_eq!(wakes.0.load(Ordering::SeqCst), 2);
    assert_eq!(before.as_mut().poll(&mut cx), Poll::Ready(true));
    assert_eq!(during.as_mut().poll(&mut cx), Poll::Ready(true));
    assert!(policy.stamp().admit());
}

#[test]
fn retained_privacy_nested_response_pauses_wait_for_the_last_rejection() {
    let policy = ResponseDeliveryPolicy::default();
    let stamp = policy.stamp();
    let first = policy.suspend();
    let second = policy.suspend();
    let mut response = std::pin::pin!(stamp.admit_async());
    let mut cx = Context::from_waker(Waker::noop());
    assert!(response.as_mut().poll(&mut cx).is_pending());
    first.resume();
    assert!(response.as_mut().poll(&mut cx).is_pending());
    second.resume();
    assert_eq!(response.as_mut().poll(&mut cx), Poll::Ready(true));
}

#[test]
fn retained_privacy_response_pause_defaults_to_retirement() {
    let policy = ResponseDeliveryPolicy::default();
    let before = policy.stamp();
    let pause = policy.suspend();
    let during = policy.stamp();
    let mut before = std::pin::pin!(before.admit_async());
    let mut during = std::pin::pin!(during.admit_async());
    let mut cx = Context::from_waker(Waker::noop());
    assert!(before.as_mut().poll(&mut cx).is_pending());
    assert!(during.as_mut().poll(&mut cx).is_pending());
    drop(pause);
    assert_eq!(before.as_mut().poll(&mut cx), Poll::Ready(false));
    assert_eq!(during.as_mut().poll(&mut cx), Poll::Ready(false));
    assert!(policy.stamp().admit());
}

#[test]
fn retained_privacy_response_rejection_cannot_revive_a_retired_generation() {
    let policy = ResponseDeliveryPolicy::default();
    let original = policy.stamp();
    let first = policy.suspend();
    let second = policy.suspend();
    drop(first);
    assert!(!original.admit());
    let mut after_retirement = std::pin::pin!(policy.stamp().admit_async());
    let mut cx = Context::from_waker(Waker::noop());
    assert!(after_retirement.as_mut().poll(&mut cx).is_pending());
    second.resume();
    assert_eq!(after_retirement.as_mut().poll(&mut cx), Poll::Ready(true));
}

#[test]
fn retained_privacy_response_unclassified_unwind_retires_the_stamp() {
    let policy = ResponseDeliveryPolicy::default();
    let stamp = policy.stamp();
    let unwound = std::panic::catch_unwind(|| {
        let _pause = policy.suspend();
        panic!("synthetic failure before the durable outcome is classified");
    });
    assert!(unwound.is_err());
    assert!(!stamp.admit());
    assert!(policy.stamp().admit());
}

#[test]
fn retained_privacy_cancelled_response_admission_releases_its_scheduler() {
    let policy = ResponseDeliveryPolicy::default();
    let pause = policy.suspend();
    let wakes = Arc::new(WakeCount::default());
    let waker = Waker::from(wakes.clone());
    let mut cx = Context::from_waker(&waker);
    let mut response = Box::pin(policy.stamp().admit_async());
    assert!(response.as_mut().poll(&mut cx).is_pending());
    drop(response);
    drop(waker);
    assert_eq!(Arc::strong_count(&wakes), 1);
    pause.resume();
    assert_eq!(wakes.0.load(Ordering::SeqCst), 0);
}

struct ReenterPolicy(ResponseDeliveryPolicy);

impl Wake for ReenterPolicy {
    fn wake(self: Arc<Self>) {
        assert!(self.0.stamp().admit());
    }
}

#[test]
fn retained_privacy_response_wakes_the_adapter_without_a_policy_lock() {
    let policy = ResponseDeliveryPolicy::default();
    let pause = policy.suspend();
    let waker = Waker::from(Arc::new(ReenterPolicy(policy.clone())));
    let mut cx = Context::from_waker(&waker);
    let mut response = std::pin::pin!(policy.stamp().admit_async());
    assert!(response.as_mut().poll(&mut cx).is_pending());
    pause.resume();
    assert_eq!(response.as_mut().poll(&mut cx), Poll::Ready(true));
}

#[test]
fn retained_privacy_synchronous_response_waits_for_the_forget_decision() {
    use std::sync::mpsc;
    use std::time::Duration;

    let policy = ResponseDeliveryPolicy::default();
    let stamp = policy.stamp();
    let pause = policy.suspend();
    let (started, entered) = mpsc::channel();
    let (sent, received) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        started.send(()).unwrap();
        sent.send(stamp.admit()).unwrap();
    });
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let early = received.recv_timeout(Duration::from_millis(50)).ok();
    // Release before any assertion, so a failing implementation cannot strand the worker.
    pause.resume();
    let result = early.unwrap_or_else(|| received.recv_timeout(Duration::from_secs(5)).unwrap());
    worker.join().unwrap();
    assert!(
        early.is_none(),
        "the synchronous adapter did not wait for the decision"
    );
    assert!(result, "the rejected forget retired a synchronous reply");
}
