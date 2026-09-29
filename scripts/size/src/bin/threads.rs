//! Threads that count under a `Mutex` and signal a `Condvar`.

#![cfg_attr(feature = "lite", no_std, no_main)]

#[cfg(feature = "lite")]
#[macro_use]
extern crate litestd as std;

use std::{
    prelude::rust_2024::*,
    sync::{Arc, Condvar, Mutex},
    thread,
};

size_probe::main! {
    let state = Arc::new((Mutex::new(0u32), Condvar::new()));
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let state = Arc::clone(&state);
            thread::spawn(move || {
                let (count, changed) = &*state;
                *count.lock().unwrap() += 1;
                changed.notify_one();
            })
        })
        .collect();
    let (count, changed) = &*state;
    let mut done = count.lock().unwrap();
    while *done < 4 {
        done = changed.wait(done).unwrap();
    }
    drop(done);
    for worker in workers {
        worker.join().unwrap();
    }
    println!("{} threads done", count.lock().unwrap());
}
