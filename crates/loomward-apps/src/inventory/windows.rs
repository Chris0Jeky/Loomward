use super::*;
use std::ptr::{null, null_mut};
use windows_sys::Win32::{
    Foundation::{
        ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_INVALID_DATA, ERROR_NO_MORE_ITEMS,
        ERROR_PATH_NOT_FOUND, ERROR_SUCCESS,
    },
    System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryInfoKeyW, RegQueryValueExW, HKEY,
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
    },
};

const UNINSTALL: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall";
const FIELDS: [&str; 9] = [
    "DisplayName",
    "DisplayVersion",
    "Publisher",
    "InstallLocation",
    "UninstallString",
    "DisplayIcon",
    "ParentKeyName",
    "SystemComponent",
    "EstimatedSize",
];
// shortcut: finite registry budgets can truncate coverage; raise only with memory evidence.
const MAX_ENTRIES: u32 = 2048;
const MAX_VALUE_BYTES: usize = 4096;

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: this handle was returned by a successful RegOpenKeyExW.
        unsafe { RegCloseKey(self.0) };
    }
}

fn access(view: RegistryView) -> (HKEY, u32) {
    match view {
        RegistryView::Machine64 => (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_64KEY),
        RegistryView::Machine32 => (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_32KEY),
        // HKCU SOFTWARE (including Uninstall) is shared, not redirected by WOW64.
        RegistryView::CurrentUser => (HKEY_CURRENT_USER, KEY_READ | KEY_WOW64_64KEY),
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

fn open(parent: HKEY, name: &[u16], rights: u32) -> Result<Key, u32> {
    let mut handle = null_mut();
    // SAFETY: name is NUL-terminated, parent is predefined or held open, output is valid.
    let status = unsafe { RegOpenKeyExW(parent, name.as_ptr(), 0, rights, &mut handle) };
    if status == ERROR_SUCCESS {
        Ok(Key(handle))
    } else {
        Err(status)
    }
}

fn failure(status: u32) -> RegistryRead {
    RegistryRead {
        state: match status {
            ERROR_ACCESS_DENIED => ProviderState::Denied,
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => ProviderState::Unavailable,
            _ => ProviderState::Partial,
        },
        reason: Some(format!(
            "registry provider open failed (Windows error {status})"
        )),
        entries: Vec::new(),
    }
}

fn partial(result: &mut RegistryRead, reason: String) {
    result.state = ProviderState::Partial;
    result.reason.get_or_insert(reason);
}

/// Seam over RegQueryValueExW: fills `data` and returns (status, kind, bytes required or written).
trait ValueSource {
    fn query(&self, name: &[u16], data: &mut [u8]) -> (u32, u32, u32);
}

impl ValueSource for Key {
    fn query(&self, name: &[u16], data: &mut [u8]) -> (u32, u32, u32) {
        let mut kind = 0;
        let mut bytes = data.len() as u32;
        // SAFETY: live key, terminated name, valid outputs and a buffer of bytes capacity.
        let status = unsafe {
            RegQueryValueExW(
                self.0,
                name.as_ptr(),
                null(),
                &mut kind,
                data.as_mut_ptr(),
                &mut bytes,
            )
        };
        (status, kind, bytes)
    }
}

fn read_value(source: &impl ValueSource, field: &str) -> Option<RegistryValue> {
    let name = wide(field);
    let mut data = vec![0u8; MAX_VALUE_BYTES];
    let (status, kind, bytes) = source.query(&name, &mut data);
    if status == ERROR_FILE_NOT_FOUND {
        return None;
    }
    if status != ERROR_SUCCESS || bytes as usize > data.len() {
        return Some(RegistryValue::Unknown {
            reason: format!(
                "registry value unreadable or exceeds byte budget (Windows error {status})"
            ),
        });
    }
    data.truncate(bytes as usize);
    Some(RegistryValue::Bytes { kind, data })
}

pub struct WindowsRegistry;

impl RegistryReader for WindowsRegistry {
    fn read(&self, view: RegistryView) -> RegistryRead {
        let (hive, rights) = access(view);
        // The 32-bit flag redirects the standard path to WOW6432Node; do not append it twice.
        let root = match open(hive, &wide(UNINSTALL), rights) {
            Ok(key) => key,
            Err(status) => return failure(status),
        };
        read_subkeys(&Subkeys { root, rights })
    }
}

trait SubkeyReader {
    fn count(&self) -> Result<u32, u32>;
    fn name(&self, index: u32) -> Result<Option<String>, u32>;
    fn entry(&self, name: &str) -> Result<RegistryEntry, u32>;
}

struct Subkeys {
    root: Key,
    rights: u32,
}

impl SubkeyReader for Subkeys {
    fn count(&self) -> Result<u32, u32> {
        let mut count = 0;
        // SAFETY: live root, valid count output, unused outputs are null.
        let status = unsafe {
            RegQueryInfoKeyW(
                self.root.0,
                null_mut(),
                null_mut(),
                null(),
                &mut count,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
            )
        };
        if status == ERROR_SUCCESS {
            Ok(count)
        } else {
            Err(status)
        }
    }

    fn name(&self, index: u32) -> Result<Option<String>, u32> {
        // Windows registry key names are at most 255 characters, plus a terminator.
        let mut name = [0u16; 256];
        let mut length = name.len() as u32;
        // SAFETY: live root, valid name buffer and length, unused outputs are null.
        let status = unsafe {
            RegEnumKeyExW(
                self.root.0,
                index,
                name.as_mut_ptr(),
                &mut length,
                null(),
                null_mut(),
                null_mut(),
                null_mut(),
            )
        };
        if status == ERROR_NO_MORE_ITEMS {
            return Ok(None);
        }
        if status != ERROR_SUCCESS {
            return Err(status);
        }
        if length as usize >= name.len() {
            return Err(ERROR_INVALID_DATA);
        }
        String::from_utf16(&name[..length as usize])
            .map(Some)
            .map_err(|_| ERROR_INVALID_DATA)
    }

    fn entry(&self, name: &str) -> Result<RegistryEntry, u32> {
        let key = open(self.root.0, &wide(name), self.rights)?;
        let values = FIELDS
            .into_iter()
            .filter_map(|field| read_value(&key, field).map(|value| (field.into(), value)))
            .collect();
        Ok(RegistryEntry {
            key: name.into(),
            values,
        })
    }
}

fn checked_count(
    reader: &impl SubkeyReader,
    previous: Option<u32>,
    result: &mut RegistryRead,
) -> Option<u32> {
    match reader.count() {
        Ok(count) => {
            if previous.is_some_and(|previous| previous != count) {
                partial(
                    result,
                    "registry subkey count changed during inventory".into(),
                );
            }
            Some(count)
        }
        Err(status) => {
            partial(
                result,
                format!("registry subkey count unreadable (Windows error {status})"),
            );
            None
        }
    }
}

fn read_subkeys(reader: &impl SubkeyReader) -> RegistryRead {
    let mut result = RegistryRead {
        state: ProviderState::Available,
        reason: None,
        entries: Vec::new(),
    };
    let before = checked_count(reader, None, &mut result);
    let mut names = Vec::new();
    // Snapshot names before reading entries so deletions cannot shift later read indices.
    for index in 0..=MAX_ENTRIES {
        match reader.name(index) {
            Ok(Some(_)) if index == MAX_ENTRIES => {
                partial(&mut result, "registry entry budget exceeded".into());
                break;
            }
            Ok(Some(name)) => names.push(name),
            Ok(None) => break,
            Err(status) => {
                partial(
                    &mut result,
                    format!("registry enumeration incomplete (Windows error {status})"),
                );
                break;
            }
        }
    }
    let after = checked_count(reader, before, &mut result);
    if after.is_some_and(|count| count.min(MAX_ENTRIES) as usize != names.len()) {
        partial(
            &mut result,
            "registry subkey count changed during enumeration".into(),
        );
    }
    for name in names {
        match reader.entry(&name) {
            Ok(entry) => result.entries.push(entry),
            Err(status) => partial(
                &mut result,
                format!("registry subkey unreadable or disappeared (Windows error {status})"),
            ),
        }
    }
    checked_count(reader, after, &mut result);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use windows_sys::Win32::Foundation::ERROR_MORE_DATA;

    #[derive(Clone, Copy)]
    enum Mutation {
        None,
        DeleteReadKey,
        DeleteNextKey,
        DeleteDuringEnumeration,
        AddDuringEnumeration,
        AddDuringRead,
        CountDenied,
    }

    struct FakeSubkeys {
        names: RefCell<Vec<String>>,
        mutation: Mutation,
    }

    impl FakeSubkeys {
        fn new(count: u32, mutation: Mutation) -> Self {
            Self {
                names: RefCell::new((0..count).map(|i| format!("synthetic-{i}")).collect()),
                mutation,
            }
        }
    }

    impl SubkeyReader for FakeSubkeys {
        fn count(&self) -> Result<u32, u32> {
            if matches!(self.mutation, Mutation::CountDenied) {
                Err(ERROR_ACCESS_DENIED)
            } else {
                Ok(self.names.borrow().len() as u32)
            }
        }

        fn name(&self, index: u32) -> Result<Option<String>, u32> {
            let mut names = self.names.borrow_mut();
            if index == 1 {
                match self.mutation {
                    Mutation::DeleteDuringEnumeration => {
                        names.remove(0);
                    }
                    Mutation::AddDuringEnumeration => names.push("synthetic-added".into()),
                    _ => {}
                }
            }
            Ok(names.get(index as usize).cloned())
        }

        fn entry(&self, name: &str) -> Result<RegistryEntry, u32> {
            let mut names = self.names.borrow_mut();
            if !names.iter().any(|n| n == name) {
                return Err(ERROR_FILE_NOT_FOUND);
            }
            if name == "synthetic-0" {
                match self.mutation {
                    Mutation::DeleteReadKey => {
                        names.remove(0);
                    }
                    Mutation::DeleteNextKey => {
                        names.remove(1);
                    }
                    Mutation::AddDuringRead => names.push("synthetic-added".into()),
                    _ => {}
                }
            }
            Ok(RegistryEntry {
                key: name.into(),
                values: BTreeMap::new(),
            })
        }
    }

    fn assert_partial(result: &RegistryRead) {
        assert_eq!(result.state, ProviderState::Partial);
        assert!(result.reason.as_ref().is_some_and(|r| !r.is_empty()));
    }

    #[test]
    fn snapshot_names_preserves_remaining_keys_after_deletion() {
        let result = read_subkeys(&FakeSubkeys::new(3, Mutation::DeleteReadKey));
        assert_eq!(
            result
                .entries
                .iter()
                .map(|e| e.key.as_str())
                .collect::<Vec<_>>(),
            ["synthetic-0", "synthetic-1", "synthetic-2"]
        );
        assert_partial(&result);
    }

    #[test]
    fn vanished_snapshot_key_reports_partial() {
        let result = read_subkeys(&FakeSubkeys::new(3, Mutation::DeleteNextKey));
        assert_partial(&result);
        assert!(result.reason.as_ref().unwrap().contains("disappeared"));
        assert_eq!(
            result
                .entries
                .iter()
                .map(|e| e.key.as_str())
                .collect::<Vec<_>>(),
            ["synthetic-0", "synthetic-2"]
        );
    }

    #[test]
    fn count_changes_during_enumeration_report_partial() {
        for mutation in [
            Mutation::DeleteDuringEnumeration,
            Mutation::AddDuringEnumeration,
        ] {
            let result = read_subkeys(&FakeSubkeys::new(3, mutation));
            assert_partial(&result);
            assert!(result.reason.as_ref().unwrap().contains("count changed"));
        }
    }

    #[test]
    fn count_changes_during_entry_reads_report_partial() {
        let result = read_subkeys(&FakeSubkeys::new(3, Mutation::AddDuringRead));
        assert_partial(&result);
        assert!(result.reason.as_ref().unwrap().contains("count changed"));
        assert_eq!(result.entries.len(), 3);
    }

    #[test]
    fn unreadable_count_cannot_claim_available() {
        let result = read_subkeys(&FakeSubkeys::new(3, Mutation::CountDenied));
        assert_partial(&result);
        assert_eq!(result.entries.len(), 3);
    }

    #[test]
    fn only_more_than_entry_budget_is_truncated() {
        for count in [0, MAX_ENTRIES - 1, MAX_ENTRIES, MAX_ENTRIES + 1] {
            let result = read_subkeys(&FakeSubkeys::new(count, Mutation::None));
            assert_eq!(result.entries.len(), count.min(MAX_ENTRIES) as usize);
            if count > MAX_ENTRIES {
                assert_partial(&result);
                assert!(result.reason.as_ref().unwrap().contains("budget"));
            } else {
                assert_eq!(result.state, ProviderState::Available);
                assert!(result.reason.is_none());
            }
        }
    }

    #[test]
    fn registry_access_is_read_only_and_views_are_explicit() {
        for (view, hive, flag) in [
            (RegistryView::Machine64, HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY),
            (RegistryView::Machine32, HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY),
            (
                RegistryView::CurrentUser,
                HKEY_CURRENT_USER,
                KEY_WOW64_64KEY,
            ),
        ] {
            assert_eq!(access(view), (hive, KEY_READ | flag));
        }
        assert!(!UNINSTALL.contains("WOW6432Node"));
    }

    #[test]
    fn errors_distinguish_denied_missing_and_partial() {
        for (error, state) in [
            (ERROR_ACCESS_DENIED, ProviderState::Denied),
            (ERROR_FILE_NOT_FOUND, ProviderState::Unavailable),
            (ERROR_PATH_NOT_FOUND, ProviderState::Unavailable),
            (1234, ProviderState::Partial),
        ] {
            let result = failure(error);
            assert_eq!(result.state, state);
            assert!(result.reason.is_some());
            assert!(result.entries.is_empty());
        }
        let mut result = failure(ERROR_FILE_NOT_FOUND);
        partial(&mut result, "synthetic partial".into());
        assert_eq!(result.state, ProviderState::Partial);
    }

    struct FakeValue {
        status: u32,
        kind: u32,
        bytes: u32,
    }

    impl ValueSource for FakeValue {
        fn query(&self, _: &[u16], data: &mut [u8]) -> (u32, u32, u32) {
            let written = (self.bytes as usize).min(data.len());
            data[..written].fill(7);
            (self.status, self.kind, self.bytes)
        }
    }

    fn value_of(status: u32, kind: u32, bytes: u32) -> Option<RegistryValue> {
        read_value(
            &FakeValue {
                status,
                kind,
                bytes,
            },
            "DisplayName",
        )
    }

    #[test]
    fn missing_registry_value_is_absent_not_unknown() {
        assert!(value_of(ERROR_FILE_NOT_FOUND, 0, 0).is_none());
    }

    #[test]
    fn registry_value_reads_exact_bytes_within_the_budget() {
        match value_of(ERROR_SUCCESS, 1, 5) {
            Some(RegistryValue::Bytes { kind, data }) => {
                assert_eq!(kind, 1);
                assert_eq!(data, vec![7; 5]);
            }
            other => panic!("expected bytes, got {other:?}"),
        }
        match value_of(ERROR_SUCCESS, 3, 0) {
            Some(RegistryValue::Bytes { kind, data }) => {
                assert_eq!(kind, 3);
                assert!(data.is_empty());
            }
            other => panic!("expected empty bytes, got {other:?}"),
        }
        match value_of(ERROR_SUCCESS, 1, 4096) {
            Some(RegistryValue::Bytes { data, .. }) => assert_eq!(data.len(), 4096),
            other => panic!("expected bytes at the budget, got {other:?}"),
        }
    }

    #[test]
    fn registry_value_over_budget_or_unreadable_is_unknown() {
        for (status, bytes) in [
            (ERROR_SUCCESS, 4097),
            (ERROR_MORE_DATA, 4097),
            (ERROR_ACCESS_DENIED, 0),
        ] {
            assert!(
                matches!(
                    value_of(status, 1, bytes),
                    Some(RegistryValue::Unknown { .. })
                ),
                "status {status} bytes {bytes}"
            );
        }
    }
}
