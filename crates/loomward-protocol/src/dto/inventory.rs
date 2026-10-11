use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryProviderState {
    Available,
    Denied,
    Unavailable,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryTextState {
    Known,
    Missing,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationScope {
    PerMachine,
    PerUser,
    Portable,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopFolder {
    Desktop,
    PublicDesktop,
    Documents,
    Downloads,
    Onedrive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopOwnership {
    PerUser,
    Public,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopLocation {
    Filesystem,
    Virtual,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutHealth {
    Present,
    Missing,
    VolumeUnavailable,
    NetworkUnreachableUnknown,
    Ambiguous,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutDesktop {
    Desktop,
    PublicDesktop,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataPageRequest {
    pub limit: Int<1, 200>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub cursor: Option<Cursor>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryProvider {
    pub provider: BoundedStr<1, 64>,
    pub state: InventoryProviderState,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub reason: Option<BoundedStr<0, 1024>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryText {
    pub state: InventoryTextState,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub value: Option<BoundedStr<0, 4096>>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub expandable: Option<bool>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub reason: Option<BoundedStr<0, 1024>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledApplication {
    pub provider: BoundedStr<1, 64>,
    pub scope: ApplicationScope,
    pub registry_key: BoundedStr<0, 4096>,
    pub name: InventoryText,
    pub version: InventoryText,
    pub publisher: InventoryText,
    pub install_location: InventoryText,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub estimated_size_bytes: Option<Bytes>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub size_reason: Option<BoundedStr<0, 1024>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationPage {
    pub observed_at: Timestamp,
    pub providers: BoundedVec<InventoryProvider, 0, 5>,
    pub applications: BoundedVec<InstalledApplication, 0, 200>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub next_cursor: Option<Cursor>,
    pub budget_hit: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopContext {
    pub folder: DesktopFolder,
    pub ownership: DesktopOwnership,
    pub location: DesktopLocation,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub current_path: Option<DisplayPath>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub default_path: Option<DisplayPath>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub redirected: Option<bool>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub reason: Option<BoundedStr<0, 1024>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopShortcut {
    pub desktop: ShortcutDesktop,
    pub name: BoundedStr<1, 4096>,
    pub health: ShortcutHealth,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub reason: Option<BoundedStr<0, 1024>>,
    pub target: InventoryText,
    pub arguments: InventoryText,
    pub working_directory: InventoryText,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopPage {
    pub observed_at: Timestamp,
    pub contexts: BoundedVec<DesktopContext, 0, 5>,
    pub providers: BoundedVec<InventoryProvider, 0, 2>,
    pub shortcuts: BoundedVec<DesktopShortcut, 0, 200>,
    #[serde(deserialize_with = "crate::types::required_nullable")]
    pub next_cursor: Option<Cursor>,
    pub budget_hit: bool,
}
