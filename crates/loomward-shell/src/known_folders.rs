use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Folder {
    Profile,
    Desktop,
    PublicDesktop,
    Documents,
    Downloads,
    OneDrive,
}
pub const FOLDERS: [Folder; 5] = [
    Folder::Desktop,
    Folder::PublicDesktop,
    Folder::Documents,
    Folder::Downloads,
    Folder::OneDrive,
];
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ownership {
    PerUser,
    Public,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    /// Configured paths only: this does not claim existence or an online cloud provider.
    FileSystem {
        current: PathBuf,
        /// None keeps an unavailable default-path query unknown.
        default: Option<PathBuf>,
    },
    Virtual,
    Unavailable {
        reason: String,
    },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    pub folder: Folder,
    pub ownership: Ownership,
    pub location: Location,
}
pub trait FolderProvider {
    fn location(&self, folder: Folder) -> Location;
}
pub fn contexts(provider: &impl FolderProvider) -> Vec<Context> {
    FOLDERS
        .into_iter()
        .map(|folder| Context {
            folder,
            ownership: if folder == Folder::PublicDesktop {
                Ownership::Public
            } else {
                Ownership::PerUser
            },
            location: provider.location(folder),
        })
        .collect()
}
pub struct WindowsFolders;
impl FolderProvider for WindowsFolders {
    fn location(&self, folder: Folder) -> Location {
        #[cfg(windows)]
        {
            match native::path(folder, false) {
                Ok(current) => Location::FileSystem {
                    current,
                    default: native::path(folder, true).ok(),
                },
                Err(reason) => Location::Unavailable { reason },
            }
        }
        #[cfg(not(windows))]
        {
            let _ = folder;
            Location::Unavailable {
                reason: "Windows known folders unsupported".into(),
            }
        }
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::{ffi::OsString, os::windows::ffi::OsStringExt, ptr};
    use windows_sys::Win32::{System::Com::CoTaskMemFree, UI::Shell::*};

    pub(super) fn path(folder: Folder, default: bool) -> Result<PathBuf, String> {
        let id = match folder {
            Folder::Profile => FOLDERID_Profile,
            Folder::Desktop => FOLDERID_Desktop,
            Folder::PublicDesktop => FOLDERID_PublicDesktop,
            Folder::Documents => FOLDERID_Documents,
            Folder::Downloads => FOLDERID_Downloads,
            Folder::OneDrive => FOLDERID_SkyDrive,
        };
        // Resolve configured redirection without verifying, creating or enumerating a provider.
        let flags = KF_FLAG_DONT_VERIFY | if default { KF_FLAG_DEFAULT_PATH } else { 0 };
        let mut out = ptr::null_mut();
        let result = unsafe { SHGetKnownFolderPath(&id, flags as u32, ptr::null_mut(), &mut out) };
        if result < 0 || out.is_null() {
            if !out.is_null() {
                unsafe { CoTaskMemFree(out.cast()) };
            }
            return Err(format!("known folder provider HRESULT {result:#010x}"));
        }
        let value = (0..32768)
            .find(|i| unsafe { *out.add(*i) == 0 })
            .map(|len| {
                PathBuf::from(OsString::from_wide(unsafe {
                    std::slice::from_raw_parts(out, len)
                }))
            });
        unsafe { CoTaskMemFree(out.cast()) };
        match value {
            Some(path) if path.is_absolute() => Ok(path),
            _ => Err("known folder provider returned invalid path".into()),
        }
    }
}
