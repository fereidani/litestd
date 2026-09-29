//! `Condvar`: notifications, predicates, timeouts and unwinding predicates.
#![cfg(feature = "sync")]

extern crate alloc;

use alloc::collections::VecDeque;
use core::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use std::{panic, thread, time::Instant};

use litestd::sync::{Condvar, Mutex};

const ROUNDS: usize = if cfg!(miri) { 20 } else { 10_000 };
const THREADS: usize = if cfg!(miri) { 3 } else { 8 };

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn notify_one_wakes_a_waiter() {
    let pair = (Mutex::new(false), Condvar::new());
    thread::scope(|s| {
        s.spawn(|| {
            let (lock, cvar) = &pair;
            *lock.lock().unwrap() = true;
            cvar.notify_one();
        });
        let (lock, cvar) = &pair;
        let mut started = lock.lock().unwrap();
        while !*started {
            started = cvar.wait(started).unwrap();
        }
        drop(started);
    });
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn notify_all_wakes_every_waiter() {
    let state = (Mutex::new((false, 0usize)), Condvar::new());
    thread::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                let (lock, cvar) = &state;
                let g = {
                    let mut g = lock.lock().unwrap();
                    g.1 += 1;
                    g
                };
                cvar.notify_all();
                let mut g = cvar.wait_while(g, |g| !g.0).unwrap();
                g.1 -= 1;
                drop(g);
            });
        }
        let (lock, cvar) = &state;
        // Wait until every thread waits, then release them all at once.
        let mut g = cvar
            .wait_while(lock.lock().unwrap(), |g| g.1 < THREADS)
            .unwrap();
        g.0 = true;
        drop(g);
        cvar.notify_all();
    });
    assert_eq!(state.0.lock().unwrap().1, 0);
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn ping_pong() {
    let turn = (Mutex::new(0usize), Condvar::new());
    thread::scope(|s| {
        for me in 0..2 {
            let turn = &turn;
            s.spawn(move || {
                let (lock, cvar) = turn;
                for _ in 0..ROUNDS {
                    let mut g = cvar
                        .wait_while(lock.lock().unwrap(), |t| *t % 2 != me)
                        .unwrap();
                    *g += 1;
                    drop(g);
                    cvar.notify_one();
                }
            });
        }
    });
    assert_eq!(*turn.0.lock().unwrap(), 2 * ROUNDS);
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn queue_loses_no_wakeup() {
    // Consumers sleep on an empty queue; every item must be consumed. A lost
    // wakeup would leave a consumer asleep with items queued and hang.
    let queue = (Mutex::new((VecDeque::new(), false)), Condvar::new());
    let consumed = Mutex::new(0usize);
    thread::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                let (lock, cvar) = &queue;
                loop {
                    let mut g = cvar
                        .wait_while(lock.lock().unwrap(), |q| {
                            q.0.is_empty() && !q.1
                        })
                        .unwrap();
                    if g.0.pop_front().is_none() {
                        return;
                    }
                    drop(g);
                    *consumed.lock().unwrap() += 1;
                }
            });
        }
        let (lock, cvar) = &queue;
        for i in 0..ROUNDS {
            lock.lock().unwrap().0.push_back(i);
            cvar.notify_one();
        }
        lock.lock().unwrap().1 = true;
        cvar.notify_all();
    });
    assert_eq!(*consumed.lock().unwrap(), ROUNDS);
}

#[test]
#[cfg_attr(
    target_os = "unknown",
    ignore = "wasm32-unknown-unknown has no clock"
)]
fn notify_without_waiters_is_harmless() {
    let cvar = Condvar::default();
    cvar.notify_one();
    cvar.notify_all();
    let m = Mutex::new(());
    let dur = Duration::from_millis(1);
    let (_g, r) = cvar.wait_timeout(m.lock().unwrap(), dur).unwrap();
    assert!(r.timed_out());
}

#[test]
#[cfg_attr(
    target_os = "unknown",
    ignore = "wasm32-unknown-unknown has no clock"
)]
fn wait_timeout_elapses() {
    let (m, cvar) = (Mutex::new(()), Condvar::new());
    let dur = Duration::from_millis(10);
    let start = Instant::now();
    let (g, r) = cvar.wait_timeout(m.lock().unwrap(), dur).unwrap();
    assert!(r.timed_out());
    assert!(start.elapsed() >= dur);
    drop(g);

    let (g, r) = cvar
        .wait_timeout(m.lock().unwrap(), Duration::ZERO)
        .unwrap();
    assert!(r.timed_out());
    drop(g);
}

#[test]
#[cfg_attr(
    target_os = "unknown",
    ignore = "wasm32-unknown-unknown has no clock"
)]
fn wait_timeout_while_times_out() {
    let (m, cvar) = (Mutex::new(0), Condvar::new());
    let dur = Duration::from_millis(10);
    let start = Instant::now();
    let (g, r) = cvar
        .wait_timeout_while(m.lock().unwrap(), dur, |v| *v == 0)
        .unwrap();
    assert!(r.timed_out());
    assert_eq!(*g, 0);
    assert!(start.elapsed() >= dur);
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn wait_timeout_while_sees_the_condition() {
    let pair = (Mutex::new(false), Condvar::new());
    thread::scope(|s| {
        s.spawn(|| {
            thread::sleep(Duration::from_millis(5));
            *pair.0.lock().unwrap() = true;
            pair.1.notify_one();
        });
        let (lock, cvar) = &pair;
        // The longest timeout must not overflow the deadline computation.
        let (g, r) = cvar
            .wait_timeout_while(lock.lock().unwrap(), Duration::MAX, |v| !*v)
            .unwrap();
        assert!(*g);
        assert!(!r.timed_out());
    });
    // An already false condition returns at once, without timing out.
    let (lock, cvar) = &pair;
    let (_g, r) = cvar
        .wait_timeout_while(lock.lock().unwrap(), Duration::ZERO, |v| !*v)
        .unwrap();
    assert!(!r.timed_out());
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn wait_timeout_while_keeps_its_deadline() {
    // Notifications that leave the condition true do not extend the wait.
    let (lock, cvar) = (Mutex::new(0_u32), Condvar::new());
    let done = AtomicBool::new(false);
    let dur = Duration::from_millis(30);
    thread::scope(|s| {
        s.spawn(|| {
            // Stops after 10 seconds even if the wait never ends.
            for _ in 0..10_000 {
                if done.load(Ordering::Relaxed) {
                    break;
                }
                *lock.lock().unwrap() += 1;
                cvar.notify_all();
                thread::sleep(Duration::from_millis(1));
            }
        });
        let start = Instant::now();
        let (g, r) = cvar
            .wait_timeout_while(lock.lock().unwrap(), dur, |n| *n < u32::MAX)
            .unwrap();
        let elapsed = start.elapsed();
        drop(g);
        done.store(true, Ordering::Relaxed);
        assert!(r.timed_out());
        assert!(elapsed >= dur);
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    });
}

#[test]
#[allow(
    clippy::significant_drop_tightening,
    reason = "the guard moves into the wait, which must start before the \
              other thread can take the lock"
)]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn wait_timeout_while_checks_the_condition_after_the_deadline() {
    // The condition turns false without a notification: the wait runs into
    // its deadline, sees the condition, and reports no timeout.
    let (lock, cvar) = (Mutex::new(false), Condvar::new());
    // Miri runs slowly, and a WebAssembly runtime may take long to start a
    // thread: either may spend a short timeout before the other thread
    // runs, and a wait whose deadline has passed may return at once.
    let slow = cfg!(any(miri, target_family = "wasm"));
    let dur = Duration::from_millis(if slow { 1000 } else { 20 });
    thread::scope(|s| {
        let guard = lock.lock().unwrap();
        s.spawn(|| *lock.lock().unwrap() = true);
        let (g, r) = cvar.wait_timeout_while(guard, dur, |v| !*v).unwrap();
        assert!(*g);
        drop(g);
        assert!(!r.timed_out());
    });
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn wait_timeout_can_be_notified() {
    let pair = (Mutex::new(false), Condvar::new());
    thread::scope(|s| {
        s.spawn(|| {
            *pair.0.lock().unwrap() = true;
            pair.1.notify_all();
        });
        let (lock, cvar) = &pair;
        let mut g = lock.lock().unwrap();
        while !*g {
            g = cvar.wait_timeout(g, Duration::from_secs(60)).unwrap().0;
        }
        drop(g);
    });
}

/// litestd does not support unwinding, but a std test binary can unwind out
/// of a predicate; the guard then must release the lock soundly.
#[test]
#[cfg_attr(panic = "abort", ignore = "needs unwinding")]
fn panicking_predicate_unlocks() {
    let (m, cvar) = (Mutex::new(0), Condvar::new());
    let r = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        let _g = cvar.wait_while(m.lock().unwrap(), |_| panic!("predicate"));
    }));
    assert!(r.is_err());
    assert!(m.try_lock().is_ok());
    let r = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        let _g = cvar.wait_timeout_while(
            m.lock().unwrap(),
            Duration::from_secs(1),
            |_| panic!("predicate"),
        );
    }));
    assert!(r.is_err());
    assert!(m.try_lock().is_ok());
}

#[test]
fn static_condvar() {
    static PAIR: (Mutex<bool>, Condvar) = (Mutex::new(true), Condvar::new());
    let g = PAIR.1.wait_while(PAIR.0.lock().unwrap(), |v| !*v).unwrap();
    let ready = *g;
    drop(g);
    assert!(ready);
}

/// A predicate runs with the mutex held and may notify the condition
/// variable it waits on: notifying never takes the mutex. Here it wakes a
/// thread asleep on the same condition variable, which then waits for the
/// mutex; if that thread is not asleep yet, it sees `ready` and returns.
#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn predicate_may_notify() {
    let (m, cvar) = (Mutex::new(false), Condvar::new());
    thread::scope(|s| {
        s.spawn(|| {
            let g = cvar.wait_while(m.lock().unwrap(), |ready| !*ready);
            drop(g);
        });
        thread::sleep(Duration::from_millis(10));
        let g = cvar.wait_while(m.lock().unwrap(), |ready| {
            *ready = true;
            cvar.notify_all();
            false
        });
        drop(g);
    });
}
