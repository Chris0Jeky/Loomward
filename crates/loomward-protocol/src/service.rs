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
    /// Rust-side open-file dialog for a backup receipt; the chosen path never crosses IPC. `title`
    /// names what to pick. An adapter without one never opens a dialog.
    fn pick_file(&self, title: &str) -> Option<PathBuf> {
        let _ = title;
        None
    }
    /// Rust-side save dialog for a diagnostic bundle (docs/42 ADR-V3-24); the chosen path never
    /// crosses IPC. Replacing an existing file needs [`NativeDialogs::confirm_replace`] too. An
    /// adapter without one never opens a dialog.
    fn save_file(&self, title: &str, default_name: &str) -> Option<PathBuf> {
        let _ = (title, default_name);
        None
    }
    /// Loomward's own confirmation before a diagnostic save replaces the existing file `name`.
    /// `true` only on explicit accept; Cancel is the default. Without one, nothing is replaced.
    fn confirm_replace(&self, name: &str) -> bool {
        let _ = name;
        false
    }
    /// Native modal listing recipient, model, fields and every item of the summary. `true` only on
    /// explicit accept. A display-budget refusal is an error, not an owner decline.
    fn confirm_disclosure(&self, summary: &DisclosureSummary) -> Result<bool, crate::ErrorBody>;
    /// #219: one-line native confirmation that a picked folder's volume cannot verify file
    /// identity and will be listed read-only, never acted on. `true` only on explicit accept;
    /// an adapter without it refuses.
    fn confirm_identity_unverified(&self) -> bool {
        false
    }
}

/// Who a call is for (docs/46 section 4). The adapter that hosts the caller sets it; nothing in a
/// payload or a client's own metadata can claim it (AGENTS.md invariant 7).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Principal {
    /// The owner's own UI: every command its adapter offers, unprojected.
    #[default]
    Owner,
    /// A non-owner client (`ask:<class>`, `mcp:<name>`): only the grant-scoped reads, inside its
    /// owner-issued disclosure grant (LW-067, LW-069), and nothing without one.
    Client(String),
}

/// Per-call facts the adapter knows and the payload cannot claim.
#[derive(Clone)]
pub struct CallContext {
    pub adapter: Adapter,
    /// `Some` on the desktop adapter only; HTTP has no native dialogs.
    pub dialogs: Option<Arc<dyn NativeDialogs>>,
    pub principal: Principal,
}

impl CallContext {
    pub fn http() -> Self {
        Self {
            adapter: Adapter::Http,
            dialogs: None,
            principal: Principal::Owner,
        }
    }

    /// An in-process client host (Ask, MCP): no native dialogs, HTTP's command surface, and the
    /// client principal `client`.
    pub fn client(client: impl Into<String>) -> Self {
        Self {
            principal: Principal::Client(client.into()),
            ..Self::http()
        }
    }

    pub fn tauri(dialogs: Arc<dyn NativeDialogs>) -> Self {
        Self {
            adapter: Adapter::Tauri,
            dialogs: Some(dialogs),
            principal: Principal::Owner,
        }
    }
}

impl fmt::Debug for CallContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallContext")
            .field("adapter", &self.adapter)
            .field("dialogs", &self.dialogs.as_ref().map(|_| "native"))
            .field("principal", &self.principal)
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
