//! Event names and the event envelope (docs/41 section 5.1; payload bindings in `commands.json`).

use crate::types::{Count, Protocol, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

macro_rules! event_names {
    ($($var:ident = $wire:literal, $data:literal;)*) => {
        /// The closed list of server-pushed events.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum EventName {
            $(#[serde(rename = $wire)] $var,)*
        }

        impl EventName {
            pub const ALL: &'static [EventName] = &[$(EventName::$var),*];

            /// The wire name, e.g. `scan.progress` (also the SSE `event:` field).
            pub const fn as_str(self) -> &'static str {
                match self { $(EventName::$var => $wire,)* }
            }

            /// The `$defs` name of the `data` payload.
            pub const fn data_def(self) -> &'static str {
                match self { $(EventName::$var => $data,)* }
            }
        }
    };
}

event_names! {
    StreamHello = "stream.hello", "StreamHello";
    StreamLagged = "stream.lagged", "StreamLagged";
    JobState = "job.state", "JobResult";
    ScanProgress = "scan.progress", "ScanProgressEvent";
    TreeInvalidated = "tree.invalidated", "TreeInvalidated";
    TelemetrySample = "telemetry.sample", "TelemetrySampleEvent";
    LearningUpdated = "learning.updated", "LearningUpdated";
    RootsChanged = "roots.changed", "RootList";
    VolumesChanged = "volumes.changed", "VolumeList";
    HealthWarning = "health.warning", "HealthWarning";
}

/// One pushed event. `seq` is monotonic per session and doubles as the SSE `id:`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventEnvelope {
    pub protocol: Protocol,
    pub seq: Count,
    pub event: EventName,
    pub at: Timestamp,
    /// Decoded against the payload named by [`EventName::data_def`].
    pub data: Map<String, Value>,
}
