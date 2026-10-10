use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    mem::size_of_val,
    ptr,
};
use windows_sys::Win32::System::Performance::*;

#[derive(Clone, Copy)]
#[repr(usize)]
enum Counter {
    StandbyCore,
    StandbyNormal,
    StandbyReserve,
    Modified,
    Free,
    Cpu,
    DiskRead,
    DiskWrite,
    DiskIdle,
    DiskQueue,
    GpuEngine,
    GpuDedicated,
    GpuShared,
}

const PATHS: [&str; 13] = [
    r"\Memory\Standby Cache Core Bytes",
    r"\Memory\Standby Cache Normal Priority Bytes",
    r"\Memory\Standby Cache Reserve Bytes",
    r"\Memory\Modified Page List Bytes",
    r"\Memory\Free & Zero Page List Bytes",
    r"\Processor Information(_Total)\% Processor Time",
    r"\PhysicalDisk(*)\Disk Read Bytes/sec",
    r"\PhysicalDisk(*)\Disk Write Bytes/sec",
    r"\PhysicalDisk(*)\% Idle Time",
    r"\PhysicalDisk(*)\Avg. Disk Queue Length",
    r"\GPU Engine(*)\Utilization Percentage",
    r"\GPU Adapter Memory(*)\Dedicated Usage",
    r"\GPU Adapter Memory(*)\Shared Usage",
];
const MAX_BUFFER_BYTES: u32 = 1024 * 1024;
const MAX_INSTANCES: usize = 16384;
const MAX_NAME_UNITS: usize = 1024;
// pdh.h flags omitted by windows-sys 0.61: keep units and uncapped queue/rate values.
const PDH_FMT_NOSCALE: u32 = 0x1000;
const DOUBLE_FORMAT: u32 = PDH_FMT_DOUBLE | PDH_FMT_NOSCALE | 0x8000; // PDH_FMT_NOCAP100

#[derive(Default)]
pub(crate) struct PdhSampler {
    query: PDH_HQUERY,
    counters: [PDH_HCOUNTER; 13],
    errors: [Option<String>; 13],
    localized_paths: [Option<Vec<u16>>; 13],
    previous_at: Option<i64>,
    interval: Option<f64>,
    expansion_ms: f64,
    expansions: u64,
    sample_sequence: u64,
    next_expansion: [u64; 13],
}

impl Drop for PdhSampler {
    fn drop(&mut self) {
        if !self.query.is_null() {
            unsafe { PdhCloseQuery(self.query) };
        }
    }
}

#[derive(Debug)]
struct CounterRow<T> {
    instance: String,
    value: Result<T, String>,
}

#[derive(Debug)]
struct CounterArray<T> {
    rows: Vec<CounterRow<T>>,
    malformed: usize,
}

fn valid(value: &PDH_FMT_COUNTERVALUE) -> Result<(), String> {
    match value.CStatus {
        PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA => Ok(()),
        status => Err(format!("invalid_counter_sample: {status:#x}")),
    }
}

fn double(value: &PDH_FMT_COUNTERVALUE) -> Result<f64, String> {
    valid(value)?;
    let number = unsafe { value.Anonymous.doubleValue };
    if number.is_finite() && number >= 0.0 {
        Ok(number)
    } else {
        Err("nonfinite_or_negative_counter".into())
    }
}

fn bytes(value: &PDH_FMT_COUNTERVALUE) -> Result<u64, String> {
    valid(value)?;
    u64::try_from(unsafe { value.Anonymous.largeValue }).map_err(|_| "negative_byte_counter".into())
}

// The entire PDH allocation (headers and strings) remains alive during parsing.
fn parse_array<T>(
    buffer: &[PDH_FMT_COUNTERVALUE_ITEM_W],
    used_bytes: usize,
    count: usize,
    decode: fn(&PDH_FMT_COUNTERVALUE) -> Result<T, String>,
) -> Result<CounterArray<T>, String> {
    let header_bytes = count
        .checked_mul(size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>())
        .ok_or("counter_array_header_overflow")?;
    if count > MAX_INSTANCES || header_bytes > used_bytes || used_bytes > size_of_val(buffer) {
        return Err("counter_array_bounds_or_instance_budget_exceeded".into());
    }
    let base = buffer.as_ptr() as usize;
    let mut result = CounterArray {
        rows: vec![],
        malformed: 0,
    };
    let mut seen = BTreeSet::new();
    for item in &buffer[..count] {
        let address = item.szName as usize;
        let offset = address.checked_sub(base);
        let name = offset
            .filter(|offset| *offset >= header_bytes && *offset < used_bytes && offset % 2 == 0)
            .and_then(|offset| {
                let available = ((used_bytes - offset) / 2).min(MAX_NAME_UNITS);
                // Range and alignment were checked against the actual returned buffer.
                let units = unsafe { std::slice::from_raw_parts(item.szName, available) };
                let end = units.iter().position(|unit| *unit == 0)?;
                if end == 0 {
                    return None;
                }
                String::from_utf16(&units[..end]).ok()
            });
        let Some(instance) = name else {
            result.malformed += 1;
            continue;
        };
        if !seen.insert(instance.clone()) {
            result.malformed += 1;
            continue;
        }
        if instance != "_Total" {
            result.rows.push(CounterRow {
                instance,
                value: decode(&item.FmtValue),
            });
        }
    }
    Ok(result)
}

fn reconcile_instances<T>(array: &mut CounterArray<T>, expanded: CounterArray<()>) {
    let mut instances: BTreeSet<_> = expanded.rows.into_iter().map(|row| row.instance).collect();
    // Collection and expansion can see different instances; neither side gives a complete total.
    for row in &mut array.rows {
        if !instances.remove(&row.instance) {
            row.value = Err("instance_churn".into());
        }
    }
    array
        .rows
        .extend(instances.into_iter().map(|instance| CounterRow {
            instance,
            value: Err("instance_churn".into()),
        }));
    array.malformed += expanded.malformed;
}

impl PdhSampler {
    fn expansion_due(&self, kind: Counter) -> bool {
        self.sample_sequence >= self.next_expansion[kind as usize]
    }

    fn collect(&mut self, channels: SampleChannels) -> Result<(), String> {
        unsafe {
            if self.query.is_null() {
                let status = PdhOpenQueryW(ptr::null(), 0, &mut self.query);
                if status != 0 {
                    return Err(format!("PdhOpenQueryW: {status:#x}"));
                }
            }
            for (index, path) in PATHS.iter().enumerate() {
                let enabled = match index {
                    0..=5 => channels.system,
                    6..=9 => channels.disks,
                    _ => channels.gpu,
                };
                if enabled && self.counters[index].is_null() {
                    let path: Vec<u16> = path.encode_utf16().chain([0]).collect();
                    let status = PdhAddEnglishCounterW(
                        self.query,
                        path.as_ptr(),
                        0,
                        &mut self.counters[index],
                    );
                    self.errors[index] =
                        (status != 0).then(|| format!("PdhAddEnglishCounterW: {status:#x}"));
                }
            }
            let mut at = 0;
            let status = PdhCollectQueryDataWithTime(self.query, &mut at);
            if status != 0 {
                self.previous_at = None;
                self.interval = None;
                return Err(format!("PdhCollectQueryDataWithTime: {status:#x}"));
            }
            self.interval = self
                .previous_at
                .replace(at)
                .and_then(|previous| at.checked_sub(previous))
                .filter(|delta| *delta > 0)
                .map(|delta| delta as f64 / 10_000_000.0);
        }
        Ok(())
    }

    fn counter(&self, kind: Counter) -> Result<PDH_HCOUNTER, String> {
        let index = kind as usize;
        if let Some(reason) = &self.errors[index] {
            return Err(reason.clone());
        }
        if self.counters[index].is_null() {
            return Err("counter_not_available".into());
        }
        if matches!(
            kind,
            Counter::Cpu
                | Counter::DiskRead
                | Counter::DiskWrite
                | Counter::DiskIdle
                | Counter::DiskQueue
                | Counter::GpuEngine
        ) && self.interval.is_none()
        {
            return Err("requires_two_counter_samples_with_advancing_timestamp".into());
        }
        Ok(self.counters[index])
    }

    fn scalar<T>(
        &self,
        kind: Counter,
        format: u32,
        decode: fn(&PDH_FMT_COUNTERVALUE) -> Result<T, String>,
    ) -> Result<T, String> {
        let counter = self.counter(kind)?;
        let mut value = PDH_FMT_COUNTERVALUE::default();
        let status =
            unsafe { PdhGetFormattedCounterValue(counter, format, ptr::null_mut(), &mut value) };
        if status != 0 {
            return Err(format!("PdhGetFormattedCounterValue: {status:#x}"));
        }
        decode(&value)
    }

    fn array<T>(
        &mut self,
        kind: Counter,
        format: u32,
        decode: fn(&PDH_FMT_COUNTERVALUE) -> Result<T, String>,
    ) -> Result<CounterArray<T>, String> {
        let counter = self.counter(kind)?;
        let index = kind as usize;
        let expanded = if self.expansion_due(kind) {
            let started = Instant::now();
            let expanded = self.expand(kind, counter);
            self.expansion_ms += started.elapsed().as_secs_f64() * 1000.0;
            self.expansions += 1;
            self.next_expansion[index] = self.sample_sequence.saturating_add(10);
            Some(expanded?)
        } else {
            None
        };
        // Retry churn during sizing; reconcile collection with the fresh expansion.
        for _ in 0..3 {
            let (mut length, mut count) = (0, 0);
            let status = unsafe {
                PdhGetFormattedCounterArrayW(
                    counter,
                    format,
                    &mut length,
                    &mut count,
                    ptr::null_mut(),
                )
            };
            if status != PDH_MORE_DATA {
                return Err(format!("PdhGetFormattedCounterArrayW size: {status:#x}"));
            }
            if length == 0 || length > MAX_BUFFER_BYTES {
                return Err("counter_buffer_budget_exceeded".into());
            }
            let mut buffer = vec![
                PDH_FMT_COUNTERVALUE_ITEM_W::default();
                (length as usize)
                    .div_ceil(size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>())
            ];
            let status = unsafe {
                PdhGetFormattedCounterArrayW(
                    counter,
                    format,
                    &mut length,
                    &mut count,
                    buffer.as_mut_ptr(),
                )
            };
            if status == PDH_MORE_DATA {
                continue;
            }
            if status != 0 {
                return Err(format!("PdhGetFormattedCounterArrayW data: {status:#x}"));
            }
            let mut array = parse_array(&buffer, length as usize, count as usize, decode)?;
            if let Some(expanded) = expanded {
                reconcile_instances(&mut array, expanded);
            }
            // Between audits use this collection's CStatus, never a cached value/instance set.
            if array.rows.iter().any(|row| row.value.is_err()) {
                self.next_expansion[index] = 0;
            }
            return Ok(array);
        }
        Err("counter_instances_changed_during_three_buffer_attempts".into())
    }

    fn expand(&mut self, kind: Counter, counter: PDH_HCOUNTER) -> Result<CounterArray<()>, String> {
        let index = kind as usize;
        if self.localized_paths[index].is_none() {
            let mut length = 0;
            let status =
                unsafe { PdhGetCounterInfoW(counter, false, &mut length, ptr::null_mut()) };
            if status != PDH_MORE_DATA {
                return Err(format!("PdhGetCounterInfoW size: {status:#x}"));
            }
            if length < size_of::<PDH_COUNTER_INFO_W>() as u32 || length > MAX_BUFFER_BYTES {
                return Err("counter_info_buffer_budget_exceeded".into());
            }
            let mut buffer = vec![
                PDH_COUNTER_INFO_W::default();
                (length as usize).div_ceil(size_of::<PDH_COUNTER_INFO_W>())
            ];
            let status =
                unsafe { PdhGetCounterInfoW(counter, false, &mut length, buffer.as_mut_ptr()) };
            if status != 0 {
                return Err(format!("PdhGetCounterInfoW data: {status:#x}"));
            }
            self.localized_paths[index] = Some(localized_path(&buffer, length as usize)?);
        }
        let path = self.localized_paths[index].as_ref().unwrap();
        for _ in 0..3 {
            let mut length = 0;
            let status = unsafe {
                PdhExpandWildCardPathW(
                    ptr::null(),
                    path.as_ptr(),
                    ptr::null_mut(),
                    &mut length,
                    PDH_REFRESHCOUNTERS,
                )
            };
            if status != PDH_MORE_DATA {
                return Err(format!("PdhExpandWildCardPathW size: {status:#x}"));
            }
            if length == 0 || length > MAX_BUFFER_BYTES / 2 {
                return Err("expanded_counter_paths_budget_exceeded".into());
            }
            let mut buffer = vec![0u16; length as usize];
            let status = unsafe {
                PdhExpandWildCardPathW(
                    ptr::null(),
                    path.as_ptr(),
                    buffer.as_mut_ptr(),
                    &mut length,
                    PDH_REFRESHCOUNTERS,
                )
            };
            if status == PDH_MORE_DATA {
                continue;
            }
            if status != 0 {
                return Err(format!("PdhExpandWildCardPathW data: {status:#x}"));
            }
            let paths = buffer
                .get(..length as usize)
                .ok_or("expanded_counter_paths_bounds")?;
            return expanded_instances(paths);
        }
        Err("counter_instances_changed_during_three_expansion_attempts".into())
    }

    pub(super) fn sample(
        &mut self,
        snapshot: &mut Snapshot,
        costs: &mut SamplingCosts,
        channels: SampleChannels,
    ) {
        self.expansion_ms = 0.0;
        self.expansions = 0;
        self.sample_sequence = self.sample_sequence.saturating_add(1);
        let started = Instant::now();
        let collected = self.collect(channels);
        costs.pdh_collection_ms = started.elapsed().as_secs_f64() * 1000.0;
        if let Err(reason) = collected {
            snapshot.pdh_unknowns.push(unknown("pdh", &reason));
            snapshot.gpu = Observation::Unknown {
                reason: reason.clone(),
            };
            snapshot.disk_io = Observation::Unknown {
                reason: reason.clone(),
            };
            if let Observation::Observed { value } = &mut snapshot.memory {
                value.unknowns.push(unknown("memory_lists", &reason));
            }
            return;
        }
        if let Observation::Observed { value: memory } = &mut snapshot.memory {
            let standby = [
                Counter::StandbyCore,
                Counter::StandbyNormal,
                Counter::StandbyReserve,
            ]
            .into_iter()
            .try_fold(0u64, |sum, kind| {
                sum.checked_add(self.scalar(kind, PDH_FMT_LARGE | PDH_FMT_NOSCALE, bytes)?)
                    .ok_or_else(|| "standby_counter_sum_overflow".into())
            });
            memory.standby_bytes = observe("standby_bytes", standby, &mut memory.unknowns);
            memory.modified_bytes = observe(
                "modified_bytes",
                self.scalar(Counter::Modified, PDH_FMT_LARGE | PDH_FMT_NOSCALE, bytes),
                &mut memory.unknowns,
            );
            memory.free_bytes = observe(
                "free_bytes",
                self.scalar(Counter::Free, PDH_FMT_LARGE | PDH_FMT_NOSCALE, bytes),
                &mut memory.unknowns,
            );
        }
        if channels.system {
            snapshot.system_cpu_busy_fraction = observe(
                "system_cpu_busy_fraction",
                self.scalar(Counter::Cpu, DOUBLE_FORMAT, double)
                    .map(|v| (v / 100.0).clamp(0.0, 1.0)),
                &mut snapshot.pdh_unknowns,
            );
        }
        if channels.disks {
            let disks = disk_rows(
                [
                    self.array(Counter::DiskRead, DOUBLE_FORMAT, double),
                    self.array(Counter::DiskWrite, DOUBLE_FORMAT, double),
                    self.array(Counter::DiskIdle, DOUBLE_FORMAT, double),
                    self.array(Counter::DiskQueue, DOUBLE_FORMAT, double),
                ],
                self.interval,
                snapshot,
            );
            snapshot.disk_io = disks;
        }
        if channels.gpu {
            let gpus = gpu_rows(
                self.array(Counter::GpuEngine, DOUBLE_FORMAT, double),
                self.array(
                    Counter::GpuDedicated,
                    PDH_FMT_LARGE | PDH_FMT_NOSCALE,
                    bytes,
                ),
                self.array(Counter::GpuShared, PDH_FMT_LARGE | PDH_FMT_NOSCALE, bytes),
                snapshot,
            );
            snapshot.gpu = gpus;
        }
        costs.wildcard_expansion_ms = self.expansion_ms;
        costs.wildcard_expansions = self.expansions;
    }
}

fn expanded_instances(paths: &[u16]) -> Result<CounterArray<()>, String> {
    if !paths.ends_with(&[0, 0]) {
        return Err("unterminated_expanded_counter_paths".into());
    }
    let mut result = CounterArray {
        rows: vec![],
        malformed: 0,
    };
    for units in paths
        .split(|unit| *unit == 0)
        .filter(|units| !units.is_empty())
    {
        let instance = String::from_utf16(units).ok().and_then(|path| {
            let (before_counter, counter) = path.rsplit_once('\\')?;
            if counter.is_empty() {
                return None;
            }
            let (_, instance) = before_counter.rsplit_once('(')?;
            instance
                .strip_suffix(')')
                .filter(|instance| !instance.is_empty())
                .map(str::to_owned)
        });
        match instance {
            Some(instance) if instance == "_Total" => {}
            Some(instance) => result.rows.push(CounterRow {
                instance,
                value: Ok(()),
            }),
            None => result.malformed += 1,
        }
        if result.rows.len() > MAX_INSTANCES {
            return Err("expanded_counter_instance_budget_exceeded".into());
        }
    }
    Ok(result)
}

fn observe<T>(field: &str, value: Result<T, String>, unknowns: &mut Vec<Unknown>) -> Option<T> {
    match value {
        Ok(value) => Some(value),
        Err(reason) => {
            unknowns.push(unknown(field, &reason));
            None
        }
    }
}

fn record_array<T>(
    field: &str,
    values: Result<CounterArray<T>, String>,
    snapshot: &mut Snapshot,
) -> Result<CounterArray<T>, String> {
    match &values {
        Ok(array) if array.malformed > 0 => {
            snapshot.pdh_malformed_instances += array.malformed;
            snapshot.pdh_unknowns.push(unknown(
                field,
                &format!("skipped_malformed_instances: {}", array.malformed),
            ));
        }
        Err(reason) => snapshot.pdh_unknowns.push(unknown(field, reason)),
        _ => {}
    }
    values
}

fn disk_rows(
    arrays: [Result<CounterArray<f64>, String>; 4],
    interval: Option<f64>,
    snapshot: &mut Snapshot,
) -> Observation<Vec<DiskIo>> {
    let fields = [
        "read_bytes_per_s",
        "write_bytes_per_s",
        "busy_fraction",
        "queue_length",
    ];
    let mut disks: BTreeMap<String, DiskIo> = BTreeMap::new();
    let mut errors = [None, None, None, None];
    for (index, array) in arrays.into_iter().enumerate() {
        let field = fields[index];
        match record_array(field, array, snapshot) {
            Ok(array) => {
                for row in array.rows {
                    let disk = disks.entry(row.instance.clone()).or_insert_with(|| DiskIo {
                        disk_label: row.instance,
                        sample_interval_seconds: interval,
                        ..Default::default()
                    });
                    let value = observe(field, row.value, &mut disk.unknowns);
                    match index {
                        0 => disk.read_bytes_per_s = value,
                        1 => disk.write_bytes_per_s = value,
                        2 => disk.busy_fraction = value.map(|v| (1.0 - v / 100.0).clamp(0.0, 1.0)),
                        _ => disk.queue_length = value,
                    }
                }
            }
            Err(reason) => errors[index] = Some(reason),
        }
    }
    if disks.is_empty() {
        return Observation::Unknown {
            reason: "no_physical_disk_instances_observed; see pdh_unknowns".into(),
        };
    }
    for disk in disks.values_mut() {
        for (index, value) in [
            disk.read_bytes_per_s,
            disk.write_bytes_per_s,
            disk.busy_fraction,
            disk.queue_length,
        ]
        .into_iter()
        .enumerate()
        {
            if value.is_none() && !disk.unknowns.iter().any(|u| u.field == fields[index]) {
                disk.unknowns.push(unknown(
                    fields[index],
                    errors[index]
                        .as_deref()
                        .unwrap_or("instance_missing_from_counter"),
                ));
            }
        }
    }
    Observation::Observed {
        value: disks.into_values().collect(),
    }
}

fn gpu_instance(instance: &str, engine: bool) -> Option<(String, Option<String>)> {
    let (prefix, rest) = if engine {
        instance.split_once("_luid_")?
    } else {
        ("", instance.strip_prefix("luid_")?)
    };
    if engine {
        prefix.strip_prefix("pid_")?.parse::<u32>().ok()?;
    }
    let (high, rest) = rest.split_once('_')?;
    let (low, rest) = rest.split_once("_phys_")?;
    for half in [high, low] {
        if half.len() != 10 {
            return None;
        }
        u32::from_str_radix(half.strip_prefix("0x")?, 16).ok()?;
    }
    let engine_type = if engine {
        let (physical, rest) = rest.split_once("_eng_")?;
        physical.parse::<u32>().ok()?;
        let (number, kind) = rest.split_once("_engtype_")?;
        number.parse::<u32>().ok()?;
        if kind.len() > 128 {
            return None;
        }
        Some(kind.to_owned())
    } else {
        rest.parse::<u32>().ok()?;
        None
    };
    Some((
        format!(
            "luid_{}_{}",
            high.to_ascii_lowercase(),
            low.to_ascii_lowercase()
        ),
        engine_type,
    ))
}

fn gpu_rows(
    engines: Result<CounterArray<f64>, String>,
    dedicated: Result<CounterArray<u64>, String>,
    shared: Result<CounterArray<u64>, String>,
    snapshot: &mut Snapshot,
) -> Observation<Vec<GpuAdapter>> {
    let mut adapters: BTreeMap<String, GpuAdapter> = BTreeMap::new();
    let mut types: BTreeMap<(String, String), Result<f64, String>> = BTreeMap::new();
    let engines = record_array("gpu_engines", engines, snapshot);
    let mut engine_error = engines.as_ref().err().cloned();
    if let Ok(array) = engines {
        if array.malformed > 0 {
            engine_error = Some("unidentified_malformed_engine_instance".into());
        }
        for row in array.rows {
            let Some((id, Some(kind))) = gpu_instance(&row.instance, true) else {
                snapshot.pdh_malformed_instances += 1;
                engine_error = Some("unidentified_malformed_engine_instance".into());
                continue;
            };
            adapters.entry(id.clone()).or_insert_with(|| GpuAdapter {
                adapter_id: id.clone(),
                ..Default::default()
            });
            let sum = types.entry((id, kind.clone())).or_insert(Ok(0.0));
            *sum = if kind.is_empty() {
                Err("engine_type_unavailable".into())
            } else {
                sum.clone().and_then(|sum| row.value.map(|v| sum + v))
            };
        }
    }
    let mut memory_errors = [None, None];
    for (index, array) in [dedicated, shared].into_iter().enumerate() {
        let field = if index == 0 {
            "dedicated_used_bytes"
        } else {
            "shared_used_bytes"
        };
        let array = record_array(field, array, snapshot);
        memory_errors[index] = array.as_ref().err().cloned();
        if let Ok(array) = array {
            if array.malformed > 0 {
                memory_errors[index] = Some("unidentified_malformed_adapter_instance".into());
            }
            let mut totals: BTreeMap<String, Result<u64, String>> = BTreeMap::new();
            for row in array.rows {
                let Some((id, _)) = gpu_instance(&row.instance, false) else {
                    snapshot.pdh_malformed_instances += 1;
                    memory_errors[index] = Some("unidentified_malformed_adapter_instance".into());
                    continue;
                };
                let sum = totals.entry(id).or_insert(Ok(0));
                *sum = sum.clone().and_then(|sum| {
                    row.value.and_then(|v| {
                        sum.checked_add(v)
                            .ok_or_else(|| "gpu_memory_sum_overflow".into())
                    })
                });
            }
            for (id, value) in totals {
                let adapter = adapters.entry(id.clone()).or_insert_with(|| GpuAdapter {
                    adapter_id: id,
                    ..Default::default()
                });
                let value = observe(field, value, &mut adapter.unknowns);
                if index == 0 {
                    adapter.dedicated_used_bytes = value;
                } else {
                    adapter.shared_used_bytes = value;
                }
            }
        }
    }
    if adapters.is_empty() {
        return Observation::Unknown {
            reason: "no_gpu_adapter_instances_observed; see pdh_unknowns".into(),
        };
    }
    if let Some(reason) = &engine_error {
        snapshot.pdh_unknowns.push(unknown("gpu_engines", reason));
    }
    for (index, field) in ["dedicated_used_bytes", "shared_used_bytes"]
        .into_iter()
        .enumerate()
    {
        if let Some(reason) = &memory_errors[index] {
            snapshot.pdh_unknowns.push(unknown(field, reason));
        }
    }
    for ((id, kind), sum) in types {
        let adapter = adapters.get_mut(&id).unwrap();
        let mut engine = GpuEngine {
            engine_type: (!kind.is_empty()).then_some(kind),
            ..Default::default()
        };
        if engine.engine_type.is_none() {
            engine
                .unknowns
                .push(unknown("engine_type", "engine_type_unavailable"));
        }
        engine.busy_fraction = observe(
            "busy_fraction",
            sum.map(|v| (v / 100.0).clamp(0.0, 1.0)),
            &mut engine.unknowns,
        );
        adapter.engines.push(engine);
    }
    for adapter in adapters.values_mut() {
        adapter.unknowns.push(unknown(
            "dedicated_total_bytes",
            "capacity_not_provided_by_gpu_usage_counters",
        ));
        for (index, field) in ["dedicated_used_bytes", "shared_used_bytes"]
            .into_iter()
            .enumerate()
        {
            let value = if index == 0 {
                &mut adapter.dedicated_used_bytes
            } else {
                &mut adapter.shared_used_bytes
            };
            if memory_errors[index].is_some() {
                *value = None;
            }
            if value.is_none() && !adapter.unknowns.iter().any(|u| u.field == field) {
                adapter.unknowns.push(unknown(
                    field,
                    memory_errors[index]
                        .as_deref()
                        .unwrap_or("instance_missing_from_counter"),
                ));
            }
        }
        if engine_error.is_none()
            && !adapter.engines.is_empty()
            && adapter.engines.iter().all(|e| e.busy_fraction.is_some())
        {
            adapter.engine_busy_fraction = adapter
                .engines
                .iter()
                .filter_map(|e| e.busy_fraction)
                .reduce(f64::max);
        } else {
            let reason = engine_error
                .as_deref()
                .or_else(|| {
                    adapter
                        .engines
                        .iter()
                        .flat_map(|e| &e.unknowns)
                        .find(|u| u.field == "busy_fraction")
                        .map(|u| u.reason.as_str())
                })
                .unwrap_or("missing_or_invalid_engine_instances");
            adapter
                .unknowns
                .push(unknown("engine_busy_fraction", reason));
        }
    }
    Observation::Observed {
        value: adapters.into_values().collect(),
    }
}

fn localized_path(buffer: &[PDH_COUNTER_INFO_W], length: usize) -> Result<Vec<u16>, String> {
    if buffer.is_empty() || length > size_of_val(buffer) {
        return Err("invalid_counter_info_size".into());
    }
    let address = buffer[0].szFullPath as usize;
    let base = buffer.as_ptr() as usize;
    // DataBuffer is a flexible-array member: its first string can start before sizeof(struct).
    let offset = address
        .checked_sub(base)
        .filter(|offset| {
            *offset >= std::mem::offset_of!(PDH_COUNTER_INFO_W, DataBuffer)
                && *offset < length
                && offset % 2 == 0
        })
        .ok_or("invalid_localized_counter_path")?;
    let path = unsafe { std::slice::from_raw_parts(buffer[0].szFullPath, (length - offset) / 2) };
    let end = path
        .iter()
        .position(|unit| *unit == 0)
        .ok_or("unterminated_localized_counter_path")?;
    Ok(path[..=end].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(rows: &[(&str, u32, f64)]) -> (Vec<PDH_FMT_COUNTERVALUE_ITEM_W>, usize) {
        let names: Vec<Vec<u16>> = rows
            .iter()
            .map(|(name, _, _)| name.encode_utf16().chain([0]).collect())
            .collect();
        let header_bytes = rows.len() * size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        let length = header_bytes + names.iter().map(|name| name.len() * 2).sum::<usize>();
        let mut buffer = vec![
            PDH_FMT_COUNTERVALUE_ITEM_W::default();
            length.div_ceil(size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>())
        ];
        let mut offset = header_bytes;
        for (index, ((_, status, value), name)) in rows.iter().zip(names).enumerate() {
            let address = unsafe { buffer.as_mut_ptr().cast::<u8>().add(offset).cast::<u16>() };
            unsafe { ptr::copy_nonoverlapping(name.as_ptr(), address, name.len()) };
            buffer[index].szName = address;
            buffer[index].FmtValue.CStatus = *status;
            buffer[index].FmtValue.Anonymous.doubleValue = *value;
            offset += name.len() * 2;
        }
        (buffer, length)
    }

    fn array<T>(rows: Vec<(&str, Result<T, &str>)>) -> Result<CounterArray<T>, String> {
        Ok(CounterArray {
            rows: rows
                .into_iter()
                .map(|(instance, value)| CounterRow {
                    instance: instance.into(),
                    value: value.map_err(str::to_owned),
                })
                .collect(),
            malformed: 0,
        })
    }

    #[test]
    fn wildcard_audit_waits_ten_samples_and_status_failure_requests_retry() {
        let mut sampler = PdhSampler::default();
        sampler.sample_sequence = 1;
        assert!(sampler.expansion_due(Counter::GpuEngine));
        sampler.next_expansion[Counter::GpuEngine as usize] = 11;
        for sequence in 2..11 {
            sampler.sample_sequence = sequence;
            assert!(!sampler.expansion_due(Counter::GpuEngine));
        }
        sampler.sample_sequence = 11;
        assert!(sampler.expansion_due(Counter::GpuEngine));
        sampler.next_expansion[Counter::GpuEngine as usize] = 21;
        sampler.next_expansion[Counter::GpuEngine as usize] = 0;
        assert!(sampler.expansion_due(Counter::GpuEngine));
    }

    #[test]
    fn malformed_instance_skips_only_it_and_is_counted() {
        let (mut buffer, length) = buffer(&[
            ("0 X:", PDH_CSTATUS_VALID_DATA, 10.0),
            ("bad", PDH_CSTATUS_VALID_DATA, 20.0),
            ("2 Y:", PDH_CSTATUS_NEW_DATA, 30.0),
        ]);
        buffer[1].szName = ptr::null_mut();
        let parsed = parse_array(&buffer, length, 3, double).unwrap();
        assert_eq!(parsed.malformed, 1);
        assert_eq!(parsed.rows.len(), 2);
        assert_eq!(parsed.rows[0].instance, "0 X:");
        assert_eq!(parsed.rows[1].value, Ok(30.0));
        let mut snapshot = unsupported_snapshot();
        let Observation::Observed { value } = disk_rows(
            [
                Ok(parsed),
                Err("unavailable".into()),
                Err("unavailable".into()),
                Err("unavailable".into()),
            ],
            Some(1.0),
            &mut snapshot,
        ) else {
            panic!("valid disks discarded")
        };
        assert_eq!(value.len(), 2);
        assert_eq!(snapshot.pdh_malformed_instances, 1);
        assert_eq!(value[0].read_bytes_per_s, Some(10.0));
        assert!(value[0].write_bytes_per_s.is_none());
        assert!(value[0]
            .unknowns
            .iter()
            .any(|u| u.field == "write_bytes_per_s"));
    }

    #[test]
    fn names_are_bounded_inside_returned_buffer_and_invalid_utf16_is_skipped() {
        let (mut data, length) = buffer(&[
            ("0", 0, 10.0),
            ("1", 0, 20.0),
            ("2", 0, 30.0),
            ("3", 0, 40.0),
        ]);
        data[1].szName = data.as_mut_ptr().cast(); // points into the item headers
        data[2].szName = (data.as_ptr() as usize + length + 2) as *mut u16;
        unsafe { *data[3].szName = 0xd800 }; // unpaired surrogate
        let parsed = parse_array(&data, length, 4, double).unwrap();
        assert_eq!(parsed.rows.len(), 1);
        assert_eq!(parsed.malformed, 3);
        assert!(parse_array(&data, length, 100, double).is_err());
        assert!(parse_array(&data, usize::MAX, 1, double).is_err());
        let (data, length) = buffer(&[("", 0, 1.0), (&"x".repeat(MAX_NAME_UNITS), 0, 2.0)]);
        assert_eq!(parse_array(&data, length, 2, double).unwrap().malformed, 2);
    }

    #[test]
    fn per_value_status_nonfinite_and_negative_are_unknown_not_zero() {
        let (data, length) = buffer(&[
            ("_Total", 0, 999.0),
            ("0", PDH_CSTATUS_NO_INSTANCE, 0.0),
            ("1", 0, f64::NAN),
            ("2", 0, -1.0),
            ("3", PDH_CSTATUS_NEW_DATA, 0.0),
        ]);
        let parsed = parse_array(&data, length, 5, double).unwrap();
        assert_eq!(parsed.rows.len(), 4);
        assert!(parsed.rows[..3].iter().all(|row| row.value.is_err()));
        assert_eq!(parsed.rows[3].value, Ok(0.0));
        let mut value = PDH_FMT_COUNTERVALUE::default();
        value.Anonymous.largeValue = 9_007_199_254_740_993;
        assert_eq!(bytes(&value), Ok(9_007_199_254_740_993));
        value.CStatus = PDH_CSTATUS_NO_INSTANCE;
        assert!(bytes(&value).is_err());
    }

    #[test]
    fn disk_busy_is_idle_complement_not_queue_and_invalid_values_are_local() {
        let mut snapshot = unsupported_snapshot();
        let Observation::Observed { value } = disk_rows(
            [
                array(vec![("0", Ok(10.0)), ("1", Err("new_instance"))]),
                array(vec![("0", Ok(20.0))]),
                array(vec![("0", Ok(25.0)), ("1", Ok(150.0))]),
                array(vec![("0", Ok(150.5)), ("1", Ok(0.0))]),
            ],
            Some(2.0),
            &mut snapshot,
        ) else {
            panic!("no disks")
        };
        assert_eq!(value[0].busy_fraction, Some(0.75));
        assert_eq!(value[0].queue_length, Some(150.5));
        assert_eq!(value[1].busy_fraction, Some(0.0));
        assert!(value[1].read_bytes_per_s.is_none());
        assert!(value[1].write_bytes_per_s.is_none());
        assert_eq!(value[1].unknowns.len(), 2);
    }

    const A: &str = "luid_0x00000000_0x00001234_phys_0";
    const B: &str = "luid_0x00000000_0x00005678_phys_0";
    const A_3D: &str = "pid_7_luid_0x00000000_0x00001234_phys_0_eng_0_engtype_3D";
    const A_3D_2: &str = "pid_8_luid_0x00000000_0x00001234_phys_0_eng_0_engtype_3D";
    const A_COPY: &str = "pid_7_luid_0x00000000_0x00001234_phys_0_eng_1_engtype_Copy";
    const B_3D: &str = "pid_9_luid_0x00000000_0x00005678_phys_0_eng_0_engtype_3D";

    #[test]
    fn gpu_joins_by_luid_sums_types_then_takes_max_and_uses_global_memory() {
        let mut snapshot = unsupported_snapshot();
        let Observation::Observed { value } = gpu_rows(
            array(vec![
                (A_3D, Ok(40.0)),
                (A_3D_2, Ok(40.0)),
                (A_COPY, Ok(60.0)),
                (B_3D, Ok(125.0)),
            ]),
            array(vec![(A, Ok(1024)), (B, Ok(2048))]),
            array(vec![(A, Ok(512)), (B, Ok(256))]),
            &mut snapshot,
        ) else {
            panic!("no adapters")
        };
        assert_eq!(value.len(), 2);
        assert_eq!(value[0].adapter_id, "luid_0x00000000_0x00001234");
        assert_eq!(value[0].engine_busy_fraction, Some(0.8));
        assert_eq!(value[0].engines[0].busy_fraction, Some(0.8));
        assert_eq!(value[1].engine_busy_fraction, Some(1.0));
        assert_eq!(value[0].dedicated_used_bytes, Some(1024));
        assert_eq!(value[0].shared_used_bytes, Some(512));
        assert!(value[0].dedicated_total_bytes.is_none());
    }

    fn assert_gpu_instance_unknown(rows: &[(&str, u32, f64)], expanded: &[&str], reason: &str) {
        let (data, length) = buffer(rows);
        let mut engines = parse_array(&data, length, rows.len(), double).unwrap();
        let paths: Vec<u16> = expanded
            .iter()
            .flat_map(|instance| {
                format!("\\GPU Engine({instance})\\Utilization Percentage\0")
                    .encode_utf16()
                    .collect::<Vec<_>>()
            })
            .chain([0])
            .collect();
        reconcile_instances(&mut engines, expanded_instances(&paths).unwrap());
        let mut snapshot = unsupported_snapshot();
        let Observation::Observed { value } = gpu_rows(
            Ok(engines),
            array(vec![(A, Ok(1024)), (B, Ok(2048))]),
            array(vec![(A, Ok(512)), (B, Ok(256))]),
            &mut snapshot,
        ) else {
            panic!("no adapters")
        };
        assert_eq!(value.len(), 2);
        assert!(value[0].engine_busy_fraction.is_none());
        assert!(value[0]
            .unknowns
            .iter()
            .any(|u| u.field == "engine_busy_fraction" && u.reason == reason));
        let engine = value[0]
            .engines
            .iter()
            .find(|e| e.engine_type.as_deref() == Some("3D"))
            .unwrap();
        assert!(engine.busy_fraction.is_none());
        assert!(engine
            .unknowns
            .iter()
            .any(|u| u.field == "busy_fraction" && u.reason == reason));
        let copy = value[0]
            .engines
            .iter()
            .find(|e| e.engine_type.as_deref() == Some("Copy"))
            .unwrap();
        assert_eq!(copy.busy_fraction, Some(0.6));
        assert_eq!(value[0].dedicated_used_bytes, Some(1024));
        assert_eq!(value[0].shared_used_bytes, Some(512));
        assert_eq!(value[1].engine_busy_fraction, Some(0.2));
        assert_eq!(value[1].engines[0].busy_fraction, Some(0.2));
        assert_eq!(snapshot.pdh_malformed_instances, 0);
        assert!(snapshot.pdh_unknowns.is_empty());
    }

    #[test]
    fn removed_gpu_instance_makes_only_its_type_and_adapter_unknown() {
        assert_gpu_instance_unknown(
            &[
                (A_3D, 0, 40.0),
                (A_3D_2, 0, 40.0),
                (A_COPY, 0, 60.0),
                (B_3D, 0, 20.0),
            ],
            &[A_3D, A_COPY, B_3D],
            "instance_churn",
        );
    }

    #[test]
    fn new_gpu_instance_makes_only_its_type_and_adapter_unknown() {
        assert_gpu_instance_unknown(
            &[(A_3D, 0, 40.0), (A_COPY, 0, 60.0), (B_3D, 0, 20.0)],
            &[A_3D, A_3D_2, A_COPY, B_3D],
            "instance_churn",
        );
    }

    #[test]
    fn invalid_gpu_instance_status_makes_only_its_type_and_adapter_unknown() {
        assert_gpu_instance_unknown(
            &[
                (A_3D, 0, 40.0),
                (A_3D_2, PDH_CSTATUS_NO_INSTANCE, 0.0),
                (A_COPY, 0, 60.0),
                (B_3D, 0, 20.0),
            ],
            &[A_3D, A_3D_2, A_COPY, B_3D],
            &format!("invalid_counter_sample: {PDH_CSTATUS_NO_INSTANCE:#x}"),
        );
    }

    #[test]
    fn invalid_gpu_value_makes_aggregate_unknown_and_churn_has_no_stale_adapter() {
        let mut snapshot = unsupported_snapshot();
        let Observation::Observed { value } = gpu_rows(
            array(vec![
                (A_3D, Ok(20.0)),
                (A_3D_2, Err("no_instance")),
                (A_COPY, Ok(90.0)),
            ]),
            array(vec![(A, Ok(1024))]),
            Err("denied".into()),
            &mut snapshot,
        ) else {
            panic!("no adapters")
        };
        assert!(value[0].engine_busy_fraction.is_none());
        assert!(value[0].engines[0].busy_fraction.is_none());
        assert_eq!(value[0].dedicated_used_bytes, Some(1024));
        assert!(value[0].shared_used_bytes.is_none());
        let Observation::Observed { value } = gpu_rows(
            array(vec![(B_3D, Ok(10.0))]),
            array(vec![(B, Ok(2048))]),
            array(vec![(B, Ok(256))]),
            &mut snapshot,
        ) else {
            panic!("no adapters")
        };
        assert_eq!(value.len(), 1);
        assert_eq!(value[0].adapter_id, "luid_0x00000000_0x00005678");
        assert_eq!(value[0].engine_busy_fraction, Some(0.1));
    }

    #[test]
    fn unnamed_gpu_engine_is_unknown_only_on_its_adapter() {
        let mut snapshot = unsupported_snapshot();
        let unnamed = "pid_7_luid_0x00000000_0x00001234_phys_0_eng_5_engtype_";
        let Observation::Observed { value } = gpu_rows(
            array(vec![(A_3D, Ok(10.0)), (unnamed, Ok(5.0)), (B_3D, Ok(20.0))]),
            array(vec![(A, Ok(100)), (B, Ok(200))]),
            array(vec![(A, Ok(50)), (B, Ok(10))]),
            &mut snapshot,
        ) else {
            panic!("no adapters")
        };
        assert_eq!(snapshot.pdh_malformed_instances, 0);
        assert!(value[0].engine_busy_fraction.is_none());
        assert!(value[0].engines[0].engine_type.is_none());
        assert!(value[0].engines[0]
            .unknowns
            .iter()
            .any(|u| u.field == "engine_type" && u.reason == "engine_type_unavailable"));
        assert_eq!(value[1].engine_busy_fraction, Some(0.2));
    }

    #[test]
    fn malformed_gpu_instance_cannot_become_a_zero_or_partial_total() {
        let mut snapshot = unsupported_snapshot();
        let Observation::Observed { value } = gpu_rows(
            array(vec![(A_3D, Ok(10.0)), ("pid_7_luid_BAD", Ok(0.0))]),
            array(vec![(A, Ok(100)), ("luid_BAD", Ok(200))]),
            array(vec![(A, Ok(50))]),
            &mut snapshot,
        ) else {
            panic!("no adapters")
        };
        assert_eq!(value.len(), 1);
        assert_eq!(snapshot.pdh_malformed_instances, 2);
        assert!(value[0].engine_busy_fraction.is_none());
        assert!(value[0].dedicated_used_bytes.is_none());
        assert_eq!(value[0].shared_used_bytes, Some(50));
        for instance in [
            "pid_1_luid_0x0000000_0x00001234_phys_0_eng_0_engtype_3D",
            "pid_bad_luid_0x00000000_0x00001234_phys_0_eng_0_engtype_3D",
            "pid_1_luid_0x00000000_0x00001234_phys_0_eng_bad_engtype_3D",
        ] {
            assert!(gpu_instance(instance, true).is_none());
        }
    }

    #[test]
    fn wildcard_expansion_is_fresh_and_bad_paths_do_not_discard_good_instances() {
        let paths: Vec<u16> = "\\host\\PhysicalDisk(0 X:)\\Disk Read Bytes/sec\0bad\0\\host\\PhysicalDisk(_Total)\\Disk Read Bytes/sec\0\0".encode_utf16().collect();
        let first = expanded_instances(&paths).unwrap();
        assert_eq!(first.rows.len(), 1);
        assert_eq!(first.rows[0].instance, "0 X:");
        assert_eq!(first.malformed, 1);
        let paths: Vec<u16> = "\\host\\PhysicalDisk(2 Y:)\\Disk Read Bytes/sec\0\0"
            .encode_utf16()
            .collect();
        let second = expanded_instances(&paths).unwrap();
        assert_eq!(second.rows.len(), 1);
        assert_eq!(second.rows[0].instance, "2 Y:");
        assert!(expanded_instances(&paths[..paths.len() - 1]).is_err());
    }

    #[test]
    fn localized_path_accepts_flexible_member_and_checks_buffer_bounds() {
        let path: Vec<u16> = "\\PhysicalDisk(*)\\Disk Read Bytes/sec\0"
            .encode_utf16()
            .collect();
        let offset = std::mem::offset_of!(PDH_COUNTER_INFO_W, DataBuffer);
        let length = offset + path.len() * 2;
        let mut buffer =
            vec![PDH_COUNTER_INFO_W::default(); length.div_ceil(size_of::<PDH_COUNTER_INFO_W>())];
        let address = unsafe { buffer.as_mut_ptr().cast::<u8>().add(offset).cast::<u16>() };
        unsafe { ptr::copy_nonoverlapping(path.as_ptr(), address, path.len()) };
        buffer[0].szFullPath = address;
        assert_eq!(localized_path(&buffer, length).unwrap(), path);
        assert!(localized_path(&buffer, length - 2).is_err());
        buffer[0].szFullPath = buffer.as_mut_ptr().cast();
        assert!(localized_path(&buffer, length).is_err());
    }
}
