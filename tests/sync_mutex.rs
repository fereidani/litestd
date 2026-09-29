//! `Mutex`: mutual exclusion, the nonblocking and exclusive accessors, and
//! behavior when a guard is dropped by unwinding.
#![cfg(feature = "sync")]

use core::{
    fmt::Debug,
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
    time::Duration,
};
use std::{panic, thread};

use litestd::sync::{Mutex, TryLockError};

const THREADS: usize = if cfg!(miri) { 3 } else { 8 };
const ITERS: usize = if cfg!(miri) { 30 } else { 20_000 };

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn excludes_concurrent_access() {
    // Two plain counters updated in separate steps: any overlap of critical
    // sections loses updates or breaks their equality.
    let m = Mutex::new((0usize, 0usize));
    thread::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                for _ in 0..ITERS {
                    let mut g = m.lock().unwrap();
                    assert_eq!(g.0, g.1);
                    g.0 += 1;
                    g.1 += 1;
                }
            });
        }
    });
    assert_eq!(m.into_inner().unwrap(), (THREADS * ITERS, THREADS * ITERS));
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn blocked_threads_are_woken() {
    // The holder keeps the lock long enough for the others to sleep, so
    // their wakeup goes through the futex.
    let m = Mutex::new(0usize);
    let g = m.lock().unwrap();
    thread::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| *m.lock().unwrap() += 1);
        }
        thread::sleep(Duration::from_millis(20));
        drop(g);
    });
    assert_eq!(*m.lock().unwrap(), THREADS);
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn try_lock_fails_while_locked() {
    let m = Mutex::new(1);
    let g = m.try_lock().unwrap();
    assert!(matches!(m.try_lock(), Err(TryLockError::WouldBlock)));
    thread::scope(|s| {
        s.spawn(|| assert!(m.try_lock().is_err()));
    });
    drop(g);
    assert_eq!(*m.try_lock().unwrap(), 1);
}

#[test]
fn accessors() {
    let mut m = Mutex::new(String::from("a"));
    m.get_mut().unwrap().push('b');
    assert_eq!(*m.lock().unwrap(), "ab");
    assert!(!m.is_poisoned());
    m.clear_poison();
    assert_eq!(m.into_inner().unwrap(), "ab");

    let m: Mutex<Vec<u8>> = Mutex::default();
    assert!(m.lock().unwrap().is_empty());
    let m = Mutex::from(7);
    assert_eq!(*m.lock().unwrap(), 7);
}

#[test]
fn guard_formats_like_its_data() {
    let m = Mutex::new(5);
    let g = m.lock().unwrap();
    let text = format!("{g} {g:?} {g:>3}");
    drop(g);
    assert_eq!(text, "5 5   5");
}

#[test]
fn unsized_data() {
    let m: &Mutex<[i32]> = &Mutex::new([1, 2, 3]);
    m.lock().unwrap()[0] = 5;
    assert_eq!(*m.lock().unwrap(), [5, 2, 3]);

    let b: Box<Mutex<dyn Debug + Send>> = Box::new(Mutex::new(4));
    assert_eq!(format!("{:?}", &*b.lock().unwrap()), "4");
}

#[test]
fn static_mutex() {
    static M: Mutex<Vec<i32>> = Mutex::new(Vec::new());
    M.lock().unwrap().push(1);
    assert_eq!(*M.lock().unwrap(), [1]);
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn guard_can_be_shared() {
    let m = Mutex::new(3);
    let g = m.lock().unwrap();
    thread::scope(|s| {
        s.spawn(|| assert_eq!(*g, 3));
    });
}

/// litestd does not support unwinding, but a std test binary can unwind
/// through a guard; dropping it then must release the lock soundly.
#[test]
#[cfg_attr(panic = "abort", ignore = "needs unwinding")]
fn unwinding_guard_releases_the_lock() {
    let m = Mutex::new(0);
    let r = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        let mut g = m.lock().unwrap();
        *g = 1;
        assert!(*g != 1, "inside the lock");
        drop(g);
    }));
    assert!(r.is_err());
    assert!(!m.is_poisoned());
    assert_eq!(*m.try_lock().unwrap(), 1);
}

#[test]
fn drops_data_once() {
    static DROPS: AtomicUsize = AtomicUsize::new(0);
    struct D;
    impl Drop for D {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Relaxed);
        }
    }
    drop(Mutex::new(D));
    assert_eq!(DROPS.load(Relaxed), 1);
    let d = Mutex::new(D).into_inner().unwrap();
    assert_eq!(DROPS.load(Relaxed), 1);
    drop(d);
    assert_eq!(DROPS.load(Relaxed), 2);
}
