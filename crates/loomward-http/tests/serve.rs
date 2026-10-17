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

/// #441: a `Connection: close` reply makes the server close first, so every exchange leaves a
/// server-side TIME_WAIT for its address pair, for minutes. Windows picks connect-time client
/// ports out of order (measured against one server: 19042, 19055, ..., 32899, 52954) and can repeat
/// one within a test's hundred connections; the SYN then matches that TIME_WAIT, is dropped, and
/// connect fails after its 21 s of retries (10060). So bind a port not yet used against `addr`.
fn connect(addr: SocketAddr) -> TcpStream {
    static USED: std::sync::Mutex<std::collections::BTreeSet<(SocketAddr, u16)>> =
        std::sync::Mutex::new(std::collections::BTreeSet::new());
    // Rejected sockets stay bound until a fresh port turns up, so the next bind cannot repeat them.
    let mut rejected = Vec::new();
    let s: TcpStream = loop {
        let socket =
            socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
        socket
            .bind(&SocketAddr::from(([127, 0, 0, 1], 0)).into())
            .unwrap();
        let port = socket.local_addr().unwrap().as_socket().unwrap().port();
        if USED.lock().unwrap().insert((addr, port)) {
            socket.connect(&addr.into()).unwrap();
            break socket.into();
        }
        rejected.push(socket);
    };
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
    if head.is_empty() {
        // Closed before any response (status 0): a caller decides whether that is a failure.
        return Err(Response {
            status: 0,
            head,
            body: Vec::new(),
        });
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

#[test]
fn gates_precede_a_stalled_body() {
    let (h, _) = start_with(
        fixture(),
        Options {
            read_timeout: Duration::from_millis(500),
            ..Options::default()
        },
    );
    let good = call_head(&h, 100, "");
    for head in [
        good.replace(&host(&h), "Host: evil.example"),
        format!("{good}\r\nOrigin: http://evil.example"),
        good.replace(h.token(), &"0".repeat(64)),
    ] {
        let mut tcp = connect(h.addr());
        tcp.write_all(format!("{head}\r\nConnection: close\r\n\r\n").as_bytes())
            .unwrap();
        let mut raw = Vec::new();
        tcp.read_to_end(&mut raw).unwrap();
        assert_eq!(
            Response::parse(&raw).status,
            403,
            "gate lost to the stalled body"
        );
    }
    assert_eq!(call(&h, HELLO).status, 200);
}

fn non_readers_release_connections(
    write_timeout: Duration,
    connection_lifetime: Duration,
    name: &str,
) {
    let base = static_tree(name);
    std::fs::write(base.join("dist/large.bin"), vec![0; 4 * 1024 * 1024]).unwrap();
    let (h, _) = start_with(
        fixture(),
        Options {
            static_dir: Some(base.join("dist")),
            write_timeout,
            connection_lifetime,
            ..Options::default()
        },
    );
    let clients: Vec<_> = (0..16)
        .map(|_| {
            let socket = socket2::Socket::new(
                socket2::Domain::IPV4,
                socket2::Type::STREAM,
                Some(socket2::Protocol::TCP),
            )
            .unwrap();
            socket.set_recv_buffer_size(1024).unwrap();
            socket.connect(&h.addr().into()).unwrap();
            let mut tcp: TcpStream = socket.into();
            // Pipeline enough responses to exceed Windows socket buffering without reading.
            tcp.write_all(
                format!("GET /large.bin HTTP/1.1\r\n{}\r\n\r\n", host(&h))
                    .repeat(32)
                    .as_bytes(),
            )
            .unwrap();
            tcp
        })
        .collect();
    // Give the accepted requests time to fill each client's small receive window.
    std::thread::sleep(Duration::from_millis(300));
    let t = Instant::now();
    assert_eq!(call(&h, HELLO).status, 200);
    assert!(
        t.elapsed() < Duration::from_secs(8),
        "17th client starved for {:?}",
        t.elapsed()
    );
    drop(clients);
    drop(h);
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn sixteen_non_readers_release_connections_at_the_write_deadline() {
    non_readers_release_connections(
        Duration::from_secs(1),
        Duration::from_secs(60),
        "write-deadline",
    );
}

#[test]
fn sixteen_non_readers_release_connections_at_the_lifetime_cap() {
    non_readers_release_connections(
        Duration::from_secs(2),
        Duration::from_secs(1),
        "lifetime-cap",
    );
}

#[test]
fn static_roots_cannot_overlap_grants_or_state_in_either_direction() {
    let base = static_tree("scope");
    let dist = base.join("dist");
    let child = dist.join("assets");
    for protected in [&base, &dist, &child] {
        for state in [false, true] {
            let options = Options {
                static_dir: Some(dist.clone()),
                grant_roots: if state {
                    vec![]
                } else {
                    vec![protected.canonicalize().unwrap()]
                },
                state_dir: state.then(|| protected.join(".").to_path_buf()),
                ..Options::default()
            };
            let err = serve(Arc::new(fixture()), options)
                .err()
                .expect("overlapping static root accepted");
            assert!(err.to_string().contains("overlaps"));
        }
    }
    let sibling = base.join("dist-other");
    std::fs::create_dir_all(&sibling).unwrap();
    let (h, _) = start_with(
        fixture(),
        Options {
            static_dir: Some(dist),
            grant_roots: vec![sibling.canonicalize().unwrap()],
            state_dir: Some(sibling),
            ..Options::default()
        },
    );
    assert_eq!(
        exchange(h.addr(), &format!("GET / HTTP/1.1\r\n{}", host(&h)), b"").status,
        200
    );
    drop(h);
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn static_files_are_limited_to_four_mib() {
    let base = static_tree("size-limit");
    std::fs::write(base.join("dist/edge.bin"), vec![0; 4 * 1024 * 1024]).unwrap();
    std::fs::write(base.join("dist/large.bin"), vec![0; 4 * 1024 * 1024 + 1]).unwrap();
    let (h, _) = start_with(
        fixture(),
        Options {
            static_dir: Some(base.join("dist")),
            ..Options::default()
        },
    );
    for (path, status) in [("edge.bin", 200), ("large.bin", 404)] {
        let res = exchange(
            h.addr(),
            &format!("GET /{path} HTTP/1.1\r\n{}", host(&h)),
            b"",
        );
        assert_eq!(res.status, status);
    }
    drop(h);
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn a_second_bind_to_the_serve_port_is_refused() {
    let (h, _) = start();
    assert!(serve(
        Arc::new(fixture()),
        Options {
            bind: h.addr(),
            ..Options::default()
        }
    )
    .is_err());
    #[cfg(windows)]
    {
        let hostile = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )
        .unwrap();
        hostile.set_reuse_address(true).unwrap();
        assert!(
            hostile.bind(&h.addr().into()).is_err(),
            "SO_REUSEADDR shared the token-bearing port"
        );
    }
    assert_eq!(call(&h, HELLO).status, 200);
}

#[test]
fn serve_binary_excludes_its_state_directory_from_static_files() {
    let base = static_tree("cli-state");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_loomward-serve"))
        .args([
            "--static",
            base.to_str().unwrap(),
            "--state-dir",
            base.join("dist/.").to_str().unwrap(),
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let end = Instant::now() + Duration::from_secs(8);
    let exited = loop {
        if child.try_wait().unwrap().is_some() {
            break true;
        }
        if Instant::now() >= end {
            child.kill().unwrap();
            break false;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let output = child.wait_with_output().unwrap();
    assert!(exited, "unsafe static server started instead of refusing");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("overlaps"));
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn sse_connections_outlive_the_non_sse_lifetime_cap() {
    let (h, svc) = start_with(
        fixture().with_stream_limits(Duration::from_millis(100), 1024),
        Options {
            connection_lifetime: Duration::from_millis(300),
            ..Options::default()
        },
    );
    let mut sse = open_events(&h, "").ok().unwrap();
    assert_eq!(sse.next().event, "stream.hello");
    std::thread::sleep(Duration::from_millis(700));
    assert_eq!(sse.next().event, "stream.hello");
    assert_eq!(svc.open_streams(), 1);
}

/// `loomward-serve`'s real configuration: the engine service behind the same boundary.
#[test]
fn the_engine_service_answers_through_the_adapter() {
    let tmp = tempfile::tempdir_in(loomward_service::paths::test_roots_base()).unwrap();
    let service = loomward_service::Service::open(loomward_service::Config {
        state_dir: tmp.path().into(),
        dataset: DatasetClass::Synthetic,
        allow_personal: false,
        allow_identity_unverified: false,
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

struct FollowupService {
    fixture: FixtureService,
    call_entered: Option<std::sync::mpsc::Sender<()>>,
    call_release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    stream_end: std::sync::Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

impl loomward_protocol::ViewService for FollowupService {
    fn call(
        &self,
        request: loomward_protocol::RequestEnvelope,
        ctx: &loomward_protocol::CallContext,
    ) -> loomward_protocol::ResponseEnvelope {
        if let Some(entered) = &self.call_entered {
            entered.send(()).unwrap();
            self.call_release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(3))
                .unwrap();
        }
        self.fixture.call(request, ctx)
    }

    fn subscribe(&self, resume: Option<(String, u64)>) -> Box<dyn loomward_protocol::EventStream> {
        Box::new(FollowupStream {
            source: self.fixture.subscribe(resume),
            first: true,
            end: self.stream_end.lock().unwrap().take().unwrap(),
        })
    }
}

struct FollowupStream {
    source: Box<dyn loomward_protocol::EventStream>,
    first: bool,
    end: std::sync::mpsc::Receiver<()>,
}

impl loomward_protocol::EventStream for FollowupStream {
    fn epoch(&self) -> &str {
        self.source.epoch()
    }
    fn recv_timeout(&mut self, timeout: Duration) -> loomward_protocol::RecvOutcome {
        if self.first {
            self.first = false;
            return self.source.recv_timeout(timeout);
        }
        match self.end.recv_timeout(timeout) {
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                loomward_protocol::RecvOutcome::Timeout
            }
            _ => loomward_protocol::RecvOutcome::Closed(
                loomward_protocol::CloseReason::ServiceShutdown,
            ),
        }
    }
    fn close(&mut self) {
        self.source.close();
    }
}

#[test]
fn followup_lifetime_expiry_drains_an_in_flight_call() {
    let (entered, entry) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let h = serve(
        Arc::new(FollowupService {
            fixture: fixture(),
            call_entered: Some(entered),
            call_release: std::sync::Mutex::new(released),
            stream_end: std::sync::Mutex::new(None),
        }),
        Options {
            connection_lifetime: Duration::from_millis(300),
            write_timeout: Duration::from_secs(1),
            ..Options::default()
        },
    )
    .unwrap();
    let addr = h.addr();
    let head = call_head(&h, HELLO.len(), "");
    let client = std::thread::spawn(move || exchange(addr, &head, HELLO.as_bytes()));
    entry.recv_timeout(Duration::from_secs(2)).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    release.send(()).unwrap();
    let response = client.join().unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.json()["ok"], true);
}

#[test]
fn followup_finished_sse_restores_the_lifetime_cap() {
    let lifetime = Duration::from_millis(300);
    // The cap exempts a stream only once it is open. On a loaded host the lifetime can expire
    // before the server answers, which closes the connection correctly but proves nothing here.
    // An attempt counts only when the head arrived within the lifetime measured from before the
    // connect: the server marks the stream open before it writes the head, so it was open before
    // the cap fired (#320).
    for _ in 0..20 {
        let (_, released) = std::sync::mpsc::channel();
        let (end, ended) = std::sync::mpsc::channel();
        let h = serve(
            Arc::new(FollowupService {
                fixture: fixture(),
                call_entered: None,
                call_release: std::sync::Mutex::new(released),
                stream_end: std::sync::Mutex::new(Some(ended)),
            }),
            Options {
                connection_lifetime: lifetime,
                write_timeout: Duration::from_millis(300),
                read_timeout: Duration::from_secs(5),
                ..Options::default()
            },
        )
        .unwrap();
        let connecting = Instant::now();
        let Ok(mut sse) = open_events(&h, "") else {
            continue;
        };
        if connecting.elapsed() >= lifetime {
            continue;
        }
        assert_eq!(sse.next().event, "stream.hello");
        std::thread::sleep(Duration::from_millis(500));
        end.send(()).unwrap();
        sse.reader
            .get_ref()
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut rest = Vec::new();
        sse.reader
            .read_to_end(&mut rest)
            .expect("connection remained exempt after SSE ended");
        assert_eq!(
            rest, b"0\r\n\r\n",
            "SSE must finish before the connection closes"
        );
        return;
    }
    panic!("no attempt opened the stream within the connection lifetime");
}

#[cfg(windows)]
fn followup_refused_start(
    state: &std::path::Path,
    static_dir: &std::path::Path,
    grant: Option<&std::path::Path>,
    personal: bool,
) {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_loomward-serve"));
    command
        .arg("--state-dir")
        .arg(state)
        .arg("--static")
        .arg(static_dir);
    if let Some(grant) = grant {
        command.arg("--grant-root").arg(grant);
    }
    if personal {
        command.args(["--dataset", "personal", "--allow-personal"]);
    }
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let end = Instant::now() + Duration::from_secs(3);
    let exited = loop {
        if child.try_wait().unwrap().is_some() {
            break true;
        }
        if Instant::now() >= end {
            child.kill().unwrap();
            break false;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let output = child.wait_with_output().unwrap();
    assert!(exited, "overlapping static server started");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("overlaps"));
}

#[cfg(windows)]
#[test]
fn followup_stored_grants_refuse_static_without_recording_new_grants() {
    if loomward_windows::enumerate::running_elevated() {
        // Native identity refuses elevation (invariant 2, #217), so no lab root can be registered.
        let tmp = tempfile::tempdir_in(loomward_service::paths::test_roots_base()).unwrap();
        let err = loomward_service::lab::register(
            &tmp.path().join("state"),
            &tmp.path().join("lab"),
            "s",
            "S",
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap_err();
        assert!(err.contains("identity_unavailable"), "{err}");
        return;
    }
    use loomward_protocol::{CallContext, RequestEnvelope, ViewService};
    use loomward_service::{lab, Config, Service};
    let tmp = tempfile::tempdir_in(loomward_service::paths::test_roots_base()).unwrap();
    let state = tmp.path().join("state");
    let dist = tmp.path().join("dist");
    let old = dist.join("stored-lab");
    let new = tmp.path().join("new-lab");
    lab::register(
        &state,
        &old,
        "s",
        "S",
        "sha256:0000000000000000000000000000000000000000000000000000000000000000",
    )
    .unwrap();
    lab::register(
        &state,
        &new,
        "s",
        "S",
        "sha256:0000000000000000000000000000000000000000000000000000000000000000",
    )
    .unwrap();
    let cfg = |roots| Config {
        state_dir: state.clone(),
        dataset: DatasetClass::Synthetic,
        allow_personal: false,
        allow_identity_unverified: false,
        grant_roots: roots,
    };
    drop(Service::open(cfg(vec![old.canonicalize().unwrap()])).unwrap());
    followup_refused_start(&state, &dist, Some(&new), false);
    let service = Service::open(cfg(vec![])).unwrap();
    let response = service.call(RequestEnvelope::from_slice(br#"{"protocol":"loomward/3","request_id":"r_roots","command":"roots.list","payload":{}}"#).unwrap(), &CallContext::http());
    let roots = serde_json::to_value(response).unwrap();
    assert_eq!(roots["result"]["roots"].as_array().unwrap().len(), 1);
    assert!(roots["result"]["roots"][0]["display_path"]["text"]
        .as_str()
        .unwrap()
        .ends_with("stored-lab"));
    drop(service);
    // A disjoint static tree still permits the stored root and the proposed grant.
    let safe = tmp.path().join("safe-static");
    std::fs::create_dir(&safe).unwrap();
    assert!(Service::open_with_static(cfg(vec![new.canonicalize().unwrap()]), Some(&safe)).is_ok());
}

#[cfg(windows)]
#[test]
fn followup_proposed_static_overlap_leaves_no_active_grant() {
    if loomward_windows::enumerate::running_elevated() {
        // Native identity refuses elevation (invariant 2, #217), so no lab root can be registered.
        let tmp = tempfile::tempdir_in(loomward_service::paths::test_roots_base()).unwrap();
        let err = loomward_service::lab::register(
            &tmp.path().join("state"),
            &tmp.path().join("lab"),
            "s",
            "S",
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap_err();
        assert!(err.contains("identity_unavailable"), "{err}");
        return;
    }
    use loomward_protocol::{CallContext, RequestEnvelope, ViewService};
    use loomward_service::{lab, Config, Service};
    let tmp = tempfile::tempdir_in(loomward_service::paths::test_roots_base()).unwrap();
    let state = tmp.path().join("state");
    let grant = tmp.path().join("dist/lab");
    lab::register(
        &state,
        &grant,
        "s",
        "S",
        "sha256:0000000000000000000000000000000000000000000000000000000000000000",
    )
    .unwrap();
    followup_refused_start(&state, &tmp.path().join("dist"), Some(&grant), false);
    let service = Service::open(Config {
        state_dir: state,
        dataset: DatasetClass::Synthetic,
        allow_personal: false,
        allow_identity_unverified: false,
        grant_roots: vec![],
    })
    .unwrap();
    let response = service.call(RequestEnvelope::from_slice(br#"{"protocol":"loomward/3","request_id":"r_roots","command":"roots.list","payload":{}}"#).unwrap(), &CallContext::http());
    assert_eq!(
        serde_json::to_value(response).unwrap()["result"]["roots"],
        json!([])
    );
}

#[cfg(windows)]
#[test]
fn followup_stored_picker_grant_refuses_static() {
    use loomward_protocol::{CallContext, NativeDialogs, RequestEnvelope, ViewService};
    struct Picker(PathBuf);
    impl NativeDialogs for Picker {
        fn pick_folder(&self) -> Option<PathBuf> {
            Some(self.0.clone())
        }
        fn confirm_disclosure(
            &self,
            _: &loomward_protocol::dto::DisclosureSummary,
        ) -> Result<bool, loomward_protocol::ErrorBody> {
            Ok(false)
        }
    }
    let tmp = tempfile::tempdir_in(loomward_service::paths::test_roots_base()).unwrap();
    let state = tmp.path().join("state");
    let dist = tmp.path().join("dist");
    let root = dist.join("picked-fixture");
    std::fs::create_dir_all(&root).unwrap();
    // Personal policy is exercised using only disposable synthetic files.
    let service = loomward_service::Service::open(loomward_service::Config {
        state_dir: state.clone(),
        dataset: DatasetClass::Personal,
        allow_personal: true,
        allow_identity_unverified: false,
        grant_roots: vec![],
    })
    .unwrap();
    let response = service.call(RequestEnvelope::from_slice(br#"{"protocol":"loomward/3","request_id":"r_pick","command":"roots.request_grant","payload":{"purpose":"metadata_scan"}}"#).unwrap(), &CallContext::tauri(Arc::new(Picker(root))));
    let granted = serde_json::to_value(response).unwrap();
    if loomward_windows::enumerate::running_elevated() {
        // Native identity refuses elevation (invariant 2, #217): the picker grant is refused.
        assert_eq!(granted["result"]["outcome"], "refused");
        return;
    }
    assert_eq!(granted["result"]["outcome"], "granted");
    assert_eq!(granted["result"]["root"]["granted_via"], "desktop_picker");
    drop(service);
    followup_refused_start(&state, &dist, None, true);
    let dir = dist.canonicalize().unwrap();
    let volume = dir.ancestors().last().unwrap().to_path_buf();
    assert!(
        loomward_service::paths::validate_static_dir(Some(&dir), &[volume], None)
            .unwrap_err()
            .contains("overlaps")
    );
}

// ----------------------------------------------------------------------------------------------
// Issue #191: security behaviours with no pinning test.
// ----------------------------------------------------------------------------------------------

#[test]
fn conflicting_content_length_is_refused_and_identical_duplicates_coalesce() {
    let (h, _) = start();
    // Control: a single framing succeeds.
    let ok = call(&h, HELLO);
    assert_eq!(ok.status, 200);
    assert_eq!(ok.json()["ok"], true);
    // Identical duplicated values frame the body the same way: RFC 9112 section 6.3 lets a
    // recipient treat them as one, and hyper coalesces them before the adapter sees them.
    let dup_same = format!("\r\nContent-Length: {}", HELLO.len());
    let r = exchange(
        h.addr(),
        &call_head(&h, HELLO.len(), &dup_same),
        HELLO.as_bytes(),
    );
    assert_eq!(r.status, 200);
    // Conflicting values are the smuggling case: hyper refuses them with a bare 400 before the
    // adapter runs, so no body is framed two ways. The head alone decides it, so no body is sent:
    // hyper replies and closes without reading one, and a body still in flight meets that close
    // as a connection reset that discards the unread 400 (a loaded host loses this race).
    let dup_diff = format!("\r\nContent-Length: {}", HELLO.len() + 1);
    let r = exchange(h.addr(), &call_head(&h, HELLO.len(), &dup_diff), b"");
    assert_eq!(r.status, 400);
}

#[test]
fn symlink_inside_static_root_pointing_outside_is_refused() {
    let base = static_tree("symlink-escape");
    let outside = base.join("secret.txt");
    let link = base.join("dist").join("escape.txt");
    let _ = std::fs::remove_file(&link);
    #[cfg(unix)]
    let escape = {
        std::os::unix::fs::symlink(&outside, &link).expect(
            "security regression requires a real symlink fixture; setup failure is not a pass",
        );
        "/escape.txt"
    };
    // A file symlink needs Developer Mode or the symlink privilege. A directory junction needs
    // neither, so hosts without the privilege still exercise an escape; fixture failure fails.
    #[cfg(windows)]
    let escape = if std::os::windows::fs::symlink_file(&outside, &link).is_ok() {
        "/escape.txt"
    } else {
        let target = base.join("outside");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("secret.txt"), b"TOPSECRET").unwrap();
        let made = std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(base.join("dist").join("escape-dir"))
            .arg(&target)
            .output()
            .unwrap();
        assert!(made.status.success(), "junction fixture failed: {made:?}");
        "/escape-dir/secret.txt"
    };
    let (h, _) = start_with(
        fixture(),
        Options {
            static_dir: Some(base.join("dist")),
            ..Options::default()
        },
    );
    // Control: a real file under the root is still served.
    let js = exchange(
        h.addr(),
        &format!("GET /assets/app.js HTTP/1.1\r\n{}", host(&h)),
        b"",
    );
    assert_eq!(js.status, 200);
    assert_eq!(js.body, b"console.log(1)");
    // The link resolves outside the root, so it is refused and never leaks.
    let r = exchange(
        h.addr(),
        &format!("GET {escape} HTTP/1.1\r\n{}", host(&h)),
        b"",
    );
    assert_eq!(r.status, 404);
    assert_eq!(r.json()["error"], "not found");
    assert!(
        !String::from_utf8_lossy(&r.body).contains("TOPSECRET"),
        "link leaked: {:?}",
        String::from_utf8_lossy(&r.body)
    );
    drop(h);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn session_token_is_64_lowercase_hex_and_unique_per_handle() {
    let (h1, _) = start();
    let (h2, _) = start();
    for token in [h1.token(), h2.token()] {
        assert_eq!(token.len(), 64, "token {token}");
        assert_eq!(
            token
                .bytes()
                .filter(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
                .count(),
            64,
            "token {token} is not 64 lowercase hex"
        );
    }
    assert_ne!(h1.token(), h2.token(), "two Handles shared a token");
    // Boundary: a non-hex byte is never accepted.
    let mut bad = h1.token().to_string().into_bytes();
    bad[0] = b'G';
    let bad = String::from_utf8(bad).unwrap();
    let head = call_head(&h1, HELLO.len(), "").replace(h1.token(), &bad);
    assert_eq!(
        exchange(h1.addr(), &head, HELLO.as_bytes()).status,
        403,
        "non-hex token accepted"
    );
    // Boundary: a truncated token is never accepted.
    let short = h1.token()[..32].to_string();
    let head = call_head(&h1, HELLO.len(), "").replace(h1.token(), &short);
    assert_eq!(
        exchange(h1.addr(), &head, HELLO.as_bytes()).status,
        403,
        "truncated token accepted"
    );
}

#[test]
fn handle_url_carries_token_in_fragment_never_query() {
    let (h, _) = start();
    let url = h.url();
    assert_eq!(url, format!("http://{}/#token={}", h.addr(), h.token()));
    assert_eq!(url.split('?').count(), 1, "query present in {url}");
    assert_eq!(
        url.split('#').nth(1).unwrap(),
        format!("token={}", h.token())
    );
    assert!(!url.contains('?'), "{url}");
    // Negative: a token in the query without the header never authorises.
    let with_query = format!(
        "GET /api/v3/call?token={} HTTP/1.1\r\n{}",
        h.token(),
        host(&h)
    );
    assert_eq!(exchange(h.addr(), &with_query, b"").status, 403);
    // Negative: no header at all is refused even though the fragment never reaches the server.
    let bare = format!("GET /api/v3/call HTTP/1.1\r\n{}", host(&h));
    assert_eq!(exchange(h.addr(), &bare, b"").status, 403);
}

#[test]
fn corp_and_coop_are_same_origin_on_every_response() {
    let dir = static_tree("corp-coop");
    let (h, _) = start_with(
        fixture(),
        Options {
            static_dir: Some(dir.join("dist")),
            ..Options::default()
        },
    );
    let ok = call(&h, HELLO);
    assert_eq!(ok.status, 200);
    let malformed = call(&h, "{");
    assert_eq!(malformed.status, 200);
    let bad_token = exchange(
        h.addr(),
        &call_head(&h, HELLO.len(), "").replace(h.token(), &"0".repeat(64)),
        HELLO.as_bytes(),
    );
    assert_eq!(bad_token.status, 403);
    let bad_type = exchange(
        h.addr(),
        &call_head(&h, HELLO.len(), "").replace("application/json", "text/plain"),
        HELLO.as_bytes(),
    );
    assert_eq!(bad_type.status, 415);
    let unknown = exchange(
        h.addr(),
        &format!(
            "GET /api/v3/other HTTP/1.1\r\n{}\r\nX-Loomward-Token: {}",
            host(&h),
            h.token()
        ),
        b"",
    );
    assert_eq!(unknown.status, 404);
    let wrong_method = exchange(
        h.addr(),
        &format!(
            "GET /api/v3/call HTTP/1.1\r\n{}\r\nX-Loomward-Token: {}",
            host(&h),
            h.token()
        ),
        b"",
    );
    assert_eq!(wrong_method.status, 405);
    let page = exchange(h.addr(), &format!("GET / HTTP/1.1\r\n{}", host(&h)), b"");
    assert_eq!(page.status, 200);
    let missing = exchange(
        h.addr(),
        &format!("GET /nope HTTP/1.1\r\n{}", host(&h)),
        b"",
    );
    assert_eq!(missing.status, 404);
    for (name, r) in [
        ("ok", &ok),
        ("malformed", &malformed),
        ("bad_token", &bad_token),
        ("bad_type", &bad_type),
        ("unknown", &unknown),
        ("wrong_method", &wrong_method),
        ("page", &page),
        ("missing", &missing),
    ] {
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
    // The streaming response carries the same pair.
    let mut s = connect(h.addr());
    write!(
        s,
        "GET /api/v3/events HTTP/1.1\r\n{}\r\nX-Loomward-Token: {}\r\nAccept: text/event-stream\r\n\r\n",
        host(&h),
        h.token()
    )
    .unwrap();
    let mut reader = BufReader::new(s);
    let mut head = String::new();
    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line).unwrap();
        assert_ne!(
            bytes, 0,
            "EOF before SSE response headers completed: {head}"
        );
        head.push_str(&line);
        if line == "\r\n" {
            break;
        }
    }
    let lower = head.to_ascii_lowercase();
    assert!(
        lower.contains("cross-origin-resource-policy: same-origin"),
        "{head}"
    );
    assert!(
        lower.contains("cross-origin-opener-policy: same-origin"),
        "{head}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn duplicated_last_event_id_resyncs_as_epoch_changed() {
    let service = fixture().with_stream_limits(Duration::from_secs(15), 1024);
    let (h, svc) = start_with(service, Options::default());
    svc.publish((1..=3).map(health_warning)).unwrap();
    let epoch = svc.epoch().to_string();
    // Control: a single header resumes within the buffer.
    let mut sse = open_events(&h, &format!("\r\nLast-Event-ID: {epoch}.1"))
        .ok()
        .unwrap();
    assert_eq!(sse.next().event, "stream.hello");
    assert_eq!(sse.next().id, format!("{epoch}.2"));
    assert_eq!(sse.next().id, format!("{epoch}.3"));
    drop(sse);
    // Duplicated identical headers resync instead of resuming.
    let mut sse = open_events(
        &h,
        &format!("\r\nLast-Event-ID: {epoch}.1\r\nLast-Event-ID: {epoch}.1"),
    )
    .ok()
    .unwrap();
    assert_eq!(sse.next().event, "stream.hello");
    let lagged = sse.next();
    assert_eq!(lagged.event, "stream.lagged");
    assert_eq!(lagged.data["data"]["reason"], "epoch_changed");
    drop(sse);
    // Boundary: duplicated differing headers also resync.
    let mut sse = open_events(
        &h,
        &format!("\r\nLast-Event-ID: {epoch}.1\r\nLast-Event-ID: {epoch}.2"),
    )
    .ok()
    .unwrap();
    assert_eq!(sse.next().event, "stream.hello");
    assert_eq!(sse.next().data["data"]["reason"], "epoch_changed");
    drop(sse);
}

#[test]
fn early_large_body_keeps_reading_before_preserving_the_auth_reply() {
    let (h, _) = start_with(
        fixture(),
        Options {
            read_timeout: Duration::from_secs(2),
            ..Default::default()
        },
    );
    let head =
        call_head(&h, loomward_http::DRAIN_LIMIT * 2, "").replace(h.token(), &"0".repeat(64));
    let mut tcp = connect(h.addr());
    tcp.write_all(format!("{head}\r\nConnection: close\r\n\r\n").as_bytes())
        .unwrap();
    tcp.write_all(&[b'x'; 4096]).unwrap();
    tcp.set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let error = tcp
        .read(&mut [0; 1])
        .expect_err("large body was rejected before the bounded drain");
    assert!(matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    ));
    tcp.write_all(&vec![b'x'; loomward_http::DRAIN_LIMIT - 4096])
        .unwrap();
    tcp.shutdown(std::net::Shutdown::Write).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut raw = Vec::new();
    tcp.read_to_end(&mut raw).unwrap();
    let response = Response::parse(&raw);
    assert_eq!(response.status, 403);
    assert_eq!(
        response.json()["error"],
        "a valid local session token is required"
    );
}

#[test]
fn fixture_publish_invalid_batch_leaves_sequence_and_buffer_unchanged() {
    use loomward_protocol::{RecvOutcome, ViewService};
    let service = fixture();
    let mut stream = service.subscribe(None);
    let RecvOutcome::Event(hello) = stream.recv_timeout(Duration::ZERO) else {
        panic!("hello")
    };
    assert_eq!(hello.seq.get(), 0);
    assert!(service
        .publish([
            health_warning(1),
            (EventName::HealthWarning, Payload::new())
        ])
        .is_err());
    assert!(matches!(
        stream.recv_timeout(Duration::ZERO),
        RecvOutcome::Timeout
    ));
    service.publish([health_warning(2)]).unwrap();
    let RecvOutcome::Event(event) = stream.recv_timeout(Duration::ZERO) else {
        panic!("event")
    };
    assert_eq!(event.seq.get(), 1);
    assert_eq!(event.data["message"], "warning 2");
    stream.close();
}

#[test]
fn declared_length_above_drain_limit_is_refused_after_a_bounded_drain_timeout() {
    let (h, _) = start_with(
        fixture(),
        Options {
            read_timeout: Duration::from_millis(100),
            ..Default::default()
        },
    );
    let over = loomward_http::DRAIN_LIMIT as u64 + 1;
    let huge = loomward_http::DRAIN_LIMIT as u64 * 2;
    for declared in [over, huge] {
        let head = call_head(&h, declared as usize, "");
        let mut tcp = connect(h.addr());
        tcp.write_all(format!("{head}\r\nConnection: close\r\n\r\n").as_bytes())
            .unwrap();
        // No body bytes follow; the bounded drain times out without changing the refusal.
        let t = Instant::now();
        let mut raw = Vec::new();
        tcp.read_to_end(&mut raw).unwrap();
        let r = Response::parse(&raw);
        assert_eq!(r.status, 413, "declared {declared}");
        assert!(
            t.elapsed() >= Duration::from_millis(50),
            "drain was skipped"
        );
        assert_eq!(
            r.json()["error"],
            "the request body exceeds 64 KiB or has no valid Content-Length",
            "declared {declared}"
        );
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "declared {declared} waited {:?} for a body that was never sent",
            t.elapsed()
        );
    }
    // Boundary: valid framing still succeeds.
    assert_eq!(call(&h, HELLO).status, 200);
    // A huge declaration with a bad token preserves the auth refusal after the drain timeout.
    let bad = call_head(&h, huge as usize, "").replace(h.token(), &"0".repeat(64));
    let mut tcp = connect(h.addr());
    tcp.write_all(format!("{bad}\r\nConnection: close\r\n\r\n").as_bytes())
        .unwrap();
    let mut raw = Vec::new();
    tcp.read_to_end(&mut raw).unwrap();
    assert_eq!(Response::parse(&raw).status, 403);
}

#[test]
fn api_errors_carry_json_content_type_and_connection_close() {
    let (h, _) = start();
    // Without asking for close, an error still closes: the close comes from the
    // handler, not from the request.
    let head_no_close = format!(
        "POST /api/v3/call HTTP/1.1\r\n{}\r\nX-Loomward-Token: {}\r\nContent-Type: application/json\r\nContent-Length: {}",
        host(&h),
        "0".repeat(64),
        HELLO.len()
    );
    let mut tcp = connect(h.addr());
    tcp.write_all(format!("{head_no_close}\r\n\r\n{HELLO}").as_bytes())
        .unwrap();
    let mut raw = Vec::new();
    tcp.read_to_end(&mut raw).unwrap();
    let r = Response::parse(&raw);
    assert_eq!(r.status, 403);
    assert_eq!(
        r.header("content-type").as_deref(),
        Some("application/json; charset=utf-8")
    );
    assert_eq!(r.header("connection").as_deref(), Some("close"));
    assert_eq!(r.json()["error"], "a valid local session token is required");
    // Wrong media type.
    let r = exchange(
        h.addr(),
        &call_head(&h, HELLO.len(), "").replace("application/json", "text/plain"),
        HELLO.as_bytes(),
    );
    assert_eq!(r.status, 415);
    assert_eq!(
        r.header("content-type").as_deref(),
        Some("application/json; charset=utf-8"),
        "wrong-type"
    );
    assert_eq!(
        r.header("connection").as_deref(),
        Some("close"),
        "wrong-type"
    );
    assert_eq!(r.json()["error"], "use application/json");
    // Oversized body.
    let big = vec![b'x'; 70_000];
    let r = exchange(h.addr(), &call_head(&h, big.len(), ""), &big);
    assert_eq!(r.status, 413);
    assert_eq!(
        r.header("content-type").as_deref(),
        Some("application/json; charset=utf-8"),
        "oversized"
    );
    assert_eq!(
        r.header("connection").as_deref(),
        Some("close"),
        "oversized"
    );
    assert_eq!(
        r.json()["error"],
        "the request body exceeds 64 KiB or has no valid Content-Length"
    );
    // Unknown API path.
    let r = exchange(
        h.addr(),
        &format!(
            "GET /api/v3/other HTTP/1.1\r\n{}\r\nX-Loomward-Token: {}",
            host(&h),
            h.token()
        ),
        b"",
    );
    assert_eq!(r.status, 404);
    assert_eq!(
        r.header("content-type").as_deref(),
        Some("application/json; charset=utf-8"),
        "unknown-api"
    );
    assert_eq!(
        r.header("connection").as_deref(),
        Some("close"),
        "unknown-api"
    );
    assert_eq!(r.json()["error"], "not found");
    // Known API path with the wrong method.
    let r = exchange(
        h.addr(),
        &format!(
            "GET /api/v3/call HTTP/1.1\r\n{}\r\nX-Loomward-Token: {}",
            host(&h),
            h.token()
        ),
        b"",
    );
    assert_eq!(r.status, 405);
    assert_eq!(
        r.header("content-type").as_deref(),
        Some("application/json; charset=utf-8"),
        "wrong-method"
    );
    assert_eq!(
        r.header("connection").as_deref(),
        Some("close"),
        "wrong-method"
    );
    assert_eq!(r.json()["error"], "method not allowed");
    // Control: success is JSON too.
    let ok = call(&h, HELLO);
    assert_eq!(
        ok.header("content-type").as_deref(),
        Some("application/json; charset=utf-8")
    );
    assert_eq!(ok.json()["ok"], true);
}

#[cfg(windows)]
#[test]
fn revoked_duplicates_are_redacted_from_last_event_id_replay_and_cancel_receipts() {
    if loomward_windows::enumerate::running_elevated() {
        return;
    }
    let tmp = tempfile::tempdir_in(loomward_service::paths::test_roots_base()).unwrap();
    let state = tmp.path().join("state");
    let root = tmp.path().join("fixture");
    loomward_service::lab::register(
        &state,
        &root,
        "duplicates-replay",
        "S",
        "sha256:0000000000000000000000000000000000000000000000000000000000000000",
    )
    .unwrap();
    std::fs::write(root.join("equal-a.txt"), b"same").unwrap();
    std::fs::write(root.join("equal-b.txt"), b"same").unwrap();
    let svc = loomward_service::Service::open(loomward_service::Config {
        state_dir: state,
        dataset: DatasetClass::Synthetic,
        allow_personal: false,
        allow_identity_unverified: false,
        grant_roots: vec![root.canonicalize().unwrap()],
    })
    .unwrap();
    let h = serve(Arc::new(svc), Options::default()).unwrap();
    let mut n = 0;
    let mut command = |command: &str, payload: Value| {
        n += 1;
        let reply = call(&h, &json!({"protocol":"loomward/3","request_id":format!("r_privacy_{n}"),"command":command,"payload":payload}).to_string()).json();
        assert_eq!(reply["ok"], true, "{reply}");
        reply["result"].clone()
    };
    let roots = command("roots.list", json!({}));
    let root_id = roots["roots"][0]["root_id"].clone();
    let wait = |command: &mut dyn FnMut(&str, Value) -> Value, job: Value| {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            let current = command("jobs.get", json!({"job_id":job["job"]["job_id"]}));
            if current["job"]["state"] == "completed" {
                return current;
            }
            assert_ne!(current["job"]["state"], "failed", "{current}");
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let scan = command("scan.start", json!({"root_id":root_id,"mode":"full"}));
    wait(&mut command, scan);
    let mut sse = open_events(&h, "").ok().unwrap();
    let hello = sse.next();
    let resume = format!(
        "{}.{}",
        hello.data["epoch"].as_str().unwrap(),
        hello.data["seq"]
    );
    drop(sse);
    let inspection = command(
        "duplicates.inspect",
        json!({"root_id":root_id,"budget":{"max_candidates":5,"max_bytes":"1024","max_ms":5000}}),
    );
    let completed = wait(&mut command, inspection);
    assert_eq!(
        completed["job"]["duplicates"]["entries"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let cancel_body = json!({"protocol":"loomward/3","request_id":"r_cancel_privacy","command":"duplicates.cancel","payload":{"job_id":completed["job"]["job_id"]}}).to_string();
    let receipt = call(&h, &cancel_body).json();
    assert_eq!(receipt["ok"], true);
    assert!(receipt["result"]["job"].get("duplicates").is_none());
    let generic = command("jobs.cancel", json!({"job_id":completed["job"]["job_id"]}));
    assert!(generic["job"].get("duplicates").is_none());
    command(
        "roots.revoke",
        json!({"root_id":root_id,"purge_catalog":false}),
    );
    assert_eq!(call(&h, &cancel_body).json(), receipt);
    let mut sse = open_events(&h, &format!("\r\nLast-Event-ID: {resume}"))
        .ok()
        .unwrap();
    let end = sse.next().data["seq"].as_u64().unwrap();
    let mut states = 0;
    loop {
        let frame = sse.next();
        if frame.event == "job.state" && frame.data["data"]["job"]["kind"] == "duplicates" {
            states += 1;
            assert!(
                frame.data["data"]["job"].get("duplicates").is_none(),
                "{}",
                frame.data
            );
        }
        if frame.data["seq"].as_u64().unwrap() == end {
            break;
        }
    }
    assert!(
        states >= 3,
        "queued, running and terminal snapshots must replay"
    );
}
