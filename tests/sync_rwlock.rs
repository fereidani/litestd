//! `RwLock`: shared and exclusive access, writer preference, downgrading,
//! and the accessors.
#![cfg(feature = "sync")]

use core::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst},
    time::Duration,
};
use std::{thread, time::Instant};

use litestd::sync::{RwLock, RwLockWriteGuard, TryLockError};

const THREADS: usize = if cfg!(miri) { 3 } else { 8 };
const ITERS: usize = if cfg!(miri) { 30 } else { 10_000 };

/// Polls `done` until it returns `true`, failing the test after a generous
/// deadline instead of hanging.
fn wait_until(mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    // Ends when `done` holds or the deadline passes.
    while !done() {
        if Instant::now() > deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(1));
    }
    true
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn readers_share_the_lock() {
    let lock = RwLock::new(1);
    let r = lock.read().unwrap();
    thread::scope(|s| {
        s.spawn(|| {
            let r2 = lock.try_read().unwrap();
            assert_eq!(*r2, 1);
            assert!(matches!(lock.try_write(), Err(TryLockError::WouldBlock)));
        });
    });
    drop(r);
    assert!(lock.try_write().is_ok());
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn writer_excludes_everyone() {
    let lock = RwLock::new(1);
    let w = lock.write().unwrap();
    thread::scope(|s| {
        s.spawn(|| {
            assert!(matches!(lock.try_read(), Err(TryLockError::WouldBlock)));
            assert!(lock.try_write().is_err());
        });
    });
    drop(w);
    assert!(lock.try_read().is_ok());
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn readers_never_see_a_partial_write() {
    let lock = RwLock::new((0usize, 0usize));
    thread::scope(|s| {
        for i in 0..THREADS {
            let lock = &lock;
            s.spawn(move || {
                for _ in 0..ITERS {
                    if i % 2 == 0 {
                        let mut w = lock.write().unwrap();
                        w.0 += 1;
                        w.1 += 1;
                        drop(w);
                    } else {
                        let r = lock.read().unwrap();
                        assert_eq!(r.0, r.1);
                        drop(r);
                    }
                }
            });
        }
    });
    let writers = THREADS.div_ceil(2);
    assert_eq!(
        lock.into_inner().unwrap(),
        (writers * ITERS, writers * ITERS)
    );
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn blocked_threads_are_woken() {
    // The writer holds the lock long enough for readers and writers to
    // sleep, so every wakeup path of the unlock runs.
    let lock = RwLock::new(0usize);
    let w = lock.write().unwrap();
    thread::scope(|s| {
        for i in 0..THREADS {
            let lock = &lock;
            s.spawn(move || {
                if i % 2 == 0 {
                    *lock.write().unwrap() += 1;
                } else {
                    let _r = lock.read().unwrap();
                }
            });
        }
        thread::sleep(Duration::from_millis(20));
        drop(w);
    });
    assert_eq!(*lock.read().unwrap(), THREADS.div_ceil(2));
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn waiting_writer_holds_back_new_readers() {
    let lock = RwLock::new(0);
    let r = lock.read().unwrap();
    let written = AtomicBool::new(false);
    thread::scope(|s| {
        s.spawn(|| {
            *lock.write().unwrap() = 1;
            written.store(true, SeqCst);
        });
        // Once the writer waits, a new reader must not join the current one.
        assert!(wait_until(|| lock.try_read().is_err()));
        assert!(!written.load(SeqCst));
        drop(r);
        assert!(wait_until(|| written.load(SeqCst)));
    });
    assert_eq!(*lock.read().unwrap(), 1);
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn writers_are_not_starved_by_readers() {
    // Readers keep the lock read-locked with overlapping guards; the writer
    // must still get through all of its writes.
    let lock = RwLock::new(0usize);
    let stop = AtomicBool::new(false);
    let writes = if cfg!(miri) { 3 } else { 200 };
    thread::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                // Ends when the writer is done and sets `stop`.
                while !stop.load(SeqCst) {
                    let r = lock.read().unwrap();
                    thread::yield_now();
                    drop(r);
                }
            });
        }
        for _ in 0..writes {
            *lock.write().unwrap() += 1;
        }
        stop.store(true, SeqCst);
    });
    assert_eq!(lock.into_inner().unwrap(), writes);
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn downgrade_lets_waiting_readers_in() {
    let lock = RwLock::new(0);
    let mut w = lock.write().unwrap();
    let read = AtomicUsize::new(0);
    thread::scope(|s| {
        for _ in 0..2 {
            s.spawn(|| {
                let r = lock.read().unwrap();
                read.store(*r, SeqCst);
                drop(r);
            });
        }
        thread::sleep(Duration::from_millis(10));
        *w = 7;
        let r = RwLockWriteGuard::downgrade(w);
        // The readers get in while the downgraded guard is still held.
        assert!(wait_until(|| read.load(SeqCst) == 7));
        assert_eq!(*r, 7);
        assert!(lock.try_write().is_err());
        drop(r);
    });
    assert!(lock.try_write().is_ok());
}

#[test]
fn downgrade_without_waiters() {
    let lock = RwLock::new(String::from("a"));
    let r = RwLockWriteGuard::downgrade({
        let mut w = lock.write().unwrap();
        w.push('b');
        w
    });
    assert_eq!(*r, "ab");
    assert_eq!(*lock.try_read().unwrap(), "ab");
    assert!(lock.try_write().is_err());
    drop(r);
    assert!(lock.try_write().is_ok());
}

#[test]
fn accessors() {
    let mut lock = RwLock::new(vec![1]);
    lock.get_mut().unwrap().push(2);
    assert!(!lock.is_poisoned());
    lock.clear_poison();
    assert_eq!(lock.into_inner().unwrap(), [1, 2]);

    let lock: RwLock<u8> = RwLock::default();
    assert_eq!(*lock.read().unwrap(), 0);
    let lock = RwLock::from("x");
    assert_eq!(*lock.read().unwrap(), "x");
}

#[test]
fn guards_format_like_their_data() {
    let lock = RwLock::new(5);
    let r = lock.read().unwrap();
    let read = format!("{r} {r:?} {r:>3}");
    drop(r);
    let w = lock.write().unwrap();
    let written = format!("{w} {w:?} {w:>3}");
    drop(w);
    assert_eq!(read, "5 5   5");
    assert_eq!(written, read);
}

#[test]
fn unsized_data() {
    let lock: &RwLock<[i32]> = &RwLock::new([1, 2, 3]);
    lock.write().unwrap()[1] = 9;
    assert_eq!(*lock.read().unwrap(), [1, 9, 3]);
}

#[test]
fn static_rwlock() {
    static LOCK: RwLock<Vec<i32>> = RwLock::new(Vec::new());
    LOCK.write().unwrap().push(1);
    assert_eq!(*LOCK.read().unwrap(), [1]);
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn guards_can_be_shared() {
    let lock = RwLock::new(3);
    let r = lock.read().unwrap();
    thread::scope(|s| {
        s.spawn(|| assert_eq!(*r, 3));
    });
    drop(r);
    let w = lock.write().unwrap();
    thread::scope(|s| {
        s.spawn(|| assert_eq!(*w, 3));
    });
    drop(w);
}
