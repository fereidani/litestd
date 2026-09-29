//! The sync types have exactly std's auto traits, variance and formatting.
#![cfg(feature = "sync")]

extern crate alloc;

use alloc::rc::Rc;
use core::{
    any::Any,
    cell::Cell,
    marker::PhantomPinned,
    panic::{RefUnwindSafe, UnwindSafe},
    time::Duration,
};
use std::sync::{
    Mutex as StdMutex, OnceLock as StdOnceLock, PoisonError as StdPoisonError,
    RwLock as StdRwLock, TryLockError as StdTryLockError,
};

/// Evaluates to whether `$ty` implements `$trait`, on stable Rust: the
/// inherent constant exists only where the bound holds, and the trait's
/// default fills in elsewhere.
macro_rules! implements {
    ($ty:ty: $trait:path) => {{
        #[allow(dead_code)]
        struct Probe<T: ?Sized>(core::marker::PhantomData<T>);
        #[allow(dead_code)]
        trait Fallback {
            const IMPLS: bool = false;
        }
        impl<T: ?Sized> Fallback for Probe<T> {}
        #[allow(dead_code)]
        impl<T: ?Sized + $trait> Probe<T> {
            const IMPLS: bool = true;
        }
        <Probe<$ty>>::IMPLS
    }};
}

/// Asserts that two types agree on every auto trait.
macro_rules! assert_same_auto_traits {
    ($ours:ty, $std:ty) => {
        assert_eq!(
            [
                implements!($ours: Send),
                implements!($ours: Sync),
                implements!($ours: Unpin),
                implements!($ours: UnwindSafe),
                implements!($ours: RefUnwindSafe),
            ],
            [
                implements!($std: Send),
                implements!($std: Sync),
                implements!($std: Unpin),
                implements!($std: UnwindSafe),
                implements!($std: RefUnwindSafe),
            ],
            "Send, Sync, Unpin, UnwindSafe, RefUnwindSafe of {}",
            stringify!($ours),
        );
    };
}

/// Checks a generic family against std for parameters that cover every
/// combination of auto traits.
macro_rules! check_family {
    ($ours:ident, $std:ident, [$($param:ty),* $(,)?]) => {
        $(assert_same_auto_traits!($ours<$param>, $std<$param>);)*
    };
}

type StdMutexGuard = std::sync::MutexGuard<'static, i32>;

/// The comparisons below would pass with a probe that always answers the
/// same; check that it tells both answers apart.
#[test]
fn probe_distinguishes_impls() {
    assert!(implements!(i32: Send));
    assert!(!implements!(Rc<i32>: Send));
    assert!(implements!(Cell<i32>: Send));
    assert!(!implements!(Cell<i32>: Sync));
    assert!(!implements!(PhantomPinned: Unpin));
    assert!(!implements!(&'static mut i32: UnwindSafe));
    assert!(!implements!(Cell<i32>: RefUnwindSafe));
    assert!(!implements!(litestd::sync::MutexGuard<'static, i32>: Send));
    assert!(implements!(litestd::sync::MutexGuard<'static, i32>: Sync));
}

#[test]
fn locks_match_std() {
    use litestd::sync::{Mutex, RwLock};
    check_family!(Mutex, StdMutex, [
        i32,
        Cell<i32>,
        Rc<i32>,
        &'static mut i32,
        PhantomPinned,
        StdMutexGuard,
        *const i32,
        str,
        dyn Any,
        dyn Any + Send,
    ]);
    check_family!(RwLock, StdRwLock, [
        i32,
        Cell<i32>,
        Rc<i32>,
        &'static mut i32,
        PhantomPinned,
        StdMutexGuard,
        *const i32,
        str,
        dyn Any,
        dyn Any + Sync,
    ]);
}

type OurMutexGuard<T> = litestd::sync::MutexGuard<'static, T>;
type StdMutexGuardOf<T> = std::sync::MutexGuard<'static, T>;
type OurReadGuard<T> = litestd::sync::RwLockReadGuard<'static, T>;
type StdReadGuard<T> = std::sync::RwLockReadGuard<'static, T>;
type OurWriteGuard<T> = litestd::sync::RwLockWriteGuard<'static, T>;
type StdWriteGuard<T> = std::sync::RwLockWriteGuard<'static, T>;

#[test]
fn guards_match_std() {
    check_family!(OurMutexGuard, StdMutexGuardOf, [
        i32,
        Cell<i32>,
        Rc<i32>,
        &'static mut i32,
        PhantomPinned,
        StdMutexGuard,
        *const i32,
        str,
        dyn Any,
    ]);
    check_family!(OurReadGuard, StdReadGuard, [
        i32,
        Cell<i32>,
        Rc<i32>,
        &'static mut i32,
        PhantomPinned,
        StdMutexGuard,
        *const i32,
        str,
        dyn Any,
    ]);
    check_family!(OurWriteGuard, StdWriteGuard, [
        i32,
        Cell<i32>,
        Rc<i32>,
        &'static mut i32,
        PhantomPinned,
        StdMutexGuard,
        *const i32,
        str,
        dyn Any,
    ]);
}

type OurLazy<F> = litestd::sync::LazyLock<i32, F>;
type StdLazy<F> = std::sync::LazyLock<i32, F>;
type OurLazyOf<T> = litestd::sync::LazyLock<T>;
type StdLazyOf<T> = std::sync::LazyLock<T>;

#[test]
fn cells_match_std() {
    use litestd::sync::OnceLock;
    check_family!(OnceLock, StdOnceLock, [
        i32,
        Cell<i32>,
        Rc<i32>,
        &'static mut i32,
        PhantomPinned,
        StdMutexGuard,
        *const i32,
    ]);
    check_family!(OurLazyOf, StdLazyOf, [
        i32,
        Cell<i32>,
        Rc<i32>,
        &'static mut i32,
        PhantomPinned,
        StdMutexGuard,
        *const i32,
    ]);
    check_family!(OurLazy, StdLazy, [
        fn() -> i32,
        Cell<i32>,
        Rc<i32>,
        &'static mut i32,
        PhantomPinned,
        StdMutexGuard,
        *const i32,
    ]);
}

#[test]
fn plain_types_match_std() {
    use litestd::sync::{
        Barrier, BarrierWaitResult, Condvar, Once, OnceState, WaitTimeoutResult,
    };
    assert_same_auto_traits!(Condvar, std::sync::Condvar);
    assert_same_auto_traits!(WaitTimeoutResult, std::sync::WaitTimeoutResult);
    assert_same_auto_traits!(Once, std::sync::Once);
    assert_same_auto_traits!(OnceState, std::sync::OnceState);
    assert_same_auto_traits!(Barrier, std::sync::Barrier);
    assert_same_auto_traits!(BarrierWaitResult, std::sync::BarrierWaitResult);
}

/// `Default` exactly where std implements it, including `Once`, whose impl
/// std stabilizes in Rust 1.100. Not compared against the std of the running
/// toolchain, which may predate that impl.
#[test]
fn default_impls_match_std() {
    use litestd::sync::{
        Barrier, Condvar, LazyLock, Mutex, Once, OnceLock, OnceState, RwLock,
    };
    assert!(implements!(Mutex<i32>: Default));
    assert!(implements!(RwLock<i32>: Default));
    assert!(implements!(Condvar: Default));
    assert!(implements!(OnceLock<i32>: Default));
    assert!(implements!(LazyLock<i32>: Default));
    assert!(implements!(Once: Default));
    assert!(!implements!(OnceState: Default));
    assert!(!implements!(Barrier: Default));
}

#[test]
fn errors_match_std() {
    use litestd::sync::{PoisonError, TryLockError};
    check_family!(PoisonError, StdPoisonError, [
        i32,
        Cell<i32>,
        Rc<i32>,
        &'static mut i32,
        PhantomPinned,
        StdMutexGuard,
        *const i32,
    ]);
    check_family!(TryLockError, StdTryLockError, [
        i32,
        Cell<i32>,
        Rc<i32>,
        &'static mut i32,
        PhantomPinned,
        StdMutexGuard,
        *const i32,
    ]);
}

/// A read guard is covariant over its data, like `&T`. This only has to
/// compile.
#[allow(dead_code, clippy::missing_const_for_fn)]
fn read_guard_is_covariant<'a>(
    guard: litestd::sync::RwLockReadGuard<'a, &'static str>,
) -> litestd::sync::RwLockReadGuard<'a, &'a str> {
    guard
}

#[test]
fn debug_matches_std() {
    let ours = litestd::sync::Mutex::new(vec![1]);
    let theirs = std::sync::Mutex::new(vec![1]);
    assert_eq!(format!("{ours:?}"), format!("{theirs:?}"));
    {
        let (a, b) = (ours.lock().unwrap(), theirs.lock().unwrap());
        assert_eq!(format!("{ours:?}"), format!("{theirs:?}"));
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
    }

    let ours = litestd::sync::RwLock::new("x");
    let theirs = std::sync::RwLock::new("x");
    assert_eq!(format!("{ours:?}"), format!("{theirs:?}"));
    {
        let (a, b) = (ours.write().unwrap(), theirs.write().unwrap());
        assert_eq!(format!("{ours:?}"), format!("{theirs:?}"));
        assert_eq!(format!("{a:?} {a}"), format!("{b:?} {b}"));
    }
    {
        let (a, b) = (ours.read().unwrap(), theirs.read().unwrap());
        assert_eq!(format!("{a:?} {a}"), format!("{b:?} {b}"));
    }

    assert_eq!(
        format!("{:?}", litestd::sync::Condvar::new()),
        format!("{:?}", std::sync::Condvar::new()),
    );
    assert_eq!(
        format!("{:?}", litestd::sync::Once::new()),
        format!("{:?}", std::sync::Once::new()),
    );
    assert_eq!(
        format!("{:?}", litestd::sync::Barrier::new(3)),
        format!("{:?}", std::sync::Barrier::new(3)),
    );
    assert_eq!(
        format!("{:?}", litestd::sync::Barrier::new(1).wait()),
        format!("{:?}", std::sync::Barrier::new(1).wait()),
    );
}

#[test]
fn cell_debug_matches_std() {
    let ours = litestd::sync::OnceLock::new();
    let theirs = std::sync::OnceLock::new();
    assert_eq!(format!("{ours:?}"), format!("{theirs:?}"));
    ours.set(3).unwrap();
    theirs.set(3).unwrap();
    assert_eq!(format!("{ours:?}"), format!("{theirs:?}"));

    let ours = litestd::sync::LazyLock::new(|| 5);
    let theirs = std::sync::LazyLock::new(|| 5);
    assert_eq!(format!("{ours:?}"), format!("{theirs:?}"));
    assert_eq!(*ours, *theirs);
    assert_eq!(format!("{ours:?}"), format!("{theirs:?}"));

    let mut ours_state = String::new();
    let mut theirs_state = String::new();
    litestd::sync::Once::new()
        .call_once_force(|s| ours_state = format!("{s:?}"));
    std::sync::Once::new().call_once_force(|s| theirs_state = format!("{s:?}"));
    assert_eq!(ours_state, theirs_state);
}

#[test]
#[cfg_attr(
    target_os = "unknown",
    ignore = "wasm32-unknown-unknown has no clock"
)]
fn wait_timeout_result_debug_matches_std() {
    let (m, c) = (litestd::sync::Mutex::new(()), litestd::sync::Condvar::new());
    let (sm, sc) = (std::sync::Mutex::new(()), std::sync::Condvar::new());
    let dur = Duration::from_millis(1);
    let (_g, ours) = c.wait_timeout(m.lock().unwrap(), dur).unwrap();
    let (_h, theirs) = sc.wait_timeout(sm.lock().unwrap(), dur).unwrap();
    assert!(ours.timed_out());
    assert_eq!(format!("{ours:?}"), format!("{theirs:?}"));
    assert_eq!(ours, ours.clone());
}
