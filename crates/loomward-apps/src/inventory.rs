//! Registry evidence is opaque text, never an executable command or resolved path.
use std::collections::BTreeMap;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::WindowsRegistry;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistryView {
    Machine64,
    Machine32,
    CurrentUser,
}

pub const REGISTRY_VIEWS: [RegistryView; 3] = [
    RegistryView::Machine64,
    RegistryView::Machine32,
    RegistryView::CurrentUser,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    PerMachine,
    PerUser,
    Portable,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Registry(RegistryView),
    Packaged,
    Portable,
}

impl Provider {
    pub fn scope(self) -> Scope {
        match self {
            Self::Registry(RegistryView::Machine64 | RegistryView::Machine32) => Scope::PerMachine,
            Self::Registry(RegistryView::CurrentUser) => Scope::PerUser,
            Self::Portable => Scope::Portable,
            Self::Packaged => Scope::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderState {
    Available,
    Denied,
    Unavailable,
    Partial,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Evidence<T> {
    Missing,
    Known(T),
    Unknown { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text {
    pub value: String,
    /// REG_EXPAND_SZ is preserved literally; environment variables are never expanded.
    pub expandable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Application {
    pub provider: Provider,
    pub scope: Scope,
    pub registry_key: String,
    pub display_name: Evidence<Text>,
    pub display_version: Evidence<Text>,
    pub publisher: Evidence<Text>,
    pub install_location: Evidence<Text>,
    pub uninstall_string: Evidence<Text>,
    pub display_icon: Evidence<Text>,
    pub parent_key_name: Evidence<Text>,
    pub system_component: Evidence<bool>,
}

#[derive(Clone, Debug)]
pub struct ProviderInventory {
    pub provider: Provider,
    pub state: ProviderState,
    /// Coverage reasons contain no registry names, values or user paths.
    pub reason: Option<String>,
    pub applications: Vec<Application>,
}

#[derive(Clone, Debug)]
pub enum RegistryValue {
    Bytes { kind: u32, data: Vec<u8> },
    Unknown { reason: String },
}

#[derive(Clone, Debug)]
pub struct RegistryEntry {
    pub key: String,
    /// Absent fields are missing; read failures must be represented as Unknown.
    pub values: BTreeMap<String, RegistryValue>,
}

#[derive(Clone, Debug)]
pub struct RegistryRead {
    pub state: ProviderState,
    pub reason: Option<String>,
    pub entries: Vec<RegistryEntry>,
}

pub trait RegistryReader {
    fn read(&self, view: RegistryView) -> RegistryRead;
}

/// Keeps cross-view duplicates: display names and paths are not application identities.
pub fn inventory(reader: &impl RegistryReader) -> Vec<ProviderInventory> {
    let mut providers = Vec::new();
    for view in REGISTRY_VIEWS {
        let read = reader.read(view);
        let provider = Provider::Registry(view);
        let mut result = ProviderInventory {
            provider,
            state: read.state,
            reason: read.reason,
            applications: Vec::new(),
        };
        for entry in read.entries {
            let mut incomplete = false;
            let mut text = |name: &str| {
                let evidence = text_evidence(entry.values.get(name));
                incomplete |= matches!(evidence, Evidence::Unknown { .. });
                evidence
            };
            let mut app = Application {
                provider,
                scope: provider.scope(),
                registry_key: entry.key,
                display_name: text("DisplayName"),
                display_version: text("DisplayVersion"),
                publisher: text("Publisher"),
                install_location: text("InstallLocation"),
                uninstall_string: text("UninstallString"),
                display_icon: text("DisplayIcon"),
                parent_key_name: text("ParentKeyName"),
                system_component: Evidence::Missing,
            };
            app.system_component = system_evidence(entry.values.get("SystemComponent"));
            incomplete |= matches!(app.system_component, Evidence::Unknown { .. });
            if incomplete && result.state == ProviderState::Available {
                result.state = ProviderState::Partial;
                result.reason =
                    Some("one or more evidence fields are unreadable or malformed".into());
            }
            result.applications.push(app);
        }
        providers.push(result);
    }
    for (provider, reason) in [
        (Provider::Packaged, "general Appx enumeration requires a WinRT PackageManager projection absent from windows-sys; no shell fallback"),
        (Provider::Portable, "portable applications cannot be established from Uninstall registry views; no filesystem discovery"),
    ] {
        providers.push(ProviderInventory {
            provider,
            state: ProviderState::Unavailable,
            reason: Some(reason.into()),
            applications: Vec::new(),
        });
    }
    providers
}

fn unknown<T>(reason: &str) -> Evidence<T> {
    Evidence::Unknown {
        reason: reason.into(),
    }
}

fn text_evidence(value: Option<&RegistryValue>) -> Evidence<Text> {
    let (kind, data) = match value {
        None => return Evidence::Missing,
        Some(RegistryValue::Unknown { reason }) => return unknown(reason),
        Some(RegistryValue::Bytes { kind, data }) => (*kind, data),
    };
    if !matches!(kind, 1 | 2) || data.len() % 2 != 0 {
        return unknown("expected REG_SZ or REG_EXPAND_SZ UTF-16LE");
    }
    let mut units: Vec<u16> = data
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
    if units.last() == Some(&0) {
        units.pop();
    }
    if units.contains(&0) {
        return unknown("embedded NUL in registry text");
    }
    match String::from_utf16(&units) {
        Ok(value) => Evidence::Known(Text {
            value,
            expandable: kind == 2,
        }),
        Err(_) => unknown("invalid UTF-16 in registry text"),
    }
}

fn system_evidence(value: Option<&RegistryValue>) -> Evidence<bool> {
    match value {
        None => Evidence::Missing,
        Some(RegistryValue::Unknown { reason }) => unknown(reason),
        Some(RegistryValue::Bytes { kind: 4, data }) if data.len() == 4 => {
            match u32::from_le_bytes(data.as_slice().try_into().expect("four bytes")) {
                0 => Evidence::Known(false),
                1 => Evidence::Known(true),
                _ => unknown("SystemComponent is neither zero nor one"),
            }
        }
        _ => unknown("expected a four-byte REG_DWORD SystemComponent"),
    }
}

#[cfg(test)]
mod tests;
