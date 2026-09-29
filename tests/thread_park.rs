//! Parking and unparking.

#![cfg(feature = "thread")]

use core::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};
use std::{sync::mpsc, time::Instant};

use litestd::{sync::Arc, thread};

/// Round trips per test; Miri is slow.
const ROUNDS: usize = if cfg!(miri) { 20 } else { 1000 };

#[test]
fn unpark_before_park_makes_park_return() {
    thread::current().unpark();
    thread::park();
}

#[test]
#[cfg_attr(
    target_os = "unknown",
    ignore = "wasm32-unknown-unknown has no clock"
)]
fn tokens_do_not_accumulate() {
    let me = thread::current();
    me.unpark();
    me.unpark();
    // Consumes the one token.
    thread::park();
    // No token is left, so this waits for the timeout, barring a spurious
    // wakeup, which would have to come early to fail the check.
    let timeout = Duration::from_millis(50);
    let start = Instant::now();
    thread::park_timeout(timeout);
    assert!(start.elapsed() >= timeout / 2);
}

#[test]
#[cfg_attr(
    target_os = "unknown",
    ignore = "wasm32-unknown-unknown has no clock"
)]
fn park_timeout_returns_without_a_token() {
    for ms in [0, 1, 10] {
        thread::park_timeout(Duration::from_millis(ms));
    }
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn park_timeout_with_a_huge_timeout_is_unparked() {
    let parked = Arc::new(AtomicBool::new(false));
    let handle = {
        let parked = parked.clone();
        thread::spawn(move || {
            parked.store(true, Ordering::SeqCst);
            thread::park_timeout(Duration::MAX);
        })
    };
    while !parked.load(Ordering::SeqCst) {
        thread::yield_now();
    }
    handle.thread().unpark();
    handle.join().unwrap();
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn unpark_wakes_a_parked_thread() {
    let flag = Arc::new(AtomicBool::new(false));
    let queued = Arc::new(AtomicBool::new(false));
    let handle = {
        let (flag, queued) = (flag.clone(), queued.clone());
        thread::spawn(move || {
            queued.store(true, Ordering::Release);
            while !flag.load(Ordering::Acquire) {
                thread::park();
            }
        })
    };
    while !queued.load(Ordering::Acquire) {
        thread::yield_now();
    }
    flag.store(true, Ordering::Release);
    handle.thread().unpark();
    handle.join().unwrap();
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn ping_pong_through_park_and_unpark() {
    // Each side waits for its turn, parking in between; the other side
    // hands over the turn and unparks it.
    let turn = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = mpsc::channel();
    let partner = {
        let turn = turn.clone();
        thread::spawn(move || {
            let main: thread::Thread = rx.recv().unwrap();
            for round in 0..ROUNDS {
                while turn.load(Ordering::Acquire) != 2 * round + 1 {
                    thread::park();
                }
                turn.store(2 * round + 2, Ordering::Release);
                main.unpark();
            }
        })
    };
    tx.send(thread::current()).unwrap();
    for round in 0..ROUNDS {
        turn.store(2 * round + 1, Ordering::Release);
        partner.thread().unpark();
        while turn.load(Ordering::Acquire) != 2 * round + 2 {
            thread::park();
        }
    }
    partner.join().unwrap();
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn unparking_a_running_thread_is_harmless() {
    let handle = thread::spawn(|| thread::sleep(Duration::from_millis(1)));
    for _ in 0..ROUNDS {
        handle.thread().unpark();
    }
    handle.join().unwrap();
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn unpark_through_a_cloned_handle() {
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        tx.send(thread::current()).unwrap();
        thread::park();
    });
    let received = rx.recv().unwrap();
    let clone = received.clone();
    drop(received);
    assert_eq!(clone.id(), handle.thread().id());
    clone.unpark();
    handle.join().unwrap();
}
