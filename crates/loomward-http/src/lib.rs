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
mod static_handle;

#[cfg(test)]
mod forget_delivery_tests;

#[cfg(test)]
mod response_delivery_tests;

#[cfg(windows)]
mod windows_socket;
#[cfg(windows)]
use windows_socket::exclusive_address_use;

use axum::body::{Body, Bytes};
use axum::http::{header, HeaderMap, HeaderValue, Method, Response, StatusCode};
use futures_util::{stream, StreamExt};
use http_body_util::{BodyExt, Limited};
use hyper::body::Incoming;
use hyper_util::rt::{TokioIo, TokioTimer};
use loomward_protocol::limits::MAX_REQUEST_BYTES;
use loomward_protocol::{
    Adapter, CallContext, CloseReason, Command, ErrorBody, ErrorCode, EventDeliveryStamp,
    EventEnvelope, EventStream, PendingResponse, RecvOutcome, RequestEnvelope, ResponseEnvelope,
    StreamEpoch, ViewService,
};
use std::convert::Infallible;
use std::future::Future;
use std::io::{self, Read};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::thread::JoinHandle;
use std::time::Duration;
use subtle::ConstantTimeEq;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{mpsc, watch, Semaphore};

/// The session-token header on every API request, SSE included.
pub const TOKEN_HEADER: &str = "x-loomward-token";
/// Largest body read just to drain it before an early reply (LW-001: Windows resets a socket
/// closed with unread bytes, which can destroy the reply before the client reads it).
pub const DRAIN_LIMIT: usize = 1024 * 1024;
/// Concurrent SSE streams per service; the next one gets HTTP 429 (`semantics.md` section 7).
pub const MAX_STREAMS: usize = 4;
/// Largest static file served.
const MAX_STATIC_BYTES: u64 = 4 * 1024 * 1024;
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
    /// Validated grant roots, kept separate from unauthenticated static files.
    pub grant_roots: Vec<PathBuf>,
    /// State directory, also excluded from the static tree.
    pub state_dir: Option<PathBuf>,
    /// Bound on receiving a request's headers, and separately its body.
    pub read_timeout: Duration,
    /// Maximum time without progress on a pending response write.
    pub write_timeout: Duration,
    /// Maximum lifetime outside an active SSE body; then drain for at most `write_timeout`.
    pub connection_lifetime: Duration,
    /// Maximum time the SSE pump waits for space in its bounded channel.
    pub sse_send_timeout: Duration,
    /// Connections served at once; further ones wait in the listen backlog.
    pub max_connections: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            allow_origins: Vec::new(),
            static_dir: None,
            grant_roots: Vec::new(),
            state_dir: None,
            read_timeout: Duration::from_secs(10),
            write_timeout: Duration::from_secs(30),
            connection_lifetime: Duration::from_secs(60),
            sse_send_timeout: Duration::from_secs(30),
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
    // The overlap gate compares canonical forms and refuses what it cannot resolve: an unresolved
    // spelling used to fall back to a text comparison that misses `..` and relative paths.
    let grants = options
        .grant_roots
        .iter()
        .enumerate()
        .map(|(i, root)| {
            root.canonicalize()
                .map_err(|_| invalid(format!("grant root #{} cannot be resolved", i + 1)))
        })
        .collect::<io::Result<Vec<_>>>()?;
    let state = options
        .state_dir
        .as_deref()
        .map(loomward_service::paths::prospective_dir)
        .transpose()
        .map_err(invalid)?;
    let static_dir = loomward_service::paths::validate_static_dir(
        options.static_dir.as_deref(),
        &grants,
        state.as_deref(),
    )
    .map_err(invalid)?;
    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw).map_err(|e| io::Error::other(e.to_string()))?;
    let token: String = raw.iter().map(|b| format!("{b:02x}")).collect();

    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )?;
    #[cfg(windows)]
    exclusive_address_use(&socket)?;
    socket.bind(&options.bind.into())?;
    socket.listen(128)?;
    let listener: std::net::TcpListener = socket.into();
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
        write_timeout: options.write_timeout,
        connection_lifetime: options.connection_lifetime,
        sse_send_timeout: options.sse_send_timeout,
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
    // Service teardown may wait for native workers; keep the final owner outside Tokio.
    let service = shared.service.clone();
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
            drop(service);
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
    write_timeout: Duration,
    connection_lifetime: Duration,
    sse_send_timeout: Duration,
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
            let lifetime = shared.connection_lifetime;
            let write_timeout = shared.write_timeout;
            let io = WriteDeadline {
                tcp,
                timeout: shared.write_timeout,
                stalled: None,
            };
            let (opened, mut sse) = watch::channel(false);
            let service = hyper::service::service_fn(move |req| {
                let shared = shared.clone();
                let opened = opened.clone();
                async move {
                    let mut res = handle(&shared, req).await;
                    if res
                        .headers()
                        .get(header::CONTENT_TYPE)
                        .is_some_and(|v| v == "text/event-stream")
                    {
                        let _ = opened.send(true);
                        res = res.map(|body| {
                            Body::from_stream(stream::unfold(
                                (body.into_data_stream(), opened),
                                |(mut body, opened)| async move {
                                    match body.next().await {
                                        Some(frame) => Some((frame, (body, opened))),
                                        None => {
                                            let _ = opened.send(false);
                                            None
                                        }
                                    }
                                },
                            ))
                        });
                    }
                    Ok::<_, Infallible>(res)
                }
            });
            let mut builder = hyper::server::conn::http1::Builder::new();
            builder
                .timer(TokioTimer::new())
                .header_read_timeout(timeout);
            let connection = builder.serve_connection(TokioIo::new(io), service);
            tokio::pin!(connection);
            tokio::select! {
                _ = &mut connection => {},
                _ = tokio::time::sleep(lifetime) => {
                    while *sse.borrow() {
                        tokio::select! {
                            _ = &mut connection => return,
                            _ = sse.changed() => {},
                        }
                    }
                    connection.as_mut().graceful_shutdown();
                    let _ = tokio::time::timeout(write_timeout, &mut connection).await;
                }
            }
        });
    }
}

/// Start the clock only when a write stalls; reads and quiet SSE streams have no write deadline.
struct WriteDeadline {
    tcp: tokio::net::TcpStream,
    timeout: Duration,
    stalled: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl WriteDeadline {
    fn progress<T>(
        &mut self,
        cx: &mut Context<'_>,
        result: Poll<io::Result<T>>,
    ) -> Poll<io::Result<T>> {
        if result.is_ready() {
            self.stalled = None;
            return result;
        }
        let timer = self
            .stalled
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(self.timeout)));
        if timer.as_mut().poll(cx).is_ready() {
            Poll::Ready(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "response write stalled",
            )))
        } else {
            Poll::Pending
        }
    }
}

impl AsyncRead for WriteDeadline {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.tcp).poll_read(cx, buf)
    }
}

impl AsyncWrite for WriteDeadline {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.tcp).poll_write(cx, buf);
        self.progress(cx, result)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // TcpStream flush is a no-op; it must not reset a stalled write's clock.
        Pin::new(&mut self.tcp).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.tcp).poll_shutdown(cx)
    }
}

async fn handle(s: &Shared, req: hyper::Request<Incoming>) -> Response<Body> {
    let api = req.uri().path().starts_with("/api/");
    let mut res = route(s, req.map(Body::new)).await;
    security_headers(res.headers_mut(), api);
    res
}

/// What a body turned out to be after the bounded drain.
enum Drained {
    Complete(Bytes),
    /// Longer than the drain limit; the remainder is left unread.
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

async fn drain(body: Body, _headers: &HeaderMap, timeout: Duration) -> Drained {
    match tokio::time::timeout(timeout, Limited::new(body, DRAIN_LIMIT).collect()).await {
        Ok(Ok(collected)) => Drained::Complete(collected.to_bytes()),
        Ok(Err(e)) if e.is::<http_body_util::LengthLimitError>() => Drained::TooLarge,
        _ => Drained::Failed,
    }
}

async fn early_reply(
    s: &Shared,
    body: Body,
    headers: &HeaderMap,
    res: Response<Body>,
) -> Response<Body> {
    // LW-001: a bounded drain preserves early replies on Windows; it cannot override the gate.
    let _ = drain(body, headers, s.read_timeout).await;
    res
}

async fn route(s: &Shared, req: hyper::Request<Body>) -> Response<Body> {
    let (parts, body) = req.into_parts();
    if !host_ok(s, &parts) {
        return early_reply(
            s,
            body,
            &parts.headers,
            refuse(StatusCode::FORBIDDEN, "unexpected Host"),
        )
        .await;
    }
    let cross_origin = match origin_check(s, &parts.headers) {
        Ok(cross) => cross,
        Err(()) => {
            return early_reply(
                s,
                body,
                &parts.headers,
                refuse(StatusCode::FORBIDDEN, "cross-origin requests are forbidden"),
            )
            .await
        }
    };
    let path = parts.uri.path();
    if !path.starts_with("/api/") {
        let res = match parts.method {
            Method::GET | Method::HEAD => static_file(s, path).await,
            _ => refuse(StatusCode::METHOD_NOT_ALLOWED, "method not allowed"),
        };
        return early_reply(s, body, &parts.headers, res).await;
    }
    let known = matches!(path, "/api/v3/call" | "/api/v3/events");
    let early = if parts.method == Method::OPTIONS && known && cross_origin.is_some() {
        Some(preflight())
    } else if !token_ok(s, &parts.headers) {
        Some(refuse(
            StatusCode::FORBIDDEN,
            "a valid local session token is required",
        ))
    } else {
        match (&parts.method, path) {
            (&Method::POST, "/api/v3/call") => call_header_refusal(&parts.headers),
            (&Method::GET, "/api/v3/events") => None,
            _ if known => Some(refuse(StatusCode::METHOD_NOT_ALLOWED, "method not allowed")),
            _ => Some(refuse(StatusCode::NOT_FOUND, "not found")),
        }
    };
    let mut res = if let Some(res) = early {
        early_reply(s, body, &parts.headers, res).await
    } else if path == "/api/v3/events" {
        // A stalled request body must not reserve a subscription or an SSE permit.
        let _ = drain(body, &parts.headers, s.read_timeout).await;
        events(s, &parts.headers).await
    } else {
        let read = match tokio::time::timeout(
            s.read_timeout,
            Limited::new(body, MAX_REQUEST_BYTES).collect(),
        )
        .await
        {
            Ok(Ok(collected)) => Drained::Complete(collected.to_bytes()),
            Ok(Err(e)) if e.is::<http_body_util::LengthLimitError>() => Drained::TooLarge,
            _ => Drained::Failed,
        };
        if matches!(read, Drained::Failed) {
            refuse(
                StatusCode::BAD_REQUEST,
                "the request body could not be read",
            )
        } else {
            call(s, &parts.headers, read).await
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

fn call_header_refusal(headers: &HeaderMap) -> Option<Response<Body>> {
    if headers.contains_key(header::TRANSFER_ENCODING) {
        return Some(refuse(
            StatusCode::BAD_REQUEST,
            "chunked request bodies are not supported",
        ));
    }
    if !content_length(headers).is_some_and(|n| n <= MAX_REQUEST_BYTES as u64) {
        return Some(refuse(
            StatusCode::PAYLOAD_TOO_LARGE,
            "the request body exceeds 64 KiB or has no valid Content-Length",
        ));
    }
    let json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json"));
    if !json {
        return Some(refuse(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "use application/json",
        ));
    }
    None
}

async fn call(s: &Shared, headers: &HeaderMap, read: Drained) -> Response<Body> {
    let bytes = match (content_length(headers), read) {
        (Some(n), Drained::Complete(b)) if n == b.len() as u64 && b.len() <= MAX_REQUEST_BYTES => b,
        _ => {
            return refuse(
                StatusCode::PAYLOAD_TOO_LARGE,
                "the request body exceeds 64 KiB or has no valid Content-Length",
            )
        }
    };
    let service = s.service.clone();
    match tokio::task::spawn_blocking(move || dispatch_pending(&*service, &bytes)).await {
        Ok(pending) => pending_json_response(pending),
        Err(_) => refuse(StatusCode::INTERNAL_SERVER_ERROR, "the service failed"),
    }
}

/// One call through the protocol's ingress (semantics.md section 1): strict parse, protocol,
/// command, payload, then the HTTP adapter's availability, then the service.
pub fn dispatch(service: &dyn ViewService, bytes: &[u8]) -> ResponseEnvelope {
    dispatch_pending(service, bytes).admit()
}

fn pending_json_response(pending: PendingResponse) -> Response<Body> {
    let encoded = pending.serialize();
    // Construction is not admission: this body may sit unpolled across a root forget. The
    // first poll chooses the original bytes or the existing nonretryable privacy error envelope.
    let frame =
        stream::once(async move { Ok::<_, Infallible>(Bytes::from(encoded.admit_async().await)) });
    let mut response = Response::new(Body::from_stream(frame));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    response
}

fn dispatch_pending(service: &dyn ViewService, bytes: &[u8]) -> PendingResponse {
    let request = match RequestEnvelope::from_slice(bytes) {
        Ok(r) => r,
        Err(rejected) => return PendingResponse::unrestricted(rejected.into_response()),
    };
    let offered =
        Command::from_name(&request.command).is_some_and(|c| c.available_on(Adapter::Http));
    if !offered {
        let error = ErrorBody::new(
            ErrorCode::CapabilityUnavailable,
            "this command is not offered over HTTP",
            false,
        );
        return PendingResponse::unrestricted(ResponseEnvelope::error(
            Some(request.request_id),
            error,
            None,
        ));
    }
    service.call_pending(request, &CallContext::http())
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
    events_with_spawn(s, headers, |run| {
        std::thread::Builder::new()
            .name("loomward-sse".into())
            .spawn(run)
    })
    .await
}

async fn events_with_spawn(
    s: &Shared,
    headers: &HeaderMap,
    spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> io::Result<JoinHandle<()>>,
) -> Response<Body> {
    let Ok(permit) = s.streams.clone().try_acquire_owned() else {
        return refuse(StatusCode::TOO_MANY_REQUESTS, "at most four event streams");
    };
    let resume = parse_last_event_id(headers);
    let service = s.service.clone();
    let opened = tokio::task::spawn_blocking(move || {
        let mut stream = service.subscribe(resume);
        let stamp = stream.delivery_policy().stamp();
        let first = stream.recv_timeout(FIRST_EVENT_TIMEOUT);
        (stream, stamp, first)
    })
    .await;
    let Ok((mut stream, stamp, first)) = opened else {
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
    let stream = ClosingStream(stream);
    let (tx, rx) = mpsc::channel::<PendingFrame>(32);
    let _ = tx.try_send(PendingFrame {
        bytes: sse_frame(&first),
        stamp,
    });
    let stop = s.stop.clone();
    let runtime = tokio::runtime::Handle::current();
    let send_timeout = s.sse_send_timeout;
    let spawned = spawn(Box::new(move || {
        let _permit = permit;
        pump(stream, &tx, &stop, &runtime, send_timeout);
    }));
    if spawned.is_err() {
        return refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "the event stream did not open",
        );
    }
    let body = stream::unfold(rx, |mut rx| async move {
        let pending = rx.recv().await?;
        // The final synchronous admission check follows serialization, with no await before the
        // handoff. A retired queue closes here; the browser reconnects from its last applied ID
        // and reads the now-redacted replay. Dropping rx also releases a blocked extra send.
        if !pending.stamp.admit() {
            return None;
        }
        Some((Ok::<_, Infallible>(pending.bytes), rx))
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
struct ClosingStream(Box<dyn EventStream>);

#[derive(Debug)]
struct PendingFrame {
    bytes: Bytes,
    stamp: EventDeliveryStamp,
}
impl Drop for ClosingStream {
    fn drop(&mut self) {
        self.0.close();
    }
}

fn pump(
    mut stream: ClosingStream,
    tx: &mpsc::Sender<PendingFrame>,
    stop: &AtomicBool,
    runtime: &tokio::runtime::Handle,
    timeout: Duration,
) {
    let delivery = stream.0.delivery_policy();
    while !stop.load(Ordering::SeqCst) && !tx.is_closed() {
        let stamp = delivery.stamp();
        match stream.0.recv_timeout(PUMP_POLL) {
            RecvOutcome::Event(e) => {
                let pending = PendingFrame {
                    bytes: sse_frame(&e),
                    stamp,
                };
                if runtime.block_on(tx.send_timeout(pending, timeout)).is_err() {
                    break;
                }
            }
            RecvOutcome::Timeout => {}
            RecvOutcome::Closed(_) => break,
        }
    }
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
    read_static_file(root, std::fs::File::open(root.join(rel)).ok()?)
}

fn read_static_file(root: &Path, file: std::fs::File) -> Option<(Vec<u8>, &'static str)> {
    let path = static_handle::path(&file)?;
    // Check the object we read, even if a link or junction changed while opening it.
    if !path.starts_with(root) {
        return None;
    }
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_STATIC_BYTES {
        return None;
    }
    Some((read_static_bytes(file)?, mime(&path)))
}

fn read_static_bytes(file: std::fs::File) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(MAX_STATIC_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= MAX_STATIC_BYTES).then_some(bytes)
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
    fn sse_body_drain_does_not_hold_a_stream_permit() {
        let shared = Shared {
            service: Arc::new(
                fixture::FixtureService::new(loomward_protocol::DatasetClass::Synthetic).unwrap(),
            ),
            host: "127.0.0.1:12345".into(),
            self_origin: "http://127.0.0.1:12345".into(),
            allow_origins: vec![],
            token: "0".repeat(64),
            static_dir: None,
            read_timeout: Duration::from_secs(60),
            write_timeout: Duration::from_secs(1),
            connection_lifetime: Duration::from_secs(60),
            sse_send_timeout: Duration::from_secs(1),
            streams: Arc::new(Semaphore::new(MAX_STREAMS)),
            stop: Arc::new(AtomicBool::new(false)),
        };
        let draining = Arc::new(AtomicBool::new(false));
        let polled = draining.clone();
        let body = Body::from_stream(stream::poll_fn(move |_| {
            polled.store(true, Ordering::SeqCst);
            Poll::<Option<Result<Bytes, Infallible>>>::Pending
        }));
        let request = hyper::Request::builder()
            .method(Method::GET)
            .uri("/api/v3/events")
            .header(header::HOST, &shared.host)
            .header(TOKEN_HEADER, &shared.token)
            .header(header::CONTENT_LENGTH, DRAIN_LIMIT * 2)
            .body(body)
            .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut response = Box::pin(route(&shared, request));
            futures_util::future::poll_fn(|cx| {
                assert!(response.as_mut().poll(cx).is_pending());
                if draining.load(Ordering::SeqCst) {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
            let available = shared.streams.available_permits();
            shared.stop.store(true, Ordering::SeqCst);
            drop(response);
            assert_eq!(
                available, MAX_STREAMS,
                "a body not yet drained must not reserve an SSE stream"
            );
        });
    }

    #[test]
    fn early_large_body_drains_to_the_byte_bound() {
        let read = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = read.clone();
        let body = Body::from_stream(stream::repeat_with(move || {
            count.fetch_add(1, Ordering::SeqCst);
            Ok::<_, Infallible>(Bytes::from(vec![0; DRAIN_LIMIT / 4]))
        }));
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_LENGTH,
            (DRAIN_LIMIT * 2).to_string().parse().unwrap(),
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        assert!(matches!(
            runtime.block_on(drain(body, &headers, Duration::from_secs(1))),
            Drained::TooLarge
        ));
        assert_eq!(read.load(Ordering::SeqCst), 5);
    }

    #[test]
    fn shutdown_drops_the_service_outside_the_runtime() {
        struct DropProbe(std::sync::mpsc::Sender<bool>);
        impl ViewService for DropProbe {
            fn call(&self, _: RequestEnvelope, _: &CallContext) -> ResponseEnvelope {
                unreachable!()
            }
            fn subscribe(&self, _: Option<(String, u64)>) -> Box<dyn EventStream> {
                unreachable!()
            }
        }
        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.0
                    .send(tokio::runtime::Handle::try_current().is_ok())
                    .unwrap();
            }
        }
        let (dropped, received) = std::sync::mpsc::channel();
        serve(Arc::new(DropProbe(dropped)), Options::default())
            .unwrap()
            .shutdown();
        assert!(
            !received.recv_timeout(Duration::from_secs(5)).unwrap(),
            "a blocking service destructor ran inside the Tokio runtime"
        );
    }

    #[test]
    fn sse_spawn_failure_explicitly_closes_the_subscription() {
        struct Service(Arc<std::sync::atomic::AtomicUsize>);
        struct Source {
            fixture: Box<dyn EventStream>,
            closed: Arc<std::sync::atomic::AtomicUsize>,
        }
        impl EventStream for Source {
            fn epoch(&self) -> &str {
                self.fixture.epoch()
            }
            fn recv_timeout(&mut self, timeout: Duration) -> RecvOutcome {
                self.fixture.recv_timeout(timeout)
            }
            fn close(&mut self) {
                self.closed.fetch_add(1, Ordering::SeqCst);
                self.fixture.close();
            }
        }
        impl ViewService for Service {
            fn call(&self, _: RequestEnvelope, _: &CallContext) -> ResponseEnvelope {
                unreachable!()
            }
            fn subscribe(&self, resume: Option<(String, u64)>) -> Box<dyn EventStream> {
                Box::new(Source {
                    fixture: fixture::FixtureService::new(
                        loomward_protocol::DatasetClass::Synthetic,
                    )
                    .unwrap()
                    .subscribe(resume),
                    closed: self.0.clone(),
                })
            }
        }
        let closed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let shared = Shared {
            service: Arc::new(Service(closed.clone())),
            host: String::new(),
            self_origin: String::new(),
            allow_origins: vec![],
            token: String::new(),
            static_dir: None,
            read_timeout: Duration::from_secs(1),
            write_timeout: Duration::from_secs(1),
            connection_lifetime: Duration::from_secs(1),
            sse_send_timeout: Duration::from_secs(1),
            streams: Arc::new(Semaphore::new(MAX_STREAMS)),
            stop: Arc::new(AtomicBool::new(false)),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let response = runtime.block_on(events_with_spawn(&shared, &HeaderMap::new(), |_| {
            Err(io::Error::other("synthetic spawn failure"))
        }));
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(closed.load(Ordering::SeqCst), 1);
        assert_eq!(shared.streams.available_permits(), MAX_STREAMS);
    }

    #[test]
    fn stalled_sse_channel_closes_stream_and_releases_permit() {
        use loomward_protocol::DatasetClass;
        let service = Arc::new(
            fixture::FixtureService::new(DatasetClass::Synthetic)
                .unwrap()
                .with_stream_limits(Duration::from_millis(10), 1024),
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let handle = runtime.handle().clone();
        let slots = Arc::new(Semaphore::new(MAX_STREAMS));
        let permit = slots.clone().try_acquire_owned().unwrap();
        let source = service.subscribe(None);
        let (tx, rx) = mpsc::channel(32);
        let worker = std::thread::spawn(move || {
            let _permit = permit;
            pump(
                ClosingStream(source),
                &tx,
                &AtomicBool::new(false),
                &handle,
                Duration::from_millis(500),
            );
        });
        // Retain the receiver but never read: 32 heartbeats fill the channel, then send stalls.
        let closed = runtime.block_on(async {
            let end = tokio::time::Instant::now() + Duration::from_secs(8);
            while service.open_streams() != 0 && tokio::time::Instant::now() < end {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            service.open_streams() == 0
        });
        drop(rx); // Also lets the unfixed blocking_send exit after a failed assertion's deadline.
        worker.join().unwrap();
        assert!(closed, "stalled SSE pump never closed its source");
        assert_eq!(slots.available_permits(), MAX_STREAMS);
    }

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

    #[test]
    fn followup_static_reads_the_opened_object_after_path_replacement() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let path = root.join("asset.txt");
        std::fs::write(&path, b"original").unwrap();
        let file = std::fs::File::open(&path).unwrap();
        std::fs::rename(&path, root.join("renamed.txt")).unwrap();
        std::fs::write(&path, b"replacement").unwrap();
        assert_eq!(read_static_file(&root, file).unwrap().0, b"original");
        let outside = tempfile::tempdir().unwrap();
        let file = std::fs::File::open(&path).unwrap();
        std::fs::rename(&path, outside.path().join("outside.txt")).unwrap();
        assert!(read_static_file(&root, file).is_none());
    }

    #[test]
    fn followup_static_read_is_bounded_when_a_file_grows_after_metadata() {
        use std::io::Seek;
        let mut file = tempfile::tempfile().unwrap();
        file.set_len(MAX_STATIC_BYTES).unwrap();
        assert_eq!(file.metadata().unwrap().len(), MAX_STATIC_BYTES);
        // Model growth after the metadata check but before the read.
        file.set_len(MAX_STATIC_BYTES + 100).unwrap();
        let reader = file.try_clone().unwrap();
        assert!(read_static_bytes(reader).is_none());
        assert_eq!(file.stream_position().unwrap(), MAX_STATIC_BYTES + 1);
        file.rewind().unwrap();
        file.set_len(MAX_STATIC_BYTES).unwrap();
        assert_eq!(
            read_static_bytes(file).unwrap().len() as u64,
            MAX_STATIC_BYTES
        );
    }

    #[test]
    fn a_duplicated_content_length_is_never_read_as_a_length() {
        // The wire cannot reach this branch: hyper coalesces identical duplicates and refuses
        // conflicting ones first. The adapter's own rule is pinned here, at its only seam.
        let mut one = HeaderMap::new();
        one.insert(header::CONTENT_LENGTH, HeaderValue::from_static("5"));
        assert_eq!(content_length(&one), Some(5));
        let mut same = one.clone();
        same.append(header::CONTENT_LENGTH, HeaderValue::from_static("5"));
        assert_eq!(content_length(&same), None);
        let mut differ = one.clone();
        differ.append(header::CONTENT_LENGTH, HeaderValue::from_static("6"));
        assert_eq!(content_length(&differ), None);
    }
}
