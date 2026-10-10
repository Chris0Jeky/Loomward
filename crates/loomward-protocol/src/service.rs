//! The service trait every adapter calls, and the event stream behind it (docs/41 section 5.3).

use crate::dto::DisclosureSummary;
use crate::envelope::{RequestEnvelope, ResponseEnvelope};
use crate::event::EventEnvelope;
pub use crate::types::Adapter;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Native UI the Rust side owns. The webview never supplies a path or a confirmation.
pub trait NativeDialogs: Send + Sync {
    /// Rust-side folder picker; the chosen path never crosses IPC.
    fn pick_folder(&self) -> Option<PathBuf>;
    /// Native modal listing recipient, model, fields and every item of the summary. `true` only on
    /// explicit accept.
    fn confirm_disclosure(&self, summary: &DisclosureSummary) -> bool;
}

/// Per-call facts the adapter knows and the payload cannot claim.
#[derive(Clone)]
pub struct CallContext {
    pub adapter: Adapter,
    /// `Some` on the desktop adapter only; HTTP has no native dialogs.
    pub dialogs: Option<Arc<dyn NativeDialogs>>,
}

impl CallContext {
    pub fn http() -> Self {
        Self {
            adapter: Adapter::Http,
            dialogs: None,
        }
    }

    pub fn tauri(dialogs: Arc<dyn NativeDialogs>) -> Self {
        Self {
            adapter: Adapter::Tauri,
            dialogs: Some(dialogs),
        }
    }
}

impl fmt::Debug for CallContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallContext")
            .field("adapter", &self.adapter)
            .field("dialogs", &self.dialogs.as_ref().map(|_| "native"))
            .finish()
    }
}

/// The view-model service. Adapters run `call` on a blocking pool and pump the [`EventStream`]
/// into the SSE response or the Tauri `Channel`.
pub trait ViewService: Send + Sync + 'static {
    /// Synchronous; long work returns a Job. Deadlines per `semantics.md` section 2.
    fn call(&self, request: RequestEnvelope, ctx: &CallContext) -> ResponseEnvelope;
    /// Opens a stream. `resume` is the client's (epoch, last applied seq), if any.
    fn subscribe(&self, resume: Option<(String, u64)>) -> Box<dyn EventStream>;
}

/// Why a stream ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    ServiceShutdown,
    ClosedByAdapter,
    /// More than four concurrent HTTP streams; the adapter answers HTTP 429.
    TooManyStreams,
}

/// One step of an [`EventStream`].
#[derive(Debug, Clone, PartialEq)]
pub enum RecvOutcome {
    Event(EventEnvelope),
    Timeout,
    Closed(CloseReason),
}

/// A subscriber's view of the event stream. The queue behind it is bounded
/// ([`EVENT_QUEUE_CAPACITY`]); a slow reader gets `stream.lagged`, never an unbounded backlog.
pub trait EventStream: Send {
    fn epoch(&self) -> &str;
    /// Blocks up to `timeout`. The first item is always `stream.hello`; replay or `stream.lagged`
    /// follow per `semantics.md` section 7, and `stream.hello` repeats after 15 s of silence.
    fn recv_timeout(&mut self, timeout: Duration) -> RecvOutcome;
    /// Idempotent; releases the subscriber slot. Adapters call it on client disconnect or window close.
    fn close(&mut self);
}

/// Per-subscriber queue depth (docs/41 section 5.1).
pub const EVENT_QUEUE_CAPACITY: usize = 1024;
