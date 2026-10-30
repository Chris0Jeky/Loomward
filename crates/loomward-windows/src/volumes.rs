use serde::Serialize;
use std::io;

/// Space on the volume containing one explicitly granted root; no volume enumeration or contents.
#[derive(Debug, Clone, serde::Deserialize, Serialize)]
pub struct Space {
    pub capacity_bytes: String,
    pub free_bytes: String,
    pub read_only: Option<bool>,
    pub cluster_bytes: Option<u64>,
}

#[cfg(not(windows))]
pub fn space(_: &std::path::Path) -> io::Result<Space> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Windows volume space is unavailable",
    ))
}

#[cfg(windows)]
pub fn space(root: &std::path::Path) -> io::Result<Space> {
    crate::win::non_elevated()?;
    use windows_sys::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetDiskFreeSpaceW, GetVolumePathNameW,
    };
    let path = crate::win::wide(root)?;
    let mut mount = vec![0u16; 32768];
    if unsafe { GetVolumePathNameW(path.as_ptr(), mount.as_mut_ptr(), mount.len() as u32) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let (mut available, mut capacity, mut free) = (0, 0, 0);
    if unsafe { GetDiskFreeSpaceExW(mount.as_ptr(), &mut available, &mut capacity, &mut free) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if capacity == 0 || available > free || free > capacity {
        return Err(io::Error::other("unusable volume space"));
    }
    let (mut sectors, mut bytes, mut free_clusters, mut clusters) = (0, 0, 0, 0);
    let cluster_bytes = (unsafe {
        GetDiskFreeSpaceW(
            mount.as_ptr(),
            &mut sectors,
            &mut bytes,
            &mut free_clusters,
            &mut clusters,
        )
    } != 0)
        .then(|| u64::from(sectors) * u64::from(bytes))
        .filter(|n| *n > 0);
    Ok(Space {
        capacity_bytes: capacity.to_string(),
        free_bytes: available.to_string(),
        read_only: native::filesystem(&mount).ok().map(|fs| fs.read_only),
        cluster_bytes,
    })
}

#[derive(Debug, Serialize)]
pub struct Classification {
    pub drive_type: u32,
    pub bus_type: String,
    pub removable_media: bool,
    pub filesystem: String,
}
impl Classification {
    pub fn detachable(&self) -> io::Result<bool> {
        removable_device(self.drive_type, &self.bus_type, self.removable_media)
    }
    pub fn unsupported_root(&self) -> io::Result<bool> {
        let detachable = self.detachable()?;
        Ok(detachable
            && !(self.drive_type == 3
                && self.bus_type == "usb"
                && !self.removable_media
                && crate::enumerate::verifies_identity(&self.filesystem)))
    }
    pub fn unsupported_copy_destination(&self, personal: bool) -> io::Result<bool> {
        Ok(self.unsupported_root()? || (personal && self.detachable()?))
    }
}

/// Classification for one explicit path; no volume enumeration, capacity or file contents.
#[cfg(windows)]
pub fn classify(path: &std::path::Path) -> io::Result<Classification> {
    crate::win::non_elevated()?;
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumePathNameW};
    let path = crate::win::wide(path)?;
    let mut mount = vec![0u16; 32768];
    if unsafe { GetVolumePathNameW(path.as_ptr(), mount.as_mut_ptr(), mount.len() as u32) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let kind = unsafe { GetDriveTypeW(mount.as_ptr()) };
    removable_drive_type(kind)?;
    native::classify(&mount, kind)
}

#[cfg(not(windows))]
pub fn classify(_: &std::path::Path) -> io::Result<Classification> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Windows volume classification unavailable",
    ))
}

/// Detachability is a device fact, not the scan/grant root fence.
#[cfg(windows)]
pub fn is_removable(path: &std::path::Path) -> io::Result<bool> {
    classify(path)?.detachable()
}
#[cfg(not(windows))]
pub fn is_removable(_: &std::path::Path) -> io::Result<bool> {
    Ok(false)
}

#[cfg(windows)]
pub fn unsupported_root(path: &std::path::Path) -> io::Result<bool> {
    classify(path)?.unsupported_root()
}
#[cfg(not(windows))]
pub fn unsupported_root(_: &std::path::Path) -> io::Result<bool> {
    Ok(false)
}

fn removable_drive_type(kind: u32) -> io::Result<bool> {
    match kind {
        2 => Ok(true),
        3 => Ok(false),
        _ => Err(io::Error::other("local drive type unavailable")),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Fact<T> {
    Known { value: T },
    Unknown { reason: String },
}
impl<T> Fact<T> {
    #[cfg(windows)]
    fn from_result(result: io::Result<T>) -> Self {
        match result {
            Ok(value) => Self::Known { value },
            Err(error) => Self::Unknown {
                reason: error.to_string(),
            },
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Capacity {
    pub total_bytes: String,
    pub free_bytes: String,
    pub available_bytes: String,
}
#[derive(Debug, Serialize)]
pub struct Filesystem {
    pub name: String,
    pub serial: String,
    pub flags: u32,
    pub hard_links: bool,
    pub alternate_streams: bool,
    pub sparse: bool,
    pub compression: bool,
    /// Filesystem support, not the case-sensitive mode of any particular directory.
    pub case_sensitive_search: bool,
    pub reparse_points: bool,
    pub read_only: bool,
}
#[derive(Debug, Serialize)]
pub struct Volume {
    pub volume_name: String,
    pub mount_paths: Fact<Vec<String>>,
    pub filesystem: Fact<Filesystem>,
    pub capacity: Fact<Capacity>,
    pub drive_type: Fact<String>,
    pub bus_type: Fact<String>,
    pub removable_media: Fact<bool>,
    /// A device hint, not a benchmark or guaranteed medium/speed classification.
    pub incurs_seek_penalty: Fact<bool>,
    pub trim_enabled: Fact<bool>,
    pub availability: Fact<bool>,
}
impl Volume {
    pub fn validate(&self) -> io::Result<()> {
        if let Fact::Known { value } = &self.capacity {
            let parse = |s: &str| {
                s.parse::<u64>()
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid capacity"))
            };
            let total = parse(&value.total_bytes)?;
            let free = parse(&value.free_bytes)?;
            if free > total || parse(&value.available_bytes)? > free {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "capacity requires available <= free <= total",
                ));
            }
        }
        Ok(())
    }
}

/// A volume that fails validation keeps its row; only its capacity becomes an Unknown with a reason.
#[cfg(any(windows, test))]
fn settle(mut volume: Volume) -> Volume {
    if let Err(error) = volume.validate() {
        volume.capacity = Fact::Unknown {
            reason: error.to_string(),
        };
    }
    volume
}

#[cfg(not(windows))]
pub fn enumerate() -> io::Result<Vec<Volume>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Windows volume inventory is unavailable on this platform",
    ))
}
#[cfg(windows)]
pub fn enumerate() -> io::Result<Vec<Volume>> {
    native::enumerate()
}

#[cfg(windows)]
pub(crate) fn storage_query(
    path: &std::path::Path,
) -> io::Result<crate::storage_path::StorageObservation> {
    native::storage_query(path)
}

#[cfg(windows)]
mod native {
    use super::*;
    use crate::win::{open, wide, Handle};
    use std::{mem::size_of, path::Path, ptr};
    use windows_sys::Win32::{
        Foundation::*,
        Storage::FileSystem::*,
        System::{Ioctl::*, SystemServices::*, WindowsProgramming::*, IO::DeviceIoControl},
    };

    pub(super) fn storage_query(
        path: &Path,
    ) -> io::Result<crate::storage_path::StorageObservation> {
        use crate::storage_path::*;
        crate::win::non_elevated()?;
        let path = wide(path)?;
        let mut mount = vec![0u16; 32768];
        if unsafe { GetVolumePathNameW(path.as_ptr(), mount.as_mut_ptr(), mount.len() as u32) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut volume = vec![0u16; 1024];
        if unsafe {
            GetVolumeNameForVolumeMountPointW(
                mount.as_ptr(),
                volume.as_mut_ptr(),
                volume.len() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let len = volume
            .iter()
            .position(|c| *c == 0)
            .ok_or_else(|| io::Error::other("invalid volume name"))?;
        let guid = String::from_utf16(&volume[..len])
            .map_err(|_| io::Error::other("invalid volume name"))?;
        let handle = open(
            Path::new(guid.trim_end_matches('\\')),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        )?;
        let unknown = || Fact::Unknown {
            reason: "not queried by the IOCTL storage provider".into(),
        };
        let bus = Fact::from_result(
            property::<STORAGE_DEVICE_DESCRIPTOR>(&handle, StorageDeviceProperty).and_then(|d| {
                if d.BusType == BusTypeUnknown {
                    Err(io::Error::other("device reports unknown bus type"))
                } else {
                    Ok(bus_name(d.BusType))
                }
            }),
        );
        let geometry = property::<STORAGE_ACCESS_ALIGNMENT_DESCRIPTOR>(
            &handle,
            StorageAccessAlignmentProperty,
        )
        .and_then(|d| {
            sector_alignment(d.BytesPerLogicalSector, d.BytesPerPhysicalSector)?;
            Ok((d.BytesPerLogicalSector, d.BytesPerPhysicalSector))
        });
        let (logical_sector_bytes, physical_sector_bytes) = match geometry {
            Ok((logical, physical)) => (
                Fact::Known { value: logical },
                Fact::Known { value: physical },
            ),
            Err(error) => (
                Fact::Unknown {
                    reason: error.to_string(),
                },
                Fact::Unknown {
                    reason: error.to_string(),
                },
            ),
        };
        let write_cache_enabled = Fact::from_result(
            property::<STORAGE_WRITE_CACHE_PROPERTY>(&handle, StorageDeviceWriteCacheProperty)
                .and_then(|d| {
                    if d.WriteCacheEnabled == WriteCacheEnabled {
                        Ok(true)
                    } else if d.WriteCacheEnabled == WriteCacheDisabled {
                        Ok(false)
                    } else {
                        Err(io::Error::other("device reports unknown write-cache state"))
                    }
                }),
        );
        let health = observe_health(&bus, &guid);
        Ok(StorageObservation {
            storage_path: StoragePath {
                volume_guid: guid,
                disk_model: unknown(),
                firmware: unknown(),
                bus_type: bus.clone(),
                usb_vid: Fact::Unknown {
                    reason: "USB/PnP discovery not implemented".into(),
                },
                usb_pid: Fact::Unknown {
                    reason: "USB/PnP discovery not implemented".into(),
                },
                bridge: unknown(),
                usb_transport: Fact::Unknown {
                    reason: "BOT/UAS discovery not implemented".into(),
                },
                logical_sector_bytes,
                physical_sector_bytes,
                // RemovableMedia is not the PnP removal policy and must not stand in for it.
                removal_policy: Fact::Unknown {
                    reason: "PnP removal policy not queried".into(),
                },
                write_cache_enabled,
                windows_build: Fact::Unknown {
                    reason: "Windows build not queried".into(),
                },
            },
            mount_paths: Fact::from_result(mounts(&volume[..=len])),
            health,
        })
    }

    /// #876: NVMe health log where a non-elevated query exists; every other bus keeps its reason.
    fn observe_health(bus: &Fact<String>, guid: &str) -> crate::storage_path::HealthObservation {
        use crate::{reliability, storage_path::HealthObservation};
        match bus {
            Fact::Known { value } if value == "nvme" => {
                HealthObservation::from_counters(match reliability::nvme_counters(guid) {
                    Ok(value) => Fact::Known { value },
                    Err(error) => Fact::Unknown {
                        reason: reliability::failure_reason(&error),
                    },
                })
            }
            Fact::Known { value } => HealthObservation::UnsupportedOrHidden {
                reason: reliability::no_query_reason(value),
            },
            Fact::Unknown { .. } => HealthObservation::UnsupportedOrHidden {
                reason: reliability::no_query_reason("unknown"),
            },
        }
    }

    pub(super) fn classify(mount: &[u16], kind: u32) -> io::Result<Classification> {
        let mut volume = vec![0u16; 32768];
        if unsafe {
            GetVolumeNameForVolumeMountPointW(
                mount.as_ptr(),
                volume.as_mut_ptr(),
                volume.len() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let len = volume
            .iter()
            .position(|c| *c == 0)
            .ok_or_else(|| io::Error::other("invalid volume name"))?;
        let name = String::from_utf16(&volume[..len])
            .map_err(|_| io::Error::other("invalid volume name"))?;
        let handle = open(
            Path::new(name.trim_end_matches('\\')),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        )?;
        let descriptor = property::<STORAGE_DEVICE_DESCRIPTOR>(&handle, StorageDeviceProperty)?;
        let classification = Classification {
            drive_type: kind,
            bus_type: bus_name(descriptor.BusType),
            removable_media: descriptor.RemovableMedia,
            filesystem: filesystem(mount)?.name,
        };
        classification.detachable()?;
        Ok(classification)
    }

    struct Search(HANDLE);
    impl Drop for Search {
        fn drop(&mut self) {
            if self.0 != INVALID_HANDLE_VALUE {
                unsafe {
                    FindVolumeClose(self.0);
                }
            }
        }
    }
    pub(super) fn enumerate() -> io::Result<Vec<Volume>> {
        crate::win::non_elevated()?;
        let mut name = vec![0u16; 1024];
        let search = Search(unsafe { FindFirstVolumeW(name.as_mut_ptr(), name.len() as u32) });
        if search.0 == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let mut result = Vec::new();
        loop {
            result.push(volume(
                &name[..name.iter().position(|c| *c == 0).unwrap_or(name.len())],
            )?);
            if unsafe { FindNextVolumeW(search.0, name.as_mut_ptr(), name.len() as u32) } == 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                    break;
                }
                return Err(error);
            }
        }
        result.sort_by(|a, b| a.volume_name.cmp(&b.volume_name));
        Ok(result)
    }
    fn mounts(name: &[u16]) -> io::Result<Vec<String>> {
        let mut length = 0;
        let mut buffer = vec![0; 1024];
        loop {
            if unsafe {
                GetVolumePathNamesForVolumeNameW(
                    name.as_ptr(),
                    buffer.as_mut_ptr(),
                    buffer.len() as u32,
                    &mut length,
                )
            } != 0
            {
                break;
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_MORE_DATA as i32) {
                return Err(error);
            }
            if length as usize > 1024 * 1024 {
                return Err(io::Error::other("mount path response exceeds bound"));
            }
            buffer.resize(length as usize, 0);
        }
        Ok(buffer
            .split(|c| *c == 0)
            .take_while(|part| !part.is_empty())
            .map(String::from_utf16_lossy)
            .collect())
    }
    pub(super) fn filesystem(name: &[u16]) -> io::Result<Filesystem> {
        let (mut serial, mut max_component, mut flags) = (0, 0, 0);
        let mut fs = [0; 256];
        if unsafe {
            GetVolumeInformationW(
                name.as_ptr(),
                ptr::null_mut(),
                0,
                &mut serial,
                &mut max_component,
                &mut flags,
                fs.as_mut_ptr(),
                fs.len() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Filesystem {
            name: String::from_utf16_lossy(
                &fs[..fs.iter().position(|c| *c == 0).unwrap_or(fs.len())],
            ),
            serial: serial.to_string(),
            flags,
            hard_links: flags & FILE_SUPPORTS_HARD_LINKS != 0,
            alternate_streams: flags & FILE_NAMED_STREAMS != 0,
            sparse: flags & FILE_SUPPORTS_SPARSE_FILES != 0,
            compression: flags & FILE_FILE_COMPRESSION != 0,
            case_sensitive_search: flags & FILE_CASE_SENSITIVE_SEARCH != 0,
            reparse_points: flags & FILE_SUPPORTS_REPARSE_POINTS != 0,
            read_only: flags & FILE_READ_ONLY_VOLUME != 0,
        })
    }
    fn capacity(name: &[u16]) -> io::Result<Capacity> {
        let (mut available, mut total, mut free) = (0, 0, 0);
        if unsafe { GetDiskFreeSpaceExW(name.as_ptr(), &mut available, &mut total, &mut free) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if free > total || available > free {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "native capacity bounds invalid",
            ));
        }
        Ok(Capacity {
            total_bytes: total.to_string(),
            free_bytes: free.to_string(),
            available_bytes: available.to_string(),
        })
    }
    fn property<T: Default>(handle: &Handle, property: STORAGE_PROPERTY_ID) -> io::Result<T> {
        let query = STORAGE_PROPERTY_QUERY {
            PropertyId: property,
            QueryType: PropertyStandardQuery,
            ..Default::default()
        };
        // Only fixed descriptor prefixes are used; vendor/serial strings are not collected.
        let mut bytes = vec![0u8; 4096];
        let mut returned = 0;
        if unsafe {
            DeviceIoControl(
                handle.0,
                IOCTL_STORAGE_QUERY_PROPERTY,
                (&query as *const STORAGE_PROPERTY_QUERY).cast(),
                size_of::<STORAGE_PROPERTY_QUERY>() as u32,
                bytes.as_mut_ptr().cast(),
                bytes.len() as u32,
                &mut returned,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if (returned as usize) < size_of::<T>() || returned as usize > bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "short storage descriptor",
            ));
        }
        let version = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let declared_size = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        if version < size_of::<T>() || declared_size < size_of::<T>() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid storage descriptor header",
            ));
        }
        Ok(unsafe { ptr::read_unaligned(bytes.as_ptr().cast::<T>()) })
    }
    #[allow(non_upper_case_globals)] // Win32 constants retain their published SDK names.
    fn bus_name(bus: STORAGE_BUS_TYPE) -> String {
        match bus {
            BusTypeUnknown => "unknown",
            BusTypeScsi => "scsi",
            BusTypeAtapi => "atapi",
            BusTypeAta => "ata",
            BusType1394 => "1394",
            BusTypeSsa => "ssa",
            BusTypeFibre => "fibre",
            BusTypeUsb => "usb",
            BusTypeRAID => "raid",
            BusTypeiScsi => "iscsi",
            BusTypeSas => "sas",
            BusTypeSata => "sata",
            BusTypeSd => "sd",
            BusTypeMmc => "mmc",
            BusTypeVirtual => "virtual",
            BusTypeFileBackedVirtual => "file_backed_virtual",
            BusTypeSpaces => "spaces",
            BusTypeNvme => "nvme",
            _ => return format!("other({bus})"),
        }
        .to_owned()
    }
    fn volume(name: &[u16]) -> io::Result<Volume> {
        let text = String::from_utf16_lossy(name);
        let wide_name = wide(Path::new(&text))?;
        let fs = Fact::from_result(filesystem(&wide_name));
        let availability = match &fs {
            Fact::Known { .. } => Fact::Known { value: true },
            Fact::Unknown { reason } => Fact::Unknown {
                reason: reason.clone(),
            },
        };
        let drive_type = match unsafe { GetDriveTypeW(wide_name.as_ptr()) } {
            DRIVE_REMOVABLE => Fact::Known {
                value: "removable".into(),
            },
            DRIVE_FIXED => Fact::Known {
                value: "fixed".into(),
            },
            DRIVE_REMOTE => Fact::Known {
                value: "remote".into(),
            },
            DRIVE_CDROM => Fact::Known {
                value: "cdrom".into(),
            },
            DRIVE_RAMDISK => Fact::Known {
                value: "ramdisk".into(),
            },
            other => Fact::Unknown {
                reason: format!("GetDriveTypeW returned {other}"),
            },
        };
        let handle = open(
            Path::new(text.trim_end_matches('\\')),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        );
        let seek = Fact::from_result(match &handle {
            Ok(h) => {
                property::<DEVICE_SEEK_PENALTY_DESCRIPTOR>(h, StorageDeviceSeekPenaltyProperty)
                    .map(|d| d.IncursSeekPenalty)
            }
            Err(e) => Err(io::Error::new(e.kind(), e.to_string())),
        });
        let trim = Fact::from_result(match &handle {
            Ok(h) => property::<DEVICE_TRIM_DESCRIPTOR>(h, StorageDeviceTrimProperty)
                .map(|d| d.TrimEnabled),
            Err(e) => Err(io::Error::new(e.kind(), e.to_string())),
        });
        let descriptor = match &handle {
            Ok(h) => property::<STORAGE_DEVICE_DESCRIPTOR>(h, StorageDeviceProperty),
            Err(e) => Err(io::Error::new(e.kind(), e.to_string())),
        };
        let (bus_type, removable_media) = match descriptor {
            Ok(d) => (
                if d.BusType == BusTypeUnknown {
                    Fact::Unknown {
                        reason: "device reports unknown bus type".into(),
                    }
                } else {
                    Fact::Known {
                        value: bus_name(d.BusType),
                    }
                },
                Fact::Known {
                    value: d.RemovableMedia,
                },
            ),
            Err(e) => (
                Fact::Unknown {
                    reason: e.to_string(),
                },
                Fact::Unknown {
                    reason: e.to_string(),
                },
            ),
        };
        let result = Volume {
            volume_name: text,
            mount_paths: Fact::from_result(mounts(&wide_name)),
            filesystem: fs,
            capacity: Fact::from_result(capacity(&wide_name)),
            drive_type,
            bus_type,
            removable_media,
            incurs_seek_penalty: seek,
            trim_enabled: trim,
            availability,
        };
        Ok(settle(result))
    }
}

#[cfg(test)]
mod hardening_tests {
    #[test]
    fn hardening_removable_drive_classification() {
        assert!(super::removable_drive_type(2).unwrap());
        assert!(!super::removable_drive_type(3).unwrap());
        assert!(super::removable_drive_type(0).is_err());
        assert!(super::removable_drive_type(4).is_err());
    }
}

#[cfg(test)]
mod settle_tests {
    use super::*;

    fn volume_with_capacity(capacity: Fact<Capacity>) -> Volume {
        Volume {
            volume_name: "synthetic-volume".into(),
            mount_paths: Fact::Known { value: vec![] },
            filesystem: Fact::Unknown {
                reason: "synthetic".into(),
            },
            capacity,
            drive_type: Fact::Known {
                value: "fixed".into(),
            },
            bus_type: Fact::Unknown {
                reason: "synthetic".into(),
            },
            removable_media: Fact::Known { value: false },
            incurs_seek_penalty: Fact::Unknown {
                reason: "synthetic".into(),
            },
            trim_enabled: Fact::Unknown {
                reason: "synthetic".into(),
            },
            availability: Fact::Known { value: true },
        }
    }

    #[test]
    fn invalid_capacity_is_named_unknown_and_the_volume_is_kept() {
        let invalid = Capacity {
            total_bytes: "10".into(),
            free_bytes: "20".into(),
            available_bytes: "5".into(),
        };
        let raw = volume_with_capacity(Fact::Known { value: invalid });
        assert!(raw.validate().is_err());
        let settled = settle(raw);
        assert!(settled.validate().is_ok());
        assert_eq!(settled.volume_name, "synthetic-volume");
        assert!(matches!(
            &settled.capacity,
            Fact::Unknown { reason } if reason == "capacity requires available <= free <= total"
        ));
        assert!(matches!(
            &settled.drive_type,
            Fact::Known { value } if value == "fixed"
        ));
    }

    #[test]
    fn valid_capacity_is_left_alone() {
        let valid = Capacity {
            total_bytes: "20".into(),
            free_bytes: "10".into(),
            available_bytes: "5".into(),
        };
        let settled = settle(volume_with_capacity(Fact::Known { value: valid }));
        assert!(matches!(&settled.capacity, Fact::Known { value } if value.free_bytes == "10"));
    }
}

fn removable_device(kind: u32, bus: &str, removable_media: bool) -> io::Result<bool> {
    let removable = removable_drive_type(kind)?;
    match bus {
        "usb" | "sd" => Ok(true),
        "scsi" | "atapi" | "ata" | "raid" | "sas" | "sata" | "spaces" | "nvme" => {
            Ok(removable || removable_media)
        }
        _ => Err(io::Error::other("local bus type unavailable")),
    }
}

#[cfg(test)]
mod followup_tests {
    #[test]
    fn only_known_local_storage_buses_are_eligible() {
        for bus in [
            "scsi", "atapi", "ata", "raid", "sas", "sata", "spaces", "nvme",
        ] {
            assert!(!super::removable_device(3, bus, false).unwrap(), "{bus}");
        }
        for bus in [
            "unknown",
            "other(99)",
            "iscsi",
            "1394",
            "file_backed_virtual",
            "mmc",
            "virtual",
            "fibre",
            "ssa",
        ] {
            assert!(super::removable_device(3, bus, false).is_err(), "{bus}");
        }
    }

    #[test]
    fn unsupported_drive_types_cannot_bypass_the_bus_fence() {
        for kind in [0, 1, 4, 5, 6] {
            assert!(super::removable_drive_type(kind).is_err(), "{kind}");
        }
    }

    #[test]
    fn fixed_usb_roots_require_verified_identity_but_remain_detachable() {
        for bus in ["usb", "sd"] {
            assert!(super::removable_device(3, bus, false).unwrap());
        }
        assert!(super::removable_device(3, "sata", true).unwrap());
        assert!(!super::removable_device(3, "sata", false).unwrap());
        assert!(super::removable_device(2, "sata", false).unwrap());
        assert!(super::removable_device(3, "unknown", false).is_err());
        for (kind, media) in [(3, false), (3, true), (2, false)] {
            let unknown = super::Classification {
                drive_type: kind,
                bus_type: "unknown".into(),
                removable_media: media,
                filesystem: "NTFS".into(),
            };
            assert!(unknown.unsupported_root().is_err());
            assert!(unknown.unsupported_copy_destination(false).is_err());
        }
        for (kind, bus, media, fs, refused) in [
            (3, "usb", false, "NTFS", false),
            (3, "usb", false, "ReFS", false),
            (3, "usb", true, "NTFS", true),
            (3, "sd", false, "NTFS", true),
            (2, "usb", false, "NTFS", true),
            (3, "sata", false, "NTFS", false),
            (3, "sata", false, "FAT32", false),
            (3, "usb", false, "FAT32", true),
            (3, "usb", false, "exFAT", true),
        ] {
            let volume = super::Classification {
                drive_type: kind,
                bus_type: bus.into(),
                removable_media: media,
                filesystem: fs.into(),
            };
            assert_eq!(volume.unsupported_root().unwrap(), refused, "{volume:?}");
            assert_eq!(volume.unsupported_copy_destination(false).unwrap(), refused);
            assert_eq!(
                volume.unsupported_copy_destination(true).unwrap(),
                refused || bus == "usb"
            );
        }
    }
}
