//! F3 gate pins: restart backoff and its rolling-hour budget, the hint-path cap, and the
//! automatic refresh start gates (auto flag, retry budget, cooldown, explicit fence, refunds).
//!
//! The Restart and Tracker tests are pure and need no lab. The refresh tests use the lab root and
//! never sleep: a started automatic refresh is observed through the one-shot
//! `fail_automatic_start` / `refuse_automatic_budget` hooks, which the start path consumes, so no
//! background scan races the assertions. Only the Busy refund runs a real engine refusal.
use super::*;
use crate::tests::{f3_wait_live, lab, settle, start};
use loomward_windows::watch::Change;

fn tracked() -> Tracked {
    Tracked {
        restart: Restart::default(),
        watch: None,
        checkpoint: Arc::default(),
        epoch: 0,
        stale_since: None,
        at: 0,
        caught_up: false,
        full: false,
        paths: BTreeSet::new(),
        targets: BTreeSet::new(),
        auto: false,
        attempts: 0,
        retry_after: None,
        changed: Instant::now(),
        job: None,
        started: None,
        completed: None,
    }
}

#[test]
fn f3_gate_restart_backoff_doubles_from_five_seconds_to_a_ten_minute_cap() {
    let mut restart = Restart::default();
    let origin = Instant::now();
    assert!(!restart.ready(origin), "the first restart waited no 5 s");
    assert!(!restart.ready(origin + Duration::from_secs(4)));
    assert!(restart.ready(origin + Duration::from_secs(5)));
    assert_eq!(restart.delay, Duration::from_secs(10));
    let mut at = origin + Duration::from_secs(5);
    for want in [10, 20, 40, 80, 160, 320, 600, 600] {
        let want = Duration::from_secs(want);
        assert_eq!(restart.delay, want, "backoff before the next restart");
        assert!(!restart.ready(at), "a restart ran before its backoff");
        assert!(
            !restart.ready(at + want - Duration::from_nanos(1)),
            "a restart ran one nanosecond early"
        );
        at += want;
        assert!(
            restart.ready(at),
            "a restart was not admitted at its backoff"
        );
    }
    assert_eq!(
        restart.delay,
        Duration::from_secs(600),
        "backoff must stop at 600 s"
    );
    assert_eq!(
        restart.history.len(),
        9,
        "every admitted restart is counted"
    );
}

#[test]
fn f3_gate_restart_admits_twelve_per_rolling_hour_and_no_more() {
    let mut restart = Restart::default();
    let origin = Instant::now();
    let mut now = origin;
    for n in 1..=12 {
        assert!(!restart.ready(now), "recovery {n} ignored its 5 s backoff");
        now += Duration::from_secs(5);
        assert!(
            restart.ready(now),
            "recovery {n} was refused inside the hour"
        );
        // Recovery resets the backoff, so only the hourly budget can hold the next restart.
        restart.recovered();
    }
    assert_eq!(restart.history.len(), 12);
    for second in [60, 65, 600, 3604] {
        assert!(
            !restart.ready(origin + Duration::from_secs(second)),
            "a 13th restart was admitted at {second} s"
        );
    }
    assert!(
        restart.ready(origin + Duration::from_secs(3605)),
        "the oldest restart must age out at exactly one hour"
    );
    assert_eq!(restart.history.len(), 12);
}

#[test]
fn f3_gate_hint_paths_stop_at_1024_and_force_a_full_refresh() {
    let tracker = Tracker::default();
    tracker.roots.lock().unwrap().insert(7, tracked());
    let hint = |i: usize| Change::Name {
        action: 1,
        name: format!("d{i}\\f").encode_utf16().collect(),
    };
    for i in 0..1024 {
        tracker.record(7, vec![hint(i)]);
    }
    {
        let roots = tracker.roots.lock().unwrap();
        assert_eq!(roots[&7].paths.len(), 1024);
        assert!(!roots[&7].full, "1024 distinct hints must stay path-scoped");
    }
    tracker.record(7, vec![hint(1024)]);
    let roots = tracker.roots.lock().unwrap();
    assert_eq!(
        roots[&7].paths.len(),
        1024,
        "the path set grew past its cap"
    );
    assert!(
        roots[&7].full,
        "the 1025th distinct hint must force a full refresh"
    );
}

/// Makes lab root 1 due for one automatic refresh: auto, stale, and quiet for the 500 ms cooldown.
fn make_due(svc: &crate::Service) {
    let mut roots = svc.inner.freshness.roots.lock().unwrap();
    let root = roots.get_mut(&1).expect("the lab root is tracked");
    root.auto = true;
    root.full = true;
    root.stale_since.get_or_insert(root.at);
    root.job = None;
    root.changed = Instant::now() - Duration::from_millis(500);
}

fn fresh_budget(svc: &crate::Service) {
    let mut roots = svc.inner.freshness.roots.lock().unwrap();
    let root = roots.get_mut(&1).expect("the lab root is tracked");
    root.attempts = 0;
    root.retry_after = None;
}

fn set_auto(svc: &crate::Service, auto: bool) {
    svc.inner
        .freshness
        .roots
        .lock()
        .unwrap()
        .get_mut(&1)
        .unwrap()
        .auto = auto;
}

fn set_changed(svc: &crate::Service, changed: Instant) {
    svc.inner
        .freshness
        .roots
        .lock()
        .unwrap()
        .get_mut(&1)
        .unwrap()
        .changed = changed;
}

fn attempts(svc: &crate::Service) -> u8 {
    svc.inner.freshness.attempts(1)
}

fn retry_after(svc: &crate::Service) -> Option<Instant> {
    svc.inner.freshness.roots.lock().unwrap()[&1].retry_after
}

/// Arms the synthetic start failure, which the start path consumes only when it reaches a start.
fn fail_next_start(svc: &crate::Service) {
    svc.inner
        .hooks
        .fail_automatic_start
        .store(true, Ordering::SeqCst);
}

fn start_attempted(svc: &crate::Service) -> bool {
    !svc.inner.hooks.fail_automatic_start.load(Ordering::SeqCst)
}

#[test]
fn f3_gate_only_auto_marked_roots_start_a_refresh() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    svc.inner.freshness.pause_pump();
    make_due(svc);
    set_auto(svc, false);
    fail_next_start(svc);
    refresh(&svc.inner);
    assert!(
        !start_attempted(svc),
        "a root without the auto flag started a refresh"
    );
    assert_eq!(attempts(svc), 0);
    set_auto(svc, true);
    refresh(&svc.inner);
    assert!(
        start_attempted(svc),
        "an auto root did not start its refresh"
    );
    assert_eq!(attempts(svc), 1);
}

#[test]
fn f3_gate_third_failed_attempt_waits_thirty_seconds_for_a_new_hint() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    svc.inner.freshness.pause_pump();
    for n in 1..=3u8 {
        make_due(svc);
        fail_next_start(svc);
        let before = Instant::now();
        refresh(&svc.inner);
        assert!(start_attempted(svc), "attempt {n} was not started");
        assert_eq!(attempts(svc), n);
        if n == 3 {
            let at = retry_after(svc).expect("the third attempt must arm a retry");
            // refresh arms `now + 30 s` at or after `before`, and the check runs after it, so
            // these bounds hold on any machine; they fail only if the 30 s constant changes.
            assert!(
                (before + Duration::from_secs(29)..=Instant::now() + Duration::from_secs(31))
                    .contains(&at),
                "retry must arm about 30 s ahead, got {:?}",
                at.checked_duration_since(before)
            );
        }
    }
    // Inside the retry window a hint records the change but grants no attempt.
    svc.inner
        .freshness
        .roots
        .lock()
        .unwrap()
        .get_mut(&1)
        .unwrap()
        .retry_after = Some(Instant::now() + Duration::from_secs(3600));
    svc.inner.freshness.record(1, vec![Change::RootDirty]);
    make_due(svc);
    fail_next_start(svc);
    refresh(&svc.inner);
    assert!(
        !start_attempted(svc),
        "a hint inside the retry window reopened the budget"
    );
    assert_eq!(attempts(svc), 3);
    // After the window, one new hint reopens exactly one attempt.
    svc.inner.freshness.expire_retry(1);
    svc.inner.freshness.record(1, vec![Change::RootDirty]);
    assert_eq!(
        attempts(svc),
        2,
        "a hint after the retry window must reopen one attempt"
    );
    make_due(svc);
    fail_next_start(svc);
    refresh(&svc.inner);
    assert!(start_attempted(svc), "the reopened attempt did not start");
    assert_eq!(attempts(svc), 3);
}

#[test]
fn f3_gate_a_root_changed_within_half_a_second_waits() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    svc.inner.freshness.pause_pump();
    make_due(svc);
    // `elapsed()` saturates to zero for a future stamp: a change "just now" without a sleep.
    set_changed(svc, Instant::now() + Duration::from_secs(3600));
    fail_next_start(svc);
    refresh(&svc.inner);
    assert!(
        !start_attempted(svc),
        "a root changed within 500 ms started a refresh"
    );
    assert_eq!(attempts(svc), 0);
    // Exactly 500 ms of quiet is eligible: the cooldown compares strictly less-than.
    set_changed(svc, Instant::now() - Duration::from_millis(500));
    refresh(&svc.inner);
    assert!(
        start_attempted(svc),
        "a root quiet for 500 ms did not start"
    );
    assert_eq!(attempts(svc), 1);
}

#[test]
fn f3_gate_explicit_claim_fences_automatic_starts_until_released() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    svc.inner.freshness.pause_pump();
    make_due(svc);
    fail_next_start(svc);
    let claim = svc.inner.freshness.explicit(1);
    refresh(&svc.inner);
    assert!(
        !start_attempted(svc),
        "an automatic refresh started under an explicit claim"
    );
    assert_eq!(attempts(svc), 0, "a fenced refresh consumed a retry");
    drop(claim);
    refresh(&svc.inner);
    assert!(
        start_attempted(svc),
        "releasing the explicit claim did not free the root"
    );
    assert_eq!(attempts(svc), 1);
}

#[test]
fn f3_gate_budget_refusal_refunds_the_attempt() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    svc.inner.freshness.pause_pump();
    make_due(svc);
    fresh_budget(svc);
    svc.inner
        .hooks
        .refuse_automatic_budget
        .store(true, Ordering::SeqCst);
    refresh(&svc.inner);
    assert!(
        !svc.inner
            .hooks
            .refuse_automatic_budget
            .load(Ordering::SeqCst),
        "refresh never reached the start"
    );
    assert_eq!(
        attempts(svc),
        0,
        "a resource-budget refusal consumed a retry"
    );
    assert!(
        svc.inner.freshness.automatic_jobs().is_empty(),
        "a refused start left a job behind"
    );
}

#[test]
fn f3_gate_busy_engine_refusal_refunds_the_attempt() {
    let Some(lab) = lab() else { return };
    let svc = &lab.svc;
    settle(svc, &start(svc, &lab.root, "full"));
    f3_wait_live(svc);
    svc.inner.freshness.pause_pump();
    // Hold an explicit scan inside its publication: the engine still counts the root as scanning.
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let (arrived, waiting) = std::sync::mpsc::sync_channel(1);
    let worker = barrier.clone();
    *svc.inner.hooks.before_freshness_finish.lock().unwrap() = Some(Box::new(move || {
        arrived.send(()).unwrap();
        worker.wait();
    }));
    let job = start(svc, &lab.root, "full");
    waiting.recv_timeout(Duration::from_secs(30)).unwrap();
    assert!(
        svc.inner.freshness.unclaimed(1),
        "precondition: no explicit claim is held"
    );
    make_due(svc);
    fresh_budget(svc);
    refresh(&svc.inner);
    let attempted = attempts(svc);
    let job_started = !svc.inner.freshness.automatic_jobs().is_empty();
    barrier.wait();
    assert_eq!(settle(svc, &job)["state"], "completed");
    assert!(!job_started, "a busy root admitted an automatic job");
    assert_eq!(attempted, 0, "a busy engine refusal consumed a retry");
}
