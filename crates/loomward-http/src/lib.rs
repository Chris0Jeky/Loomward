//! `loomward-serve`: the loopback HTTP + SSE adapter between a browser and the [`ViewService`]
//! (docs/41 sections 5.1 and 12, ADR-V3-03, ADR-V3-12).
//!
//! This crate is a security boundary. It binds `127.0.0.1` only and exposes exactly
//! `POST /api/v3/call`, `GET /api/v3/events` and, with `--static`, the files of one directory.
//! Every request passes the same gate: exact `Host`, `Origin` absent or allowed, and for the API a
//! 256-bit session token compared in constant time. Calls are bounded (64 KiB, a valid
//! `Content-Length`, `application/json`, no chunking) and enter the service only through
//! [`RequestEnvelope::from_slice`]. Nothing here accepts a path, runs a command outside the
//! protocol or has an effect (AGENTS.md invariant 1).
//!
//! tokio is confined to this crate (ADR-V3-02): [`serve`] runs its own runtime on a background
//! thread and returns a synchronous [`Handle`].

pub mod fixture;
pub mod grants;

use axum::body::{Body, Bytes};
use axum::http::{header, HeaderMap, HeaderValue, Method, Response, StatusCode};
use futures_util::stream;
use http_body_util::{BodyExt, Limited};
use hyper::body::Incoming;
use hyper_util::rt::{TokioIo, TokioTimer};
use loomward_protocol::limits::MAX_REQUEST_BYTES;
use loomward_protocol::{
    Adapter, CallContext, CloseReason, Command, ErrorBody, ErrorCode, EventEnvelope, EventStream,
    RecvOutcome, RequestEnvelope, ResponseEnvelope, StreamEpoch, ViewService,
};
use std::convert::Infallible;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;
use subtle::ConstantTimeEq;
use tokio::sync::{mpsc, watch, Semaphore};

/// The session-token header on every API request, SSE included.
pub const TOKEN_HEADER: &str = "x-loomward-token";
/// Largest body read just to drain it before an early reply (LW-001: Windows resets a socket
/// closed with unread bytes, which can destroy the reply before the client reads it).
pub const DRAIN_LIMIT: usize = 1024 * 1024;
/// Concurrent SSE streams per service; the next one gets HTTP 429 (`semantics.md` section 7).
pub const MAX_STREAMS: usize = 4;
/// Largest static file served.
const MAX_STATIC_BYTES: u64 = 16 * 1024 * 1024;
/// How long the first event (`stream.hello`) may take before the stream is refused.
const FIRST_EVENT_TIMEOUT: Duration = Duration::from_secs(5);
/// How often an idle pump rechecks for a gone client or a shutdown.
const PUMP_POLL: Duration = Duration::from_millis(250);

const API_CSP: &str =
    "default-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";
/// The built app's CSP (`app/vite.config.ts`) narrowed to this origin, plus `frame-ancestors`.
const STATIC_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; \
    img-src 'self' data:; font-src 'self'; connect-src 'self'; base-uri 'none'; \
    form-action 'none'; frame-ancestors 'none'";

/// How to run the adapter. `Options::default()` binds `127.0.0.1:0` with no static files.
#[derive(Debug, Clone)]
pub struct Options {
    /// Must be `127.0.0.1`; any other address is refused.
    pub bind: SocketAddr,
    /// Extra origins allowed to call the API cross-origin (for example Vite dev at
    /// `http://localhost:5173`). Loopback `http` origins only.
    pub allow_origins: Vec<String>,
    /// Directory served for non-API `GET`s (the built `app/dist`).
    pub static_dir: Option<PathBuf>,
    /// Bound on receiving a request's headers, and separately its body.
    pub read_timeout: Duration,
    /// Connections served at once; further ones wait in the listen backlog.
    pub max_connections: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            allow_origins: Vec::new(),
            static_dir: None,
            read_timeout: Duration::from_secs(10),
            max_connections: 16,
        }
    }
}

/// A running adapter. Dropping it shuts the adapter down.
pub struct Handle {
    addr: SocketAddr,
    token: String,
    stop: Arc<AtomicBool>,
    stop_tx: Option<watch::Sender<bool>>,
    thread: Option<JoinHandle<()>>,
}

impl Handle {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The session token: 32 random bytes, hex encoded.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The URL to open, token in the fragment so it never reaches a server log or `Referer`.
    pub fn url(&self) -> String {
        format!("http://{}/#token={}", self.addr, self.token)
    }

    /// Blocks until the adapter stops.
    pub fn wait(mut self) {
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    pub fn shutdown(self) {
        drop(self);
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(true);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

/// Whether `origin` is exactly `http://<loopback host>:<port>`.
fn is_loopback_origin(origin: &str) -> bool {
    let Some(rest) = origin.strip_prefix("http://") else {
        return false;
    };
    let Some((host, port)) = rest.rsplit_once(':') else {
        return false;
    };
    matches!(host, "127.0.0.1" | "localhost" | "[::1]")
        && !port.is_empty()
        && port.len() <= 5
        && port.bytes().all(|b| b.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|p| p != 0)
}

/// Binds and starts the adapter. Refuses any address but `127.0.0.1`, non-loopback allowed
/// origins and a static directory that is not a directory.
pub fn serve(service: Arc<dyn ViewService>, options: Options) -> io::Result<Handle> {
    if options.bind.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) {
        return Err(invalid(format!(
            "loomward-serve binds 127.0.0.1 only, not {}",
            options.bind.ip()
        )));
    }
    if let Some(bad) = options
        .allow_origins
        .iter()
        .find(|o| !is_loopback_origin(o))
    {
        return Err(invalid(format!(
            "--allow-origin takes a loopback http origin such as http://localhost:5173, not {bad}"
        )));
    }
    let static_dir = match &options.static_dir {
        Some(dir) => {
            let dir = dir.canonicalize()?;
            if !dir.is_dir() {
                return Err(invalid("--static must name a directory".into()));
            }
            Some(dir)
        }
        None => None,
    };
    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw).map_err(|e| io::Error::other(e.to_string()))?;
    let token: String = raw.iter().map(|b| format!("{b:02x}")).collect();

    let listener = std::net::TcpListener::bind(options.bind)?;
    listener.set_nonblocking(true)?;
    let addr = listener.local_addr()?;
    let stop = Arc::new(AtomicBool::new(false));
    let shared = Arc::new(Shared {
        service,
        host: addr.to_string(),
        self_origin: format!("http://{addr}"),
        allow_origins: options.allow_origins,
        token: token.clone(),
        static_dir,
        read_timeout: options.read_timeout,
        streams: Arc::new(Semaphore::new(MAX_STREAMS)),
        stop: stop.clone(),
    });
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("loomward-http")
        .build()?;
    let (stop_tx, stop_rx) = watch::channel(false);
    let connections = Arc::new(Semaphore::new(options.max_connections.max(1)));
    let thread = std::thread::Builder::new()
        .name("loomward-serve".into())
        .spawn(move || {
            runtime.block_on(async move {
                match tokio::net::TcpListener::from_std(listener) {
                    Ok(listener) => accept_loop(listener, shared, connections, stop_rx).await,
                    Err(e) => eprintln!("loomward-serve: listener failed: {e}"),
                }
            });
            runtime.shutdown_timeout(Duration::from_secs(2));
        })?;
    Ok(Handle {
        addr,
        token,
        stop,
        stop_tx: Some(stop_tx),
        thread: Some(thread),
    })
}

struct Shared {
    service: Arc<dyn ViewService>,
    /// `127.0.0.1:<port>`, the only acceptable `Host`.
    host: String,
    self_origin: String,
    allow_origins: Vec<String>,
    token: String,
    static_dir: Option<PathBuf>,
    read_timeout: Duration,
    streams: Arc<Semaphore>,
    stop: Arc<AtomicBool>,
}

async fn accept_loop(
    listener: tokio::net::TcpListener,
    shared: Arc<Shared>,
    connections: Arc<Semaphore>,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        // Take a slot before accepting, so excess connections wait in the backlog instead of
        // being accepted and reset.
        let permit = tokio::select! {
            p = connections.clone().acquire_owned() => match p { Ok(p) => p, Err(_) => return },
            _ = stop.changed() => return,
        };
        let tcp = tokio::select! {
            r = listener.accept() => match r {
                Ok((tcp, _)) => tcp,
                Err(_) => { tokio::time::sleep(Duration::from_millis(50)).await; continue; }
            },
            _ = stop.changed() => return,
        };
        let shared = shared.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let timeout = shared.read_timeout;
            let service = hyper::service::service_fn(move |req| {
                let shared = shared.clone();
                async move { Ok::<_, Infallible>(handle(&shared, req).await) }
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(timeout)
                .serve_connection(TokioIo::new(tcp), service)
                .await;
        });
    }
}

async fn handle(s: &Shared, req: hyper::Request<Incoming>) -> Response<Body> {
    let api = req.uri().path().starts_with("/api/");
    let mut res = route(s, req).await;
    security_headers(res.headers_mut(), api);
    res
}

/// What a body turned out to be after the bounded drain.
enum Drained {
    Complete(Bytes),
    /// Longer than the drain limit; left unread.
    TooLarge,
    /// Stalled past the read timeout, or the connection failed.
    Failed,
}

fn content_length(headers: &HeaderMap) -> Option<u64> {
    let mut values = headers.get_all(header::CONTENT_LENGTH).iter();
    let v = values.next()?;
    if values.next().is_some() {
        return None;
    }
    v.to_str().ok()?.parse().ok()
}

async fn drain(body: Incoming, headers: &HeaderMap, timeout: Duration) -> Drained {
    if content_length(headers).is_some_and(|n| n > DRAIN_LIMIT as u64) {
        return Drained::TooLarge;
    }
    match tokio::time::timeout(timeout, Limited::new(body, DRAIN_LIMIT).collect()).await {
        Ok(Ok(collected)) => Drained::Complete(collected.to_bytes()),
        Ok(Err(e)) if e.is::<http_body_util::LengthLimitError>() => Drained::TooLarge,
        _ => Drained::Failed,
    }
}

async fn route(s: &Shared, req: hyper::Request<Incoming>) -> Response<Body> {
    let (parts, body) = req.into_parts();
    // LW-001: read or drain a bounded body before any reply, early errors included.
    let drained = drain(body, &parts.headers, s.read_timeout).await;
    if matches!(drained, Drained::Failed) {
        return refuse(
            StatusCode::BAD_REQUEST,
            "the request body could not be read",
        );
    }
    if !host_ok(s, &parts) {
        return refuse(StatusCode::FORBIDDEN, "unexpected Host");
    }
    let cross_origin = match origin_check(s, &parts.headers) {
        Ok(cross) => cross,
        Err(()) => return refuse(StatusCode::FORBIDDEN, "cross-origin requests are forbidden"),
    };
    let path = parts.uri.path();
    if !path.starts_with("/api/") {
        return match parts.method {
            Method::GET | Method::HEAD => static_file(s, path).await,
            _ => refuse(StatusCode::METHOD_NOT_ALLOWED, "method not allowed"),
        };
    }
    let known = matches!(path, "/api/v3/call" | "/api/v3/events");
    let mut res = if parts.method == Method::OPTIONS && known && cross_origin.is_some() {
        preflight()
    } else if !token_ok(s, &parts.headers) {
        refuse(
            StatusCode::FORBIDDEN,
            "a valid local session token is required",
        )
    } else {
        match (&parts.method, path) {
            (&Method::POST, "/api/v3/call") => call(s, &parts.headers, drained).await,
            (&Method::GET, "/api/v3/events") => events(s, &parts.headers).await,
            _ if known => refuse(StatusCode::METHOD_NOT_ALLOWED, "method not allowed"),
            _ => refuse(StatusCode::NOT_FOUND, "not found"),
        }
    };
    if let Some(origin) = cross_origin {
        let h = res.headers_mut();
        h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        h.insert(header::VARY, HeaderValue::from_static("Origin"));
    }
    res
}

/// Exactly one `Host`, equal to `127.0.0.1:<port>` (DNS rebinding), and no foreign authority in
/// an absolute-form target.
fn host_ok(s: &Shared, parts: &axum::http::request::Parts) -> bool {
    let mut hosts = parts.headers.get_all(header::HOST).iter();
    let one = matches!((hosts.next(), hosts.next()), (Some(h), None) if h.as_bytes() == s.host.as_bytes());
    one && parts.uri.authority().is_none_or(|a| a.as_str() == s.host)
}

/// `Ok(None)`: no `Origin` or this origin. `Ok(Some(origin))`: an allowlisted loopback origin,
/// which gets CORS headers. `Err`: anything else, including `null` and a repeated header.
fn origin_check(s: &Shared, headers: &HeaderMap) -> Result<Option<HeaderValue>, ()> {
    let mut origins = headers.get_all(header::ORIGIN).iter();
    match (origins.next(), origins.next()) {
        (None, _) => Ok(None),
        (Some(o), None) if o.as_bytes() == s.self_origin.as_bytes() => Ok(None),
        (Some(o), None) if s.allow_origins.iter().any(|a| a.as_bytes() == o.as_bytes()) => {
            Ok(Some(o.clone()))
        }
        _ => Err(()),
    }
}

/// Exactly one token header, compared in constant time.
fn token_ok(s: &Shared, headers: &HeaderMap) -> bool {
    let mut tokens = headers.get_all(TOKEN_HEADER).iter();
    match (tokens.next(), tokens.next()) {
        (Some(t), None) => bool::from(t.as_bytes().ct_eq(s.token.as_bytes())),
        _ => false,
    }
}

fn preflight() -> Response<Body> {
    let mut res = Response::new(Body::empty());
    *res.status_mut() = StatusCode::NO_CONTENT;
    let h = res.headers_mut();
    h.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST"),
    );
    h.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("content-type, x-loomward-token, last-event-id"),
    );
    h.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("600"),
    );
    res
}

fn json_response(status: StatusCode, body: Vec<u8>) -> Response<Body> {
    let mut res = Response::new(Body::from(body));
    *res.status_mut() = status;
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    res
}

/// A transport fault: not an envelope, and the connection closes after it.
fn refuse(status: StatusCode, message: &str) -> Response<Body> {
    let body = serde_json::to_vec(&serde_json::json!({ "error": message }))
        .expect("a string map always serialises");
    let mut res = json_response(status, body);
    res.headers_mut()
        .insert(header::CONNECTION, HeaderValue::from_static("close"));
    res
}

fn security_headers(h: &mut HeaderMap, api: bool) {
    let csp = if api { API_CSP } else { STATIC_CSP };
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(csp),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    h.insert(
        "cross-origin-opener-policy",
        HeaderValue::from_static("same-origin"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
}

async fn call(s: &Shared, headers: &HeaderMap, drained: Drained) -> Response<Body> {
    if headers.contains_key(header::TRANSFER_ENCODING) {
        return refuse(
            StatusCode::BAD_REQUEST,
            "chunked request bodies are not supported",
        );
    }
    let bytes = match (content_length(headers), drained) {
        (Some(n), Drained::Complete(b))
            if n as usize == b.len() && b.len() <= MAX_REQUEST_BYTES =>
        {
            b
        }
        _ => {
            return refuse(
                StatusCode::PAYLOAD_TOO_LARGE,
                "the request body exceeds 64 KiB or has no valid Content-Length",
            )
        }
    };
    let json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json"));
    if !json {
        return refuse(StatusCode::UNSUPPORTED_MEDIA_TYPE, "use application/json");
    }
    let service = s.service.clone();
    match tokio::task::spawn_blocking(move || dispatch(&*service, &bytes)).await {
        Ok(envelope) => json_response(
            StatusCode::OK,
            serde_json::to_vec(&envelope).expect("a response envelope always serialises"),
        ),
        Err(_) => refuse(StatusCode::INTERNAL_SERVER_ERROR, "the service failed"),
    }
}

/// One call through the protocol's ingress (semantics.md section 1): strict parse, protocol,
/// command, payload, then the HTTP adapter's availability, then the service.
pub fn dispatch(service: &dyn ViewService, bytes: &[u8]) -> ResponseEnvelope {
    let request = match RequestEnvelope::from_slice(bytes) {
        Ok(r) => r,
        Err(rejected) => return rejected.into_response(),
    };
    let offered =
        Command::from_name(&request.command).is_some_and(|c| c.available_on(Adapter::Http));
    if !offered {
        let error = ErrorBody::new(
            ErrorCode::CapabilityUnavailable,
            "this command is not offered over HTTP",
            false,
        );
        return ResponseEnvelope::error(Some(request.request_id), error, None);
    }
    service.call(request, &CallContext::http())
}

/// `Last-Event-ID: <epoch>.<seq>`. A malformed value resumes from an unknown epoch, so the
/// service answers `stream.lagged { reason: "epoch_changed" }` and the client resyncs.
pub fn parse_last_event_id(headers: &HeaderMap) -> Option<(String, u64)> {
    let mut values = headers.get_all("last-event-id").iter();
    let first = values.next()?;
    let parsed = first
        .to_str()
        .ok()
        .filter(|_| values.next().is_none())
        .and_then(|v| v.rsplit_once('.'))
        .and_then(|(epoch, seq)| {
            let seq = seq
                .bytes()
                .all(|b| b.is_ascii_digit())
                .then(|| seq.parse::<u64>().ok())??;
            Some((StreamEpoch::new(epoch).ok()?.into(), seq))
        });
    Some(parsed.unwrap_or_default())
}

/// One SSE frame. JSON from `serde_json` holds no raw newline, so `data:` is one line.
pub fn sse_frame(e: &EventEnvelope) -> Bytes {
    let data = serde_json::to_string(e).expect("an event envelope always serialises");
    Bytes::from(format!(
        "id: {}.{}\nevent: {}\ndata: {}\n\n",
        e.epoch.as_str(),
        e.seq.get(),
        e.event.as_str(),
        data
    ))
}

async fn events(s: &Shared, headers: &HeaderMap) -> Response<Body> {
    let Ok(permit) = s.streams.clone().try_acquire_owned() else {
        return refuse(StatusCode::TOO_MANY_REQUESTS, "at most four event streams");
    };
    let resume = parse_last_event_id(headers);
    let service = s.service.clone();
    let opened = tokio::task::spawn_blocking(move || {
        let mut stream = service.subscribe(resume);
        let first = stream.recv_timeout(FIRST_EVENT_TIMEOUT);
        (stream, first)
    })
    .await;
    let Ok((mut stream, first)) = opened else {
        return refuse(StatusCode::INTERNAL_SERVER_ERROR, "the service failed");
    };
    let first = match first {
        RecvOutcome::Event(e) => e,
        RecvOutcome::Closed(CloseReason::TooManyStreams) => {
            stream.close();
            return refuse(StatusCode::TOO_MANY_REQUESTS, "at most four event streams");
        }
        _ => {
            stream.close();
            return refuse(
                StatusCode::SERVICE_UNAVAILABLE,
                "the event stream did not open",
            );
        }
    };
    let (tx, rx) = mpsc::channel::<Bytes>(32);
    let _ = tx.try_send(sse_frame(&first));
    let stop = s.stop.clone();
    let spawned = std::thread::Builder::new()
        .name("loomward-sse".into())
        .spawn(move || {
            let _permit = permit;
            pump(stream, &tx, &stop);
        });
    if spawned.is_err() {
        return refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "the event stream did not open",
        );
    }
    let body = stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|b| (Ok::<_, Infallible>(b), rx))
    });
    let mut res = Response::new(Body::from_stream(body));
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    res
}

/// Moves events from the service's stream into the response until the client goes, the
/// service closes the stream or the adapter stops. Always releases the subscriber slot.
fn pump(mut stream: Box<dyn EventStream>, tx: &mpsc::Sender<Bytes>, stop: &AtomicBool) {
    while !stop.load(Ordering::SeqCst) && !tx.is_closed() {
        match stream.recv_timeout(PUMP_POLL) {
            RecvOutcome::Event(e) => {
                if tx.blocking_send(sse_frame(&e)).is_err() {
                    break;
                }
            }
            RecvOutcome::Timeout => {}
            RecvOutcome::Closed(_) => break,
        }
    }
    stream.close();
}

/// Maps a URL path to a file under the static root, or `None`. Only plain segments of
/// `[A-Za-z0-9._-]` that do not start or end with a dot pass, so `..`, percent-encoding,
/// backslashes, drive letters, alternate data streams (`:`), 8.3 names (`~`) and empty
/// segments never reach the file system; Windows device names are refused too.
pub fn static_relative(path: &str) -> Option<PathBuf> {
    let rel = path.strip_prefix('/')?;
    let rel = if rel.is_empty() { "index.html" } else { rel };
    let mut out = PathBuf::new();
    for seg in rel.split('/') {
        let plain = !seg.is_empty()
            && !seg.starts_with('.')
            && !seg.ends_with('.')
            && seg
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
        if !plain || is_device_name(seg) {
            return None;
        }
        out.push(seg);
    }
    Some(out)
}

fn is_device_name(seg: &str) -> bool {
    let stem = seg.split('.').next().unwrap_or(seg).to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ((stem.starts_with("COM") || stem.starts_with("LPT"))
        && stem.len() == 4
        && stem.as_bytes()[3].is_ascii_digit())
}

fn mime(file: &Path) -> &'static str {
    match file.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn read_static(root: &Path, rel: &Path) -> Option<(Vec<u8>, &'static str)> {
    let file = root.join(rel).canonicalize().ok()?;
    // Symlinks and junctions inside the root resolve first; the result must still be inside.
    if !file.starts_with(root) {
        return None;
    }
    let meta = std::fs::metadata(&file).ok()?;
    if !meta.is_file() || meta.len() > MAX_STATIC_BYTES {
        return None;
    }
    Some((std::fs::read(&file).ok()?, mime(&file)))
}

async fn static_file(s: &Shared, path: &str) -> Response<Body> {
    let (Some(root), Some(rel)) = (s.static_dir.clone(), static_relative(path)) else {
        return refuse(StatusCode::NOT_FOUND, "not found");
    };
    match tokio::task::spawn_blocking(move || read_static(&root, &rel)).await {
        Ok(Some((bytes, kind))) => {
            let mut res = Response::new(Body::from(bytes));
            res.headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
            res
        }
        _ => refuse(StatusCode::NOT_FOUND, "not found"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_paths_never_escape() {
        assert_eq!(static_relative("/"), Some(PathBuf::from("index.html")));
        assert_eq!(
            static_relative("/assets/index-Ab_1.js"),
            Some(PathBuf::from("assets").join("index-Ab_1.js"))
        );
        for bad in [
            "/..",
            "/../secret",
            "/assets/../../x",
            "/%2e%2e/x",
            "/%2E%2E%2Fx",
            "/..%5cx",
            "/assets\\..\\x",
            "/C:/Windows/win.ini",
            "/C:",
            "/index.html::$DATA",
            "/index.html:stream",
            "/PROGRA~1",
            "/.env",
            "/index.html.",
            "//x",
            "/a//b",
            "/CON",
            "/nul.txt",
            "/com1",
            "relative",
        ] {
            assert_eq!(static_relative(bad), None, "{bad}");
        }
    }

    #[test]
    fn loopback_origins_only() {
        for good in [
            "http://localhost:5173",
            "http://127.0.0.1:8080",
            "http://[::1]:5173",
        ] {
            assert!(is_loopback_origin(good), "{good}");
        }
        for bad in [
            "https://localhost:5173",
            "http://localhost",
            "http://localhost:5173/",
            "http://evil.example:80",
            "http://127.0.0.1.evil:80",
            "http://localhost:0",
            "http://localhost:99999",
            "null",
        ] {
            assert!(!is_loopback_origin(bad), "{bad}");
        }
    }

    #[test]
    fn last_event_id_parses_or_resumes_from_an_unknown_epoch() {
        let mut h = HeaderMap::new();
        assert_eq!(parse_last_event_id(&h), None);
        h.insert("last-event-id", HeaderValue::from_static("e_k3J9x2Qa.42"));
        assert_eq!(parse_last_event_id(&h), Some(("e_k3J9x2Qa".into(), 42)));
        for bad in ["42", "e_k3J9x2Qa.", "e_k3J9x2Qa.+4", "bad epoch.4", "x.4"] {
            h.insert("last-event-id", HeaderValue::from_static(bad));
            assert_eq!(parse_last_event_id(&h), Some((String::new(), 0)), "{bad}");
        }
    }
}
