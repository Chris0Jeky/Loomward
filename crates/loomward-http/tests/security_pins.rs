//! Pins for security-relevant adapter behaviour that the other integration tests do not name
//! directly (issue #191). Every test here drives the real server over raw TCP and fails if the
//! behaviour it names breaks. Overlap with `serve.rs` is deliberate: these pins stand on their own.

use loomward_http::fixture::FixtureService;
use loomward_http::{serve, Handle, Options, DRAIN_LIMIT};
use loomward_protocol::{DatasetClass, EventName, Payload};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

const HELLO: &str =
    r#"{"protocol":"loomward/3","request_id":"r_1","command":"session.hello","payload":{}}"#;
/// Mirrors the adapter's private `MAX_STATIC_BYTES` (4 MiB).
const MAX_STATIC_BYTES: usize = 4 * 1024 * 1024;
const JSON_TYPE: &str = "application/json; charset=utf-8";

fn start_with(service: FixtureService, options: Options) -> (Handle, Arc<FixtureService>) {
    let service = Arc::new(service);
    let handle = serve(service.clone(), options).unwrap();
    (handle, service)
}

fn start() -> Handle {
    start_with(
        FixtureService::new(DatasetClass::Synthetic).unwrap(),
        Options::default(),
    )
    .0
}

fn with_static(dir: &Path) -> Handle {
    start_with(
        FixtureService::new(DatasetClass::Synthetic).unwrap(),
        Options {
            static_dir: Some(dir.to_path_buf()),
            ..Options::default()
        },
    )
    .0
}

fn host(h: &Handle) -> String {
    format!("Host: 127.0.0.1:{}", h.addr().port())
}

/// A POST head for `/api/v3/call` with a valid token; `extra` starts with CRLF when present.
fn call_head(h: &Handle, len: usize, extra: &str) -> String {
    format!(
        "POST /api/v3/call HTTP/1.1\r\n{}\r\nX-Loomward-Token: {}\r\nContent-Type: application/json\r\nContent-Length: {len}{extra}",
        host(h),
        h.token()
    )
}

/// Sends `head`, a `Connection: close` line, then `body`, and reads the reply to EOF.
fn send(addr: SocketAddr, head: &str, body: &[u8]) -> Reply {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(format!("{head}\r\nConnection: close\r\n\r\n").as_bytes())
        .unwrap();
    s.write_all(body).unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).unwrap();
    Reply::parse(&raw)
}

/// Like [`send`], but the request does not ask for close, so the reply's `Connection: close` (if any)
/// is the adapter's own. Reads to EOF, which a keep-alive connection never gives without it.
fn send_keep_alive(addr: SocketAddr, head: &str, body: &[u8]) -> Reply {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(format!("{head}\r\n\r\n").as_bytes()).unwrap();
    s.write_all(body).unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).unwrap();
    Reply::parse(&raw)
}

struct Reply {
    status: u16,
    /// Lower-cased head, without the blank line.
    head: String,
    body: Vec<u8>,
}

impl Reply {
    fn parse(raw: &[u8]) -> Reply {
        let split = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap_or_else(|| panic!("no response head in {:?}", String::from_utf8_lossy(raw)));
        let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
        let status = head[9..12].parse().unwrap();
        Reply {
            status,
            head,
            body: raw[split + 4..].to_vec(),
        }
    }

    fn header(&self, name: &str) -> Option<String> {
        self.head.lines().find_map(|l| {
            l.strip_prefix(&format!("{name}: "))
                .map(|v| v.trim().to_string())
        })
    }
}

fn get(h: &Handle, path: &str) -> Reply {
    send(
        h.addr(),
        &format!("GET {path} HTTP/1.1\r\n{}", host(h)),
        b"",
    )
}

fn hello(h: &Handle) -> Reply {
    send(h.addr(), &call_head(h, HELLO.len(), ""), HELLO.as_bytes())
}

/// A contract-shaped `health.warning` event, as `serve.rs` builds them.
fn health_warning(i: u64) -> (EventName, Payload) {
    let mut v: Value = serde_json::from_str(include_str!(
        "../../../contracts/v3/examples/events/health.warning.json"
    ))
    .unwrap();
    v["message"] = json!(format!("warning {i}"));
    match v {
        Value::Object(m) => (EventName::HealthWarning, m),
        _ => unreachable!("the example is an object"),
    }
}

// --- HIGH: a duplicate Content-Length never frames a body --------------------------------------

#[test]
fn conflicting_duplicate_content_length_is_refused_before_dispatch() {
    let h = start();
    // Control: one framing succeeds.
    assert_eq!(hello(&h).status, 200);
    // Two different lengths are the smuggling shape: the server refuses it and never returns the
    // envelope. The refusal comes from hyper before the adapter runs, so only the status is pinned.
    let extra = format!("\r\nContent-Length: {}", HELLO.len() + 1);
    let r = send(
        h.addr(),
        &call_head(&h, HELLO.len(), &extra),
        HELLO.as_bytes(),
    );
    assert_eq!(r.status, 400);
}

// --- HIGH: a junction inside the static root that leaves it is refused -------------------------

#[cfg(windows)]
#[test]
fn junction_inside_static_root_pointing_outside_is_refused() {
    let base = tempfile::tempdir().unwrap();
    let dist = base.path().join("dist");
    let outside = base.path().join("outside");
    std::fs::create_dir_all(dist.join("assets")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(dist.join("index.html"), b"<!doctype html>").unwrap();
    std::fs::write(outside.join("secret.txt"), b"TOPSECRET").unwrap();
    let junction = dist.join("escape-dir");
    let made = std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(&junction)
        .arg(&outside)
        .output()
        .expect("cmd runs");
    // A fixture that cannot be built fails the pin; it never passes unexercised.
    assert!(made.status.success(), "mklink /J failed: {made:?}");
    let h = with_static(&dist);
    // Control: a real file under the root is served.
    let ok = get(&h, "/index.html");
    assert_eq!(ok.status, 200);
    // The junction leads outside the root: refused, and the secret never appears.
    let r = get(&h, "/escape-dir/secret.txt");
    assert_eq!(r.status, 404);
    assert!(
        !String::from_utf8_lossy(&r.body).contains("TOPSECRET"),
        "junction leaked the file outside the static root"
    );
    drop(h);
    let _ = std::fs::remove_dir(&junction);
}

// --- HIGH: the session token is 64 lowercase hex characters and differs per Handle -------------

#[test]
fn session_token_is_64_hex_characters_and_unique_per_handle() {
    let handles: Vec<Handle> = (0..4).map(|_| start()).collect();
    let mut seen = HashSet::new();
    for h in &handles {
        let token = h.token();
        assert_eq!(token.len(), 64, "token {token}");
        assert!(
            token
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "token {token} is not lowercase hex"
        );
        assert!(seen.insert(token.to_string()), "a token repeated: {token}");
    }
}

// --- MEDIUM: a declared length above the drain limit is refused without reading the body -------

#[test]
fn declared_length_above_drain_limit_is_refused_without_waiting_for_the_body() {
    let h = start();
    let declared = DRAIN_LIMIT + 1;
    // No body follows; a correct server answers from the headers alone, well inside the read
    // timeout (10 s). A server that drains first would wait out that timeout.
    let started = Instant::now();
    let r = send(h.addr(), &call_head(&h, declared, ""), b"");
    assert_eq!(r.status, 413);
    assert_eq!(
        r.header("content-type").as_deref(),
        Some(JSON_TYPE),
        "declared {declared}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "declared {declared} waited {:?} for a body that was never sent",
        started.elapsed()
    );
}

// --- MEDIUM: cross-origin-resource-policy and cross-origin-opener-policy on every response ------

#[test]
fn cross_origin_policy_headers_are_present_on_api_and_static_responses() {
    let dir = tempfile::tempdir().unwrap();
    let dist = dir.path().join("dist");
    std::fs::create_dir_all(&dist).unwrap();
    std::fs::write(dist.join("index.html"), b"<!doctype html>").unwrap();
    let h = with_static(&dist);
    let replies = [
        ("api ok", hello(&h)),
        (
            "api refused",
            send(
                h.addr(),
                &call_head(&h, HELLO.len(), "").replace(h.token(), &"0".repeat(64)),
                HELLO.as_bytes(),
            ),
        ),
        ("static ok", get(&h, "/index.html")),
        ("static missing", get(&h, "/nope.html")),
    ];
    for (name, r) in &replies {
        assert_eq!(
            r.header("cross-origin-resource-policy").as_deref(),
            Some("same-origin"),
            "{name}"
        );
        assert_eq!(
            r.header("cross-origin-opener-policy").as_deref(),
            Some("same-origin"),
            "{name}"
        );
    }
}

// --- MEDIUM: a duplicated Last-Event-ID resyncs instead of resuming ---------------------------

struct Frame {
    event: String,
    data: Value,
}

/// Minimal SSE reader over a raw connection. Chunk-size lines are skipped by prefix.
struct Sse {
    reader: BufReader<TcpStream>,
}

impl Sse {
    fn open(h: &Handle, extra: &str) -> Sse {
        let mut s = TcpStream::connect(h.addr()).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        write!(
            s,
            "GET /api/v3/events HTTP/1.1\r\n{}\r\nX-Loomward-Token: {}\r\nAccept: text/event-stream{extra}\r\n\r\n",
            host(h),
            h.token()
        )
        .unwrap();
        let mut reader = BufReader::new(s);
        let mut status = String::new();
        reader.read_line(&mut status).unwrap();
        assert!(status.contains(" 200 "), "stream refused: {status}");
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
        }
        Sse { reader }
    }

    fn next(&mut self) -> Frame {
        let mut event = String::new();
        loop {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line).unwrap();
            assert_ne!(n, 0, "stream ended before a frame");
            let line = line.trim_end();
            if let Some(name) = line.strip_prefix("event: ") {
                event = name.to_string();
            } else if let Some(data) = line.strip_prefix("data: ") {
                return Frame {
                    event: std::mem::take(&mut event),
                    data: serde_json::from_str(data).unwrap(),
                };
            }
        }
    }
}

#[test]
fn duplicated_last_event_id_resyncs_as_epoch_changed() {
    let service = FixtureService::new(DatasetClass::Synthetic)
        .unwrap()
        .with_stream_limits(Duration::from_secs(15), 1024);
    let (h, svc) = start_with(service, Options::default());
    svc.publish((1..=3).map(health_warning)).unwrap();
    let epoch = svc.epoch().to_string();

    // Control: one header resumes after seq 1, so the next event is seq 2.
    let mut sse = Sse::open(&h, &format!("\r\nLast-Event-ID: {epoch}.1"));
    assert_eq!(sse.next().event, "stream.hello");
    let resumed = sse.next();
    assert_eq!(resumed.event, "health.warning");
    drop(sse);

    // Two identical headers are ambiguous: the stream resyncs rather than resuming.
    let mut sse = Sse::open(
        &h,
        &format!("\r\nLast-Event-ID: {epoch}.1\r\nLast-Event-ID: {epoch}.1"),
    );
    assert_eq!(sse.next().event, "stream.hello");
    let lagged = sse.next();
    assert_eq!(lagged.event, "stream.lagged");
    assert_eq!(lagged.data["data"]["reason"], "epoch_changed");
    drop(sse);
}

// --- MEDIUM: Handle::url() carries the token in the fragment, never the query ------------------

#[test]
fn handle_url_puts_the_token_in_the_fragment_only() {
    let h = start();
    let url = h.url();
    assert!(!url.contains('?'), "query present in {url}");
    let (before, fragment) = url.split_once('#').expect("fragment present");
    assert!(
        !before.contains("token"),
        "token before the fragment: {url}"
    );
    assert_eq!(fragment, format!("token={}", h.token()));
    assert_eq!(before, format!("http://{}/", h.addr()));
}

// --- MEDIUM: static files over the size limit are refused --------------------------------------

#[test]
fn static_file_over_the_size_limit_is_refused_and_the_limit_is_inclusive() {
    let dir = tempfile::tempdir().unwrap();
    let dist = dir.path().join("dist");
    std::fs::create_dir_all(&dist).unwrap();
    std::fs::write(dist.join("edge.bin"), vec![0u8; MAX_STATIC_BYTES]).unwrap();
    std::fs::write(dist.join("large.bin"), vec![0u8; MAX_STATIC_BYTES + 1]).unwrap();
    let h = with_static(&dist);
    // Control: exactly the limit is served.
    let edge = get(&h, "/edge.bin");
    assert_eq!(edge.status, 200);
    assert_eq!(edge.body.len(), MAX_STATIC_BYTES);
    // One byte over: refused, and no part of the file is returned.
    let large = get(&h, "/large.bin");
    assert_eq!(large.status, 404);
    assert!(large.body.len() < 1024, "served {} bytes", large.body.len());
}

// --- LOW: API errors are JSON and close the connection -----------------------------------------

#[test]
fn api_errors_carry_json_content_type_and_connection_close() {
    // Keep-alive requests (no `Connection: close` from the client): the close must come from the
    // adapter's own refusal, not be echoed by hyper.
    let h = start();
    let mut cases: Vec<(&str, Reply)> = Vec::new();
    cases.push((
        "bad token",
        send_keep_alive(
            h.addr(),
            &call_head(&h, HELLO.len(), "").replace(h.token(), &"0".repeat(64)),
            HELLO.as_bytes(),
        ),
    ));
    cases.push((
        "wrong media type",
        send_keep_alive(
            h.addr(),
            &call_head(&h, HELLO.len(), "").replace("application/json", "text/plain"),
            HELLO.as_bytes(),
        ),
    ));
    let big = vec![b'x'; 70_000];
    cases.push((
        "oversized",
        send_keep_alive(h.addr(), &call_head(&h, big.len(), ""), &big),
    ));
    cases.push((
        "unknown api path",
        send_keep_alive(
            h.addr(),
            &format!(
                "GET /api/v3/other HTTP/1.1\r\n{}\r\nX-Loomward-Token: {}",
                host(&h),
                h.token()
            ),
            b"",
        ),
    ));
    cases.push((
        "wrong method",
        send_keep_alive(
            h.addr(),
            &format!(
                "GET /api/v3/call HTTP/1.1\r\n{}\r\nX-Loomward-Token: {}",
                host(&h),
                h.token()
            ),
            b"",
        ),
    ));
    for (name, r) in &cases {
        assert!(r.status >= 400, "{name}: status {}", r.status);
        assert_eq!(
            r.header("content-type").as_deref(),
            Some(JSON_TYPE),
            "{name}"
        );
        assert_eq!(r.header("connection").as_deref(), Some("close"), "{name}");
        assert!(
            serde_json::from_slice::<Value>(&r.body).is_ok(),
            "{name}: body is not JSON"
        );
    }
}
