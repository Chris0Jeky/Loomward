//! Event names and the event envelope (docs/41 section 5.1; payload bindings in `commands.json`).

use crate::dto::*;
use crate::envelope::{decode_exact, Payload};
use crate::error::ErrorBody;
use crate::types::{required_nullable, Count, Generation, Protocol, StreamEpoch, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;

macro_rules! event_names {
    ($($var:ident = $wire:literal, $data:ident;)*) => {
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
                match self { $(EventName::$var => stringify!($data),)* }
            }

            /// Decodes `data` as this event's payload DTO, exactly (no unknown fields, in bounds).
            pub fn validate_data(self, data: &Payload) -> Result<(), ErrorBody> {
                match self {
                    $(EventName::$var => decode_exact::<$data>(Value::Object(data.clone())).map(drop),)*
                }
            }
        }
    };
}

event_names! {
    StreamHello = "stream.hello", StreamHello;
    StreamLagged = "stream.lagged", StreamLagged;
    JobState = "job.state", JobResult;
    ScanProgress = "scan.progress", ScanProgressEvent;
    TreeInvalidated = "tree.invalidated", TreeInvalidated;
    TelemetrySample = "telemetry.sample", TelemetrySampleEvent;
    LearningUpdated = "learning.updated", LearningUpdated;
    RootsChanged = "roots.changed", RootList;
    VolumesChanged = "volumes.changed", VolumeList;
    HealthWarning = "health.warning", HealthWarning;
}

/// One pushed event. `seq` is monotonic per session and doubles as the SSE `id:`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventEnvelope {
    pub protocol: Protocol,
    /// Random per service start; `seq` is only comparable within one epoch.
    pub epoch: StreamEpoch,
    pub seq: Count,
    pub event: EventName,
    pub at: Timestamp,
    /// Catalogue revision of the commit the event describes; `None` for telemetry and state-only events.
    #[serde(deserialize_with = "required_nullable")]
    pub catalog_rev: Option<Generation>,
    /// `state.db` revision of the commit the event describes; `None` for catalogue-only events and telemetry.
    #[serde(deserialize_with = "required_nullable")]
    pub state_rev: Option<Generation>,
    /// Decoded against the payload named by [`EventName::data_def`].
    pub data: Payload,
}

impl EventEnvelope {
    /// Complete-envelope check: `data` must be exactly the payload of `event`.
    pub fn validate(&self) -> Result<(), ErrorBody> {
        self.event.validate_data(&self.data)
    }
}
