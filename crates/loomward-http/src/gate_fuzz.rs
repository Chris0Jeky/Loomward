//! Property fuzzing of the private header and body gate (docs/45 row 6): `route` with its
//! `host_ok`, `origin_check`, `token_ok` and `call_header_refusal`, driven in-process with
//! arbitrary methods, targets, header maps (absent, repeated, near-miss and arbitrary values),
//! tokens, origins and body sizes. A test-only module: nothing here exists in a release build.
//!
//! Properties: `route` never panics; the service is reached only by an exact `POST /api/v3/call`
//! or `GET /api/v3/events` with the right `Host`, an allowed or absent `Origin` and exactly one
//! correct token; an admitted call body is at most 64 KiB and exactly its single `Content-Length`;
//! every status is the one an independent reading of the gate predicts, from the documented set
//! (semantics.md section 1: 400/403/413/415 for transport faults, 429/503 for streams), and every
//! refusal is a `{"error": ...}` JSON body that closes the connection.
#[path = "../../../fixtures/fuzz/support.rs"]
mod support;

use super::*;
use proptest::prelude::*;
use std::sync::atomic::AtomicUsize;
use std::sync::OnceLock;
use support::config;

const HOST: &str = "127.0.0.1:12345";
const SELF_ORIGIN: &str = "http://127.0.0.1:12345";
const ALLOWED: &str = "http://localhost:5173";
const HELLO: &[u8] =
    br#"{"protocol":"loomward/3","request_id":"r_1","command":"session.hello","payload":{}}"#;

fn token() -> String {
    "a5".repeat(32)
}

fn fixture() -> &'static fixture::FixtureService {
    static F: OnceLock<fixture::FixtureService> = OnceLock::new();
    F.get_or_init(|| {
        fixture::FixtureService::new(loomward_protocol::DatasetClass::Synthetic).unwrap()
    })
}

/// Counts every entry into the service; calls are answered by the contract-example fixture and
/// every stream closes at once, so no SSE pump thread starts.
#[derive(Default)]
struct Probe {
    calls: AtomicUsize,
    subscriptions: AtomicUsize,
}

struct Closed;
impl EventStream for Closed {
    fn epoch(&self) -> &str {
        "e"
    }
    fn recv_timeout(&mut self, _: Duration) -> RecvOutcome {
        RecvOutcome::Closed(CloseReason::ServiceShutdown)
    }
    fn close(&mut self) {}
}

impl ViewService for Probe {
    fn call(&self, request: RequestEnvelope, ctx: &CallContext) -> ResponseEnvelope {
        self.calls.fetch_add(1, Ordering::SeqCst);
        fixture().call(request, ctx)
    }
    fn subscribe(&self, _: Option<(String, u64)>) -> Box<dyn EventStream> {
        self.subscriptions.fetch_add(1, Ordering::SeqCst);
        Box::new(Closed)
    }
}

/// One header: absent, its valid value, the valid value twice, a near miss, or arbitrary bytes.
#[derive(Debug, Clone)]
enum Slot {
    Absent,
    Good,
    Twice,
    Near(usize),
    Raw(Vec<u8>),
}

fn slot(good_weight: u32) -> impl Strategy<Value = Slot> {
    prop_oneof![
        good_weight => Just(Slot::Good),
        1 => Just(Slot::Absent),
        1 => Just(Slot::Twice),
        2 => any::<usize>().prop_map(Slot::Near),
        1 => prop::collection::vec(any::<u8>(), 0..24).prop_map(Slot::Raw),
    ]
}

#[derive(Debug, Clone)]
struct Case {
    method: &'static str,
    target: String,
    host: Slot,
    origin: Slot,
    token: Slot,
    length: Slot,
    kind: Slot,
    chunked: Slot,
    extra: Vec<(&'static str, Vec<u8>)>,
    body_len: usize,
    body: u8,
}

const METHODS: [&str; 11] = [
    "POST", "POST", "POST", "GET", "GET", "OPTIONS", "HEAD", "PUT", "DELETE", "PATCH", "post",
];
const TARGETS: [&str; 18] = [
    "/api/v3/call",
    "/api/v3/call",
    "/api/v3/call",
    "/api/v3/events",
    "/api/v3/events",
    "/api/v3/call?x=1",
    "/api/v3/call/",
    "/API/v3/call",
    "/api/v3/calls",
    "/api/v3/%63all",
    "//api/v3/call",
    "/api/",
    "/",
    "/index.html",
    "http://127.0.0.1:12345/api/v3/call",
    "http://127.0.0.1:12346/api/v3/call",
    "http://evil.example/api/v3/call",
    "http://127.0.0.1:12345/api/v3/events",
];
const EXTRA_NAMES: [&str; 8] = [
    "HOST",
    "Origin",
    "X-Loomward-Token",
    "content-length",
    "transfer-encoding",
    "content-type",
    "last-event-id",
    "x-noise",
];
const SIZES: [usize; 9] = [
    0, 1, 2, 65_535, 65_536, 65_537, 70_000, 1_048_576, 1_048_577,
];

fn case() -> impl Strategy<Value = Case> {
    let target = prop_oneof![
        8 => prop::sample::select(TARGETS.to_vec()).prop_map(String::from),
        1 => "/[ -~]{0,24}",
    ];
    let body_len = prop_oneof![
        3 => Just(HELLO.len()),
        2 => prop::sample::select(SIZES.to_vec()),
        2 => 0usize..256,
    ];
    let heads = (slot(6), slot(6), slot(6), slot(6), slot(6), slot(8));
    let extra = prop::collection::vec(
        (
            prop::sample::select(EXTRA_NAMES.to_vec()),
            prop::collection::vec(any::<u8>(), 0..16),
        ),
        0..3,
    );
    (
        prop::sample::select(METHODS.to_vec()),
        target,
        heads,
        extra,
        body_len,
        0u8..3,
    )
        .prop_map(
            |(
                method,
                target,
                (host, origin, token, length, kind, chunked),
                extra,
                body_len,
                body,
            )| {
                Case {
                    method,
                    target,
                    host,
                    origin,
                    token,
                    length,
                    kind,
                    chunked,
                    extra,
                    body_len,
                    body,
                }
            },
        )
}

fn body(c: &Case) -> Vec<u8> {
    match c.body {
        0 => HELLO.to_vec(),
        // Trailing whitespace keeps the envelope valid at any length past it.
        1 => {
            let mut b = HELLO.to_vec();
            b.resize(c.body_len.max(HELLO.len()), b' ');
            b
        }
        _ => vec![b'x'; c.body_len],
    }
}

/// The values a slot puts on the wire for one header.
fn values(slot: &Slot, good: &[u8], near: &[Vec<u8>]) -> Vec<Vec<u8>> {
    match slot {
        Slot::Absent => vec![],
        Slot::Good => vec![good.to_vec()],
        Slot::Twice => vec![good.to_vec(), good.to_vec()],
        Slot::Near(i) => vec![near[i % near.len()].clone()],
        Slot::Raw(v) => vec![v.clone()],
    }
}

fn request(c: &Case, body: &[u8]) -> Option<hyper::Request<Body>> {
    let token = token();
    let len = body.len().to_string();
    let b = |s: &str| s.as_bytes().to_vec();
    let mut changed = token.clone().into_bytes();
    changed[63] = b'6';
    let headers: [(&str, Vec<Vec<u8>>); 6] = [
        (
            "host",
            values(
                &c.host,
                HOST.as_bytes(),
                &[
                    b("127.0.0.1:12346"),
                    b("localhost:12345"),
                    b("127.0.0.1"),
                    b(""),
                    b("127.0.0.1:12345."),
                ],
            ),
        ),
        (
            "origin",
            values(
                &c.origin,
                [SELF_ORIGIN, ALLOWED][c.body_len % 2].as_bytes(),
                &[
                    b("null"),
                    b("http://127.0.0.1:12345/"),
                    b("HTTP://127.0.0.1:12345"),
                    b("https://127.0.0.1:12345"),
                    b("http://localhost:5174"),
                    b("http://localhost:5173/"),
                    b("http://evil.example"),
                    b(ALLOWED),
                    b(SELF_ORIGIN),
                ],
            ),
        ),
        (
            TOKEN_HEADER,
            values(
                &c.token,
                token.as_bytes(),
                &[
                    changed,
                    token.to_uppercase().into_bytes(),
                    b(&token[..63]),
                    format!("{token}0").into_bytes(),
                    format!("Bearer {token}").into_bytes(),
                    b(""),
                ],
            ),
        ),
        (
            "content-length",
            values(
                &c.length,
                len.as_bytes(),
                &[
                    b(&(body.len() + 1).to_string()),
                    b(&body.len().saturating_sub(1).to_string()),
                    b("65536"),
                    b("65537"),
                    format!("+{len}").into_bytes(),
                    format!("0{len}").into_bytes(),
                    format!("{len}, {len}").into_bytes(),
                    b("-1"),
                    b("18446744073709551616"),
                ],
            ),
        ),
        (
            "content-type",
            values(
                &c.kind,
                b"application/json",
                &[
                    b("Application/JSON; charset=utf-8"),
                    b(" application/json ;x"),
                    b("text/plain"),
                    b("application/jsonx"),
                    b("application/json, text/plain"),
                    b(""),
                ],
            ),
        ),
        // `Good` is no Transfer-Encoding at all.
        (
            "transfer-encoding",
            match c.chunked {
                Slot::Good => vec![],
                ref other => values(other, b"chunked", &[b("chunked"), b("identity"), b("gzip")]),
            },
        ),
    ];
    let mut builder = hyper::Request::builder()
        .method(Method::from_bytes(c.method.as_bytes()).ok()?)
        .uri(c.target.as_str());
    for (name, vs) in headers {
        for v in vs {
            builder = builder.header(name, HeaderValue::from_bytes(&v).ok()?);
        }
    }
    for (name, v) in &c.extra {
        builder = builder.header(*name, HeaderValue::from_bytes(v).ok()?);
    }
    builder.body(Body::from(body.to_vec())).ok()
}

fn exactly_one<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a [u8]> {
    let mut all = headers.get_all(name).iter();
    match (all.next(), all.next()) {
        (Some(v), None) => Some(v.as_bytes()),
        _ => None,
    }
}

/// What an independent reading of the gate (module docs above `route`, semantics.md section 1)
/// says the status must be. `None` for an admitted call, whose status is 200.
fn expected(
    method: &Method,
    path: &str,
    authority: Option<&str>,
    h: &HeaderMap,
    body_len: usize,
) -> Option<StatusCode> {
    let host =
        exactly_one(h, "host") == Some(HOST.as_bytes()) && authority.is_none_or(|a| a == HOST);
    if !host {
        return Some(StatusCode::FORBIDDEN);
    }
    let cross = match h.get_all("origin").iter().collect::<Vec<_>>().as_slice() {
        [] => false,
        [o] if o.as_bytes() == SELF_ORIGIN.as_bytes() => false,
        [o] if o.as_bytes() == ALLOWED.as_bytes() => true,
        _ => return Some(StatusCode::FORBIDDEN),
    };
    if !path.starts_with("/api/") {
        // No static directory: every GET or HEAD is 404, every other method 405.
        return Some(if matches!(*method, Method::GET | Method::HEAD) {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::METHOD_NOT_ALLOWED
        });
    }
    let known = path == "/api/v3/call" || path == "/api/v3/events";
    if *method == Method::OPTIONS && known && cross {
        return Some(StatusCode::NO_CONTENT);
    }
    if exactly_one(h, TOKEN_HEADER) != Some(token().as_bytes()) {
        return Some(StatusCode::FORBIDDEN);
    }
    match (method, path) {
        (&Method::GET, "/api/v3/events") => Some(StatusCode::SERVICE_UNAVAILABLE),
        (&Method::POST, "/api/v3/call") => {
            if h.contains_key("transfer-encoding") {
                return Some(StatusCode::BAD_REQUEST);
            }
            let length = exactly_one(h, "content-length")
                .filter(|v| !v.is_empty() && v.iter().all(u8::is_ascii_digit))
                .and_then(|v| std::str::from_utf8(v).ok()?.parse::<u64>().ok());
            if !length.is_some_and(|n| n <= 65_536) {
                return Some(StatusCode::PAYLOAD_TOO_LARGE);
            }
            let json = h
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(|v| {
                    v.split(';')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_ascii_lowercase()
                });
            if json.as_deref() != Some("application/json") {
                return Some(StatusCode::UNSUPPORTED_MEDIA_TYPE);
            }
            (length != Some(body_len as u64) || body_len > 65_536)
                .then_some(StatusCode::PAYLOAD_TOO_LARGE)
        }
        _ if known => Some(StatusCode::METHOD_NOT_ALLOWED),
        _ => Some(StatusCode::NOT_FOUND),
    }
}

fn shared(probe: Arc<Probe>) -> Shared {
    Shared {
        service: probe,
        host: HOST.into(),
        self_origin: SELF_ORIGIN.into(),
        allow_origins: vec![ALLOWED.into()],
        token: token(),
        static_dir: None,
        read_timeout: Duration::from_secs(5),
        write_timeout: Duration::from_secs(1),
        connection_lifetime: Duration::from_secs(5),
        sse_send_timeout: Duration::from_secs(1),
        streams: Arc::new(Semaphore::new(MAX_STREAMS)),
        stop: Arc::new(AtomicBool::new(false)),
    }
}

fn check(c: &Case) -> Result<(), TestCaseError> {
    let body = body(c);
    let Some(req) = request(c, &body) else {
        return Ok(()); // not representable as an HTTP request (bad method, target or header bytes)
    };
    let (method, path) = (req.method().clone(), req.uri().path().to_owned());
    let authority = req.uri().authority().map(|a| a.as_str().to_owned());
    let headers = req.headers().clone();
    let want = expected(&method, &path, authority.as_deref(), &headers, body.len());

    let probe = Arc::new(Probe::default());
    let s = shared(probe.clone());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (status, res_headers, bytes) = runtime.block_on(async {
        let res = tokio::time::timeout(Duration::from_secs(10), route(&s, req))
            .await
            .expect("route finished");
        let (parts, b) = res.into_parts();
        (
            parts.status,
            parts.headers,
            b.collect().await.unwrap().to_bytes(),
        )
    });
    let calls = probe.calls.load(Ordering::SeqCst);
    let subscriptions = probe.subscriptions.load(Ordering::SeqCst);

    // The service is reached only through the exact routes, past every gate.
    // An admitted body that is no envelope is refused by the protocol ingress, before the service.
    prop_assert_eq!(
        calls,
        usize::from(want.is_none() && c.body != 2),
        "status {}",
        status
    );
    prop_assert_eq!(
        subscriptions,
        usize::from(want == Some(StatusCode::SERVICE_UNAVAILABLE)),
        "status {}",
        status
    );
    if calls > 0 {
        prop_assert!(body.len() <= MAX_REQUEST_BYTES);
        // Exactly one plain-digit Content-Length (leading zeros are legal) equal to the body.
        let declared = exactly_one(&headers, "content-length")
            .filter(|v| !v.is_empty() && v.iter().all(u8::is_ascii_digit))
            .and_then(|v| std::str::from_utf8(v).ok()?.parse::<u64>().ok());
        prop_assert_eq!(declared, Some(body.len() as u64));
    }
    prop_assert_eq!(status, want.unwrap_or(StatusCode::OK));
    // CORS headers go on every API reply past the Host and Origin checks, refusals included.
    let cross = path.starts_with("/api/")
        && exactly_one(&headers, "origin") == Some(ALLOWED.as_bytes())
        && exactly_one(&headers, "host") == Some(HOST.as_bytes())
        && authority.as_deref().is_none_or(|a| a == HOST);
    prop_assert_eq!(
        res_headers
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .map(|v| v.as_bytes()),
        cross.then_some(ALLOWED.as_bytes())
    );
    match status {
        StatusCode::OK => {
            let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            prop_assert_eq!(wire["protocol"].as_str(), Some("loomward/3"));
        }
        StatusCode::NO_CONTENT => prop_assert!(bytes.is_empty()),
        _ => {
            let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            prop_assert!(wire["error"].as_str().is_some_and(|m| !m.is_empty()));
            prop_assert_eq!(
                res_headers.get(header::CONNECTION).map(|v| v.as_bytes()),
                Some(&b"close"[..])
            );
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(config(3000))]

    #[test]
    fn the_gate_admits_only_exact_requests_and_refuses_with_documented_codes(c in case()) {
        check(&c)?;
    }
}

/// Spot checks that the strategy reaches the admitted path and every refusal branch, so a
/// generator that never produced them cannot pass silently.
#[test]
fn the_generator_reaches_every_gate_branch() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    let mut runner = TestRunner::new(config(3000));
    let mut seen = std::collections::BTreeSet::new();
    let strategy = case();
    for _ in 0..3000 {
        let c = strategy.new_tree(&mut runner).unwrap().current();
        let body = body(&c);
        if let Some(req) = request(&c, &body) {
            let authority = req.uri().authority().map(|a| a.as_str().to_owned());
            let want = expected(
                req.method(),
                req.uri().path(),
                authority.as_deref(),
                req.headers(),
                body.len(),
            );
            seen.insert(want.map_or(200, |s| s.as_u16()));
        }
    }
    for code in [200, 204, 400, 403, 404, 405, 413, 415, 503] {
        assert!(
            seen.contains(&code),
            "the generator never produced {code}: {seen:?}"
        );
    }
}
