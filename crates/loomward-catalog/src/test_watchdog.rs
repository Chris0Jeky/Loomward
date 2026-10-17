//! Test-only hang watchdog (#495). A catalogue test that outlives its deadline ends the test
//! process instead of stalling CI, and on Linux first prints every thread's kernel state and Rust
//! backtrace: the hosted ubuntu runners froze this binary twice without a trace, and the freeze has
//! never reproduced anywhere a debugger was attached.
use crossbeam_channel::{bounded, RecvTimeoutError, Sender};
use std::time::Duration;

/// Ends the test process once the calling test outlives five minutes (these tests take seconds),
/// after reporting every thread. Drop the returned sender when the test ends.
#[must_use]
pub(crate) fn watchdog() -> Sender<()> {
    let name = std::thread::current()
        .name()
        .unwrap_or("catalog test")
        .to_owned();
    let (done, wait) = bounded::<()>(0);
    std::thread::spawn(move || {
        if wait.recv_timeout(Duration::from_secs(300)) == Err(RecvTimeoutError::Timeout) {
            // The report below is best effort (its signal handler is not async-signal-safe); this
            // backstop aborts regardless, so a stuck report can never turn a hang back into a hang.
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_secs(30));
                std::process::abort();
            });
            // Straight to stderr: test output capture would swallow `eprintln!` from this thread.
            let mut report = format!("{name} hung for 5 minutes (#495); every thread follows\n");
            #[cfg(target_os = "linux")]
            report.push_str(&linux::thread_report(|_| true));
            report.push_str("aborting the test process\n");
            let _ = std::io::Write::write_all(&mut std::io::stderr(), report.as_bytes());
            std::process::abort();
        }
    });
    done
}

#[cfg(target_os = "linux")]
pub(crate) mod linux {
    use std::backtrace::Backtrace;
    use std::fmt::Write;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// Filled by the signal handler, one entry per interrupted thread: (name, tid, backtrace).
    static CAPTURED: Mutex<Vec<(String, i32, Backtrace)>> = Mutex::new(Vec::new());

    /// Captures the interrupted thread's stack. Not async-signal-safe (it allocates): it runs only
    /// on the way to an abort, or in this module's own test, and every wait on it is bounded.
    extern "C" fn capture(_: libc::c_int) {
        let name = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .to_owned();
        let backtrace = Backtrace::force_capture();
        if let Ok(mut captured) = CAPTURED.lock() {
            captured.push((name, unsafe { libc::gettid() }, backtrace));
        }
    }

    fn read(tid: &str, file: &str) -> String {
        std::fs::read_to_string(format!("/proc/self/task/{tid}/{file}"))
            .unwrap_or_default()
            .trim()
            .to_owned()
    }

    /// The run state, which follows the parenthesised command name: R running, S sleeping, D disk.
    fn state(tid: &str) -> String {
        let stat = read(tid, "stat");
        stat.rsplit_once(") ")
            .map_or("?", |(_, rest)| &rest[..1])
            .to_owned()
    }

    /// Kernel state (run state, wait channel, current syscall) of every thread of this process,
    /// then the Rust backtrace of each thread `wanted` selects, waiting at most two seconds each.
    pub(crate) fn thread_report(wanted: impl Fn(i32) -> bool) -> String {
        let mut report = String::new();
        let me = unsafe { libc::gettid() };
        let tids: Vec<String> = std::fs::read_dir("/proc/self/task")
            .map(|dir| {
                dir.filter_map(|e| e.ok()?.file_name().into_string().ok())
                    .collect()
            })
            .unwrap_or_default();
        for tid in &tids {
            let _ = writeln!(
                report,
                "thread {tid} {:?} state {} wchan {:?} syscall {:?}",
                read(tid, "comm"),
                state(tid),
                read(tid, "wchan"),
                read(tid, "syscall"),
            );
        }
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = capture as extern "C" fn(libc::c_int) as libc::sighandler_t;
            // Restart interrupted syscalls, so a thread that is only waiting carries on unharmed.
            action.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(libc::SIGUSR2, &action, std::ptr::null_mut());
        }
        let pid = unsafe { libc::getpid() };
        for tid in tids.iter().filter_map(|t| t.parse::<i32>().ok()) {
            if tid == me || !wanted(tid) {
                continue;
            }
            let before = CAPTURED.lock().map_or(0, |c| c.len());
            unsafe { libc::syscall(libc::SYS_tgkill, pid, tid, libc::SIGUSR2) };
            let give_up = Instant::now() + Duration::from_secs(2);
            while CAPTURED.lock().map_or(0, |c| c.len()) == before && Instant::now() < give_up {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        if let Ok(mut captured) = CAPTURED.lock() {
            for (name, tid, backtrace) in captured.drain(..) {
                let _ = writeln!(report, "--- thread {tid} {name:?}\n{backtrace}");
            }
        }
        report
    }

    #[test]
    fn the_report_names_a_blocked_thread_and_where_it_waits() {
        use crossbeam_channel::{bounded, Receiver, Sender};
        fn parked_for_the_watchdog_report(tid: Sender<i32>, release: Receiver<()>) {
            tid.send(unsafe { libc::gettid() }).unwrap();
            let _ = release.recv_timeout(Duration::from_secs(30));
        }
        let (send, receive) = bounded(1);
        let (release, wait) = bounded(0);
        let parked = std::thread::Builder::new()
            .name("parked-495".into())
            .spawn(move || parked_for_the_watchdog_report(send, wait))
            .unwrap();
        let tid = receive.recv_timeout(Duration::from_secs(30)).unwrap();
        // The send returns before the thread blocks: wait until the kernel has it asleep.
        let give_up = Instant::now() + Duration::from_secs(30);
        while state(&tid.to_string()) != "S"
            || !read(&tid.to_string(), "wchan").starts_with("futex")
        {
            assert!(Instant::now() < give_up, "the parked thread never blocked");
            std::thread::sleep(Duration::from_millis(10));
        }
        let report = thread_report(|t| t == tid);
        assert!(
            report.contains(&format!("thread {tid} \"parked-495\" state S")),
            "{report}"
        );
        assert!(
            report.contains("parked_for_the_watchdog_report"),
            "{report}"
        );
        // Only the selected thread is interrupted.
        assert_eq!(report.matches("--- thread").count(), 1, "{report}");
        drop(release);
        parked.join().unwrap();
    }
}
