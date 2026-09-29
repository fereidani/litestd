//! Spawning, joining, naming, stack sizes and thread identity.

#![cfg(feature = "thread")]

use core::{
    cell::Cell,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};
use std::{collections::HashSet, sync::mpsc, time::Instant};

use litestd::{
    rc::Rc,
    sync::Arc,
    thread::{self, Builder, JoinHandle, Scope, ScopedJoinHandle, Thread},
};

/// Threads per test; Miri is slow.
const THREADS: usize = if cfg!(miri) { 3 } else { 32 };

/// Spins until `done` returns true, yielding in between, or panics after a
/// generous timeout.
fn wait_until(mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < Duration::from_secs(30), "timed out");
        thread::yield_now();
    }
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn spawn_returns_the_closure_result() {
    let handle = thread::spawn(|| String::from("result"));
    assert_eq!(handle.join().unwrap(), "result");

    let unit = thread::spawn(|| {});
    unit.join().unwrap();

    let big = thread::spawn(|| [7u8; 4096]);
    assert!(big.join().unwrap().iter().all(|&b| b == 7));
}

#[test]
// All threads are spawned before the first join.
#[allow(clippy::needless_collect)]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn many_threads_join_with_their_results() {
    let handles: Vec<JoinHandle<usize>> =
        (0..THREADS).map(|i| thread::spawn(move || i * 2)).collect();
    for (i, handle) in handles.into_iter().enumerate() {
        assert_eq!(handle.join().unwrap(), i * 2);
    }
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn closure_and_result_are_dropped_once() {
    struct Counted(Arc<AtomicUsize>);
    impl Drop for Counted {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    // The closure's captures are dropped on the thread.
    let drops = Arc::new(AtomicUsize::new(0));
    let captured = Counted(drops.clone());
    thread::spawn(move || drop(captured)).join().unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);

    // A joined result belongs to the joiner.
    let drops = Arc::new(AtomicUsize::new(0));
    let result = {
        let drops = drops.clone();
        thread::spawn(move || Counted(drops)).join().unwrap()
    };
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(result);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn detached_results_are_dropped_by_the_last_owner() {
    struct Counted(Arc<AtomicUsize>);
    impl Drop for Counted {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    // Handle dropped while the thread still runs: the thread drops it.
    let drops = Arc::new(AtomicUsize::new(0));
    let go = Arc::new(AtomicBool::new(false));
    let handle = {
        let (drops, go) = (drops.clone(), go.clone());
        thread::spawn(move || {
            wait_until(|| go.load(Ordering::SeqCst));
            Counted(drops)
        })
    };
    drop(handle);
    go.store(true, Ordering::SeqCst);
    wait_until(|| drops.load(Ordering::SeqCst) == 1);

    // Handle dropped after the thread finished: the handle drops it.
    let drops = Arc::new(AtomicUsize::new(0));
    let handle = {
        let drops = drops.clone();
        thread::spawn(move || Counted(drops))
    };
    wait_until(|| handle.is_finished());
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(handle);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn detached_thread_keeps_running() {
    let (tx, rx) = mpsc::channel();
    let go = Arc::new(AtomicBool::new(false));
    let handle = {
        let go = go.clone();
        thread::spawn(move || {
            wait_until(|| go.load(Ordering::SeqCst));
            tx.send(thread::current().id()).unwrap();
        })
    };
    let id = handle.thread().id();
    drop(handle);
    go.store(true, Ordering::SeqCst);
    assert_eq!(rx.recv().unwrap(), id);
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn is_finished_turns_true_once_the_closure_returned() {
    let go = Arc::new(AtomicBool::new(false));
    let handle = {
        let go = go.clone();
        thread::spawn(move || wait_until(|| go.load(Ordering::SeqCst)))
    };
    assert!(!handle.is_finished());
    go.store(true, Ordering::SeqCst);
    wait_until(|| handle.is_finished());
    assert!(handle.is_finished());
    handle.join().unwrap();
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn builder_names_are_kept_exactly() {
    let names = [
        "worker",
        "a name longer than fifteen bytes",
        "\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{1f600}",
        "with\0nul",
        "",
    ];
    for name in names {
        let handle = Builder::new()
            .name(name.to_string())
            .spawn(move || {
                assert_eq!(thread::current().name(), Some(name));
                thread::current().name().map(str::to_string)
            })
            .unwrap();
        assert_eq!(handle.thread().name(), Some(name));
        assert_eq!(handle.join().unwrap().as_deref(), Some(name));
    }
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn unnamed_threads_have_no_name() {
    let handle = thread::spawn(|| thread::current().name().map(str::to_string));
    assert_eq!(handle.thread().name(), None);
    assert_eq!(handle.join().unwrap(), None);
}

/// Reads the calling thread's OS name.
#[cfg(all(target_os = "linux", not(miri)))]
fn os_name() -> String {
    // An unreadable file yields an empty name, which fails the comparison.
    let comm =
        std::fs::read_to_string("/proc/thread-self/comm").unwrap_or_default();
    comm.trim_end_matches('\n').to_string()
}

#[cfg(all(target_os = "linux", not(miri)))]
#[test]
fn os_names_are_truncated_on_character_boundaries() {
    let cases = [
        ("worker", "worker"),
        ("exactly15bytes!", "exactly15bytes!"),
        ("sixteen bytes!!!", "sixteen bytes!!"),
        // Two-byte characters: 7 fit in 15 bytes, the 8th would split.
        (
            "\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}",
            "\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}",
        ),
        ("cut\0here", "cut"),
    ];
    for (name, expected) in cases {
        let os = Builder::new()
            .name(name.to_string())
            .spawn(os_name)
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(os, expected, "name {name:?}");
    }
}

/// Reads the calling thread's OS name, or an empty one if that fails.
#[cfg(windows)]
fn os_name() -> String {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        System::Threading::{GetCurrentThread, GetThreadDescription},
    };
    let mut desc = core::ptr::null_mut();
    // SAFETY: the pseudo handle names this thread; `desc` takes the result.
    let hr = unsafe { GetThreadDescription(GetCurrentThread(), &raw mut desc) };
    if hr < 0 || desc.is_null() {
        return String::new();
    }
    // SAFETY: on success `desc` is a NUL-terminated string, which this
    // thread frees once, with `LocalFree`.
    unsafe {
        // Descriptions are `UNICODE_STRING`s, shorter than 2^15 units.
        let len = (0..1 << 15).take_while(|&i| *desc.add(i) != 0).count();
        let name =
            String::from_utf16_lossy(core::slice::from_raw_parts(desc, len));
        LocalFree(desc.cast());
        name
    }
}

/// Reads the calling thread's OS name, or an empty one if that fails.
#[cfg(target_vendor = "apple")]
fn os_name() -> String {
    let mut buf = [0u8; 64];
    // SAFETY: `buf` is valid for writes of its length, and the thread is
    // the calling one.
    let r = unsafe {
        libc::pthread_getname_np(
            libc::pthread_self(),
            buf.as_mut_ptr().cast(),
            buf.len(),
        )
    };
    let name = core::ffi::CStr::from_bytes_until_nul(&buf).unwrap_or_default();
    if r == 0 {
        name.to_str().unwrap_or_default()
    } else {
        ""
    }
    .to_string()
}

/// macOS keeps 63 bytes of a name, cut to whole characters.
#[cfg(target_vendor = "apple")]
#[test]
fn os_names_are_truncated_on_character_boundaries() {
    let cases = [
        ("worker".to_string(), "worker".to_string()),
        ("x".repeat(63), "x".repeat(63)),
        ("y".repeat(64), "y".repeat(63)),
        // Two-byte characters: 31 fit in 63 bytes, the 32nd would split.
        ("\u{e9}".repeat(32), "\u{e9}".repeat(31)),
        ("cut\0here".to_string(), "cut".to_string()),
    ];
    for (name, expected) in cases {
        let os = Builder::new()
            .name(name.clone())
            .spawn(os_name)
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(os, expected, "name {name:?}");
    }
}

#[cfg(windows)]
#[test]
fn os_names_end_at_a_nul_or_the_limit() {
    let cases = [
        ("worker".to_string(), "worker".to_string()),
        ("cut\0here".to_string(), "cut".to_string()),
        ("x".repeat(300), "x".repeat(255)),
        // A surrogate pair that the limit would split is dropped whole.
        (format!("{}\u{1f600}", "x".repeat(254)), "x".repeat(254)),
    ];
    for (name, expected) in cases {
        let os = Builder::new()
            .name(name.clone())
            .spawn(os_name)
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(os, expected, "name {name:?}");
    }
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn stack_sizes_are_honored() {
    // Tiny requests round up to the platform minimum.
    for size in [0, 1, 4096, 64 * 1024] {
        let handle = Builder::new().stack_size(size).spawn(|| 5).unwrap();
        assert_eq!(handle.join().unwrap(), 5);
    }
}

#[cfg(not(miri))]
#[test]
// The frames are meant to be large.
#[allow(clippy::large_stack_arrays)]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn large_stacks_hold_large_frames() {
    fn use_stack(depth: u8) -> u8 {
        let buf = core::hint::black_box([depth; 1024 * 1024]);
        if depth == 0 {
            buf[0]
        } else {
            use_stack(depth - 1).wrapping_add(buf[1])
        }
    }
    // Four frames of 1 MiB do not fit the default 2 MiB stack.
    let handle = Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| use_stack(4))
        .unwrap();
    assert_eq!(handle.join().unwrap(), 10);
}

#[cfg(not(miri))]
#[test]
fn impossible_stack_sizes_are_errors() {
    let result = Builder::new().stack_size(usize::MAX).spawn(|| ());
    assert!(result.is_err());
    let result = Builder::new().stack_size(usize::MAX / 2).spawn(|| ());
    assert!(result.is_err());
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn spawn_unchecked_can_borrow() {
    let data = vec![1, 2, 3];
    let data_ref = &data;
    // SAFETY: the thread is joined before `data` goes out of scope.
    let handle = unsafe {
        Builder::new()
            .spawn_unchecked(move || data_ref.iter().sum::<i32>())
            .unwrap()
    };
    assert_eq!(handle.join().unwrap(), 6);
}

#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "the harness runs tests on the main thread"
)]
fn current_is_stable_on_a_foreign_thread() {
    // The test harness spawned this thread, not litestd.
    let first = thread::current();
    let second = thread::current();
    assert_eq!(first.id(), second.id());
    assert_eq!(first.name(), None);
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn current_matches_the_join_handle() {
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let me = thread::current();
        tx.send((me.id(), thread::current().id())).unwrap();
        me.id()
    });
    let (a, b) = rx.recv().unwrap();
    assert_eq!(a, b);
    assert_eq!(a, handle.thread().id());
    assert_ne!(a, thread::current().id());
    assert_eq!(handle.join().unwrap(), a);
}

/// More `current` handles than a thread makes before it adds them to the
/// shared count, which it does sooner under Miri.
const HANDLES: usize = if cfg!(miri) { 200 } else { 70_000 };

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn current_handles_outlive_their_thread() {
    let (kept, id) = thread::spawn(|| {
        let mut kept = Vec::new();
        for i in 0..HANDLES {
            let me = thread::current();
            if i % 10 == 0 {
                kept.push(me);
            }
        }
        (kept, thread::current().id())
    })
    .join()
    .unwrap();
    assert_eq!(kept.len(), HANDLES / 10);
    assert!(kept.iter().all(|t| t.id() == id));
    kept[0].unpark();
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn current_handles_drop_on_other_threads() {
    let (tx, rx) = mpsc::channel();
    let owner = thread::spawn(move || {
        for _ in 0..HANDLES / 100 {
            tx.send(thread::current()).unwrap();
            // Handles made and dropped here meanwhile.
            for _ in 0..99 {
                drop(thread::current());
            }
        }
        thread::current()
    });
    let received = rx.iter().map(|t| t.id()).collect::<Vec<_>>();
    let last = owner.join().unwrap();
    assert_eq!(received.len(), HANDLES / 100);
    assert!(received.iter().all(|&id| id == last.id()));
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn thread_ids_are_unique() {
    let mut ids: HashSet<_> = (0..THREADS)
        .map(|_| thread::spawn(|| thread::current().id()))
        .collect::<Vec<_>>()
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert_eq!(ids.len(), THREADS);
    assert!(ids.insert(thread::current().id()));
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn debug_output_matches_std() {
    let handle = Builder::new().name("dbg".into()).spawn(|| {}).unwrap();
    let thread = format!("{:?}", handle.thread());
    assert!(thread.starts_with("Thread { id: ThreadId("), "{thread}");
    assert!(thread.ends_with(", name: Some(\"dbg\"), .. }"), "{thread}");
    assert_eq!(format!("{handle:?}"), "JoinHandle { .. }");
    handle.join().unwrap();

    let builder = format!("{:?}", Builder::new().stack_size(1));
    assert!(builder.starts_with("Builder {"), "{builder}");
}

#[test]
fn auto_traits_match_std() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<Thread>();
    send_sync::<Builder>();
    send_sync::<thread::ThreadId>();
    // As in std, a join handle is `Send + Sync` whatever its result type.
    send_sync::<JoinHandle<Rc<u8>>>();
    send_sync::<ScopedJoinHandle<'static, u8>>();
    send_sync::<Scope<'static, 'static>>();
    send_sync::<thread::LocalKey<Cell<u8>>>();
}

#[test]
// All threads are spawned before the first join.
#[allow(clippy::needless_collect)]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn threads_join_in_any_order() {
    let handles: Vec<_> = (0..THREADS)
        .map(|i| {
            let delay = Duration::from_millis(u64::try_from(i % 3).unwrap());
            thread::spawn(move || thread::sleep(delay))
        })
        .collect();
    for handle in handles.into_iter().rev() {
        handle.join().unwrap();
    }
}
