//! Known-folder configuration is data, not authority to enumerate its contents.
use crate::{inventory::check, now};
use loomward_protocol::ErrorBody;
use loomward_shell::known_folders::{self, Folder, Location, Ownership};
use serde_json::{json, Value};
use std::time::Instant;

pub(crate) fn project(
    provider: &impl known_folders::FolderProvider,
    deadline: Instant,
) -> Result<Value, ErrorBody> {
    let mut contexts = Vec::new();
    for c in known_folders::contexts(provider) {
        check(deadline)?;
        let folder = match c.folder {
            Folder::Desktop => "desktop",
            Folder::PublicDesktop => "public_desktop",
            Folder::Documents => "documents",
            Folder::Downloads => "downloads",
            Folder::OneDrive => "onedrive",
            _ => unreachable!(),
        };
        let ownership = if c.ownership == Ownership::Public {
            "public"
        } else {
            "per_user"
        };
        let display = |p: &std::path::Path| {
            let s = p.to_string_lossy();
            json!({"text": s.chars().take(1024).collect::<String>(), "truncated": s.chars().count() > 1024})
        };
        let (location, current, default, redirected, reason) = match c.location {
            Location::FileSystem { current, default } => (
                "filesystem",
                display(&current),
                default.as_ref().map_or(Value::Null, |p| display(p)),
                default.as_ref().map(|p| *p != current),
                None,
            ),
            Location::Virtual => ("virtual", Value::Null, Value::Null, None, None),
            Location::Unavailable { reason } => {
                ("unavailable", Value::Null, Value::Null, None, Some(reason))
            }
        };
        contexts.push(json!({"folder": folder, "ownership": ownership, "location": location, "current_path": current, "default_path": default, "redirected": redirected, "reason": reason}));
    }
    Ok(
        json!({"observed_at": now(), "contexts": contexts, "providers": [
        {"provider": "desktop", "state": "unavailable", "reason": "desktop_enumeration_unavailable: no bounded pinned-directory listing; configured paths do not grant content reads"},
        {"provider": "public_desktop", "state": "unavailable", "reason": "desktop_enumeration_unavailable: no bounded pinned-directory listing; configured paths do not grant content reads"}
    ], "shortcuts": [], "next_cursor": null, "budget_hit": false}),
    )
}

pub(crate) fn personal(deadline: Instant) -> Result<Value, ErrorBody> {
    check(deadline)?;
    project(&known_folders::WindowsFolders, deadline)
}
