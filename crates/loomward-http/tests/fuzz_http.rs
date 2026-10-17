//! Property fuzzing of the HTTP adapter's pure parsers (docs/45 row 6, #414): `dispatch` (the
//! envelope ingress behind `POST /api/call`, over the contract-example `FixtureService`),
//! `parse_last_event_id` (the SSE resume cursor) and `static_relative` (URL path to file mapping).
//! NOT covered here: the header and body gate `call_header_refusal`/`route` (private, async, needs a
//! socket; `security_pins.rs` and `serve.rs` pin them with real requests).
#[path = "../../../fixtures/fuzz/support.rs"]
mod support;

use axum::http::{HeaderMap, HeaderValue};
use loomward_http::fixture::FixtureService;
use loomward_http::{dispatch, parse_last_event_id, static_relative};
use loomward_protocol::{
    Adapter, Command, DatasetClass, ErrorCode, RequestEnvelope, ResponseEnvelope, StreamEpoch,
};
use proptest::prelude::*;
use serde_json::{json, Value};
use std::path::{Component, Path};
use std::sync::OnceLock;
use support::{apply, config, edits};

fn service() -> &'static FixtureService {
    static S: OnceLock<FixtureService> = OnceLock::new();
    S.get_or_init(|| FixtureService::new(DatasetClass::Synthetic).unwrap())
}

fn seeds() -> &'static [Vec<u8>] {
    static S: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    S.get_or_init(load_seeds)
}

fn load_seeds() -> Vec<Vec<u8>> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/v3/examples/commands");
    Command::ALL
        .iter()
        .map(|c| {
            let payload: Value =
                serde_json::from_slice(&std::fs::read(dir.join(format!("{}.request.json", c.as_str()))).unwrap()).unwrap();
            serde_json::to_vec(&json!({"protocol": "loomward/3", "request_id": "r_fuzz", "command": c.as_str(), "payload": payload}))
                .unwrap()
        })
        .collect()
}

/// A response is a well-formed envelope that agrees with what the ingress decided.
fn check_dispatch(raw: &[u8]) -> Result<(), TestCaseError> {
    let response = dispatch(service(), raw);
    let wire = serde_json::to_value(&response).unwrap();
    prop_assert_eq!(wire["protocol"].as_str(), Some("loomward/3"));
    let parsed = RequestEnvelope::from_slice(raw);
    match response {
        ResponseEnvelope::Ok(ok) => {
            let request = parsed.expect("served a request the ingress refuses");
            prop_assert_eq!(&ok.request_id, &request.request_id);
            let command = Command::from_name(&request.command).unwrap();
            prop_assert!(command.available_on(Adapter::Http));
            prop_assert!(ok.validate_for(command).is_ok());
        }
        ResponseEnvelope::Err(err) => {
            if let Err(rejected) = parsed {
                prop_assert_eq!(err.error.code, rejected.error.code);
                prop_assert_eq!(err.request_id, rejected.request_id);
            } else {
                // Accepted by the ingress, then refused by availability or the service: still typed.
                prop_assert!(err.request_id.is_some());
            }
            prop_assert!(!err.error.message.is_empty());
            prop_assert!(
                matches!(
                    err.error.code,
                    ErrorCode::InvalidRequest
                        | ErrorCode::UnsupportedProtocol
                        | ErrorCode::UnknownCommand
                        | ErrorCode::CapabilityUnavailable
                        | ErrorCode::PermissionDenied
                        | ErrorCode::NotFound
                        | ErrorCode::Busy
                        | ErrorCode::StaleGeneration
                        | ErrorCode::PartialCoverage
                ),
                "{:?}",
                err.error.code
            );
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(config(1500))]

    #[test]
    fn dispatch_never_panics_on_arbitrary_bytes(raw in prop::collection::vec(any::<u8>(), 0..300)) {
        check_dispatch(&raw)?;
    }

    #[test]
    fn dispatch_answers_mutated_examples_with_a_typed_envelope(seed in 0usize..128, e in edits()) {
        let seeds = seeds();
        check_dispatch(&apply(seeds[seed % seeds.len()].clone(), &e))?;
    }
}

#[test]
fn dispatch_serves_every_http_example_and_refuses_the_rest_typed() {
    let mut served = 0;
    for raw in seeds() {
        let raw = raw.as_slice();
        check_dispatch(raw).unwrap();
        if matches!(dispatch(service(), raw), ResponseEnvelope::Ok(_)) {
            served += 1;
        }
    }
    assert!(served > 10, "only {served} example requests were served");
}

// ---- Last-Event-ID ----

fn header(values: &[Vec<u8>]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for v in values {
        h.append("last-event-id", HeaderValue::from_bytes(v).unwrap());
    }
    h
}

fn header_value_bytes() -> impl Strategy<Value = Vec<u8>> {
    // Anything a header can hold: visible ASCII, space, tab and obs-text.
    prop::collection::vec(
        prop_oneof![
            32u8..=126,
            Just(9u8),
            128u8..=255,
            Just(b'.'),
            Just(b'.'),
            b'0'..=b'9'
        ],
        0..90,
    )
}

proptest! {
    #![proptest_config(config(2000))]

    #[test]
    fn last_event_id_valid_cursors_round_trip(epoch in "[A-Za-z0-9_-]{8,64}", seq in any::<u64>()) {
        let got = parse_last_event_id(&header(&[format!("{epoch}.{seq}").into_bytes()]));
        prop_assert_eq!(got, Some((epoch, seq)));
    }

    #[test]
    fn last_event_id_never_panics_and_never_coerces(values in prop::collection::vec(header_value_bytes(), 0..3)) {
        let got = parse_last_event_id(&header(&values));
        if values.is_empty() {
            prop_assert_eq!(got, None);
        } else {
            let (epoch, seq) = got.clone().expect("a present header always yields a cursor");
            if !epoch.is_empty() {
                // A real cursor: exactly one header, a valid epoch, and the digits it carries.
                prop_assert_eq!(values.len(), 1);
                let text = std::str::from_utf8(&values[0]).expect("an accepted header is UTF-8");
                let (e, s) = text.rsplit_once('.').unwrap();
                prop_assert_eq!(e, epoch.as_str());
                prop_assert!(StreamEpoch::new(e).is_ok());
                prop_assert!(!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()));
                prop_assert_eq!(s.parse::<u64>().unwrap(), seq);
            } else {
                // Anything else resumes from the unknown epoch, never from a guessed position.
                prop_assert_eq!(got, Some((String::new(), 0)));
            }
        }
    }
}

// ---- static_relative ----

fn url_path() -> impl Strategy<Value = String> {
    let seg = prop_oneof![
        4 => "[A-Za-z0-9._-]{0,8}",
        1 => Just("..".to_string()),
        1 => Just(".".to_string()),
        1 => "[A-Za-z0-9]{1,4}[:~%\\\\ $*?<>|][A-Za-z0-9]{0,4}",
        1 => prop::sample::select(vec!["con", "NUL", "Com1", "lpt9.txt", "aux.tar.gz", "CONIN$", "prn.", "index.html", "app.js"]).prop_map(String::from),
        1 => any::<String>(),
    ];
    (prop::bool::weighted(0.9), prop::collection::vec(seg, 0..5))
        .prop_map(|(slash, parts)| format!("{}{}", if slash { "/" } else { "" }, parts.join("/")))
}

proptest! {
    #![proptest_config(config(8000))]

    #[test]
    fn static_relative_maps_only_plain_segments_and_never_normalises(path in url_path()) {
        match static_relative(&path) {
            None => {}
            Some(p) => {
                let rel = path.strip_prefix('/').expect("a path without the leading slash is refused");
                let rel = if rel.is_empty() { "index.html" } else { rel };
                // No coercion: the file path is the URL path, segment for segment.
                prop_assert_eq!(p.to_str().unwrap().replace('\\', "/"), rel);
                prop_assert!(p.is_relative());
                for c in p.components() {
                    let Component::Normal(s) = c else { return Err(TestCaseError::fail(format!("{c:?}"))) };
                    let s = s.to_str().unwrap();
                    prop_assert!(s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)));
                    prop_assert!(!s.starts_with('.') && !s.ends_with('.'));
                    let stem = s.split('.').next().unwrap().to_ascii_uppercase();
                    prop_assert!(!["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str()));
                    prop_assert!(!((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4
                        && stem.as_bytes()[3].is_ascii_digit()));
                }
            }
        }
    }

    #[test]
    fn static_relative_accepts_every_plain_path(parts in prop::collection::vec("[A-Za-z0-9][A-Za-z0-9_-]{0,6}(\\.[A-Za-z0-9_-]{1,4})?", 1..4)) {
        let path = format!("/{}", parts.join("/"));
        let any_device = parts.iter().any(|p| {
            let stem = p.split('.').next().unwrap().to_ascii_uppercase();
            ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
                || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit())
        });
        prop_assert_eq!(static_relative(&path).is_some(), !any_device, "{}", path);
    }
}
