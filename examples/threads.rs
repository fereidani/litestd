//! A `no_std` program that uses std's threads and locks through litestd.
//!
//! Build with `--features global-allocator`.

#![no_std]
#![no_main]
// Written against std's paths on purpose: that is what the switch provides.
#![allow(clippy::std_instead_of_core, clippy::std_instead_of_alloc)]
// A std-style program: a poisoned lock or a failed join would be a bug.
#![allow(clippy::unwrap_used)]

#[macro_use]
extern crate litestd as std;

use std::{
    ffi::{c_char, c_int},
    prelude::rust_2024::*,
    sync::{Arc, Condvar, Mutex},
    thread,
    time::{Duration, Instant},
};

/// The C runtime's entry point, which wasi-libc calls `__main_argc_argv`.
#[cfg_attr(not(target_os = "wasi"), unsafe(no_mangle))]
#[cfg_attr(target_os = "wasi", unsafe(export_name = "__main_argc_argv"))]
extern "C" fn main(_argc: c_int, _argv: *const *const c_char) -> c_int {
    let total = spawn_and_join();
    handshake();
    let data = scoped();
    println!("threads: {total} {data:?}");
    0
}

/// Threads that share a counter behind a `Mutex`.
fn spawn_and_join() -> u32 {
    let counter = Arc::new(Mutex::new(0u32));
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let counter = Arc::clone(&counter);
            thread::spawn(move || {
                for _ in 0..1000 {
                    *counter.lock().unwrap() += 1;
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let total = *counter.lock().unwrap();
    assert_eq!(total, 4000);
    total
}

/// A `Condvar` handshake, with a timed wait first.
fn handshake() {
    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let waiter = {
        let pair = Arc::clone(&pair);
        thread::spawn(move || {
            let (lock, cvar) = &*pair;
            drop(
                cvar.wait_while(lock.lock().unwrap(), |ready| !*ready)
                    .unwrap(),
            );
        })
    };

    let (lock, cvar) = &*pair;
    let start = Instant::now();
    let (guard, timeout) = cvar
        .wait_timeout(lock.lock().unwrap(), Duration::from_millis(10))
        .unwrap();
    assert!(timeout.timed_out() || start.elapsed() < Duration::from_millis(10));
    drop(guard);

    *lock.lock().unwrap() = true;
    cvar.notify_one();
    waiter.join().unwrap();
}

/// Scoped threads that borrow from the caller's stack.
fn scoped() -> Vec<u32> {
    let mut data = vec![1u32, 2, 3, 4];
    thread::scope(|s| {
        for x in &mut data {
            s.spawn(move || *x *= 10);
        }
    });
    assert_eq!(data, [10, 20, 30, 40]);
    thread::sleep(Duration::from_millis(1));
    data
}
