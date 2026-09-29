//! litestd's monotonic clock and timed waits past 2^31 seconds, beyond a
//! 32-bit `time_t`, in a child of this program started in a new time
//! namespace whose monotonic clock runs that far ahead. There the C
//! library's 32-bit calls fail with `EOVERFLOW`, and litestd must use the
//! 64-bit system calls instead.
//!
//! This test has its own `main`: std's test harness reads the monotonic
//! clock, which panics in the child where `time_t` has 32 bits. Where user
//! or time namespaces are unavailable, or the system forbids setting the
//! clock offsets, it says so and passes.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    reason = "a failure fails the test"
)]

/// Set in the child.
#[cfg(all(target_os = "linux", not(miri)))]
const CHILD: &str = "LITESTD_TIME_NAMESPACE_CHILD";

/// How far ahead the child's monotonic clock runs: past 2^31 seconds.
#[cfg(all(target_os = "linux", not(miri)))]
const OFFSET_SECS: u64 = 3_000_000_000;

fn main() {
    #[cfg(all(target_os = "linux", not(miri)))]
    if std::env::var_os(CHILD).is_some() {
        child();
    } else {
        parent();
    }
}

/// Starts the child in a new time namespace, from a new user namespace that
/// grants the right to create it, and waits for it with a deadline.
#[cfg(all(target_os = "linux", not(miri)))]
fn parent() {
    use core::time::Duration;
    use std::{process::Command, thread, time::Instant};
    // Only this process, which runs nothing else, enters the new user
    // namespace; the time namespace is for the children it creates next.
    let flags = libc::CLONE_NEWUSER | libc::CLONE_NEWTIME;
    // SAFETY: `unshare` touches no memory, and this process has the single
    // thread that a new user namespace requires.
    if unsafe { libc::unshare(flags) } != 0 {
        let err = std::io::Error::last_os_error();
        println!("time_namespace: skipped, no time namespace: {err}");
        return;
    }
    let offsets = format!("monotonic {OFFSET_SECS} 0\n");
    // Ubuntu's AppArmor confines a process in a new unprivileged user
    // namespace, which then cannot set the offsets.
    if let Err(err) = std::fs::write("/proc/self/timens_offsets", offsets) {
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied, "{err}");
        println!("time_namespace: skipped, clock offsets denied: {err}");
        return;
    }
    let mut child = Command::new(std::env::current_exe().unwrap())
        .env(CHILD, "1")
        .spawn()
        .unwrap();
    // A timed wait that never ends would hang the child.
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > Duration::from_secs(60) {
            child.kill().unwrap();
            panic!("the child hangs");
        }
        thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success(), "{status}");
    println!("time_namespace: all checks passed");
}

/// The checks, with litestd alone: std's `Instant` may fail here.
#[cfg(all(target_os = "linux", not(miri)))]
fn child() {
    use core::time::Duration;

    use litestd::{
        sync::{Condvar, Mutex},
        thread,
        time::Instant,
    };
    let now = format!("{:?}", Instant::now());
    let secs = now
        .strip_prefix("Instant { tv_sec: ")
        .and_then(|rest| rest.split(',').next())
        .and_then(|secs| secs.parse::<u64>().ok());
    assert!(secs.is_some_and(|secs| secs >= OFFSET_SECS), "{now}");

    let wait = Duration::from_millis(20);
    let start = Instant::now();
    thread::sleep(wait);
    assert!(start.elapsed() >= wait);

    let lock = Mutex::new(());
    let cvar = Condvar::new();
    let start = Instant::now();
    let (_, result) = cvar
        .wait_timeout_while(lock.lock().unwrap(), wait, |()| true)
        .unwrap();
    assert!(result.timed_out());
    assert!(start.elapsed() >= wait);

    // `park_timeout` may return early, but not every time.
    let start = Instant::now();
    for _ in 0..100 {
        let Some(left) = wait.checked_sub(start.elapsed()) else {
            break;
        };
        thread::park_timeout(left);
    }
    assert!(start.elapsed() >= wait);
}
