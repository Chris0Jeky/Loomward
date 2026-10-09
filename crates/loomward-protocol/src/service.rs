//! The service trait every adapter calls (docs/41 section 5.3).

use crate::envelope::{RequestEnvelope, ResponseEnvelope};
use crate::event::EventEnvelope;
pub use crate::types::Adapter;
use crate::types::{Count, DatasetClass, Digest, Int, TeacherField, TeacherRecipient};
use std::fmt;
use std::path::PathBuf;
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

/// What the native confirmation dialog shows before a personal disclosure: the exact fields,
/// the item count, the recipient and the digest of the bytes that would leave the machine.
#[derive(Debug, Clone, PartialEq)]
pub struct DisclosureSummary {
    pub recipient: TeacherRecipient,
    pub dataset_class: DatasetClass,
    pub fields: Vec<TeacherField>,
    pub item_count: Int<1, 25>,
    pub payload_bytes: Count,
    pub payload_digest: Digest,
}

/// Native UI the Rust side owns. The webview never supplies a path or a confirmation.
pub trait NativeDialogs: Send + Sync {
    /// Rust-side folder picker; the path never crosses IPC.
    fn pick_folder(&self) -> Option<PathBuf>;
    /// Native modal showing exact fields and item count. `false` means declined.
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

/// The view-model service. Adapters run `call` on a blocking pool and pump `subscribe` into
/// the SSE response or the Tauri `Channel`.
pub trait ViewService: Send + Sync + 'static {
    /// Synchronous and bounded by the request deadline; long work returns a Job.
    fn call(&self, request: RequestEnvelope, ctx: &CallContext) -> ResponseEnvelope;
    /// Bounded per-subscriber queue; replays from `last_seq` when still buffered, else starts
    /// with `stream.lagged`.
    fn subscribe(&self, last_seq: Option<u64>) -> EventSubscription;
}

/// Per-subscriber queue depth (docs/41 section 5.1).
pub const EVENT_QUEUE_CAPACITY: usize = 1024;

/// A subscriber's bounded queue. The service keeps the [`SyncSender`]; a full queue means the
/// subscriber lagged, so the service drops the backlog and sends `stream.lagged`. A
/// disconnected channel means the subscriber is gone.
#[derive(Debug)]
pub struct EventSubscription {
    rx: Receiver<EventEnvelope>,
}

impl EventSubscription {
    /// A queue of `capacity` events and its producer end.
    pub fn bounded(capacity: usize) -> (SyncSender<EventEnvelope>, EventSubscription) {
        let (tx, rx) = sync_channel(capacity);
        (tx, EventSubscription { rx })
    }

    /// Blocks up to `timeout`; `Disconnected` once the service dropped the producer.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<EventEnvelope, RecvTimeoutError> {
        self.rx.recv_timeout(timeout)
    }

    pub fn try_recv(&self) -> Result<EventEnvelope, TryRecvError> {
        self.rx.try_recv()
    }
}
