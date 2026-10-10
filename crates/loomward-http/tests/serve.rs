//! The adapter's security boundary, driven with raw `std::net::TcpStream` requests (no friendly
//! client) so every malformed or hostile shape reaches the server exactly as written.

use loomward_http::fixture::FixtureService;
use loomward_http::{serve, Handle, Options};
use loomward_protocol::{DatasetClass, EventName, Payload};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn fixture() -> FixtureService {
    FixtureService::new(DatasetClass::Synthetic).unwrap()
}

fn start_with(service: FixtureService, options: Options) -> (Handle, Arc<FixtureService>) {
    let service = Arc::new(service);
    let handle = serve(service.clone(), options).unwrap();
    (handle, service)
}

fn start() -> (Handle, Arc<FixtureService>) {
    start_with(fixture(), Options::default())
}

fn connect(addr: SocketAddr) -> TcpStream {
    let s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s
}

/// Sends `head` (CRLF lines, without the blank line) and `body`, then reads to EOF.
fn exchange(addr: SocketAddr, head: &str, body: &[u8]) -> Response {
    let mut s = connect(addr);
    s.write_all(format!("{head}\r\nConnection: close\r\n\r\n").as_bytes())
        .unwrap();
    s.write_all(body).unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).unwrap();
    Response::parse(&raw)
}

struct Response {
    status: u16,
    head: String,
    body: Vec<u8>,
}

impl Response {
    fn parse(raw: &[u8]) -> Response {
        let split = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap_or_else(|| panic!("no response head in {:?}", String::from_utf8_lossy(raw)));
        let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
        let status = head[9..12].parse().unwrap();
        let mut body = raw[split + 4..].to_vec();
        if head.contains("transfer-encoding: chunked") {
            body = dechunk(&body);
        }
        Response { status, head, body }
    }

    fn header(&self, name: &str) -> Option<String> {
        self.head
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{name}: ")).map(str::to_owned))
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

fn dechunk(mut b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let eol = b.windows(2).position(|w| w == b"\r\n").unwrap();
        let n = usize::from_str_radix(std::str::from_utf8(&b[..eol]).unwrap().trim(), 16).unwrap();
        if n == 0 {
            return out;
        }
        out.extend_from_slice(&b[eol + 2..eol + 2 + n]);
        b = &b[eol + 4 + n..];
    }
}

/// Like [`exchange`], but the body goes out in pieces after a pause; write errors are ignored so a
/// reset shows up where it matters, as an unreadable reply.
fn paced(addr: SocketAddr, head: &str, body: &[u8]) -> Response {
    let mut s = connect(addr);
    s.write_all(
        format!(
            "{head}
Connection: close

"
        )
        .as_bytes(),
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(30));
    for piece in body.chunks(8 * 1024) {
        if s.write_all(piece).is_err() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut raw = Vec::new();
    s.read_to_end(&mut raw)
        .unwrap_or_else(|e| panic!("reply lost: {e}"));
    Response::parse(&raw)
}

fn host(h: &Handle) -> String {
    format!("Host: 127.0.0.1:{}", h.addr().port())
}

fn call_head(h: &Handle, len: usize, extra: &str) -> String {
    format!(
        "POST /api/v3/call HTTP/1.1\r\n{}\r\nX-Loomward-Token: {}\r\nContent-Type: application/json\r\nContent-Length: {len}{extra}",
        host(h),
        h.token()
    )
}

fn call(h: &Handle, body: &str) -> Response {
    exchange(h.addr(), &call_head(h, body.len(), ""), body.as_bytes())
}

const HELLO: &str =
    r#"{"protocol":"loomward/3","request_id":"r_1","command":"session.hello","payload":{}}"#;

#[test]
fn a_valid_call_returns_an_envelope_with_200() {
    let (h, _) = start();
    let r = call(&h, HELLO);
    assert_eq!(r.status, 200);
    let v = r.json();
    assert_eq!(v["ok"], true);
    assert_eq!(v["request_id"], "r_1");
    assert_eq!(v["result"]["adapter"], "http");
    assert_eq!(v["meta"]["dataset_class"], "synthetic");
}

#[test]
fn wrong_host_is_403() {
    let (h, _) = start();
    let port = h.addr().port();
    for wrong in [
        format!("Host: localhost:{port}"),
        format!("Host: 127.0.0.1:{}", port.wrapping_add(1)),
        "Host: evil.example".to_string(),
        "Host: 127.0.0.1".to_string(),
    ] {
        let head = call_head(&h, HELLO.len(), "").replace(&host(&h), &wrong);
        let r = exchange(h.addr(), &head, HELLO.as_bytes());
        assert_eq!(r.status, 403, "{wrong}");
    }
    let head = format!("GET http://evil.example/ HTTP/1.1\r\n{}", host(&h));
    assert_eq!(
        exchange(h.addr(), &head, b"").status,
        403,
        "absolute-form foreign authority"
    );
}

#[test]
fn foreign_origin_is_403_and_an_allowed_loopback_origin_gets_cors() {
    let (h, _) = start_with(
        fixture(),
        Options {
            allow_origins: vec!["http://localhost:5173".into()],
            ..Options::default()
        },
    );
    for foreign in [
        "http://evil.example",
        "null",
        "http://localhost:5174",
        "https://localhost:5173",
    ] {
        let r = exchange(
            h.addr(),
            &call_head(&h, HELLO.len(), &format!("\r\nOrigin: {foreign}")),
            HELLO.as_bytes(),
        );
        assert_eq!(r.status, 403, "{foreign}");
    }
    let own = format!("\r\nOrigin: http://127.0.0.1:{}", h.addr().port());
    assert_eq!(
        exchange(
            h.addr(),
            &call_head(&h, HELLO.len(), &own),
            HELLO.as_bytes()
        )
        .status,
        200
    );
    let r = exchange(
        h.addr(),
        &call_head(&h, HELLO.len(), "\r\nOrigin: http://localhost:5173"),
        HELLO.as_bytes(),
    );
    assert_eq!(r.status, 200);
    assert_eq!(
        r.header("access-control-allow-origin").as_deref(),
        Some("http://localhost:5173")
    );
    let pre = exchange(
        h.addr(),
        &format!(
            "OPTIONS /api/v3/call HTTP/1.1\r\n{}\r\nOrigin: http://localhost:5173\r\nAccess-Control-Request-Method: POST",
            host(&h)
        ),
        b"",
    );
    assert_eq!(pre.status, 204);
    assert!(pre
        .header("access-control-allow-headers")
        .unwrap()
        .contains("x-loomward-token"));
}

#[test]
fn missing_or_wrong_token_is_403() {
    let (h, _) = start();
    let good = h.token().to_string();
    let mut wrong = good.clone().into_bytes();
    wrong[63] = if wrong[63] == b'0' { b'1' } else { b'0' };
    let wrong = String::from_utf8(wrong).unwrap();
    for (from, to) in [
        (
            format!("X-Loomward-Token: {good}"),
            "X-Other: 1".to_string(),
        ),
        (
            format!("X-Loomward-Token: {good}"),
            format!("X-Loomward-Token: {wrong}"),
        ),
        (
            format!("X-Loomward-Token: {good}"),
            format!("X-Loomward-Token: {}", &good[..32]),
        ),
        (
            format!("X-Loomward-Token: {good}"),
            format!("X-Loomward-Token: {good}0"),
        ),
        (
            format!("X-Loomward-Token: {good}"),
            format!("X-Loomward-Token: {good}\r\nX-Loomward-Token: {good}"),
        ),
    ] {
        let head = call_head(&h, HELLO.len(), "").replace(&from, &to);
        assert_eq!(
            exchange(h.addr(), &head, HELLO.as_bytes()).status,
            403,
            "{to}"
        );
    }
}

#[test]
fn oversized_or_unframed_body_is_413() {
    let (h, _) = start();
    let big = vec![b' '; 65_537];
    assert_eq!(
        exchange(h.addr(), &call_head(&h, big.len(), ""), &big).status,
        413
    );
    let no_length = format!(
        "POST /api/v3/call HTTP/1.1\r\n{}\r\nX-Loomward-Token: {}\r\nContent-Type: application/json",
        host(&h),
        h.token()
    );
    assert_eq!(exchange(h.addr(), &no_length, b"").status, 413);
    // Exactly the limit is allowed (it is not valid JSON, so the envelope says so).
    let mut edge = vec![b' '; 65_536];
    edge[..HELLO.len()].copy_from_slice(HELLO.as_bytes());
    let r = exchange(h.addr(), &call_head(&h, edge.len(), ""), &edge);
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["ok"], true);
}

#[test]
fn wrong_content_type_is_415() {
    let (h, _) = start();
    for ct in [
        "text/plain",
        "application/x-www-form-urlencoded",
        "application/jsonx",
    ] {
        let head = call_head(&h, HELLO.len(), "").replace("application/json", ct);
        assert_eq!(
            exchange(h.addr(), &head, HELLO.as_bytes()).status,
            415,
            "{ct}"
        );
    }
    let head = call_head(&h, HELLO.len(), "")
        .replace("application/json", "Application/JSON; charset=utf-8");
    assert_eq!(exchange(h.addr(), &head, HELLO.as_bytes()).status, 200);
}

#[test]
fn chunked_body_is_400() {
    let (h, _) = start();
    let head = format!(
        "POST /api/v3/call HTTP/1.1\r\n{}\r\nX-Loomward-Token: {}\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked",
        host(&h),
        h.token()
    );
    let body = format!("{:x}\r\n{HELLO}\r\n0\r\n\r\n", HELLO.len());
    assert_eq!(exchange(h.addr(), &head, body.as_bytes()).status, 400);
}

/// LW-001: Windows resets a socket closed with unread bytes, destroying an early reply. Every
/// early refusal after a large body must still arrive intact, many times over.
#[test]
fn early_error_reply_survives_a_large_unread_body() {
    let (h, _) = start();
    let body = vec![b'x'; 300 * 1024];
    let under = vec![b'x'; 60 * 1024];
    let bad_token = call_head(&h, body.len(), "").replace(h.token(), &"0".repeat(64));
    let wrong_type = call_head(&h, under.len(), "").replace("application/json", "text/plain");
    for i in 0..20 {
        // Paced: the head lands first and the body follows after the server could have replied,
        // the shape in which an undrained socket resets and the reply is lost.
        assert_eq!(
            paced(h.addr(), &bad_token, &body).status,
            403,
            "paced token, round {i}"
        );
        assert_eq!(
            paced(h.addr(), &wrong_type, &under).status,
            415,
            "paced type, round {i}"
        );
        let r = exchange(h.addr(), &call_head(&h, body.len(), ""), &body);
        assert_eq!(r.status, 413, "oversized, round {i}");
        assert_eq!(
            r.json()["error"],
            "the request body exceeds 64 KiB or has no valid Content-Length"
        );
        assert_eq!(
            exchange(h.addr(), &bad_token, &body).status,
            403,
            "token, round {i}"
        );
        assert_eq!(
            exchange(h.addr(), &wrong_type, &under).status,
            415,
            "type, round {i}"
        );
    }
}

#[test]
fn duplicate_json_keys_are_invalid_request_without_an_id() {
    let (h, _) = start();
    let dup = r#"{"protocol":"loomward/3","request_id":"r_1","request_id":"r_2","command":"session.hello","payload":{}}"#;
    let r = call(&h, dup);
    assert_eq!(r.status, 200);
    let v = r.json();
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["code"], "invalid_request");
    assert_eq!(v["request_id"], Value::Null);
    let nested = r#"{"protocol":"loomward/3","request_id":"r_1","command":"session.hello","payload":{"a":1,"a":2}}"#;
    assert_eq!(call(&h, nested).json()["error"]["code"], "invalid_request");
}

#[test]
fn other_protocol_is_unsupported_protocol() {
    let (h, _) = start();
    let r = call(
        &h,
        r#"{"protocol":"loomward/4","request_id":"r_9","command":"session.hello","payload":{}}"#,
    );
    let v = r.json();
    assert_eq!(v["error"]["code"], "unsupported_protocol");
    assert_eq!(v["request_id"], "r_9");
}

#[test]
fn desktop_only_and_unknown_commands_are_refused() {
    let (h, _) = start();
    let grant = r#"{"protocol":"loomward/3","request_id":"r_2","command":"roots.request_grant","payload":{"purpose":"metadata_scan"}}"#;
    let v = call(&h, grant).json();
    assert_eq!(v["error"]["code"], "capability_unavailable", "{v}");
    let v = call(
        &h,
        r#"{"protocol":"loomward/3","request_id":"r_3","command":"files.delete","payload":{}}"#,
    )
    .json();
    assert_eq!(v["error"]["code"], "unknown_command");
}

#[test]
fn nothing_but_the_two_endpoints() {
    let (h, _) = start();
    let t = format!("{}\r\nX-Loomward-Token: {}", host(&h), h.token());
    assert_eq!(
        exchange(h.addr(), &format!("GET /api/v3/call HTTP/1.1\r\n{t}"), b"").status,
        405
    );
    assert_eq!(
        exchange(h.addr(), &format!("GET /api/v3/other HTTP/1.1\r\n{t}"), b"").status,
        404
    );
    assert_eq!(
        exchange(h.addr(), &format!("GET /api/state HTTP/1.1\r\n{t}"), b"").status,
        404
    );
    assert_eq!(
        exchange(h.addr(), &format!("DELETE /x HTTP/1.1\r\n{t}"), b"").status,
        405
    );
    // No --static: there is no file serving at all.
    assert_eq!(
        exchange(h.addr(), &format!("GET / HTTP/1.1\r\n{}", host(&h)), b"").status,
        404
    );
}

#[test]
fn non_loopback_bind_is_refused() {
    for addr in [
        "0.0.0.0:0",
        "[::]:0",
        "[::1]:0",
        "127.0.0.2:0",
        "192.168.1.10:0",
    ] {
        let options = Options {
            bind: addr.parse().unwrap(),
            ..Options::default()
        };
        let err = serve(Arc::new(fixture()), options).err().expect(addr);
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput, "{addr}");
    }
    let options = Options {
        allow_origins: vec!["http://evil.example:80".into()],
        ..Options::default()
    };
    assert!(serve(Arc::new(fixture()), options).is_err());
}

fn assert_security_headers(r: &Response, api: bool) {
    assert_eq!(
        r.header("x-content-type-options").as_deref(),
        Some("nosniff")
    );
    assert_eq!(r.header("referrer-policy").as_deref(), Some("no-referrer"));
    assert_eq!(r.header("cache-control").as_deref(), Some("no-store"));
    assert_eq!(r.header("x-frame-options").as_deref(), Some("deny"));
    let csp = r.header("content-security-policy").unwrap();
    assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
    assert!(csp.contains("default-src 'none'"), "{csp}");
    if !api {
        assert!(csp.contains("script-src 'self'"), "{csp}");
    }
}

#[test]
fn security_headers_on_every_response() {
    let dir = static_tree("headers");
    let (h, _) = start_with(
        fixture(),
        Options {
            static_dir: Some(dir.join("dist")),
            ..Options::default()
        },
    );
    assert_security_headers(&call(&h, HELLO), true);
    assert_security_headers(&call(&h, "{"), true);
    let forbidden = exchange(
        h.addr(),
        &call_head(&h, 2, "").replace(h.token(), "x"),
        b"{}",
    );
    assert_security_headers(&forbidden, true);
    let page = exchange(h.addr(), &format!("GET / HTTP/1.1\r\n{}", host(&h)), b"");
    assert_eq!(page.status, 200);
    assert_security_headers(&page, false);
    assert_security_headers(
        &exchange(
            h.addr(),
            &format!("GET /nope HTTP/1.1\r\n{}", host(&h)),
            b"",
        ),
        false,
    );
}

/// `<tmp>/dist/{index.html, assets/app.js}` beside a `<tmp>/secret.txt` that must never be served.
fn static_tree(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("lw-http-static-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("dist").join("assets")).unwrap();
    std::fs::write(
        base.join("dist").join("index.html"),
        b"<!doctype html><title>t</title>",
    )
    .unwrap();
    std::fs::write(
        base.join("dist").join("assets").join("app.js"),
        b"console.log(1)",
    )
    .unwrap();
    std::fs::write(base.join("secret.txt"), b"TOPSECRET").unwrap();
    #[cfg(windows)]
    std::fs::write(base.join("dist").join("index.html:hidden"), b"TOPSECRET").unwrap();
    base
}

#[test]
fn static_files_never_escape_the_static_root() {
    let base = static_tree("escape");
    let (h, _) = start_with(
        fixture(),
        Options {
            static_dir: Some(base.join("dist")),
            ..Options::default()
        },
    );
    let get = |target: &str| {
        exchange(
            h.addr(),
            &format!("GET {target} HTTP/1.1\r\n{}", host(&h)),
            b"",
        )
    };
    let index = get("/");
    assert_eq!(index.status, 200);
    assert_eq!(
        index.header("content-type").as_deref(),
        Some("text/html; charset=utf-8")
    );
    let js = get("/assets/app.js?v=1");
    assert_eq!(js.status, 200);
    assert_eq!(js.body, b"console.log(1)");
    let drive = base.to_string_lossy().replace('\\', "/");
    for target in [
        "/../secret.txt".to_string(),
        "/assets/../../secret.txt".into(),
        "/%2e%2e/secret.txt".into(),
        "/%2E%2E%2Fsecret.txt".into(),
        "/assets/%2e%2e/%2e%2e/secret.txt".into(),
        "/..%5csecret.txt".into(),
        "/..\\secret.txt".into(),
        "/assets\\..\\..\\secret.txt".into(),
        format!("/{drive}/secret.txt"),
        "/C:/Windows/win.ini".into(),
        "/C:secret.txt".into(),
        "/index.html:hidden".into(),
        "/index.html::$DATA".into(),
        "/index.html%3ahidden".into(),
        "/.%2e/secret.txt".into(),
        "//secret.txt".into(),
        "/CON".into(),
        "/assets".into(),
    ] {
        let r = get(&target);
        assert_ne!(r.status, 200, "{target}");
        assert!(
            !String::from_utf8_lossy(&r.body).contains("TOPSECRET"),
            "{target}"
        );
    }
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn a_stalled_request_is_cut_off_by_the_read_timeout() {
    let (h, _) = start_with(
        fixture(),
        Options {
            read_timeout: Duration::from_millis(400),
            ..Options::default()
        },
    );
    let mut s = connect(h.addr());
    s.write_all(b"POST /api/v3/call HTTP/1.1\r\nHost: 127.0.0.1")
        .unwrap();
    let t = Instant::now();
    let mut buf = Vec::new();
    let _ = s.read_to_end(&mut buf);
    assert!(
        t.elapsed() < Duration::from_secs(5),
        "headers stalled for {:?}",
        t.elapsed()
    );
    // A body that stalls is cut off as well.
    let mut s = connect(h.addr());
    s.write_all(call_head(&h, 100, "").as_bytes()).unwrap();
    s.write_all(b"\r\n\r\n{\"protocol\"").unwrap();
    let t = Instant::now();
    let mut buf = Vec::new();
    let _ = s.read_to_end(&mut buf);
    assert!(
        t.elapsed() < Duration::from_secs(5),
        "body stalled for {:?}",
        t.elapsed()
    );
}

#[test]
fn concurrent_connections_are_bounded() {
    let (h, _) = start_with(
        fixture(),
        Options {
            max_connections: 2,
            ..Options::default()
        },
    );
    let idle_a = connect(h.addr());
    let idle_b = connect(h.addr());
    std::thread::sleep(Duration::from_millis(200));
    let mut third = connect(h.addr());
    third
        .set_read_timeout(Some(Duration::from_millis(700)))
        .unwrap();
    third
        .write_all(
            format!(
                "{}\r\nConnection: close\r\n\r\n{HELLO}",
                call_head(&h, HELLO.len(), "")
            )
            .as_bytes(),
        )
        .unwrap();
    let mut buf = [0u8; 16];
    assert!(
        third.read(&mut buf).is_err(),
        "a third connection was served while two were held"
    );
    drop(idle_a);
    third
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut raw = Vec::new();
    third.read_to_end(&mut raw).unwrap();
    assert_eq!(Response::parse(&raw).status, 200);
    drop(idle_b);
}

// ----------------------------------------------------------------------------------------------
// Server-sent events
// ----------------------------------------------------------------------------------------------

struct Sse {
    reader: BufReader<TcpStream>,
    pending: Vec<u8>,
}

struct Frame {
    id: String,
    event: String,
    data: Value,
}

fn open_events(h: &Handle, extra: &str) -> Result<Sse, Response> {
    let mut s = connect(h.addr());
    write!(
        s,
        "GET /api/v3/events HTTP/1.1\r\n{}\r\nX-Loomward-Token: {}\r\nAccept: text/event-stream{extra}\r\n\r\n",
        host(h),
        h.token()
    )
    .unwrap();
    let mut reader = BufReader::new(s);
    let mut head = String::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        head.push_str(&line);
        if line == "\r\n" || line.is_empty() {
            break;
        }
    }
    let status: u16 = head[9..12].parse().unwrap();
    if status != 200 {
        let mut rest = Vec::new();
        let _ = reader.read_to_end(&mut rest);
        let mut raw = head.into_bytes();
        raw.extend(rest);
        return Err(Response::parse(&raw));
    }
    let lower = head.to_ascii_lowercase();
    assert!(lower.contains("content-type: text/event-stream"), "{head}");
    assert!(lower.contains("cache-control: no-store"), "{head}");
    assert!(lower.contains("transfer-encoding: chunked"), "{head}");
    Ok(Sse {
        reader,
        pending: Vec::new(),
    })
}

impl Sse {
    fn next(&mut self) -> Frame {
        loop {
            if let Some(end) = self.pending.windows(2).position(|w| w == b"\n\n") {
                let text = String::from_utf8(self.pending.drain(..end + 2).collect()).unwrap();
                let field = |name: &str| {
                    text.lines()
                        .find_map(|l| l.strip_prefix(&format!("{name}: ")))
                        .unwrap()
                        .to_owned()
                };
                return Frame {
                    id: field("id"),
                    event: field("event"),
                    data: serde_json::from_str(&field("data")).unwrap(),
                };
            }
            let mut size = String::new();
            self.reader.read_line(&mut size).unwrap();
            let n = usize::from_str_radix(size.trim(), 16).unwrap();
            let mut chunk = vec![0u8; n + 2];
            self.reader.read_exact(&mut chunk).unwrap();
            self.pending.extend_from_slice(&chunk[..n]);
        }
    }
}

fn data(v: Value) -> Payload {
    match v {
        Value::Object(m) => m,
        _ => unreachable!(),
    }
}

fn health_warning(i: u64) -> (EventName, Payload) {
    let mut v: Value = serde_json::from_str(include_str!(
        "../../../contracts/v3/examples/events/health.warning.json"
    ))
    .unwrap();
    v["message"] = json!(format!("warning {i}"));
    (EventName::HealthWarning, data(v))
}

#[test]
fn events_need_the_token() {
    let (h, _) = start();
    for head in [
        format!("GET /api/v3/events HTTP/1.1\r\n{}", host(&h)),
        format!(
            "GET /api/v3/events HTTP/1.1\r\n{}\r\nX-Loomward-Token: {}",
            host(&h),
            "0".repeat(64)
        ),
    ] {
        assert_eq!(exchange(h.addr(), &head, b"").status, 403);
    }
}

#[test]
fn stream_opens_with_hello_and_heartbeats_after_silence() {
    let service = fixture().with_stream_limits(Duration::from_millis(300), 1024);
    let (h, svc) = start_with(service, Options::default());
    let mut sse = open_events(&h, "").ok().unwrap();
    let hello = sse.next();
    assert_eq!(hello.event, "stream.hello");
    assert_eq!(hello.id, format!("{}.0", svc.epoch()));
    assert_eq!(hello.data["data"]["epoch"], svc.epoch());
    let t = Instant::now();
    let beat = sse.next();
    assert_eq!(
        beat.event, "stream.hello",
        "the heartbeat is a repeated stream.hello"
    );
    assert_eq!(
        beat.data["data"]["session_started_at"], hello.data["data"]["session_started_at"],
        "session_started_at is fixed for the session, not regenerated per hello"
    );
    assert!(
        t.elapsed() < Duration::from_secs(3),
        "heartbeat took {:?}",
        t.elapsed()
    );
    svc.publish([health_warning(1)]).unwrap();
    let e = sse.next();
    assert_eq!(e.event, "health.warning");
    assert_eq!(e.id, format!("{}.1", svc.epoch()));
    assert_eq!(e.data["seq"], 1);
}

#[test]
fn last_event_id_resumes_within_the_buffer_else_lagged() {
    let service = fixture().with_stream_limits(Duration::from_secs(15), 4);
    let (h, svc) = start_with(service, Options::default());
    svc.publish((1..=3).map(health_warning)).unwrap();
    let epoch = svc.epoch().to_string();

    let mut sse = open_events(&h, &format!("\r\nLast-Event-ID: {epoch}.1"))
        .ok()
        .unwrap();
    assert_eq!(sse.next().event, "stream.hello");
    assert_eq!(sse.next().id, format!("{epoch}.2"));
    assert_eq!(sse.next().id, format!("{epoch}.3"));
    drop(sse);

    let mut sse = open_events(&h, "\r\nLast-Event-ID: e_00000000deadbeef.2")
        .ok()
        .unwrap();
    assert_eq!(sse.next().event, "stream.hello");
    let lagged = sse.next();
    assert_eq!(lagged.event, "stream.lagged");
    assert_eq!(lagged.data["data"]["reason"], "epoch_changed");
    drop(sse);

    // The app's current bare-seq form is not an `<epoch>.<seq>` id: resync, never a silent resume.
    let mut sse = open_events(&h, "\r\nLast-Event-ID: 2").ok().unwrap();
    sse.next();
    assert_eq!(sse.next().data["data"]["reason"], "epoch_changed");
    drop(sse);

    svc.publish((4..=10).map(health_warning)).unwrap(); // buffer now holds 7..=10
    let mut sse = open_events(&h, &format!("\r\nLast-Event-ID: {epoch}.2"))
        .ok()
        .unwrap();
    let hello = sse.next();
    assert_eq!(hello.data["data"]["oldest_replayable_seq"], 7);
    let lagged = sse.next();
    assert_eq!(lagged.data["data"]["reason"], "replay_gap");
    assert_eq!(lagged.data["data"]["dropped"], Value::Null);
}

#[test]
fn a_reader_behind_the_queue_gets_subscriber_overflow() {
    let service = fixture().with_stream_limits(Duration::from_secs(15), 4);
    let (h, svc) = start_with(service, Options::default());
    let mut sse = open_events(&h, "").ok().unwrap();
    assert_eq!(sse.next().event, "stream.hello");
    svc.publish((1..=10).map(health_warning)).unwrap();
    let lagged = sse.next();
    assert_eq!(lagged.event, "stream.lagged");
    assert_eq!(lagged.data["data"]["reason"], "subscriber_overflow");
    assert_eq!(lagged.data["data"]["dropped"], 6);
    assert_eq!(
        sse.next().id,
        format!("{}.7", svc.epoch()),
        "the seq gap is visible"
    );
}

#[test]
fn at_most_four_streams_and_close_on_disconnect() {
    let service = fixture().with_stream_limits(Duration::from_millis(200), 1024);
    let (h, svc) = start_with(service, Options::default());
    let mut streams: Vec<Sse> = (0..4).map(|_| open_events(&h, "").ok().unwrap()).collect();
    for s in &mut streams {
        assert_eq!(s.next().event, "stream.hello");
    }
    assert_eq!(svc.open_streams(), 4);
    let fifth = open_events(&h, "")
        .err()
        .expect("a fifth stream was opened");
    assert_eq!(fifth.status, 429);
    drop(streams.pop());
    let t = Instant::now();
    while svc.open_streams() > 3 {
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "close was not called on disconnect"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let t = Instant::now();
    let mut again = loop {
        match open_events(&h, "") {
            Ok(s) => break s,
            Err(r) if r.status == 429 && t.elapsed() < Duration::from_secs(5) => {
                std::thread::sleep(Duration::from_millis(50))
            }
            Err(r) => panic!("reconnect refused with {}", r.status),
        }
    };
    assert_eq!(again.next().event, "stream.hello");
    drop(streams);
    drop(again);
    let t = Instant::now();
    while svc.open_streams() > 0 {
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "streams leaked after disconnect"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// `loomward-serve`'s real configuration: the engine service behind the same boundary.
#[test]
fn the_engine_service_answers_through_the_adapter() {
    let tmp = tempfile::tempdir().unwrap();
    let service = loomward_service::Service::open(loomward_service::Config {
        state_dir: tmp.path().into(),
        dataset: DatasetClass::Synthetic,
        allow_personal: false,
        grant_roots: vec![],
    })
    .unwrap();
    let h = serve(Arc::new(service), Options::default()).unwrap();
    let v = call(&h, HELLO).json();
    assert_eq!(v["ok"], true);
    assert_eq!(
        v["result"]["engine_version"]
            .as_str()
            .unwrap()
            .split('/')
            .next(),
        Some("loomward-service")
    );
    assert_eq!(v["result"]["adapter"], "http");
    let roots = call(
        &h,
        r#"{"protocol":"loomward/3","request_id":"r_2","command":"roots.list","payload":{}}"#,
    )
    .json();
    assert_eq!(roots["result"]["roots"], json!([]));
    assert!(
        roots["meta"]["state_rev"].is_string(),
        "state reads carry their revision"
    );
    let grant = call(&h, r#"{"protocol":"loomward/3","request_id":"r_3","command":"roots.request_grant","payload":{"purpose":"metadata_scan"}}"#).json();
    assert_eq!(grant["error"]["code"], "capability_unavailable");
}
