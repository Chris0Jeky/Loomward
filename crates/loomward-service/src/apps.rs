//! Registry evidence remains opaque; neither install locations nor commands are resolved.
use crate::inventory::check;
#[cfg(any(windows, test))]
use crate::now;
#[cfg(any(windows, test))]
use loomward_apps::inventory::{self as apps, Evidence, Provider, ProviderState, Scope, Text};
use loomward_protocol::ErrorBody;
#[cfg(any(windows, test))]
use serde_json::json;
use serde_json::Value;
use std::time::Instant;

#[cfg(any(windows, test))]
fn text(e: &Evidence<Text>) -> Value {
    match e {
        Evidence::Known(t) => {
            json!({"state": "known", "value": t.value, "expandable": t.expandable, "reason": null})
        }
        Evidence::Missing => {
            json!({"state": "missing", "value": null, "expandable": null, "reason": "provider field absent"})
        }
        Evidence::Unknown { reason } => {
            json!({"state": "unknown", "value": null, "expandable": null, "reason": reason})
        }
    }
}

#[cfg(any(windows, test))]
fn provider(p: Provider) -> &'static str {
    use apps::RegistryView::*;
    match p {
        Provider::Registry(Machine64) => "registry_machine64",
        Provider::Registry(Machine32) => "registry_machine32",
        Provider::Registry(CurrentUser) => "registry_current_user",
        Provider::Packaged => "packaged",
        Provider::Portable => "portable",
    }
}

#[cfg(any(windows, test))]
pub(crate) fn project(
    inventories: Vec<apps::ProviderInventory>,
    deadline: Instant,
) -> Result<Value, ErrorBody> {
    let mut providers = Vec::new();
    let mut applications = Vec::new();
    for p in inventories {
        check(deadline)?;
        let name = provider(p.provider);
        let state = match p.state {
            ProviderState::Available => "available",
            ProviderState::Denied => "denied",
            ProviderState::Unavailable => "unavailable",
            ProviderState::Partial => "partial",
        };
        providers.push(json!({"provider": name, "state": state, "reason": p.reason}));
        for app in p.applications {
            check(deadline)?;
            let (size, reason) = match app.estimated_size_bytes {
                Evidence::Known(n) => (Some(n.to_string()), None),
                Evidence::Missing => (None, Some("provider field absent".to_string())),
                Evidence::Unknown { reason } => (None, Some(reason)),
            };
            applications.push(json!({"provider": name, "scope": match app.scope { Scope::PerMachine => "per_machine", Scope::PerUser => "per_user", Scope::Portable => "portable", Scope::Unknown => "unknown" },
                "registry_key": app.registry_key, "name": text(&app.display_name), "version": text(&app.display_version), "publisher": text(&app.publisher), "install_location": text(&app.install_location), "estimated_size_bytes": size, "size_reason": reason}));
        }
    }
    Ok(
        json!({"observed_at": now(), "providers": providers, "applications": applications, "next_cursor": null, "budget_hit": false}),
    )
}

pub(crate) fn personal(deadline: Instant) -> Result<Value, ErrorBody> {
    check(deadline)?;
    #[cfg(windows)]
    {
        project(apps::inventory(&apps::WindowsRegistry), deadline)
    }
    #[cfg(not(windows))]
    {
        Err(crate::fail(
            loomward_protocol::ErrorCode::CapabilityUnavailable,
            "Windows registry inventory unavailable",
        ))
    }
}
