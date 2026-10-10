use super::*;
use std::ptr::{null, null_mut};
use windows_sys::Win32::{
    Foundation::{
        ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_ITEMS, ERROR_PATH_NOT_FOUND,
        ERROR_SUCCESS,
    },
    System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER,
        HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
    },
};

const UNINSTALL: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall";
const FIELDS: [&str; 8] = [
    "DisplayName",
    "DisplayVersion",
    "Publisher",
    "InstallLocation",
    "UninstallString",
    "DisplayIcon",
    "ParentKeyName",
    "SystemComponent",
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

fn read_value(key: &Key, field: &str) -> Option<RegistryValue> {
    let name = wide(field);
    let mut kind = 0;
    let mut data = vec![0u8; MAX_VALUE_BYTES];
    let mut bytes = data.len() as u32;
    // SAFETY: live key, terminated name, valid outputs and a buffer of bytes capacity.
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            null(),
            &mut kind,
            data.as_mut_ptr(),
            &mut bytes,
        )
    };
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
        let mut result = RegistryRead {
            state: ProviderState::Available,
            reason: None,
            entries: Vec::new(),
        };
        for index in 0..MAX_ENTRIES {
            // Windows registry key names are at most 255 characters, plus a terminator.
            let mut name = [0u16; 256];
            let mut length = name.len() as u32;
            // SAFETY: live key, valid name buffer and length, unused outputs are null.
            let status = unsafe {
                RegEnumKeyExW(
                    root.0,
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
                return result;
            }
            if status != ERROR_SUCCESS || length as usize >= name.len() {
                partial(
                    &mut result,
                    format!("registry enumeration incomplete (Windows error {status})"),
                );
                return result;
            }
            let key_name = match String::from_utf16(&name[..length as usize]) {
                Ok(name) => name,
                Err(_) => {
                    partial(&mut result, "registry subkey has invalid UTF-16".into());
                    continue;
                }
            };
            name[length as usize] = 0;
            let key = match open(root.0, &name[..=length as usize], rights) {
                Ok(key) => key,
                Err(status) => {
                    partial(
                        &mut result,
                        format!(
                            "registry subkey unreadable or disappeared (Windows error {status})"
                        ),
                    );
                    continue;
                }
            };
            let values = FIELDS
                .into_iter()
                .filter_map(|field| read_value(&key, field).map(|value| (field.into(), value)))
                .collect();
            result.entries.push(RegistryEntry {
                key: key_name,
                values,
            });
        }
        partial(&mut result, "registry entry budget reached".into());
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
