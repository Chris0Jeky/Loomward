//! Session provenance and bounded, immutable pages for read-only machine metadata.
use crate::{
    cursors::{Position, Resume},
    fail, now, with_reason, Inner, Out, ReadWork,
};
use loomward_protocol::{
    AppsListRequest, AppsSort, Command, DatasetClass, ErrorBody, ErrorCode, MetadataPageRequest,
    RequestEnvelope,
};
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

pub(crate) trait Provider: Send + Sync {
    fn dataset(&self) -> DatasetClass;
    fn read(&self, command: Command, deadline: Instant) -> Result<Value, ErrorBody>;
}

pub(crate) fn provider(dataset: DatasetClass) -> Arc<dyn Provider> {
    match dataset {
        DatasetClass::Synthetic => Arc::new(Fixtures),
        DatasetClass::Personal => Arc::new(Personal),
    }
}

struct Fixtures;
impl Provider for Fixtures {
    fn dataset(&self) -> DatasetClass {
        DatasetClass::Synthetic
    }
    fn read(&self, command: Command, _: Instant) -> Result<Value, ErrorBody> {
        let text = match command {
            Command::AppsList => {
                include_str!("../../../contracts/v3/examples/commands/apps.list.result.json")
            }
            Command::DesktopContexts => {
                include_str!("../../../contracts/v3/examples/commands/desktop.contexts.result.json")
            }
            _ => unreachable!(),
        };
        let mut result: Value = serde_json::from_str(text)
            .map_err(|_| fail(ErrorCode::InternalError, "synthetic inventory fixture"))?;
        result["observed_at"] = json!(now());
        Ok(result)
    }
}

struct Personal;
impl Provider for Personal {
    fn dataset(&self) -> DatasetClass {
        DatasetClass::Personal
    }
    fn read(&self, command: Command, deadline: Instant) -> Result<Value, ErrorBody> {
        personal_admission()?;
        match command {
            Command::AppsList => crate::apps::personal(deadline),
            Command::DesktopContexts => crate::desktop::personal(deadline),
            _ => unreachable!(),
        }
    }
}

fn personal_admission() -> Result<(), ErrorBody> {
    #[cfg(windows)]
    let reason =
        loomward_windows::enumerate::running_elevated().then_some("personal_metadata_elevated");
    #[cfg(not(windows))]
    let reason = Some("personal_metadata_unsupported");
    if let Some(reason) = reason {
        Err(with_reason(
            fail(
                ErrorCode::CapabilityUnavailable,
                "personal metadata requires supported, non-elevated Windows access",
            ),
            reason,
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn check(deadline: Instant) -> Result<(), ErrorBody> {
    if Instant::now() >= deadline {
        Err(fail(
            ErrorCode::DeadlineExceeded,
            "metadata read deadline expired",
        ))
    } else {
        Ok(())
    }
}

pub(crate) const SNAPSHOT_BYTES: usize = 4 * 1024 * 1024;

pub(crate) fn snapshot_budget() -> ErrorBody {
    with_reason(
        fail(
            ErrorCode::ResourceBudget,
            "metadata snapshot exceeds the bounded output budget",
        ),
        "metadata_snapshot_budget",
    )
}

// Count encoded bytes without allocating an encoded copy, and stop serialization at the bound.
pub(crate) fn encoded_size(value: &impl serde::Serialize) -> Result<usize, ErrorBody> {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > SNAPSHOT_BYTES.saturating_sub(self.0) {
                return Err(std::io::Error::other("metadata snapshot budget"));
            }
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value).map_err(|_| snapshot_budget())?;
    Ok(counter.0)
}

/// Largest provider-estimated size first. A size that is absent or unparseable is unknown, not
/// zero, and goes last. The sort is stable, so ties keep registry order. It runs once on the
/// immutable snapshot, so every page of one cursor chain sees the same total order.
fn size_desc(snapshot: &mut Value) {
    if let Some(rows) = snapshot["applications"].as_array_mut() {
        rows.sort_by_key(|row| {
            std::cmp::Reverse(
                row["estimated_size_bytes"]
                    .as_str()
                    .and_then(|s| s.parse::<u64>().ok()),
            )
        });
    }
}

impl Inner {
    pub(crate) fn metadata_page(
        &self,
        req: &RequestEnvelope,
        command: Command,
        work: Option<&ReadWork>,
        pending_cursor: Option<&Mutex<Option<Position>>>,
    ) -> Result<Out, ErrorBody> {
        let (limit, cursor, sort) = if command == Command::AppsList {
            let r: AppsListRequest = req.decode_payload()?;
            (r.limit, r.cursor, r.sort)
        } else {
            let r: MetadataPageRequest = req.decode_payload()?;
            (r.limit, r.cursor, None)
        };
        let deadline = work.expect("metadata commands use read admission").deadline;
        check(deadline)?;
        let (snapshot, offset) = match &cursor {
            Some(cursor) => match self.metadata_cursors.resume(cursor, command, "", None) {
                Ok(Position::Metadata {
                    sort: issued_under, ..
                }) if issued_under != sort => {
                    return Err(fail(
                        ErrorCode::InvalidRequest,
                        "cursor belongs to another sort order",
                    ))
                }
                Ok(Position::Metadata {
                    snapshot, offset, ..
                }) => (snapshot, offset),
                Err(Resume::Expired) => {
                    return Err(with_reason(
                        fail(ErrorCode::StaleGeneration, "restart metadata paging"),
                        "cursor_expired",
                    ))
                }
                _ => {
                    return Err(fail(
                        ErrorCode::InvalidRequest,
                        "cursor belongs to another command",
                    ))
                }
            },
            None => {
                if self.metadata.dataset() != self.dataset {
                    return Err(with_reason(
                        fail(
                            ErrorCode::CapabilityUnavailable,
                            "metadata provider has the wrong dataset class",
                        ),
                        "metadata_dataset_mismatch",
                    ));
                }
                let mut value = self.metadata.read(command, deadline)?;
                check(deadline)?;
                if sort == Some(AppsSort::SizeDesc) {
                    size_desc(&mut value);
                }
                encoded_size(&value)?;
                (Arc::new(value), 0)
            }
        };
        let key = if command == Command::AppsList {
            "applications"
        } else {
            "shortcuts"
        };
        let rows = snapshot[key]
            .as_array()
            .ok_or_else(|| fail(ErrorCode::InternalError, "invalid metadata provider"))?;
        if rows.len()
            > if command == Command::AppsList {
                6144
            } else {
                400
            }
        {
            return Err(fail(
                ErrorCode::ResourceBudget,
                "metadata entry budget exceeded",
            ));
        }
        let end = (offset + limit.get() as usize).min(rows.len());
        // The receiver issues the handle; a timed-out worker must not evict a live cursor.
        if end < rows.len() {
            *pending_cursor
                .expect("metadata read receiver")
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(Position::Metadata {
                snapshot: snapshot.clone(),
                offset: end,
                sort,
            });
        }
        let mut result = (*snapshot).clone();
        result[key] = json!(&rows[offset..end]);
        result["next_cursor"] = Value::Null;
        Ok(Out {
            result,
            catalog_rev: None,
            state_rev: None,
        })
    }
}

#[cfg(test)]
mod tests;
