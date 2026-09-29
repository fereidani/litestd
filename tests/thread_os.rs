//! Access to a `litestd::thread::JoinHandle`'s OS thread against std's:
//! `os::unix::thread::JoinHandleExt` on Unix, and the `AsRawHandle` and
//! `IntoRawHandle` impls on Windows. Each scenario runs with both crates and
//! must end the same way.

#![cfg(all(feature = "thread", any(unix, windows)))]

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};

use litestd::sync::Arc;

/// A closure result that counts its drops.
struct Counted(Arc<AtomicUsize>);

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.fetch_add(1, SeqCst);
    }
}

/// What a scenario observed: whether the thread ran, how often its result
/// was dropped, and what the OS reported.
#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    ran: bool,
    drops: usize,
    os: i64,
}

/// A thread body that records that it ran and returns a counted result.
fn body(
    ran: &Arc<AtomicBool>,
    drops: &Arc<AtomicUsize>,
) -> impl FnOnce() -> Counted + Send + 'static {
    let (ran, drops) = (Arc::clone(ran), Arc::clone(drops));
    move || {
        ran.store(true, SeqCst);
        Counted(drops)
    }
}

#[cfg(unix)]
mod unix {
    use core::ptr;
    use std::os::unix::thread::JoinHandleExt as _;

    use litestd::os::unix::thread::{JoinHandleExt as _, RawPthread};

    use super::{Arc, AtomicBool, AtomicUsize, Outcome, SeqCst, body};

    /// The calling thread's id, as `JoinHandleExt` returns it.
    #[allow(clippy::unnecessary_cast, reason = "a pointer on musl")]
    fn self_id() -> RawPthread {
        // SAFETY: `pthread_self` has no preconditions.
        (unsafe { libc::pthread_self() }) as RawPthread
    }

    #[allow(clippy::unnecessary_cast, reason = "a pointer on musl")]
    const fn to_libc(raw: RawPthread) -> libc::pthread_t {
        raw as libc::pthread_t
    }

    /// Joins a thread whose ownership a join handle handed over.
    fn join(raw: RawPthread) -> i64 {
        // SAFETY: the thread was handed over neither joined nor detached,
        // and is joined once.
        i64::from(unsafe { libc::pthread_join(to_libc(raw), ptr::null_mut()) })
    }

    /// Detaches a thread whose ownership a join handle handed over.
    #[cfg(not(miri))]
    fn detach(raw: RawPthread) -> i64 {
        // SAFETY: as for `join`.
        i64::from(unsafe { libc::pthread_detach(to_libc(raw)) })
    }

    /// Runs `$scenario` with the given crate's `thread` module.
    macro_rules! outcome {
        ($krate:ident, $scenario:ident) => {{
            let ran = Arc::new(AtomicBool::new(false));
            let drops = Arc::new(AtomicUsize::new(0));
            let handle = $krate::thread::spawn(body(&ran, &drops));
            let os = $scenario(handle, &drops);
            Outcome {
                ran: ran.load(SeqCst),
                drops: drops.load(SeqCst),
                os,
            }
        }};
    }

    #[test]
    fn raw_pthread_is_stds_type() {
        let handle = litestd::thread::spawn(|| {});
        let raw: std::os::unix::thread::RawPthread = handle.as_pthread_t();
        assert_eq!(raw, handle.as_pthread_t());
        handle.join().unwrap();
    }

    #[test]
    fn as_pthread_t_names_the_thread() {
        let lite = litestd::thread::spawn(self_id);
        let real = std::thread::spawn(self_id);
        let (lite_raw, real_raw) = (lite.as_pthread_t(), real.as_pthread_t());
        assert_eq!(lite.join().unwrap(), lite_raw);
        assert_eq!(real.join().unwrap(), real_raw);
        assert_ne!(lite_raw, self_id());
    }

    #[test]
    fn into_pthread_t_hands_over_a_joinable_thread() {
        macro_rules! scenario {
            ($krate:ident) => {{
                fn run(
                    handle: $krate::thread::JoinHandle<super::Counted>,
                    _: &AtomicUsize,
                ) -> i64 {
                    join(handle.into_pthread_t())
                }
                outcome!($krate, run)
            }};
        }
        let lite = scenario!(litestd);
        assert_eq!(lite, scenario!(std));
        // Joined, the result dropped before the thread ended.
        assert_eq!(
            lite,
            Outcome {
                ran: true,
                drops: 1,
                os: 0
            }
        );
    }

    #[test]
    fn into_pthread_t_after_the_thread_finished_drops_the_result() {
        macro_rules! scenario {
            ($krate:ident) => {{
                fn run(
                    handle: $krate::thread::JoinHandle<super::Counted>,
                    drops: &AtomicUsize,
                ) -> i64 {
                    // Waits for the thread to give up its share of the
                    // result, which it does right after storing it.
                    while !handle.is_finished() {
                        $krate::thread::yield_now();
                    }
                    assert_eq!(drops.load(SeqCst), 0);
                    let raw = handle.into_pthread_t();
                    // The handle held the last share of the result.
                    assert_eq!(drops.load(SeqCst), 1);
                    join(raw)
                }
                outcome!($krate, run)
            }};
        }
        let lite = scenario!(litestd);
        assert_eq!(lite, scenario!(std));
        assert_eq!(
            lite,
            Outcome {
                ran: true,
                drops: 1,
                os: 0
            }
        );
    }

    // Miri reports threads still running when the process exits, and a
    // detached thread may be.
    #[cfg(not(miri))]
    #[test]
    fn into_pthread_t_hands_over_a_detachable_thread() {
        macro_rules! scenario {
            ($krate:ident) => {{
                fn run(
                    handle: $krate::thread::JoinHandle<super::Counted>,
                    drops: &AtomicUsize,
                ) -> i64 {
                    let os = detach(handle.into_pthread_t());
                    // The thread drops its result once it has stored it.
                    while drops.load(SeqCst) == 0 {
                        $krate::thread::yield_now();
                    }
                    os
                }
                outcome!($krate, run)
            }};
        }
        let lite = scenario!(litestd);
        assert_eq!(lite, scenario!(std));
        assert_eq!(
            lite,
            Outcome {
                ran: true,
                drops: 1,
                os: 0
            }
        );
    }

    #[test]
    fn as_pthread_t_names_a_finished_thread_until_join() {
        macro_rules! scenario {
            ($krate:ident) => {{
                let handle = $krate::thread::spawn(self_id);
                while !handle.is_finished() {
                    $krate::thread::yield_now();
                }
                let raw = handle.as_pthread_t();
                handle.join().unwrap() == raw
            }};
        }
        assert!(scenario!(litestd));
        assert!(scenario!(std));
    }
}

#[cfg(all(windows, any(feature = "fs", feature = "net")))]
mod windows {
    use std::os::windows::io::{AsRawHandle as _, IntoRawHandle as _};

    use litestd::os::windows::io::{
        AsRawHandle as _, IntoRawHandle as _, RawHandle,
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, WAIT_OBJECT_0},
        System::Threading::{
            GetCurrentThreadId, GetExitCodeThread, GetThreadId, INFINITE,
            WaitForSingleObject,
        },
    };

    use super::{Arc, AtomicBool, AtomicUsize, Outcome, SeqCst, body};

    fn current_id() -> u32 {
        // SAFETY: `GetCurrentThreadId` has no preconditions.
        unsafe { GetCurrentThreadId() }
    }

    fn thread_id(handle: RawHandle) -> u32 {
        // SAFETY: `handle` is an open thread handle.
        unsafe { GetThreadId(handle) }
    }

    /// Waits for a thread whose handle a join handle handed over, reads its
    /// exit code and closes the handle. Returns the exit code, or -1 if a
    /// call failed.
    fn wait_and_close(handle: RawHandle) -> i64 {
        // SAFETY: the handle was handed over open, and is closed once, last.
        unsafe {
            let waited = WaitForSingleObject(handle, INFINITE) == WAIT_OBJECT_0;
            let mut code = u32::MAX;
            let read = GetExitCodeThread(handle, &raw mut code) != 0;
            let closed = CloseHandle(handle) != 0;
            if waited && read && closed {
                i64::from(code)
            } else {
                -1
            }
        }
    }

    #[test]
    fn raw_handle_is_stds_type() {
        let handle = litestd::thread::spawn(|| {});
        let raw: std::os::windows::io::RawHandle = handle.as_raw_handle();
        assert_eq!(raw, handle.as_raw_handle());
        handle.join().unwrap();
    }

    #[test]
    fn as_raw_handle_names_the_thread() {
        let lite = litestd::thread::spawn(current_id);
        let real = std::thread::spawn(current_id);
        let lite_id = thread_id(lite.as_raw_handle());
        let real_id = thread_id(real.as_raw_handle());
        assert_eq!(lite.join().unwrap(), lite_id);
        assert_eq!(real.join().unwrap(), real_id);
        assert_ne!(lite_id, current_id());
    }

    #[test]
    fn into_raw_handle_hands_over_the_handle() {
        macro_rules! scenario {
            ($krate:ident) => {{
                let ran = Arc::new(AtomicBool::new(false));
                let drops = Arc::new(AtomicUsize::new(0));
                let handle = $krate::thread::spawn(body(&ran, &drops));
                let os = wait_and_close(handle.into_raw_handle());
                Outcome {
                    ran: ran.load(SeqCst),
                    drops: drops.load(SeqCst),
                    os,
                }
            }};
        }
        let lite = scenario!(litestd);
        assert_eq!(lite, scenario!(std));
        // The thread exited with code 0 after dropping its result.
        assert_eq!(
            lite,
            Outcome {
                ran: true,
                drops: 1,
                os: 0
            }
        );
    }

    #[test]
    fn into_raw_handle_after_the_thread_finished_drops_the_result() {
        macro_rules! scenario {
            ($krate:ident) => {{
                let ran = Arc::new(AtomicBool::new(false));
                let drops = Arc::new(AtomicUsize::new(0));
                let handle = $krate::thread::spawn(body(&ran, &drops));
                while !handle.is_finished() {
                    $krate::thread::yield_now();
                }
                assert_eq!(drops.load(SeqCst), 0);
                let raw = handle.into_raw_handle();
                assert_eq!(drops.load(SeqCst), 1);
                let os = wait_and_close(raw);
                Outcome {
                    ran: ran.load(SeqCst),
                    drops: drops.load(SeqCst),
                    os,
                }
            }};
        }
        let lite = scenario!(litestd);
        assert_eq!(lite, scenario!(std));
        assert_eq!(
            lite,
            Outcome {
                ran: true,
                drops: 1,
                os: 0
            }
        );
    }
}
