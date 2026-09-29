//! Helpers shared by the networking tests: a watchdog so that no test
//! hangs, comparisons of litestd's errors and results with std's, and the
//! child process in which a test can restore `SIGPIPE`.

#![allow(
    dead_code,
    clippy::panic,
    clippy::redundant_pub_crate,
    clippy::unwrap_used,
    reason = "test helpers, used by some tests each, fail the test on errors"
)]

use core::{fmt::Debug, time::Duration};
use std::{sync::mpsc, thread};

/// How long a test may take before the watchdog fails it.
pub(crate) const TEST_LIMIT: Duration = Duration::from_secs(60);

/// The read and write timeout of the sockets tests block on, so that a
/// blocking call fails instead of hanging.
pub(crate) const IO_LIMIT: Duration = Duration::from_secs(20);

/// Runs `f` on its own thread and fails the test if it does not finish
/// within [`TEST_LIMIT`]. A thread stuck in a blocking call is left behind;
/// the test fails rather than hangs.
pub(crate) fn timed<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> T {
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        // The receiver may be gone after a timeout; nothing to report then.
        let _ = tx.send(f());
    });
    match rx.recv_timeout(TEST_LIMIT) {
        Ok(value) => {
            handle.join().unwrap();
            value
        }
        // The thread panicked: join it to report its panic.
        Err(mpsc::RecvTimeoutError::Disconnected) => match handle.join() {
            Err(payload) => std::panic::resume_unwind(payload),
            Ok(()) => panic!("test thread ended without a result"),
        },
        Err(mpsc::RecvTimeoutError::Timeout) => panic!("test timed out"),
    }
}

/// An error rendered every way a program can observe it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ErrorView {
    kind: String,
    raw: Option<i32>,
    display: String,
    debug: String,
}

pub(crate) fn lite_view(e: &litestd::io::Error) -> ErrorView {
    ErrorView {
        kind: format!("{:?}", e.kind()),
        raw: e.raw_os_error(),
        display: e.to_string(),
        debug: format!("{e:?}"),
    }
}

pub(crate) fn std_view(e: &std::io::Error) -> ErrorView {
    ErrorView {
        kind: format!("{:?}", e.kind()),
        raw: e.raw_os_error(),
        display: e.to_string(),
        debug: format!("{e:?}"),
    }
}

/// Asserts that litestd and std failed alike: same kind, OS code, message
/// and `Debug` output.
#[track_caller]
pub(crate) fn same_error(lite: &litestd::io::Error, std: &std::io::Error) {
    assert_eq!(
        lite_view(lite),
        std_view(std),
        "litestd (left), std (right)"
    );
}

/// Asserts that litestd and std both succeeded with equal values, or failed
/// alike.
#[track_caller]
pub(crate) fn same_result<T: PartialEq + Debug>(
    lite: litestd::io::Result<T>,
    std: std::io::Result<T>,
) {
    match (lite, std) {
        (Ok(a), Ok(b)) => assert_eq!(a, b, "litestd (left), std (right)"),
        (Err(a), Err(b)) => same_error(&a, &b),
        (a, b) => panic!("litestd {a:?}, std {b:?}"),
    }
}

/// The largest 32-bit `time_t`, `i32::MAX` seconds.
const TIME32_MAX: u64 = 2_147_483_647;

/// The timeouts that the round trips set: none, below a microsecond, short
/// and long ones, and ones at and beyond the limit of a 32-bit `time_t`.
pub(crate) const TIMEOUTS: [Option<Duration>; 8] = [
    None,
    Some(Duration::from_nanos(1)),
    Some(Duration::from_micros(1)),
    Some(Duration::from_millis(1)),
    Some(Duration::new(1, 500_000_000)),
    Some(Duration::from_secs(86_400 * 365)),
    Some(Duration::new(TIME32_MAX, 999_999_999)),
    Some(Duration::MAX),
];

/// What std reads back from a 32-bit `time_t` that holds `i32::MIN`: the
/// value sign-extended, 2^64 - 2^31 seconds.
const STD_WRAPPED: Duration = Duration::from_secs(u64::MAX << 31);

/// Asserts that litestd reads back a timeout like std, except where std is
/// wrong. With a 32-bit `time_t` both set at most `i32::MAX` seconds, but std
/// keeps the microseconds there, which the kernel rounds up into 2^31
/// seconds. The old 32-bit `timeval` reports that as `i32::MIN`, which std
/// sign-extends; litestd sets no microseconds at the limit.
#[track_caller]
pub(crate) fn same_timeout(
    lite: litestd::io::Result<Option<Duration>>,
    std: std::io::Result<Option<Duration>>,
) {
    if matches!(std, Ok(Some(d)) if d == STD_WRAPPED) {
        assert_eq!(lite.unwrap(), Some(Duration::from_secs(TIME32_MAX)));
    } else {
        same_result(lite, std);
    }
}

/// What litestd reads from a socket whose timeout std set, given what std
/// reads: the same, except that litestd reads the 2^31 seconds that std
/// sign-extends.
pub(crate) fn lite_reading_of(std: Option<Duration>) -> Option<Duration> {
    if std == Some(STD_WRAPPED) {
        Some(Duration::from_secs(TIME32_MAX + 1))
    } else {
        std
    }
}

/// Keeps children that end abnormally from writing core files into the
/// working tree; they inherit this process's limit.
#[cfg(unix)]
pub(crate) fn forbid_core_files() {
    let limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a valid `rlimit` for the duration of the call.
    let r = unsafe { libc::setrlimit(libc::RLIMIT_CORE, &raw const limit) };
    assert_eq!(r, 0);
}

/// Runs the test named `test` of this test binary in a new process, with
/// `LITESTD_NET_CHILD` set, and returns how it ended.
pub(crate) fn run_child(test: &str) -> std::process::Output {
    #[cfg(unix)]
    forbid_core_files();
    let exe = std::env::current_exe().unwrap();
    std::process::Command::new(exe)
        .args(["--exact", test, "--nocapture", "--test-threads=1", "-q"])
        .env("LITESTD_NET_CHILD", "1")
        .output()
        .unwrap()
}

/// Whether this process is a child that [`run_child`] started.
pub(crate) fn is_child() -> bool {
    std::env::var_os("LITESTD_NET_CHILD").is_some()
}

/// Restores the default action of `SIGPIPE`, which ends the process, as it
/// is in a litestd program; std's runtime ignores it.
#[cfg(unix)]
pub(crate) fn default_sigpipe() {
    // SAFETY: `SIG_DFL` is a valid disposition and nothing else in the
    // child changes signal handling concurrently.
    let old = unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    assert_ne!(old, libc::SIG_ERR);
}

/// Opens a TCP socket with the C library, close-on-exec, which macOS sets
/// after creating it.
#[cfg(unix)]
pub(crate) fn raw_tcp_socket() -> i32 {
    #[cfg(not(target_vendor = "apple"))]
    let ty = libc::SOCK_STREAM | libc::SOCK_CLOEXEC;
    #[cfg(target_vendor = "apple")]
    let ty = libc::SOCK_STREAM;
    // SAFETY: `socket` touches no memory.
    let fd = unsafe { libc::socket(libc::AF_INET, ty, 0) };
    assert!(fd >= 0);
    #[cfg(target_vendor = "apple")]
    {
        // SAFETY: `F_SETFD` touches no memory.
        let r = unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        assert_eq!(r, 0);
    }
    fd
}

/// `127.0.0.1` with `port` as the C library takes it.
#[cfg(unix)]
pub(crate) fn loopback_v4(port: u16) -> libc::sockaddr_in {
    // SAFETY: all zeros is a valid `sockaddr_in`.
    let mut addr: libc::sockaddr_in = unsafe { core::mem::zeroed() };
    // macOS keeps the length in the address.
    #[cfg(target_vendor = "apple")]
    {
        addr.sin_len = u8::try_from(size_of::<libc::sockaddr_in>()).unwrap();
    }
    addr.sin_family = libc::sa_family_t::try_from(libc::AF_INET).unwrap();
    addr.sin_port = port.to_be();
    addr.sin_addr = libc::in_addr {
        s_addr: u32::from_ne_bytes([127, 0, 0, 1]),
    };
    addr
}

/// Waits up to [`IO_LIMIT`] for `fd` to report one of `events`.
#[cfg(unix)]
pub(crate) fn wait_for(fd: i32, events: i16) {
    let mut pollfd = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    let ms = i32::try_from(IO_LIMIT.as_millis()).unwrap();
    // SAFETY: `pollfd` is valid for reads and writes of one entry.
    let n = unsafe { libc::poll(&raw mut pollfd, 1, ms) };
    assert_eq!(n, 1, "poll: {}", std::io::Error::last_os_error());
}
