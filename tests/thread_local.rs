//! `thread_local!` and `LocalKey`.

#![cfg(feature = "thread")]

use core::{
    cell::{Cell, RefCell},
    error::Error,
    fmt,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::sync::Mutex;

use litestd::{
    thread::{self, AccessError, LocalKey, ThreadId},
    thread_local,
};

/// Threads per test; Miri is slow.
const THREADS: usize = if cfg!(miri) { 3 } else { 16 };

litestd::thread_local! {
    /// A documented key with a `const` initializer.
    pub static PUBLIC: Cell<u32> = const { Cell::new(1) };

    #[allow(dead_code)]
    pub(crate) static CRATE: RefCell<Vec<u8>> = RefCell::new(vec![1, 2]);

    static PLAIN: String = String::from("plain");
    static LAST: Cell<u8> = const { Cell::new(3) }
}

thread_local!(static SINGLE: u64 = 64);
thread_local!(
    static SINGLE_CONST: u64 = const { 65 };
);

#[test]
fn every_declaration_form_works() {
    assert_eq!(PUBLIC.get(), 1);
    CRATE.with_borrow(|v| assert_eq!(*v, [1, 2]));
    PLAIN.with(|s| assert_eq!(s, "plain"));
    assert_eq!(LAST.get(), 3);
    assert_eq!(SINGLE.with(|v| *v), 64);
    assert_eq!(SINGLE_CONST.with(|v| *v), 65);
}

#[forbid(unsafe_code)]
mod safe_crate {
    // The expansion contains no `unsafe`.
    litestd::thread_local!(pub static FORBIDDEN: u8 = 9);
}

#[test]
fn expansion_is_free_of_unsafe_code() {
    assert_eq!(safe_crate::FORBIDDEN.with(|v| *v), 9);
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn each_thread_has_its_own_value() {
    thread_local!(static COUNTER: Cell<usize> = const { Cell::new(0) });
    COUNTER.set(100);
    let handles: Vec<_> = (0..THREADS)
        .map(|i| {
            thread::spawn(move || {
                assert_eq!(COUNTER.get(), 0);
                COUNTER.set(i);
                thread::yield_now();
                COUNTER.get()
            })
        })
        .collect();
    for (i, handle) in handles.into_iter().enumerate() {
        assert_eq!(handle.join().unwrap(), i);
    }
    assert_eq!(COUNTER.get(), 100);
}

#[test]
fn cell_helpers() {
    thread_local!(static X: Cell<Option<i32>> = const { Cell::new(Some(1)) });
    assert_eq!(X.get(), Some(1));
    assert_eq!(X.take(), Some(1));
    assert_eq!(X.take(), None);
    X.set(Some(5));
    assert_eq!(X.replace(Some(6)), Some(5));
    X.update(|x| x.map(|x| x * 2));
    assert_eq!(X.get(), Some(12));
}

#[test]
fn refcell_helpers() {
    thread_local!(static V: RefCell<Vec<i32>> = RefCell::new(Vec::new()));
    V.with_borrow_mut(|v| v.push(1));
    V.with_borrow(|v| assert_eq!(*v, [1]));
    assert_eq!(V.replace(vec![2, 3]), [1]);
    V.set(vec![4]);
    assert_eq!(V.take(), [4]);
    V.with_borrow(|v| assert_eq!(v.len(), 0));
}

#[test]
fn set_skips_the_initializer() {
    thread_local!(static X: Cell<i32> = panic!("initializer ran"));
    thread_local!(static Y: RefCell<Vec<i32>> = panic!("initializer ran"));
    X.set(123);
    assert_eq!(X.get(), 123);
    Y.set(vec![1, 2, 3]);
    Y.with_borrow(|v| assert_eq!(*v, [1, 2, 3]));
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn values_are_initialized_once_per_thread() {
    static INITS: AtomicUsize = AtomicUsize::new(0);
    thread_local!(static X: u8 = {
        INITS.fetch_add(1, Ordering::SeqCst);
        0
    });
    let handle = thread::spawn(|| {
        for _ in 0..3 {
            X.with(|_| ());
        }
    });
    handle.join().unwrap();
    assert_eq!(INITS.load(Ordering::SeqCst), 1);
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn destructors_run_before_join_returns() {
    static DROPS: AtomicUsize = AtomicUsize::new(0);
    struct Counted;
    impl Drop for Counted {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }
    thread_local!(static X: Counted = const { Counted });

    let handles: Vec<_> = (0..THREADS)
        .map(|_| thread::spawn(|| X.with(|_| ())))
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(DROPS.load(Ordering::SeqCst), THREADS);
    // A thread that never touched the key has nothing to destroy.
    thread::spawn(|| ()).join().unwrap();
    assert_eq!(DROPS.load(Ordering::SeqCst), THREADS);
}

/// What the destructors in `access_during_destruction` observed.
static OBSERVED: Mutex<Vec<(&str, bool, bool)>> = Mutex::new(Vec::new());

struct First;
struct Second;

thread_local! {
    static FIRST: First = const { First };
    static SECOND: Second = const { Second };
}

impl Drop for First {
    fn drop(&mut self) {
        let own = FIRST.try_with(|_| ()).is_ok();
        let other = SECOND.try_with(|_| ()).is_ok();
        if let Ok(mut observed) = OBSERVED.lock() {
            observed.push(("first", own, other));
        }
    }
}

impl Drop for Second {
    fn drop(&mut self) {
        let own = SECOND.try_with(|_| ()).is_ok();
        let other = FIRST.try_with(|_| ()).is_ok();
        if let Ok(mut observed) = OBSERVED.lock() {
            observed.push(("second", own, other));
        }
    }
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn access_during_destruction() {
    thread::spawn(|| {
        FIRST.with(|_| ());
        SECOND.with(|_| ());
    })
    .join()
    .unwrap();
    // Newest first: `SECOND` sees `FIRST` alive; `FIRST` sees `SECOND`
    // destroyed; neither can reach itself.
    assert_eq!(
        *OBSERVED.lock().unwrap(),
        [("second", false, true), ("first", false, false)]
    );
    assert_eq!(FIRST.try_with(|_| ()), Ok(()));
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn destructors_may_initialize_other_keys() {
    static LATE_DROPS: AtomicUsize = AtomicUsize::new(0);
    struct Late;
    impl Drop for Late {
        fn drop(&mut self) {
            LATE_DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }
    struct Early;
    impl Drop for Early {
        fn drop(&mut self) {
            // `LATE` was never used on this thread; it is initialized here
            // and destroyed after this destructor.
            LATE.with(|_| ());
        }
    }
    thread_local! {
        static EARLY: Early = const { Early };
        static LATE: Late = const { Late };
    }
    thread::spawn(|| EARLY.with(|_| ())).join().unwrap();
    assert_eq!(LATE_DROPS.load(Ordering::SeqCst), 1);
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn current_and_park_work_in_destructors() {
    static SEEN: Mutex<Option<(ThreadId, Option<String>)>> = Mutex::new(None);
    struct Probe;
    impl Drop for Probe {
        fn drop(&mut self) {
            let me = thread::current();
            me.unpark();
            thread::park();
            *SEEN.lock().unwrap() =
                Some((me.id(), me.name().map(str::to_string)));
        }
    }
    thread_local!(static PROBE: Probe = const { Probe });

    let handle = thread::Builder::new()
        .name("probe".into())
        .spawn(|| PROBE.with(|_| ()))
        .unwrap();
    let id = handle.thread().id();
    handle.join().unwrap();
    assert_eq!(*SEEN.lock().unwrap(), Some((id, Some("probe".to_string()))));
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn reentrant_initialization_keeps_the_outer_value() {
    static DROPS: AtomicUsize = AtomicUsize::new(0);
    static NESTED: AtomicBool = AtomicBool::new(false);
    struct Counted(u8);
    impl Drop for Counted {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }
    thread_local!(static X: Counted = {
        if NESTED.swap(true, Ordering::SeqCst) {
            Counted(1)
        } else {
            // The initializer runs again for the inner access.
            assert_eq!(X.with(|x| x.0), 1);
            Counted(2)
        }
    });
    thread::spawn(|| {
        assert_eq!(X.with(|x| x.0), 2);
        // A native value has room for one value only: as in std, the inner
        // one is dropped when the outer initializer returns. A value in a
        // heap block keeps it until the thread exits.
        let early = usize::from(cfg!(feature = "nightly"));
        assert_eq!(DROPS.load(Ordering::SeqCst), early);
    })
    .join()
    .unwrap();
    // Both values are destroyed by the time the thread is joined.
    assert_eq!(DROPS.load(Ordering::SeqCst), 2);
}

/// As in std, a `const` initializer takes a type with elided lifetimes.
#[test]
fn const_initializers_take_elided_lifetimes() {
    thread_local! {
        static WORDS: RefCell<Vec<&str>> = const { RefCell::new(Vec::new()) };
    }
    WORDS.with_borrow_mut(|words| words.push("word"));
    assert_eq!(WORDS.with_borrow(Vec::len), 1);
}

/// Items of the caller that an initializer uses are found, whatever their
/// names, not items of the expansion.
#[test]
fn initializers_see_the_callers_items() {
    fn __init() -> u32 {
        7
    }
    const __INIT: u32 = 8;
    static __STORAGE: u32 = 9;
    thread_local! {
        static LAZY: u32 = __init();
        static EAGER: u32 = const { __INIT };
        static FROM_STATIC: u32 = __STORAGE;
    }
    assert_eq!(LAZY.with(|v| *v), 7);
    assert_eq!(EAGER.with(|v| *v), 8);
    assert_eq!(FROM_STATIC.with(|v| *v), 9);
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn values_that_need_no_drop_outlive_teardown() {
    static SEEN: Mutex<Option<(Option<u32>, Option<u32>)>> = Mutex::new(None);
    struct Probe;
    impl Drop for Probe {
        fn drop(&mut self) {
            let seen = (
                CONST.try_with(Cell::get).ok(),
                LAZY.try_with(Cell::get).ok(),
            );
            *SEEN.lock().unwrap() = Some(seen);
        }
    }
    thread_local! {
        static PROBE: Probe = const { Probe };
        static CONST: Cell<u32> = const { Cell::new(1) };
        static LAZY: Cell<u32> = Cell::new(2);
    }
    thread::spawn(|| {
        PROBE.with(|_| ());
        // Newer than `PROBE`, so destroyed before it, if at all.
        CONST.set(3);
        LAZY.set(4);
    })
    .join()
    .unwrap();
    // A native value that needs no drop has no state, or is never listed,
    // so as in std it is never destroyed. Values in heap blocks all are.
    let expected = if cfg!(feature = "nightly") {
        (Some(3), Some(4))
    } else {
        (None, None)
    };
    assert_eq!(*SEEN.lock().unwrap(), Some(expected));
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn values_first_set_are_destroyed_at_exit() {
    static DROPS: AtomicUsize = AtomicUsize::new(0);
    struct Counted;
    impl Drop for Counted {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }
    thread_local! {
        static CONST: Cell<Option<Counted>> = const { Cell::new(None) };
        static LAZY: RefCell<Option<Counted>> = panic!("initializer ran");
    }
    thread::spawn(|| {
        // The first use provides the value, which is listed all the same.
        CONST.set(Some(Counted));
        LAZY.set(Some(Counted));
        assert_eq!(DROPS.load(Ordering::SeqCst), 0);
    })
    .join()
    .unwrap();
    assert_eq!(DROPS.load(Ordering::SeqCst), 2);
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn threads_created_by_other_code_destroy_their_values() {
    static DROPS: AtomicUsize = AtomicUsize::new(0);
    struct Counted;
    impl Drop for Counted {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }
    thread_local!(static X: Counted = const { Counted });
    // A std thread has no handle of litestd's until the first value that
    // needs drop creates one, which arms the exit hook.
    std::thread::spawn(|| X.with(|_| ())).join().unwrap();
    assert_eq!(DROPS.load(Ordering::SeqCst), 1);
}

/// Checks that `$key`'s value, a `$t`, is aligned for it.
macro_rules! assert_aligned {
    ($key:expr, $t:ty) => {
        $key.with(|v| {
            let addr = core::ptr::from_ref::<$t>(v).addr();
            assert_eq!(addr % core::mem::align_of::<$t>(), 0);
        });
    };
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn over_aligned_values_are_aligned() {
    static DROPS: AtomicUsize = AtomicUsize::new(0);
    static NESTED: AtomicBool = AtomicBool::new(false);
    #[repr(align(128))]
    struct Big(Cell<u8>);
    #[repr(align(128))]
    struct BigDrop(u8);
    impl Drop for BigDrop {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }
    #[repr(align(64))]
    struct Empty;
    impl Drop for Empty {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }
    // Windows and Miri keep these in heap blocks; the others in place.
    thread_local! {
        static CONST: Big = const { Big(Cell::new(1)) };
        static LAZY: Big = Big(Cell::new(2));
        static DROP_CONST: BigDrop = const { BigDrop(3) };
        static EMPTY: Empty = Empty;
        static NESTED_INIT: BigDrop = if NESTED.swap(true, Ordering::SeqCst) {
            BigDrop(4)
        } else {
            NESTED_INIT.with(|_| ());
            BigDrop(5)
        };
    }
    fn check() {
        assert_aligned!(CONST, Big);
        assert_aligned!(LAZY, Big);
        assert_aligned!(DROP_CONST, BigDrop);
        assert_aligned!(EMPTY, Empty);
        assert_eq!(CONST.with(|v| v.0.get()) + LAZY.with(|v| v.0.get()), 3);
        assert_eq!(DROP_CONST.with(|v| v.0), 3);
    }
    check();
    let before = DROPS.load(Ordering::SeqCst);
    thread::spawn(check).join().unwrap();
    std::thread::spawn(check).join().unwrap();
    // Each thread dropped its `DROP_CONST` and `EMPTY`.
    assert_eq!(DROPS.load(Ordering::SeqCst) - before, 4);
    thread::spawn(|| {
        assert_aligned!(NESTED_INIT, BigDrop);
        assert_eq!(NESTED_INIT.with(|v| v.0), 5);
    })
    .join()
    .unwrap();
    assert_eq!(DROPS.load(Ordering::SeqCst) - before, 6);
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn many_keys_on_many_threads() {
    thread_local! {
        static K0: Cell<u32> = const { Cell::new(0) };
        static K1: Cell<u32> = const { Cell::new(1) };
        static K2: Cell<u32> = const { Cell::new(2) };
        static K3: Cell<u32> = const { Cell::new(3) };
        static K4: Cell<u32> = const { Cell::new(4) };
        static K5: Cell<u32> = const { Cell::new(5) };
        static K6: Cell<u32> = const { Cell::new(6) };
        static K7: Cell<u32> = const { Cell::new(7) };
    }
    static KEYS: [&LocalKey<Cell<u32>>; 8] =
        [&K0, &K1, &K2, &K3, &K4, &K5, &K6, &K7];
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            thread::spawn(|| {
                for (i, key) in KEYS.iter().enumerate() {
                    assert_eq!(key.get(), u32::try_from(i).unwrap());
                    key.set(key.get() + 10);
                }
                KEYS.iter().map(|key| key.get()).sum::<u32>()
            })
        })
        .collect();
    for handle in handles {
        assert_eq!(handle.join().unwrap(), 28 + 80);
    }
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn values_live_across_scoped_threads() {
    thread_local!(static NAME: RefCell<String> = RefCell::new(String::new()));
    let names = ["a", "b", "c"];
    thread::scope(|s| {
        for name in names {
            s.spawn(move || {
                NAME.with_borrow_mut(|n| n.push_str(name));
                NAME.with_borrow(|n| assert_eq!(n, name));
            });
        }
    });
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn access_error_traits_match_std() {
    fn traits<T: Clone + Copy + Eq + fmt::Debug + fmt::Display + Error>() {}
    static ERROR: Mutex<Option<AccessError>> = Mutex::new(None);
    struct Grab;
    impl Drop for Grab {
        fn drop(&mut self) {
            *ERROR.lock().unwrap() = GRAB.try_with(|_| ()).err();
        }
    }
    thread_local!(static GRAB: Grab = const { Grab });

    traits::<AccessError>();
    thread::spawn(|| GRAB.with(|_| ())).join().unwrap();
    let err = ERROR.lock().unwrap().take().unwrap();
    assert_eq!(format!("{err:?}"), "AccessError");
    assert_eq!(err.to_string(), "already destroyed");
    assert_eq!(format!("{GRAB:?}"), "LocalKey { .. }");
}

/// A fiber's handle, and without `nightly` its values, belong to the fiber;
/// native values belong to the thread, as in std. Deleting a fiber from a
/// thread with values of its own leaves the thread's values and handle
/// alone.
#[cfg(windows)]
#[test]
fn deleting_a_fiber_leaves_the_deleting_thread_alone() {
    use core::{ffi::c_void, ptr};

    use windows_sys::Win32::System::Threading::{
        ConvertFiberToThread, ConvertThreadToFiber, CreateFiber, DeleteFiber,
        SwitchToFiber,
    };

    thread_local!(static VALUE: Cell<u32> = const { Cell::new(0) });

    /// Gives the fiber a value and a handle of its own, then switches back
    /// to `main` for good.
    unsafe extern "system" fn fiber_main(main: *mut c_void) {
        VALUE.set(2);
        drop(thread::current());
        // SAFETY: `main` is the thread's fiber, which waits for this.
        unsafe { SwitchToFiber(main) };
    }

    let (value, same_id) = thread::spawn(|| {
        VALUE.set(1);
        let id = thread::current().id();
        // SAFETY: the thread is not a fiber yet. The new fiber runs until it
        // switches back, and is deleted without being resumed.
        unsafe {
            let main = ConvertThreadToFiber(ptr::null());
            assert!(!main.is_null());
            let fiber = CreateFiber(0, Some(fiber_main), main);
            assert!(!fiber.is_null());
            SwitchToFiber(fiber);
            assert_ne!(ConvertFiberToThread(), 0);
            DeleteFiber(fiber);
        }
        (VALUE.try_with(Cell::get).ok(), thread::current().id() == id)
    })
    .join()
    .unwrap();
    assert_eq!(value, Some(if cfg!(feature = "nightly") { 2 } else { 1 }));
    assert!(same_id);
}

/// A thread whose current fiber has no handle deletes a fiber that has one,
/// where the exit hook finds nothing of the caller's in the slot. Native
/// values belong to the thread, which keeps running, so they must survive
/// until it exits; values in heap blocks belong to the fiber and go with it.
#[cfg(windows)]
#[test]
fn deleting_a_fiber_from_a_thread_without_a_handle() {
    use core::{ffi::c_void, ptr};

    use windows_sys::Win32::System::Threading::{
        ConvertFiberToThread, ConvertThreadToFiber, CreateFiber, DeleteFiber,
        SwitchToFiber,
    };

    static DROPS: AtomicUsize = AtomicUsize::new(0);
    struct Counted;
    impl Drop for Counted {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }
    thread_local!(static VALUE: Counted = const { Counted });

    /// Initializes the value, which gives the fiber a handle, then switches
    /// back to `main` for good.
    unsafe extern "system" fn fiber_main(main: *mut c_void) {
        VALUE.with(|_| ());
        // SAFETY: `main` is the thread's fiber, which waits for this.
        unsafe { SwitchToFiber(main) };
    }

    // A std thread, which has no handle of litestd's.
    let after_delete = std::thread::spawn(|| {
        // SAFETY: as in `deleting_a_fiber_leaves_the_deleting_thread_alone`.
        unsafe {
            let main = ConvertThreadToFiber(ptr::null());
            assert!(!main.is_null());
            let fiber = CreateFiber(0, Some(fiber_main), main);
            assert!(!fiber.is_null());
            SwitchToFiber(fiber);
            assert_ne!(ConvertFiberToThread(), 0);
            DeleteFiber(fiber);
        }
        let drops = DROPS.load(Ordering::SeqCst);
        assert!(VALUE.try_with(|_| ()).is_ok());
        // A handle arms the exit hook for the thread's own fiber.
        drop(thread::current());
        drops
    })
    .join()
    .unwrap();
    // Without `nightly`, the thread made a value of its own after the
    // deletion, destroyed at its exit.
    let expected = if cfg!(feature = "nightly") {
        (0, 1)
    } else {
        (1, 2)
    };
    assert_eq!((after_delete, DROPS.load(Ordering::SeqCst)), expected);
}

/// Counts the drops of the values that `bump_many` creates.
static MANY_DROPS: AtomicUsize = AtomicUsize::new(0);

/// A value of one of the statics of `bump_many`.
struct Counted(Cell<u32>);

impl Drop for Counted {
    fn drop(&mut self) {
        MANY_DROPS.fetch_add(1, Ordering::SeqCst);
    }
}

/// Declares sixteen statics, increments each of them, and evaluates to the
/// sum of their values. Every expansion has statics of its own.
macro_rules! sixteen {
    () => {{
        litestd::thread_local! {
            static K0: Counted = const { Counted(Cell::new(0)) };
            static K1: Counted = const { Counted(Cell::new(0)) };
            static K2: Counted = const { Counted(Cell::new(0)) };
            static K3: Counted = const { Counted(Cell::new(0)) };
            static K4: Counted = const { Counted(Cell::new(0)) };
            static K5: Counted = const { Counted(Cell::new(0)) };
            static K6: Counted = const { Counted(Cell::new(0)) };
            static K7: Counted = const { Counted(Cell::new(0)) };
            static K8: Counted = const { Counted(Cell::new(0)) };
            static K9: Counted = const { Counted(Cell::new(0)) };
            static K10: Counted = const { Counted(Cell::new(0)) };
            static K11: Counted = const { Counted(Cell::new(0)) };
            static K12: Counted = const { Counted(Cell::new(0)) };
            static K13: Counted = const { Counted(Cell::new(0)) };
            static K14: Counted = const { Counted(Cell::new(0)) };
            static K15: Counted = const { Counted(Cell::new(0)) };
        }
        [
            &K0, &K1, &K2, &K3, &K4, &K5, &K6, &K7, &K8, &K9, &K10, &K11, &K12,
            &K13, &K14, &K15,
        ]
        .iter()
        .map(|key| {
            key.with(|counted| {
                counted.0.set(counted.0.get() + 1);
                counted.0.get()
            })
        })
        .sum::<u32>()
    }};
}

/// Adds ten separate expansions of `$e`.
macro_rules! ten {
    ($e:expr) => {
        $e + $e + $e + $e + $e + $e + $e + $e + $e + $e
    };
}

/// Increments each of 1600 statics, more than glibc's 1024 pthread keys and
/// musl's 128, and returns the sum of their values.
fn bump_many() -> u32 {
    ten!(ten!(sixteen!()))
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn more_statics_than_the_system_has_keys() {
    let threads = if cfg!(miri) { 1 } else { 2 };
    let sums: Vec<u32> = (0..threads)
        .map(|_| {
            thread::spawn(|| {
                bump_many();
                bump_many()
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    // The second round finds every value at one and makes it two.
    assert!(sums.iter().all(|&sum| sum == 2 * 1600), "{sums:?}");
    // Every thread destroyed all of its values before `join` returned.
    let drops = MANY_DROPS.load(Ordering::SeqCst);
    assert_eq!(drops, threads * 1600);
}
