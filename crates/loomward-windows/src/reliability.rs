//! Reliability counters for one volume (LW-137, #876): NVMe SMART / Health log through a
//! non-elevated, metadata-only, time-bounded `IOCTL_STORAGE_QUERY_PROPERTY`. Evidence, never a
//! health verdict. Device identity strings are not part of this log page and are never read.
use crate::storage_path::ReliabilityCounters;
use std::{
    io,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

/// Size of the NVMe SMART / Health Information log page (NVMe Base Specification).
const LOG_BYTES: usize = 512;
/// `STORAGE_PROPERTY_QUERY` up to its `AdditionalParameters`, then `STORAGE_PROTOCOL_SPECIFIC_DATA`.
const REQUEST_BYTES: usize = 8 + PROTOCOL_DATA_BYTES;
const PROTOCOL_DATA_BYTES: usize = 40;
/// `STORAGE_PROTOCOL_DATA_DESCRIPTOR`: version, size, then the echoed protocol data.
const DESCRIPTOR_BYTES: usize = 8 + PROTOCOL_DATA_BYTES;
const PROTOCOL_TYPE_NVME: u32 = 3;
const DATA_TYPE_LOG_PAGE: u32 = 2;
const LOG_PAGE_HEALTH_INFO: u32 = 2;
const STORAGE_DEVICE_PROTOCOL_SPECIFIC_PROPERTY: u32 = 50;

/// How long the health query may take before the volume row reports it as unobserved.
pub(crate) const QUERY_BUDGET: Duration = Duration::from_secs(2);
/// A query that outlives its budget keeps its worker thread until the driver answers; at most this
/// many may be stuck at once, then further queries are refused instead of piling up threads.
const MAX_STUCK: usize = 4;

fn put(buf: &mut [u8], at: usize, value: u32) {
    buf[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn get(buf: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(buf[at..at + 4].try_into().expect("four bytes"))
}

/// The standard query asking the storage stack for NVMe log page 2 (health information).
pub(crate) fn health_request() -> [u8; REQUEST_BYTES] {
    let mut q = [0u8; REQUEST_BYTES];
    put(&mut q, 0, STORAGE_DEVICE_PROTOCOL_SPECIFIC_PROPERTY); // PropertyId; QueryType 0 = standard
    put(&mut q, 8, PROTOCOL_TYPE_NVME);
    put(&mut q, 12, DATA_TYPE_LOG_PAGE);
    put(&mut q, 16, LOG_PAGE_HEALTH_INFO);
    put(&mut q, 24, PROTOCOL_DATA_BYTES as u32); // ProtocolDataOffset, from the protocol data
    put(&mut q, 28, LOG_BYTES as u32); // ProtocolDataLength
    q
}

/// Size of the response buffer a caller must provide for [`health_request`].
pub(crate) const RESPONSE_BYTES: usize = DESCRIPTOR_BYTES + LOG_BYTES;

fn invalid(what: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what)
}

/// The log page inside a `STORAGE_PROTOCOL_DATA_DESCRIPTOR` response of `returned` valid bytes.
pub(crate) fn log_page(response: &[u8], returned: usize) -> io::Result<&[u8]> {
    let response = response
        .get(..returned)
        .ok_or_else(|| invalid("storage response exceeds its buffer"))?;
    if response.len() < DESCRIPTOR_BYTES || (get(response, 4) as usize) < DESCRIPTOR_BYTES {
        return Err(invalid("short protocol descriptor"));
    }
    if get(response, 8) != PROTOCOL_TYPE_NVME || get(response, 12) != DATA_TYPE_LOG_PAGE {
        return Err(invalid("response is not an NVMe log page"));
    }
    let offset = get(response, 24) as usize;
    let length = get(response, 28) as usize;
    let start = offset
        .checked_add(8)
        .filter(|_| offset >= PROTOCOL_DATA_BYTES)
        .ok_or_else(|| invalid("log page offset overlaps the descriptor"))?;
    let end = start
        .checked_add(length)
        .ok_or_else(|| invalid("log page bounds overflow"))?;
    match response.get(start..end) {
        Some(page) if page.len() >= LOG_BYTES => Ok(&page[..LOG_BYTES]),
        _ => Err(invalid("short NVMe health log")),
    }
}

fn counter(page: &[u8], at: usize) -> Option<u64> {
    // 128-bit little-endian; a value that does not fit is unknown, never saturated.
    u64::try_from(u128::from_le_bytes(page[at..at + 16].try_into().ok()?)).ok()
}

/// Map the NVMe SMART / Health log page to counters. Fields the log has no counterpart for stay
/// `None`: the log reports one aggregate media-error count, not separate read and write errors, so
/// that count is not presented as either.
pub(crate) fn counters_from_log(page: &[u8]) -> io::Result<ReliabilityCounters> {
    if page.len() < LOG_BYTES {
        return Err(invalid("short NVMe health log"));
    }
    if page[..LOG_BYTES].iter().all(|b| *b == 0) {
        // A drive always reports a composite temperature, so an all-zero page is a driver stub.
        return Err(invalid("NVMe health log is all zero"));
    }
    let kelvin = u32::from(u16::from_le_bytes([page[1], page[2]]));
    Ok(ReliabilityCounters {
        // 0 K means not reported; anything below 273 K cannot be expressed as unsigned Celsius.
        temperature_celsius: kelvin.checked_sub(273),
        power_on_hours: counter(page, 128),
        // NVMe "Percentage Used" (endurance estimate); may exceed 100, reported as is.
        wear_percent: Some(u32::from(page[5])),
        // The nearest NVMe counterpart of start/stop cycles is Power Cycles.
        start_stop_cycles: counter(page, 112),
        ..Default::default()
    })
}

/// Why a counter query reported nothing, without device identifiers.
pub(crate) fn failure_reason(error: &io::Error) -> String {
    const ACCESS_DENIED: i32 = 5;
    const INVALID_FUNCTION: i32 = 1;
    const NOT_SUPPORTED: i32 = 50;
    const INVALID_PARAMETER: i32 = 87;
    match (error.raw_os_error(), error.kind()) {
        (Some(ACCESS_DENIED), _) => {
            "NVMe health log refused without administrator rights; Loomward never elevates".into()
        }
        (Some(INVALID_FUNCTION | NOT_SUPPORTED | INVALID_PARAMETER), _) => {
            "this device or driver does not expose an NVMe health log".into()
        }
        (_, io::ErrorKind::TimedOut) => format!(
            "NVMe health log query exceeded {} s; the device may be asleep or removed",
            QUERY_BUDGET.as_secs()
        ),
        (_, io::ErrorKind::InvalidData) => "NVMe health log response was malformed".into(),
        (_, kind) => format!("NVMe health log query failed ({kind})"),
    }
}

/// Reason for the buses where no non-elevated counter query exists. `bus` is the volume's bus name.
pub(crate) fn no_query_reason(bus: &str) -> String {
    match bus {
        "usb" => "USB bridge: device health is hidden unless the bridge passes it through, which Loomward does not attempt".into(),
        // Measured (evidence/v3/storage-path-lw137.md): the NVMe-log and temperature property
        // queries are refused by these buses' drivers. ATA SMART pass-through is not attempted.
        "sata" | "ata" | "atapi" | "scsi" | "sas" | "raid" => {
            format!("{bus}: no unelevated health query is supported by the driver; SMART pass-through is not attempted because Loomward never elevates")
        }
        "sd" | "virtual" | "file_backed_virtual" | "spaces" | "iscsi" | "mmc" | "fibre" | "1394" | "ssa" => {
            format!("{bus}: no device-level reliability counters are exposed to an unelevated query")
        }
        _ => "bus type unknown: reliability counters not queried".into(),
    }
}

/// Run `work` on a worker thread and wait at most `limit` for it. The worker is detached on timeout
/// (an IOCTL cannot be cancelled from here); `stuck` counts workers still running so a sleeping or
/// removed device cannot accumulate threads.
pub(crate) fn bounded<T: Send + 'static>(
    stuck: &'static AtomicUsize,
    limit: Duration,
    work: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    struct Slot(&'static AtomicUsize);
    impl Drop for Slot {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    if stuck.fetch_add(1, Ordering::SeqCst) >= MAX_STUCK {
        stuck.fetch_sub(1, Ordering::SeqCst);
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "earlier health queries are still waiting on their devices",
        ));
    }
    let slot = Slot(stuck);
    let (tx, rx) = mpsc::channel();
    // A failed spawn drops the closure and with it the slot.
    thread::Builder::new()
        .name("loomward-health-query".into())
        .spawn(move || {
            let _slot = slot;
            let _ = tx.send(work());
        })?;
    rx.recv_timeout(limit)
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "health query timed out"))?
}

/// NVMe counters for the volume `guid` (`\\?\Volume{...}\`), bounded by [`QUERY_BUDGET`]. The
/// volume is opened with access 0 and sent one read-only property query: no data is read from it.
#[cfg(windows)]
pub(crate) fn nvme_counters(guid: &str) -> io::Result<ReliabilityCounters> {
    static STUCK: AtomicUsize = AtomicUsize::new(0);
    let path = guid.trim_end_matches('\\').to_owned();
    bounded(&STUCK, QUERY_BUDGET, move || query_native(&path))
}

#[cfg(windows)]
fn query_native(path: &str) -> io::Result<ReliabilityCounters> {
    use std::{path::Path, ptr};
    use windows_sys::Win32::{
        Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE},
        System::{Ioctl::IOCTL_STORAGE_QUERY_PROPERTY, IO::DeviceIoControl},
    };
    let handle = crate::win::open(
        Path::new(path),
        0,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
    )?;
    let request = health_request();
    let mut response = [0u8; RESPONSE_BYTES];
    let mut returned = 0;
    if unsafe {
        DeviceIoControl(
            handle.0,
            IOCTL_STORAGE_QUERY_PROPERTY,
            request.as_ptr().cast(),
            request.len() as u32,
            response.as_mut_ptr().cast(),
            response.len() as u32,
            &mut returned,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    counters_from_log(log_page(&response, returned as usize)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{storage_path::HealthObservation, volumes::Fact};
    use std::sync::atomic::AtomicUsize;

    /// Synthetic descriptor: invented values only.
    fn response(page: &[u8; LOG_BYTES]) -> Vec<u8> {
        let mut r = vec![0u8; RESPONSE_BYTES];
        put(&mut r, 0, 1);
        put(&mut r, 4, DESCRIPTOR_BYTES as u32);
        put(&mut r, 8, PROTOCOL_TYPE_NVME);
        put(&mut r, 12, DATA_TYPE_LOG_PAGE);
        put(&mut r, 24, PROTOCOL_DATA_BYTES as u32);
        put(&mut r, 28, LOG_BYTES as u32);
        r[DESCRIPTOR_BYTES..].copy_from_slice(page);
        r
    }
    fn page() -> [u8; LOG_BYTES] {
        let mut p = [0u8; LOG_BYTES];
        p[1..3].copy_from_slice(&310u16.to_le_bytes()); // 37 C
        p[5] = 7;
        p[112..120].copy_from_slice(&41u64.to_le_bytes());
        p[128..136].copy_from_slice(&1234u64.to_le_bytes());
        p[160..168].copy_from_slice(&9u64.to_le_bytes()); // media errors: deliberately unmapped
        p
    }

    #[test]
    fn request_asks_for_the_nvme_health_log_page() {
        let q = health_request();
        assert_eq!(get(&q, 0), 50);
        assert_eq!(get(&q, 4), 0, "standard query, not exists/mask");
        assert_eq!((get(&q, 8), get(&q, 12), get(&q, 16)), (3, 2, 2));
        assert_eq!((get(&q, 24), get(&q, 28)), (40, 512));
    }

    #[test]
    fn log_page_maps_available_fields_and_leaves_the_rest_unknown() {
        let bytes = response(&page());
        let counters = counters_from_log(log_page(&bytes, bytes.len()).unwrap()).unwrap();
        assert_eq!(
            counters,
            ReliabilityCounters {
                temperature_celsius: Some(37),
                power_on_hours: Some(1234),
                wear_percent: Some(7),
                start_stop_cycles: Some(41),
                ..Default::default()
            }
        );
        // The aggregate media-error count is neither a read nor a write error total.
        assert!(counters.read_errors_total.is_none() && counters.write_errors_total.is_none());
        assert!(matches!(
            HealthObservation::from_counters(Fact::Known { value: counters }),
            HealthObservation::CountersPresent { .. }
        ));
    }

    #[test]
    fn measured_zero_counters_are_kept_and_unreported_temperature_is_unknown() {
        let mut p = [0u8; LOG_BYTES];
        p[5] = 0; // a new drive: 0 % used, 0 hours, 0 cycles
        p[1..3].copy_from_slice(&0u16.to_le_bytes());
        p[3] = 100; // available spare, only so the page is not all zero
        let c = counters_from_log(&p).unwrap();
        assert_eq!(c.temperature_celsius, None);
        assert_eq!((c.power_on_hours, c.wear_percent), (Some(0), Some(0)));
        p[1..3].copy_from_slice(&200u16.to_le_bytes()); // below 273 K does not fit an unsigned C
        assert_eq!(counters_from_log(&p).unwrap().temperature_celsius, None);
    }

    #[test]
    fn counters_too_large_for_the_field_are_unknown_not_saturated() {
        let mut p = page();
        p[128 + 8] = 1; // power-on hours above u64
        let c = counters_from_log(&p).unwrap();
        assert_eq!(c.power_on_hours, None);
        assert_eq!(c.start_stop_cycles, Some(41));
    }

    #[test]
    fn an_all_zero_page_is_a_stub_not_a_measurement() {
        assert!(counters_from_log(&[0u8; LOG_BYTES]).is_err());
        let health = match counters_from_log(&[0u8; LOG_BYTES]) {
            Ok(value) => Fact::Known { value },
            Err(e) => Fact::Unknown {
                reason: failure_reason(&e),
            },
        };
        assert!(matches!(
            HealthObservation::from_counters(health),
            HealthObservation::UnsupportedOrHidden { reason } if reason.contains("malformed")
        ));
    }

    #[test]
    fn malformed_responses_are_rejected() {
        let good = response(&page());
        assert!(
            log_page(&good, good.len() + 1).is_err(),
            "returned > buffer"
        );
        assert!(
            log_page(&good, DESCRIPTOR_BYTES + 100).is_err(),
            "short page"
        );
        assert!(log_page(&good, 10).is_err(), "short descriptor");
        let edit = |at: usize, v: u32| {
            let mut r = good.clone();
            put(&mut r, at, v);
            r
        };
        for bad in [
            edit(8, 2),         // ATA, not NVMe
            edit(12, 1),        // identify, not a log page
            edit(4, 8),         // descriptor claims to be shorter than its header
            edit(24, 8),        // data offset overlaps the descriptor
            edit(24, u32::MAX), // offset overflow
            edit(28, 100),      // log shorter than a health page
            edit(28, u32::MAX), // length overflow
        ] {
            assert!(log_page(&bad, bad.len()).is_err());
        }
    }

    #[test]
    fn failure_reasons_are_specific_and_carry_no_identifiers() {
        let reason = |e: io::Error| failure_reason(&e);
        assert!(reason(io::Error::from_raw_os_error(5)).contains("never elevates"));
        for code in [1, 50, 87] {
            assert!(reason(io::Error::from_raw_os_error(code)).contains("does not expose"));
        }
        assert!(reason(io::ErrorKind::TimedOut.into()).contains("exceeded 2 s"));
        assert!(no_query_reason("usb").contains("USB bridge"));
        assert!(no_query_reason("sata").contains("never elevates"));
        assert!(no_query_reason("unknown").contains("not queried"));
        assert!(no_query_reason("something-new").contains("not queried"));
    }

    #[test]
    fn a_slow_query_times_out_and_stuck_workers_are_capped() {
        static STUCK: AtomicUsize = AtomicUsize::new(0);
        let (release, hold) = mpsc::channel::<()>();
        let hold = std::sync::Arc::new(std::sync::Mutex::new(hold));
        let started = std::time::Instant::now();
        for _ in 0..MAX_STUCK {
            let hold = hold.clone();
            let error = bounded(&STUCK, Duration::from_millis(30), move || {
                let _ = hold.lock().unwrap().recv();
                Ok(())
            })
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        }
        assert!(started.elapsed() < Duration::from_secs(5));
        // The cap is reached: refused immediately, without starting another thread.
        let error = bounded(&STUCK, Duration::from_millis(30), || Ok(())).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        drop(release); // workers unblock and free their slots
        for _ in 0..200 {
            if STUCK.load(Ordering::SeqCst) == 0 {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(STUCK.load(Ordering::SeqCst), 0);
        assert_eq!(
            bounded(&STUCK, Duration::from_secs(2), || Ok(5)).unwrap(),
            5
        );
    }
}
