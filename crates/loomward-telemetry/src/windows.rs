use super::*;
use std::mem::size_of;
mod pdh;
pub(super) use pdh::PdhSampler;
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_NO_MORE_FILES, FILETIME, HANDLE,
        INVALID_HANDLE_VALUE,
    },
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        },
        ProcessStatus::{
            GetPerformanceInfo, GetProcessMemoryInfo, PERFORMANCE_INFORMATION,
            PROCESS_MEMORY_COUNTERS_EX, PROCESS_MEMORY_COUNTERS_EX2,
        },
        RemoteDesktop::ProcessIdToSessionId,
        SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX},
        Threading::{
            GetActiveProcessorCount, GetProcessHandleCount, GetProcessIoCounters, GetProcessTimes,
            OpenProcess, QueryFullProcessImageNameW, IO_COUNTERS,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
    },
};

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn failure(field: &str, code: u32) -> Unknown {
    Unknown {
        field: field.into(),
        reason: if code == ERROR_ACCESS_DENIED {
            "access_denied"
        } else {
            "query_failed_or_process_exited"
        }
        .into(),
        windows_error: Some(code),
    }
}

fn ticks(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

fn memory() -> Memory {
    let mut result = Memory::default();
    let mut physical = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    let mut performance = PERFORMANCE_INFORMATION {
        cb: size_of::<PERFORMANCE_INFORMATION>() as u32,
        ..Default::default()
    };
    // Each API fills only its initialized, correctly sized output structure.
    unsafe {
        if GlobalMemoryStatusEx(&mut physical) != 0 {
            result.total_bytes = Some(physical.ullTotalPhys);
            result.available_bytes = Some(physical.ullAvailPhys);
            result.load_fraction = Some(f64::from(physical.dwMemoryLoad) / 100.0);
        } else {
            result
                .unknowns
                .push(failure("physical_memory", GetLastError()));
        }
        if GetPerformanceInfo(
            &mut performance,
            size_of::<PERFORMANCE_INFORMATION>() as u32,
        ) != 0
        {
            let page = performance.PageSize as u64;
            result.page_size_bytes = Some(page);
            result.commit_bytes = (performance.CommitTotal as u64).checked_mul(page);
            result.commit_limit_bytes = (performance.CommitLimit as u64).checked_mul(page);
            result.commit_peak_bytes = (performance.CommitPeak as u64).checked_mul(page);
            result.system_cache_bytes = (performance.SystemCache as u64).checked_mul(page);
            result.kernel_paged_bytes = (performance.KernelPaged as u64).checked_mul(page);
            result.kernel_nonpaged_bytes = (performance.KernelNonpaged as u64).checked_mul(page);
        } else {
            result
                .unknowns
                .push(failure("performance_memory", GetLastError()));
        }
    }
    result
}

fn process(entry: &PROCESSENTRY32W) -> (Process, bool) {
    let mut result = Process {
        pid: entry.th32ProcessID,
        parent_pid: entry.th32ParentProcessID,
        thread_count: entry.cntThreads,
        ..Default::default()
    };
    // Only observation rights are requested; no elevation or mutation fallback.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, result.pid) };
    if handle.is_null() {
        result
            .unknowns
            .push(failure("process_handle_fields", unsafe { GetLastError() }));
        result.protected_or_unknown = true;
        return (result, false);
    }
    let handle = Handle(handle);
    unsafe {
        let (mut start, mut exit, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        if GetProcessTimes(handle.0, &mut start, &mut exit, &mut kernel, &mut user) != 0 {
            if ticks(exit) != 0 {
                result.unknowns.push(unknown(
                    "process_handle_fields",
                    "process_exited_during_observation",
                ));
                result.protected_or_unknown = true;
                return (result, true);
            }
            result.start_time_windows_100ns = Some(ticks(start).to_string());
            result.cpu_user_100ns = Some(ticks(user));
            result.cpu_kernel_100ns = Some(ticks(kernel));
        } else {
            result
                .unknowns
                .push(failure("start_and_cpu_times", GetLastError()));
        }
        let mut name = vec![0u16; 32768];
        let mut length = name.len() as u32;
        if QueryFullProcessImageNameW(handle.0, 0, name.as_mut_ptr(), &mut length) != 0 {
            let path = String::from_utf16_lossy(&name[..length as usize]);
            result.image_name = Some(path.rsplit(['\\', '/']).next().unwrap_or(&path).into());
        } else {
            result.unknowns.push(failure("image_name", GetLastError()));
        }
        let mut session = 0;
        if ProcessIdToSessionId(result.pid, &mut session) != 0 {
            result.session_id = Some(session);
        } else {
            result.unknowns.push(failure("session_id", GetLastError()));
        }
        let mut extended = PROCESS_MEMORY_COUNTERS_EX2 {
            cb: size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32,
            ..Default::default()
        };
        if GetProcessMemoryInfo(
            handle.0,
            (&mut extended as *mut PROCESS_MEMORY_COUNTERS_EX2).cast(),
            extended.cb,
        ) != 0
        {
            result.working_set_bytes = Some(extended.WorkingSetSize as u64);
            result.private_commit_bytes = Some(extended.PrivateUsage as u64);
            result.private_working_set_bytes = Some(extended.PrivateWorkingSetSize as u64);
        } else {
            let ex2_error = GetLastError();
            let mut fallback = PROCESS_MEMORY_COUNTERS_EX {
                cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
                ..Default::default()
            };
            if GetProcessMemoryInfo(
                handle.0,
                (&mut fallback as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
                fallback.cb,
            ) != 0
            {
                result.working_set_bytes = Some(fallback.WorkingSetSize as u64);
                result.private_commit_bytes = Some(fallback.PrivateUsage as u64);
                result
                    .unknowns
                    .push(failure("private_working_set_bytes", ex2_error));
            } else {
                result
                    .unknowns
                    .push(failure("process_memory", GetLastError()));
            }
        }
        let mut io = IO_COUNTERS::default();
        if GetProcessIoCounters(handle.0, &mut io) != 0 {
            result.io_read_bytes = Some(io.ReadTransferCount);
            result.io_write_bytes = Some(io.WriteTransferCount);
            result.io_other_bytes = Some(io.OtherTransferCount);
        } else {
            result.unknowns.push(failure("io_counters", GetLastError()));
        }
        let mut count = 0;
        if GetProcessHandleCount(handle.0, &mut count) != 0 {
            result.handle_count = Some(count);
        } else {
            result
                .unknowns
                .push(failure("handle_count", GetLastError()));
        }
    }
    result.protected_or_unknown = !result.unknowns.is_empty();
    (result, true)
}

pub(super) fn collect(pdh: &mut PdhSampler) -> Snapshot {
    let mut snapshot = unsupported_snapshot();
    snapshot.status = "observed".into();
    snapshot.memory = Observation::Observed { value: memory() };
    let cores = unsafe { GetActiveProcessorCount(u16::MAX) };
    snapshot.logical_processor_count = (cores > 0).then_some(cores);
    pdh.sample(&mut snapshot);
    snapshot.process_gpu = Observation::Unsupported {
        reason: "optional GPU Process Memory counters not collected; PID-only instances are not stable process identity".into(),
    };
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if raw == INVALID_HANDLE_VALUE {
        snapshot
            .process_totals
            .enumeration_unknowns
            .push(failure("process_enumeration", unsafe { GetLastError() }));
        snapshot.status = "partial".into();
        return snapshot;
    }
    let handle = Handle(raw);
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut has_entry = unsafe { Process32FirstW(handle.0, &mut entry) };
    while has_entry != 0 && snapshot.processes.len() < MAX_ENUMERATED_PROCESSES {
        let (row, opened) = process(&entry);
        let totals = &mut snapshot.process_totals;
        totals.enumerated += 1;
        totals.opened += usize::from(opened);
        totals.access_denied += usize::from(
            row.unknowns
                .iter()
                .any(|u| u.windows_error == Some(ERROR_ACCESS_DENIED)),
        );
        totals.partial_or_unknown += usize::from(row.protected_or_unknown);
        if let Some(bytes) = row.working_set_bytes {
            totals.working_set_observed += 1;
            totals.working_set_sum_bytes += bytes;
        }
        if let Some(bytes) = row.private_commit_bytes {
            totals.private_commit_observed += 1;
            totals.private_commit_sum_bytes += bytes;
        }
        snapshot.processes.push(row);
        has_entry = unsafe { Process32NextW(handle.0, &mut entry) };
    }
    if has_entry != 0 {
        snapshot.process_totals.enumeration_truncated = true;
    } else {
        let error = unsafe { GetLastError() };
        if error != ERROR_NO_MORE_FILES {
            snapshot
                .process_totals
                .enumeration_unknowns
                .push(failure("process_enumeration", error));
        }
    }
    if snapshot.process_totals.partial_or_unknown > 0
        || snapshot.process_totals.enumeration_truncated
        || !snapshot.process_totals.enumeration_unknowns.is_empty()
        || !snapshot.pdh_unknowns.is_empty()
        || matches!(&snapshot.gpu, Observation::Unknown { .. })
        || matches!(&snapshot.gpu, Observation::Observed { value } if value.iter().any(|adapter| !adapter.unknowns.is_empty()))
        || matches!(&snapshot.disk_io, Observation::Unknown { .. })
        || matches!(&snapshot.disk_io, Observation::Observed { value } if value.iter().any(|disk| !disk.unknowns.is_empty()))
        || matches!(&snapshot.memory, Observation::Observed { value } if !value.unknowns.is_empty())
    {
        snapshot.status = "partial".into();
    }
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_lived_process_is_unknown_after_exit_even_with_a_retained_handle() {
        use std::process::{Command, Stdio};
        let mut child = Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        let (row, _) = process(&PROCESSENTRY32W {
            th32ProcessID: pid,
            ..Default::default()
        });
        assert!(row.protected_or_unknown);
        assert!(row.working_set_bytes.is_none());
        assert!(row.cpu_user_100ns.is_none());
        assert!(row.start_time_windows_100ns.is_none());
        assert!(!row.unknowns.is_empty());
    }
}
