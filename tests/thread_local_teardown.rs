//! A destructor that initializes a static while its thread exits may make
//! the thread's slot table grow, which moves the slots of the values still
//! waiting for destruction. This binary holds a single test, so the slot
//! indices, which statics take in the order of their first use in the
//! process, are known. With `nightly`, the destructor lists a native value
//! while the list is being destroyed instead.

// WebAssembly has threads only with the atomics feature.
#![cfg(all(
    feature = "thread",
    not(all(target_family = "wasm", not(target_feature = "atomics")))
))]

use core::sync::atomic::{AtomicUsize, Ordering};

use litestd::{thread, thread_local};

static DROPS: AtomicUsize = AtomicUsize::new(0);

struct Counted;

impl Drop for Counted {
    fn drop(&mut self) {
        DROPS.fetch_add(1, Ordering::SeqCst);
    }
}

struct Early;

impl Drop for Early {
    fn drop(&mut self) {
        // The first use of `FRESH` takes the highest index yet, beyond the
        // exiting thread's table.
        FRESH.with(|_| ());
        DROPS.fetch_add(1, Ordering::SeqCst);
    }
}

thread_local! {
    static EARLY: Early = const { Early };
    static FRESH: Counted = const { Counted };
}

/// Declares `$name`, which initializes forty statics of its own.
macro_rules! forty {
    ($name:ident) => {
        fn $name() {
            thread_local! {
            static F0: Counted = const { Counted };
            static F1: Counted = const { Counted };
            static F2: Counted = const { Counted };
            static F3: Counted = const { Counted };
            static F4: Counted = const { Counted };
            static F5: Counted = const { Counted };
            static F6: Counted = const { Counted };
            static F7: Counted = const { Counted };
            static F8: Counted = const { Counted };
            static F9: Counted = const { Counted };
            static F10: Counted = const { Counted };
            static F11: Counted = const { Counted };
            static F12: Counted = const { Counted };
            static F13: Counted = const { Counted };
            static F14: Counted = const { Counted };
            static F15: Counted = const { Counted };
            static F16: Counted = const { Counted };
            static F17: Counted = const { Counted };
            static F18: Counted = const { Counted };
            static F19: Counted = const { Counted };
            static F20: Counted = const { Counted };
            static F21: Counted = const { Counted };
            static F22: Counted = const { Counted };
            static F23: Counted = const { Counted };
            static F24: Counted = const { Counted };
            static F25: Counted = const { Counted };
            static F26: Counted = const { Counted };
            static F27: Counted = const { Counted };
            static F28: Counted = const { Counted };
            static F29: Counted = const { Counted };
            static F30: Counted = const { Counted };
            static F31: Counted = const { Counted };
            static F32: Counted = const { Counted };
            static F33: Counted = const { Counted };
            static F34: Counted = const { Counted };
            static F35: Counted = const { Counted };
            static F36: Counted = const { Counted };
            static F37: Counted = const { Counted };
            static F38: Counted = const { Counted };
            static F39: Counted = const { Counted };
            }
            for key in [
                &F0, &F1, &F2, &F3, &F4, &F5, &F6, &F7, &F8, &F9, &F10, &F11,
                &F12, &F13, &F14, &F15, &F16, &F17, &F18, &F19, &F20, &F21,
                &F22, &F23, &F24, &F25, &F26, &F27, &F28, &F29, &F30, &F31,
                &F32, &F33, &F34, &F35, &F36, &F37, &F38, &F39,
            ] {
                key.with(|_| ());
            }
        }
    };
}

forty!(touch_first);
forty!(touch_second);

#[test]
fn slots_move_while_values_are_destroyed() {
    thread::spawn(|| {
        // Indices 0 to 39: the inline slots, and the first of a table of
        // more slots.
        touch_first();
        // Index 40, in that table.
        EARLY.with(|_| ());
        // Indices 41 to 80, on another thread, so that this thread's table
        // stays as it is.
        thread::spawn(touch_second).join().unwrap();
        // At exit, `EARLY` is destroyed first, and its destructor gives
        // `FRESH` index 81; the table grows and moves, with the slots of
        // `EARLY` and of the older values in it.
    })
    .join()
    .unwrap();
    assert_eq!(DROPS.load(Ordering::SeqCst), 40 + 1 + 1 + 40);
}
