//! `LazyLock`: one initialization under contention, exclusive forcing, and
//! drop accounting of both the closure and the value.
#![cfg(feature = "sync")]

use core::{
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
    time::Duration,
};
use std::thread;

use litestd::sync::LazyLock;

const THREADS: usize = if cfg!(miri) { 4 } else { 16 };

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn initializes_once_under_contention() {
    let runs = AtomicUsize::new(0);
    let lazy = LazyLock::new(|| {
        runs.fetch_add(1, Relaxed);
        thread::sleep(Duration::from_millis(5));
        vec![7; 8]
    });
    thread::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                // The vector is written by the initializing thread only.
                assert_eq!(lazy.iter().sum::<i32>(), 56);
                assert!(LazyLock::get(&lazy).is_some());
            });
        }
    });
    assert_eq!(runs.load(Relaxed), 1);
}

#[test]
fn force_and_get() {
    let lazy = LazyLock::new(|| String::from("value"));
    assert_eq!(LazyLock::get(&lazy), None);
    assert_eq!(LazyLock::force(&lazy), "value");
    assert_eq!(LazyLock::get(&lazy).map(String::as_str), Some("value"));
}

#[test]
fn force_mut_and_get_mut() {
    let mut lazy = LazyLock::new(|| 1);
    assert_eq!(LazyLock::get_mut(&mut lazy), None);
    *LazyLock::force_mut(&mut lazy) += 1;
    *lazy += 1;
    assert_eq!(LazyLock::get_mut(&mut lazy), Some(&mut 3));
    assert_eq!(*lazy, 3);

    // `force_mut` on an initialized lock only returns the value.
    let lazy2 = LazyLock::new(|| 10);
    assert_eq!(*lazy2, 10);
    let mut lazy2 = lazy2;
    assert_eq!(*LazyLock::force_mut(&mut lazy2), 10);
}

#[test]
fn drops_the_closure_or_the_value_once() {
    static F_DROPS: AtomicUsize = AtomicUsize::new(0);
    static T_DROPS: AtomicUsize = AtomicUsize::new(0);
    struct F;
    impl Drop for F {
        fn drop(&mut self) {
            F_DROPS.fetch_add(1, Relaxed);
        }
    }
    struct T;
    impl Drop for T {
        fn drop(&mut self) {
            T_DROPS.fetch_add(1, Relaxed);
        }
    }
    let make = || {
        let f = F;
        move || {
            let _f = &f;
            T
        }
    };

    // Never forced: the closure is dropped with the lock.
    drop(LazyLock::new(make()));
    assert_eq!((F_DROPS.load(Relaxed), T_DROPS.load(Relaxed)), (1, 0));

    // Forced: the closure is consumed by the initialization, and the value
    // is dropped with the lock.
    let lazy = LazyLock::new(make());
    let _ = &*lazy;
    assert_eq!((F_DROPS.load(Relaxed), T_DROPS.load(Relaxed)), (2, 0));
    drop(lazy);
    assert_eq!((F_DROPS.load(Relaxed), T_DROPS.load(Relaxed)), (2, 1));

    // The same through `force_mut`.
    let mut lazy = LazyLock::new(make());
    let _ = LazyLock::force_mut(&mut lazy);
    assert_eq!((F_DROPS.load(Relaxed), T_DROPS.load(Relaxed)), (3, 1));
    drop(lazy);
    assert_eq!((F_DROPS.load(Relaxed), T_DROPS.load(Relaxed)), (3, 2));
}

#[test]
fn from_value_and_default() {
    let lazy: LazyLock<i32> = LazyLock::from(5);
    assert_eq!(LazyLock::get(&lazy), Some(&5));
    assert_eq!(*lazy, 5);

    let lazy: LazyLock<Vec<u8>> = LazyLock::default();
    assert!(lazy.is_empty());
}

#[test]
fn static_lazy() {
    static VALUE: LazyLock<Vec<i32>> = LazyLock::new(|| vec![1, 2]);
    assert_eq!(*VALUE, [1, 2]);
    assert_eq!(LazyLock::get(&VALUE), Some(&vec![1, 2]));
}

/// Reading the lock from its own initializer sees no value and touches
/// neither the closure nor the value, which the initializer owns.
#[test]
fn initializer_may_read_the_lock() {
    static LAZY: LazyLock<String> = LazyLock::new(|| {
        assert!(LazyLock::get(&LAZY).is_none());
        String::from("done")
    });
    assert_eq!(*LAZY, "done");
}
