//! Scoped threads borrowing from the spawner.

// WebAssembly has threads only with the atomics feature.
#![cfg(all(
    feature = "thread",
    not(all(target_family = "wasm", not(target_feature = "atomics")))
))]

use core::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};
use std::time::Instant;

use litestd::{
    sync::Arc,
    thread::{self, Builder},
};

/// Threads per test; Miri is slow.
const THREADS: usize = if cfg!(miri) { 3 } else { 16 };

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
fn scoped_threads_borrow_and_mutate() {
    let mut a = vec![1, 2, 3];
    let mut x = 0;
    thread::scope(|s| {
        s.spawn(|| {
            assert_eq!(a.len(), 3);
        });
        s.spawn(|| {
            x += a[0] + a[2];
        });
    });
    a.push(4);
    assert_eq!(x, a.len());
}

#[test]
fn scope_returns_the_closure_result() {
    let data = [1, 2, 3, 4];
    let total = thread::scope(|s| {
        let halves = data
            .chunks(2)
            .map(|half| s.spawn(move || half.iter().sum::<i32>()));
        halves.map(|handle| handle.join().unwrap()).sum::<i32>()
    });
    assert_eq!(total, 10);
}

#[test]
fn scope_waits_for_threads_that_were_not_joined() {
    let flags: Vec<AtomicBool> =
        (0..THREADS).map(|_| AtomicBool::new(false)).collect();
    thread::scope(|s| {
        for flag in &flags {
            s.spawn(move || {
                thread::sleep(Duration::from_millis(5));
                flag.store(true, Ordering::Relaxed);
            });
        }
    });
    // The scope's end synchronizes with the threads' completion.
    assert!(flags.iter().all(|flag| flag.load(Ordering::Relaxed)));
}

#[test]
fn nested_spawns_are_waited_for() {
    let count = AtomicUsize::new(0);
    thread::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                s.spawn(|| {
                    thread::sleep(Duration::from_millis(1));
                    count.fetch_add(1, Ordering::Relaxed);
                });
                count.fetch_add(1, Ordering::Relaxed);
            });
        }
    });
    assert_eq!(count.load(Ordering::Relaxed), 2 * THREADS);
}

#[test]
fn scoped_join_and_is_finished() {
    let go = AtomicBool::new(false);
    thread::scope(|s| {
        let handle = s.spawn(|| {
            wait_until(|| go.load(Ordering::SeqCst));
            7
        });
        assert!(!handle.is_finished());
        go.store(true, Ordering::SeqCst);
        wait_until(|| handle.is_finished());
        assert_eq!(handle.join().unwrap(), 7);
    });
}

#[test]
fn unjoined_results_are_dropped_before_the_scope_ends() {
    struct Borrowing<'a>(&'a AtomicUsize);
    impl Drop for Borrowing<'_> {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let drops = AtomicUsize::new(0);
    let go = AtomicBool::new(false);
    thread::scope(|s| {
        // Dropped by the thread: the handle goes away first.
        drop(s.spawn(|| {
            wait_until(|| go.load(Ordering::SeqCst));
            Borrowing(&drops)
        }));
        // Dropped by the handle: the thread finishes first.
        let handle = s.spawn(|| Borrowing(&drops));
        wait_until(|| handle.is_finished());
        go.store(true, Ordering::SeqCst);
    });
    assert_eq!(drops.load(Ordering::SeqCst), 2);
}

#[test]
fn builder_spawn_scoped_names_threads() {
    let name = String::from("scoped worker");
    thread::scope(|s| {
        let handle = Builder::new()
            .name(name.clone())
            .spawn_scoped(s, || thread::current().name().map(str::to_string))
            .unwrap();
        assert_eq!(handle.thread().name(), Some(name.as_str()));
        assert_eq!(handle.join().unwrap().as_deref(), Some(name.as_str()));
    });
}

#[test]
fn scoped_thread_ids_differ_from_the_spawner() {
    let me = thread::current().id();
    thread::scope(|s| {
        let handle = s.spawn(|| thread::current().id());
        let id = handle.thread().id();
        assert_ne!(id, me);
        assert_eq!(handle.join().unwrap(), id);
    });
}

#[test]
fn scopes_can_be_used_from_scoped_threads() {
    let counter = Arc::new(AtomicUsize::new(0));
    thread::scope(|outer| {
        outer.spawn(|| {
            thread::scope(|inner| {
                for _ in 0..THREADS {
                    inner.spawn(|| counter.fetch_add(1, Ordering::Relaxed));
                }
            });
            assert_eq!(counter.load(Ordering::Relaxed), THREADS);
        });
    });
}

#[test]
fn debug_output() {
    thread::scope(|s| {
        let text = format!("{s:?}");
        assert!(text.starts_with("Scope { num_running_threads: "), "{text}");
        assert!(text.ends_with(".. }"), "{text}");
        let handle = s.spawn(|| {});
        assert_eq!(format!("{handle:?}"), "ScopedJoinHandle { .. }");
    });
}

#[test]
fn debug_output_while_the_scope_waits() {
    let text = litestd::sync::Mutex::new(String::new());
    thread::scope(|s| {
        let text = &text;
        s.spawn(move || {
            // By now the scope sleeps, waiting for this thread.
            thread::sleep(Duration::from_millis(20));
            *text.lock().unwrap() = format!("{s:?}");
        });
    });
    let text = text.into_inner().unwrap();
    assert_eq!(text, "Scope { num_running_threads: 1, .. }");
}

#[test]
fn borrows_end_before_the_scope_does() {
    // The borrowed data is freed right after each scope, which Miri reports
    // if a scoped thread still holds a live reference to it, even one that
    // is merely protected as a function argument when the thread signals
    // that it is done.
    for _ in 0..THREADS {
        let mut data = Box::new(0u64);
        let flag = Box::new(AtomicBool::new(false));
        thread::scope(|s| {
            let data = &mut *data;
            let flag = &*flag;
            // The result borrows too, and is dropped by whichever of the
            // thread and the handle finishes last.
            drop(s.spawn(move || {
                *data += 1;
                flag.store(true, Ordering::Relaxed);
                flag
            }));
            let reader = s.spawn(move || flag.load(Ordering::Relaxed));
            if reader.is_finished() {
                reader.join().unwrap();
            }
        });
        assert_eq!(*data, 1);
        drop((data, flag));
    }
}
