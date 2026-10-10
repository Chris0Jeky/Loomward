use super::*;
use std::{mem::size_of, ptr};
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
        Performance::*,
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
            result.physical_total_bytes = Some(physical.ullTotalPhys);
            result.physical_available_bytes = Some(physical.ullAvailPhys);
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
            result.commit_charge_bytes = (performance.CommitTotal as u64).checked_mul(page);
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

pub(super) fn collect(disk: &mut DiskSampler) -> Snapshot {
    let mut snapshot = unsupported_snapshot();
    snapshot.status = "observed".into();
    snapshot.memory = Observation::Observed { value: memory() };
    let cores = unsafe { GetActiveProcessorCount(u16::MAX) };
    snapshot.logical_processor_count = (cores > 0).then_some(cores);
    snapshot.gpu = Observation::Unsupported { reason: "windows-sys 0.61 does not expose DXGI adapter COM interfaces; no additional COM dependency added".into() };
    snapshot.process_gpu = Observation::Unsupported { reason: "optional per-process GPU PDH counters deferred; adapter budgets unavailable and PID-only counter instances are not stable process identity".into() };
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if raw == INVALID_HANDLE_VALUE {
        snapshot
            .process_totals
            .enumeration_unknowns
            .push(failure("process_enumeration", unsafe { GetLastError() }));
        snapshot.status = "partial".into();
        snapshot.disk_io = disk.sample();
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
        || matches!(&snapshot.memory, Observation::Observed { value } if !value.unknowns.is_empty())
    {
        snapshot.status = "partial".into();
    }
    snapshot.disk_io = disk.sample();
    snapshot
}

#[derive(Default)]
pub(super) struct DiskSampler {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
    previous_at: Option<i64>,
}

impl Drop for DiskSampler {
    fn drop(&mut self) {
        if !self.query.is_null() {
            unsafe {
                PdhCloseQuery(self.query);
            }
        }
    }
}

impl DiskSampler {
    fn sample(&mut self) -> Observation<Vec<DiskIo>> {
        match self.read() {
            Ok(rows) => Observation::Observed { value: rows },
            Err(reason) => Observation::Unknown { reason },
        }
    }

    fn read(&mut self) -> Result<Vec<DiskIo>, String> {
        // PDH owns its output strings inside the bounded, aligned caller buffer.
        unsafe {
            if self.query.is_null() {
                let status = PdhOpenQueryW(ptr::null(), 0, &mut self.query);
                if status != 0 {
                    return Err(format!("PdhOpenQueryW: {status:#x}"));
                }
            }
            if self.counter.is_null() {
                let path: Vec<u16> = "\\PhysicalDisk(*)\\Disk Bytes/sec\0"
                    .encode_utf16()
                    .collect();
                let status = PdhAddEnglishCounterW(self.query, path.as_ptr(), 0, &mut self.counter);
                if status != 0 {
                    return Err(format!("PdhAddEnglishCounterW: {status:#x}; counters may be unavailable without elevation"));
                }
            }
            let mut collected_at = 0;
            let status = PdhCollectQueryDataWithTime(self.query, &mut collected_at);
            if status != 0 {
                self.previous_at = None;
                return Err(format!("PdhCollectQueryDataWithTime: {status:#x}"));
            }
            let Some(previous) = self.previous_at.replace(collected_at) else {
                return Err("requires_two_disk_counter_samples".into());
            };
            let elapsed = collected_at
                .checked_sub(previous)
                .filter(|value| *value > 0)
                .ok_or("disk_counter_timestamp_did_not_advance")?;
            let seconds = elapsed as f64 / 10_000_000.0;
            let mut bytes = 0;
            let mut count = 0;
            let status = PdhGetFormattedCounterArrayW(
                self.counter,
                PDH_FMT_DOUBLE,
                &mut bytes,
                &mut count,
                ptr::null_mut(),
            );
            if status != PDH_MORE_DATA {
                return Err(format!("PdhGetFormattedCounterArrayW size: {status:#x}"));
            }
            if bytes == 0 || bytes > 1024 * 1024 {
                return Err("disk_counter_buffer_budget_exceeded".into());
            }
            let mut buffer = vec![
                PDH_FMT_COUNTERVALUE_ITEM_W::default();
                (bytes as usize)
                    .div_ceil(size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>())
            ];
            let status = PdhGetFormattedCounterArrayW(
                self.counter,
                PDH_FMT_DOUBLE,
                &mut bytes,
                &mut count,
                buffer.as_mut_ptr(),
            );
            if status != 0 {
                return Err(format!("PdhGetFormattedCounterArrayW data: {status:#x}"));
            }
            if count > 256 || count as usize > buffer.len() {
                return Err("disk_instance_budget_exceeded".into());
            }
            let mut rows = Vec::new();
            for item in &buffer[..count as usize] {
                let mut length = 0;
                if item.szName.is_null() {
                    return Err("missing_disk_instance_name".into());
                }
                while length < 1024 && *item.szName.add(length) != 0 {
                    length += 1;
                }
                if length == 1024 {
                    return Err("disk_instance_name_budget_exceeded".into());
                }
                let instance =
                    String::from_utf16_lossy(std::slice::from_raw_parts(item.szName, length));
                if instance == "_Total" {
                    continue;
                }
                let value = item.FmtValue.Anonymous.doubleValue;
                let valid = (item.FmtValue.CStatus == PDH_CSTATUS_VALID_DATA
                    || item.FmtValue.CStatus == PDH_CSTATUS_NEW_DATA)
                    && value.is_finite()
                    && value >= 0.0;
                rows.push(DiskIo {
                    instance,
                    bytes_per_second: valid.then_some(value),
                    sample_interval_seconds: seconds,
                    unknown_reason: (!valid)
                        .then(|| format!("invalid_counter_sample: {:#x}", item.FmtValue.CStatus)),
                });
            }
            if rows.is_empty() {
                return Err("no_physical_disk_instances_observed".into());
            }
            Ok(rows)
        }
    }
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
