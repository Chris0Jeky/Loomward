//! Session provenance and bounded, immutable pages for read-only machine metadata.
use crate::{
    cursors::{Position, Resume},
    fail, now, with_reason, Inner, Out, ReadWork,
};
use loomward_protocol::{
    Command, DatasetClass, ErrorBody, ErrorCode, MetadataPageRequest, RequestEnvelope,
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Instant};

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

impl Inner {
    pub(crate) fn metadata_page(
        &self,
        req: &RequestEnvelope,
        command: Command,
        work: Option<&ReadWork>,
    ) -> Result<Out, ErrorBody> {
        let r: MetadataPageRequest = req.decode_payload()?;
        let deadline = work.expect("metadata commands use read admission").deadline;
        check(deadline)?;
        let (snapshot, offset) = match &r.cursor {
            Some(cursor) => match self.metadata_cursors.resume(cursor, command, "", None) {
                Ok(Position::Metadata { snapshot, offset }) => (snapshot, offset),
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
                let value = self.metadata.read(command, deadline)?;
                check(deadline)?;
                let bytes = serde_json::to_vec(&value)
                    .map_err(|_| fail(ErrorCode::InternalError, "encode metadata"))?;
                if bytes.len() > 4 * 1024 * 1024 {
                    return Err(with_reason(
                        fail(
                            ErrorCode::ResourceBudget,
                            "metadata snapshot exceeds the bounded output budget",
                        ),
                        "metadata_snapshot_budget",
                    ));
                }
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
        let end = (offset + r.limit.get() as usize).min(rows.len());
        let next = (end < rows.len()).then(|| {
            self.metadata_cursors.issue(
                command,
                "".into(),
                Position::Metadata {
                    snapshot: snapshot.clone(),
                    offset: end,
                },
                None,
            )
        });
        let mut result = (*snapshot).clone();
        result[key] = json!(&rows[offset..end]);
        result["next_cursor"] = json!(next);
        Ok(Out {
            result,
            catalog_rev: None,
            state_rev: None,
        })
    }
}

#[cfg(test)]
mod tests;
