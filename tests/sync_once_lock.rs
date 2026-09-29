//! `OnceLock`: one initialization under contention, `set`, `wait`, taking
//! the value back, and drop accounting.
#![cfg(feature = "sync")]

use core::{
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
    time::Duration,
};
use std::thread;

use litestd::sync::OnceLock;

const THREADS: usize = if cfg!(miri) { 4 } else { 16 };

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn initializes_once_under_contention() {
    let cell = OnceLock::new();
    let runs = AtomicUsize::new(0);
    thread::scope(|s| {
        for i in 0..THREADS {
            let (cell, runs) = (&cell, &runs);
            s.spawn(move || {
                // A heap value written by the winner: reading it on the
                // other threads races unless `get_or_init` orders it.
                let v = cell.get_or_init(|| {
                    runs.fetch_add(1, Relaxed);
                    vec![i; 4]
                });
                assert_eq!(v.len(), 4);
                assert!(v.iter().all(|&x| x == v[0]));
                assert_eq!(cell.get(), Some(v));
            });
        }
    });
    assert_eq!(runs.load(Relaxed), 1);
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn exactly_one_set_wins() {
    let cell = OnceLock::new();
    let wins = AtomicUsize::new(0);
    thread::scope(|s| {
        for i in 0..THREADS {
            let (cell, wins) = (&cell, &wins);
            s.spawn(move || match cell.set(i) {
                Ok(()) => {
                    wins.fetch_add(1, Relaxed);
                }
                Err(v) => assert_eq!(v, i),
            });
        }
    });
    assert_eq!(wins.load(Relaxed), 1);
    assert!(cell.get().is_some());
}

#[test]
fn get_and_set() {
    let cell = OnceLock::new();
    assert_eq!(cell.get(), None);
    assert_eq!(cell.set(1), Ok(()));
    assert_eq!(cell.set(2), Err(2));
    assert_eq!(cell.get(), Some(&1));
    assert_eq!(cell.get_or_init(|| 3), &1);
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn wait_blocks_until_set() {
    let cell = OnceLock::new();
    thread::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| assert_eq!(cell.wait(), &"ready"));
        }
        thread::sleep(Duration::from_millis(10));
        cell.set("ready").unwrap();
    });
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn slow_initializer_blocks_other_callers() {
    let cell = OnceLock::new();
    thread::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                let v = cell.get_or_init(|| {
                    thread::sleep(Duration::from_millis(10));
                    String::from("slow")
                });
                assert_eq!(v, "slow");
            });
        }
    });
}

#[test]
fn take_and_into_inner() {
    let mut cell = OnceLock::new();
    assert_eq!(cell.take(), None);
    cell.set(String::from("a")).unwrap();
    cell.get_mut().unwrap().push('b');
    assert_eq!(cell.take().as_deref(), Some("ab"));
    assert_eq!(cell.get(), None);
    assert_eq!(cell.get_mut(), None);
    // The cell can be filled again after `take`.
    assert_eq!(cell.get_or_init(|| String::from("c")), "c");
    assert_eq!(cell.into_inner().as_deref(), Some("c"));

    let empty: OnceLock<String> = OnceLock::default();
    assert_eq!(empty.into_inner(), None);
}

#[test]
fn drops_the_value_once() {
    static DROPS: AtomicUsize = AtomicUsize::new(0);
    struct D;
    impl Drop for D {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Relaxed);
        }
    }
    drop(OnceLock::<D>::new());
    assert_eq!(DROPS.load(Relaxed), 0);

    let cell = OnceLock::new();
    cell.get_or_init(|| D);
    // A rejected value comes back to the caller, which drops it.
    drop(cell.set(D));
    assert_eq!(DROPS.load(Relaxed), 1);
    drop(cell);
    assert_eq!(DROPS.load(Relaxed), 2);

    let mut cell = OnceLock::from(D);
    let taken = cell.take();
    drop(cell);
    assert_eq!(DROPS.load(Relaxed), 2);
    drop(taken);
    assert_eq!(DROPS.load(Relaxed), 3);
}

/// As in std, the cell may hold a reference that dangles by the time the
/// cell is dropped: a reference has no destructor that could observe it.
/// This only has to compile.
#[test]
fn value_may_dangle_when_dropped() {
    let cell = OnceLock::new();
    {
        let s = String::from("borrowed");
        cell.set(s.as_str()).unwrap();
        assert_eq!(cell.get(), Some(&"borrowed"));
    }
    // `s` is gone; dropping `cell` at the end of the scope must not read it.
}

#[test]
fn clone_eq_and_from() {
    let a = OnceLock::from(5);
    let b = a.clone();
    assert_eq!(a, b);
    assert_eq!(b.get(), Some(&5));
    let empty: OnceLock<i32> = OnceLock::new();
    assert_eq!(empty.clone().into_inner(), None);
    assert_ne!(a, empty);
}

#[test]
fn static_cell() {
    static CELL: OnceLock<Vec<i32>> = OnceLock::new();
    assert_eq!(CELL.get_or_init(|| vec![1]), &[1]);
    assert_eq!(CELL.get(), Some(&vec![1]));
}

/// Reading the cell from its own initializer sees no value and touches no
/// storage; initializing it again from there would deadlock instead.
#[test]
fn initializer_may_read_the_cell() {
    let cell = OnceLock::new();
    let value = cell.get_or_init(|| {
        assert_eq!(cell.get(), None);
        String::from("done")
    });
    assert_eq!(value, "done");
}
