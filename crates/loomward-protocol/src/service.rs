//! The service trait every adapter calls, and the event stream behind it (docs/41 section 5.3).

use crate::dto::DisclosureSummary;
use crate::envelope::{RequestEnvelope, ResponseEnvelope};
use crate::event::EventEnvelope;
pub use crate::types::Adapter;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// What a Rust-side save dialog came to. Only `Cancelled` identifies the owner's choice;
/// failure and an ambiguous dialog result must not be reported as a cancel (invariant 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveDialog {
    Chosen(PathBuf),
    /// The owner closed the dialog without choosing a file.
    Cancelled,
    /// No path was returned, but the dialog backend cannot distinguish cancellation from failure.
    NoSelection,
    /// Another dialog is open, so this one was not stacked on it.
    Busy,
    /// There is no dialog to open: no owner window yet, no start folder, or an adapter without one.
    Unavailable,
}

/// Native UI the Rust side owns. The webview never supplies a path or a confirmation.
pub trait NativeDialogs: Send + Sync {
    /// Rust-side folder picker; the chosen path never crosses IPC.
    fn pick_folder(&self) -> Option<PathBuf>;
    /// Adapters can distinguish an unavailable picker from an owner cancellation, in-process.
    fn try_pick_folder(&self) -> Result<Option<PathBuf>, crate::ErrorBody> {
        Ok(self.pick_folder())
    }
    /// Rust-side open-file dialog for a backup receipt; the chosen path never crosses IPC. `title`
    /// names what to pick. An adapter without one never opens a dialog.
    fn pick_file(&self, title: &str) -> Option<PathBuf> {
        let _ = title;
        None
    }
    /// Rust-side save dialog for a diagnostic bundle (docs/42 ADR-V3-24); the chosen path never
    /// crosses IPC. Replacing an existing file needs [`NativeDialogs::confirm_replace`] too. An
    /// adapter without one never opens a dialog.
    fn save_file(&self, title: &str, default_name: &str) -> SaveDialog {
        let _ = (title, default_name);
        SaveDialog::Unavailable
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
    /// #219: names the Rust-selected folder and its filesystem, explaining that it cannot verify
    /// file identity and will be listed read-only, never acted on. `true` only on explicit accept;
    /// an adapter without it refuses.
    fn confirm_identity_unverified(&self, folder: &std::path::Path, filesystem: &str) -> bool {
        let _ = (folder, filesystem);
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
    /// The source's shared privacy policy for reply classes containing root/volume metadata.
    /// Other commands (especially roots.forget itself) return None. No wire capability changes.
    fn response_delivery_policy(
        &self,
        _command: crate::Command,
    ) -> Option<crate::ResponseDeliveryPolicy> {
        None
    }
    /// Adapters retain this pending response through any blocking-worker await. They must admit
    /// it only at the final native return or HTTP body poll, never before that await/queue.
    fn call_pending(&self, request: RequestEnvelope, ctx: &CallContext) -> crate::PendingResponse {
        let stamp = crate::Command::from_name(&request.command)
            .and_then(|command| self.response_delivery_policy(command))
            .map(|policy| policy.stamp());
        crate::PendingResponse::new(self.call(request, ctx), stamp)
    }
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
    /// In-process privacy admission for owned events waiting in an adapter. Capture a stamp before
    /// receiving and check it at the final handoff. Sources that redact retained payloads must
    /// return the same shared policy and invalidate it when they redact. Fixture-only streams
    /// without redaction can use the default policy. This adds no wire field or stream reason.
    fn delivery_policy(&self) -> crate::EventDeliveryPolicy {
        crate::EventDeliveryPolicy::default()
    }
    /// Blocks up to `timeout`. The first item is always `stream.hello`; replay or `stream.lagged`
    /// follow per `semantics.md` section 7, and `stream.hello` repeats after 15 s of silence.
    fn recv_timeout(&mut self, timeout: Duration) -> RecvOutcome;
    /// Idempotent; releases the subscriber slot. Adapters call it on client disconnect or window close.
    fn close(&mut self);
}

/// Per-subscriber queue depth (docs/41 section 5.1).
pub const EVENT_QUEUE_CAPACITY: usize = 1024;
