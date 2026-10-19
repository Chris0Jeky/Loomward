//! Final native event admission. Kept independent of Tauri so the real handoff can also be tested
//! on a portable host without replacing the platform UI or opening a personal session.

use loomward_protocol::{EventDeliveryStamp, EventEnvelope};

pub(crate) fn emit(
    stamp: EventDeliveryStamp,
    event: EventEnvelope,
    sink: &mut impl FnMut(EventEnvelope) -> bool,
) -> bool {
    // This successful check admits the owned envelope to the native sink. There is no await or
    // service/Bus lock across the emit. A later forget cannot retract data already admitted to
    // Tauri's channel or the renderer, but a pending old receive never enters that channel.
    stamp.admit() && sink(event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use loomward_protocol::EventDeliveryPolicy;

    fn event() -> EventEnvelope {
        let data: serde_json::Value = serde_json::from_str(include_str!(
            "../../contracts/v3/examples/events/roots.changed.json"
        ))
        .unwrap();
        let event: EventEnvelope = serde_json::from_value(serde_json::json!({
            "protocol": "loomward/3",
            "epoch": "e_native_delivery_fixture",
            "seq": 1,
            "event": "roots.changed",
            "at": "2026-10-19T00:00:00Z",
            "catalog_rev": "1",
            "state_rev": "1",
            "data": data,
        }))
        .unwrap();
        event.validate().unwrap();
        event
    }

    #[test]
    fn an_old_receive_never_reaches_the_native_sink() {
        let policy = EventDeliveryPolicy::default();
        let stamp = policy.stamp();
        let pending = event();
        policy.invalidate();
        let mut calls = 0;
        assert!(!emit(stamp, pending, &mut |_| {
            calls += 1;
            true
        }));
        assert_eq!(calls, 0);
    }

    #[test]
    fn native_admission_holds_no_policy_lock_across_the_sink_and_preserves_its_result() {
        let policy = EventDeliveryPolicy::default();
        let stamp = policy.stamp();
        let pending = event();
        let expected = pending.clone();
        let (sent, received) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let answer = emit(stamp, pending, &mut |actual| {
                assert_eq!(actual, expected);
                // This would deadlock if admission held its policy mutex across the native emit.
                policy.invalidate();
                false
            });
            sent.send(answer).unwrap();
        });
        assert!(!received
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("native sink was called while an admission lock remained held"));
        worker.join().unwrap();
    }
}
