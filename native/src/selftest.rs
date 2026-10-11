//! `loomward-desktop --self-test`: drives the command layer the webview reaches (`lw_call`,
//! `lw_events`) without a window: the protocol checks against the fixture service, then the engine
//! service the window hosts by default, in a scratch state directory. It needs no checkout: an
//! installed exe reads the contract examples bundled beside it. The Rust tests run it too.

use crate::dialogs::DesktopDialogs;
use crate::launch::{self, examples_dir, Launch};
use crate::Desktop;
use loomward_http::fixture::FixtureService;
use loomward_protocol::{Command, DatasetClass, EventEnvelope, EventName, ResponseEnvelope};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Heartbeat used by the self test, so a beat is observed in well under a second.
pub const TEST_HEARTBEAT: Duration = Duration::from_millis(300);

/// A fixture-backed desktop with a short heartbeat and a dialog stand-in that records nothing
/// and always declines.
pub fn fixture_desktop(heartbeat: Duration) -> Result<(Arc<FixtureService>, Desktop), String> {
    let service = FixtureService::load(DatasetClass::Synthetic, &examples_dir())
        .map_err(|e| e.to_string())?
        .with_stream_limits(heartbeat, loomward_protocol::limits::EVENT_QUEUE_CAPACITY);
    let service = Arc::new(service);
    let dialogs = DesktopDialogs::with_prompt(DatasetClass::Synthetic, |_, _| false);
    let desktop = Desktop::new(service.clone(), DatasetClass::Synthetic, Arc::new(dialogs));
    Ok((service, desktop))
}

pub fn envelope(command: &str, payload: Value) -> Value {
    json!({ "protocol": "loomward/3", "request_id": "r_selftest", "command": command, "payload": payload })
}

/// The contract's example payload for `command`.
pub fn example_payload(command: Command) -> Value {
    let file = examples_dir()
        .join("commands")
        .join(format!("{}.request.json", command.as_str()));
    std::fs::read_to_string(file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}))
}

/// A channel stand-in: what the sink receives arrives on the returned receiver.
pub fn sink() -> (
    impl FnMut(EventEnvelope) -> bool + Send + 'static,
    mpsc::Receiver<EventEnvelope>,
) {
    let (tx, rx) = mpsc::channel();
    (move |e| tx.send(e).is_ok(), rx)
}

pub fn wait_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let until = Instant::now() + limit;
    while Instant::now() < until {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    done()
}

fn ok_result(r: &ResponseEnvelope) -> Result<&serde_json::Map<String, Value>, String> {
    match r {
        ResponseEnvelope::Ok(ok) => Ok(&ok.result),
        ResponseEnvelope::Err(e) => Err(format!("{:?}", e.error)),
    }
}

fn error_code(r: &ResponseEnvelope) -> String {
    match r {
        ResponseEnvelope::Err(e) => serde_json::to_value(e.error.code)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default(),
        ResponseEnvelope::Ok(_) => "ok".into(),
    }
}

fn check(cond: bool, what: impl Into<String>) -> Result<(), String> {
    if cond {
        Ok(())
    } else {
        Err(what.into())
    }
}

/// Runs every check, reporting each through `log` as `PASS name` or `FAIL name: why`. Returns
/// whether all passed.
pub fn run(log: &mut dyn FnMut(&str)) -> bool {
    let mut all = true;
    let mut report = |name: &str, r: Result<(), String>| match r {
        Ok(()) => log(&format!("PASS {name}")),
        Err(e) => {
            all = false;
            log(&format!("FAIL {name}: {e}"));
        }
    };
    let (service, desktop) = match fixture_desktop(TEST_HEARTBEAT) {
        Ok(x) => x,
        Err(e) => {
            report("fixture service loads", Err(e));
            return false;
        }
    };

    report(
        "lw_call session.hello answers over the tauri adapter",
        (|| {
            let r = desktop.call(&envelope("session.hello", json!({})))?;
            let result = ok_result(&r)?;
            check(
                result.get("adapter") == Some(&json!("tauri")),
                format!("adapter {:?}", result.get("adapter")),
            )?;
            check(
                result.get("dataset_class") == Some(&json!("synthetic")),
                "dataset_class",
            )
        })(),
    );

    report(
        "lw_call: every command returns a complete envelope",
        (|| {
            for &command in Command::ALL {
                let r = desktop.call(&envelope(command.as_str(), example_payload(command)))?;
                match &r {
                    ResponseEnvelope::Ok(ok) => ok
                        .validate_for(command)
                        .map_err(|e| format!("{}: {e:?}", command.as_str()))?,
                    ResponseEnvelope::Err(e) => {
                        return Err(format!("{}: {:?}", command.as_str(), e.error))
                    }
                }
            }
            Ok(())
        })(),
    );

    report(
        "lw_call backups.import is refused in a synthetic session (file dialog never opens)",
        (|| {
            let r = desktop.call(&envelope(
                "backups.import",
                example_payload(Command::BackupsImport),
            ))?;
            let result = ok_result(&r)?;
            check(result.get("outcome") == Some(&json!("refused")), "outcome")?;
            check(
                result.get("refusal") == Some(&json!("synthetic_session")),
                "refusal",
            )?;
            check(result.get("evidence") == Some(&Value::Null), "evidence")
        })(),
    );

    report(
        "lw_call roots.request_grant is refused in a synthetic session (picker never opens)",
        (|| {
            let r = desktop.call(&envelope(
                "roots.request_grant",
                json!({ "purpose": "metadata_scan" }),
            ))?;
            let result = ok_result(&r)?;
            check(result.get("outcome") == Some(&json!("refused")), "outcome")?;
            check(
                result.get("refusal") == Some(&json!("synthetic_session_requires_lab_root")),
                "refusal",
            )?;
            check(result.get("root") == Some(&Value::Null), "root")
        })(),
    );

    report(
        "lw_call refuses malformed envelopes through from_slice",
        (|| {
            let cases = [
                (json!({ "protocol": "loomward/3" }), "invalid_request"),
                (json!([1, 2]), "invalid_request"),
                (envelope("files.delete", json!({})), "unknown_command"),
                (
                    json!({ "protocol": "loomward/9", "request_id": "r_1", "command": "health.get", "payload": {} }),
                    "unsupported_protocol",
                ),
                (
                    envelope("tree.slice", json!({ "path": "C:\\" })),
                    "invalid_request",
                ),
            ];
            for (req, want) in cases {
                let got = error_code(&desktop.call(&req)?);
                check(got == want, format!("{req}: got {got}, want {want}"))?;
            }
            Ok(())
        })(),
    );

    report(
        "lw_call rejects a request over 64 KiB as a transport fault",
        {
            let big = envelope("health.get", json!({ "x": "a".repeat(70_000) }));
            check(desktop.call(&big).is_err(), "accepted a 70 KB request")
        },
    );

    report(
        "lw_events: stream.hello first, then live events, revocation included",
        (|| {
            let (tx, rx) = sink();
            desktop.open_stream("selftest-a", None, None, tx)?;
            let hello = rx
                .recv_timeout(Duration::from_secs(2))
                .map_err(|e| e.to_string())?;
            check(
                hello.event == EventName::StreamHello,
                "first event is not stream.hello",
            )?;
            check(hello.epoch.as_str() == service.epoch(), "hello epoch")?;
            let mut revoked = example_event("roots.changed");
            revoked["roots"][0]["grant_state"] = json!("revoked");
            let Value::Object(data) = revoked else {
                return Err("example".into());
            };
            service
                .publish([(EventName::RootsChanged, data)])
                .map_err(|e| format!("{e:?}"))?;
            let e = next_non_hello(&rx)?;
            check(
                e.event == EventName::RootsChanged && e.seq.get() == 1,
                format!("got {:?} {}", e.event, e.seq.get()),
            )?;
            check(
                e.data["roots"][0]["grant_state"] == json!("revoked"),
                "revocation state",
            )?;
            desktop.close_webview("selftest-a");
            Ok(())
        })(),
    );

    report(
        "lw_events: stream.hello repeats after the heartbeat interval of silence",
        (|| {
            let (tx, rx) = sink();
            desktop.open_stream("selftest-b", None, None, tx)?;
            let start = Instant::now();
            let first = rx
                .recv_timeout(Duration::from_secs(2))
                .map_err(|e| e.to_string())?;
            check(first.event == EventName::StreamHello, "first")?;
            let beat = rx
                .recv_timeout(TEST_HEARTBEAT * 4)
                .map_err(|_| "no heartbeat".to_string())?;
            let gap = start.elapsed();
            check(
                beat.event == EventName::StreamHello,
                format!("got {:?}", beat.event),
            )?;
            check(
                gap >= TEST_HEARTBEAT,
                format!("beat after {gap:?}, before the interval"),
            )?;
            desktop.close_webview("selftest-b");
            Ok(())
        })(),
    );

    report(
        "lw_events resume: same epoch replays, other epoch or no epoch lags",
        (|| {
            let epoch = service.epoch().to_string();
            let (tx, rx) = sink();
            desktop.open_stream("selftest-c", Some(epoch.clone()), Some(0), tx)?;
            let hello = rx
                .recv_timeout(Duration::from_secs(2))
                .map_err(|e| e.to_string())?;
            check(hello.event == EventName::StreamHello, "hello")?;
            let replay = rx
                .recv_timeout(Duration::from_secs(2))
                .map_err(|e| e.to_string())?;
            check(
                replay.event == EventName::RootsChanged && replay.seq.get() == 1,
                "replay of seq 1",
            )?;
            for (e, s) in [
                (Some("e_00000000".to_string()), Some(1)),
                (None, Some(1)),
                (Some("not an epoch!".into()), None),
            ] {
                let (tx, rx) = sink();
                desktop.open_stream("selftest-c", e.clone(), s, tx)?;
                let _hello = rx
                    .recv_timeout(Duration::from_secs(2))
                    .map_err(|e| e.to_string())?;
                let lag = rx
                    .recv_timeout(Duration::from_secs(2))
                    .map_err(|e| e.to_string())?;
                check(
                    lag.event == EventName::StreamLagged,
                    format!("{e:?}/{s:?}: got {:?}", lag.event),
                )?;
                check(lag.data["reason"] == json!("epoch_changed"), "reason")?;
                desktop.close_webview("selftest-c");
            }
            Ok(())
        })(),
    );

    report(
        "lw_events: a fifth stream is busy until the oldest releases; window close tears all down",
        (|| {
            // Earlier checks' pumps stop within PUMP_POLL; start from a quiet service.
            wait_until(Duration::from_secs(3), || service.open_streams() == 0);
            let base = service.open_streams();
            let mut keep = Vec::new();
            for _ in 0..crate::MAX_STREAMS_PER_WEBVIEW {
                let (tx, rx) = sink();
                desktop.open_stream("selftest-d", None, None, tx)?;
                keep.push(rx);
            }
            let (tx, _rx) = sink();
            check(
                desktop.open_stream("selftest-d", None, None, tx).is_err(),
                "fifth stream admitted before release",
            )?;
            check(
                wait_until(Duration::from_secs(3), || {
                    desktop.open_streams("selftest-d") == 3 && service.open_streams() == base + 3
                }),
                "oldest did not release",
            )?;
            let (tx, rx) = sink();
            desktop.open_stream("selftest-d", None, None, tx)?;
            keep.push(rx);
            check(
                desktop.open_streams("selftest-d") == 4,
                "four open after release",
            )?;
            desktop.close_webview("selftest-d");
            let closed = wait_until(Duration::from_secs(3), || {
                desktop.open_streams("selftest-d") == 0 && service.open_streams() == base
            });
            check(
                closed,
                format!(
                    "{} adapter / {} service streams still open",
                    desktop.open_streams("selftest-d"),
                    service.open_streams()
                ),
            )
        })(),
    );

    report(
        "lw_events: a channel that stops receiving ends its stream",
        (|| {
            // Earlier checks' pumps stop within PUMP_POLL; start from a quiet service.
            wait_until(Duration::from_secs(3), || service.open_streams() == 0);
            let base = service.open_streams();
            let mut left = 1;
            desktop.open_stream("selftest-e", None, None, move |_| {
                left -= 1;
                left >= 0
            })?;
            let ended = wait_until(Duration::from_secs(3), || service.open_streams() == base);
            check(ended, "stream still open after the sink failed")
        })(),
    );

    for (dataset, name) in [
        (
            DatasetClass::Synthetic,
            "engine service (synthetic, scratch state): hello, picker refused, stream.hello first",
        ),
        (
            DatasetClass::Personal,
            "default session: personal engine service (scratch state) offers the native picker",
        ),
    ] {
        report(name, engine_session(dataset));
    }

    all
}

/// The engine service as `main` opens it, in a scratch state directory that is removed after.
fn engine_session(dataset: DatasetClass) -> Result<(), String> {
    let scratch = scratch_dir();
    let result = (|| {
        let launch = Launch {
            self_test: false,
            dataset,
            fixtures: false,
            state_dir: Some(scratch.clone()),
        };
        let service = launch::open(&launch, None)?;
        let dialogs = DesktopDialogs::with_prompt(dataset, |_, _| false);
        let desktop = Desktop::new(service, dataset, Arc::new(dialogs));
        let r = desktop.call(&envelope("session.hello", json!({})))?;
        let hello = ok_result(&r)?;
        let engine = hello.get("engine_version").and_then(Value::as_str);
        check(
            engine.is_some_and(|v| v.starts_with("loomward-service/")),
            format!("engine_version {engine:?}"),
        )?;
        check(hello.get("adapter") == Some(&json!("tauri")), "adapter")?;
        check(
            hello.get("dataset_class") == Some(&json!(dataset)),
            "dataset_class",
        )?;
        let personal = dataset == DatasetClass::Personal;
        check(
            hello["features"]["grant_picker"] == json!(personal),
            format!("grant_picker {}", hello["features"]["grant_picker"]),
        )?;
        check(
            scratch.join("state.db").is_file(),
            "state.db is not in the state directory",
        )?;
        if !personal {
            let r = desktop.call(&envelope(
                "roots.request_grant",
                json!({ "purpose": "metadata_scan" }),
            ))?;
            check(
                ok_result(&r)?.get("refusal")
                    == Some(&json!("synthetic_session_requires_lab_root")),
                "synthetic picker request not refused",
            )?;
            let (tx, rx) = sink();
            desktop.open_stream("selftest-engine", None, None, tx)?;
            let hello = rx
                .recv_timeout(Duration::from_secs(2))
                .map_err(|e| e.to_string())?;
            check(hello.event == EventName::StreamHello, "first event")?;
            desktop.close_webview("selftest-engine");
        }
        Ok(())
    })();
    // Best effort: a store the engine still holds open stays in the temp directory.
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

fn scratch_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    std::env::temp_dir().join(format!("loomward-selftest-{}-{nanos}", std::process::id()))
}

fn example_event(name: &str) -> Value {
    let file = examples_dir().join("events").join(format!("{name}.json"));
    std::fs::read_to_string(file)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .map(|v| v.get("data").cloned().unwrap_or(v))
        .unwrap_or(Value::Null)
}

fn next_non_hello(rx: &mpsc::Receiver<EventEnvelope>) -> Result<EventEnvelope, String> {
    loop {
        let e = rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|e| e.to_string())?;
        if e.event != EventName::StreamHello {
            return Ok(e);
        }
    }
}
