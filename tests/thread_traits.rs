//! The thread types implement exactly the traits of their std counterparts,
//! auto traits included.

#![cfg(feature = "thread")]

use core::{
    cell::Cell,
    error::Error,
    fmt::{Debug, Display},
    hash::Hash,
    panic::{RefUnwindSafe, UnwindSafe},
};

use litestd::{rc::Rc, thread};

/// Evaluates to whether `$ty` implements `$tr`. The inherent constant takes
/// precedence over the trait's, but exists only where the bound holds.
macro_rules! implements {
    ($ty:ty: $tr:path) => {{
        struct Probe<T: ?Sized>(core::marker::PhantomData<T>);
        #[allow(dead_code)]
        trait Fallback {
            const YES: bool = false;
        }
        impl<T: ?Sized> Fallback for Probe<T> {}
        #[allow(dead_code)]
        impl<T: ?Sized + $tr> Probe<T> {
            const YES: bool = true;
        }
        <Probe<$ty>>::YES
    }};
}

/// Asserts that `$lite` and `$std` agree on every trait listed.
macro_rules! same_traits {
    ($lite:ty, $std:ty: $($tr:path),+ $(,)?) => {
        $(
            assert_eq!(
                implements!($lite: $tr),
                implements!($std: $tr),
                "{} and {} disagree on {}",
                stringify!($lite),
                stringify!($std),
                stringify!($tr),
            );
        )+
    };
}

/// Asserts that `$lite` and `$std` agree on the auto traits and on the
/// common derivable and formatting traits.
macro_rules! same_type_traits {
    ($lite:ty, $std:ty) => {
        same_traits!(
            $lite, $std: Send, Sync, Unpin, UnwindSafe, RefUnwindSafe, Clone,
            Copy, Debug, Default, Display, Eq, Error, Hash, Ord, PartialEq,
            PartialOrd,
        );
    };
}

#[test]
#[cfg_attr(
    all(miri, target_vendor = "apple"),
    ignore = "std parks with pthreads under Miri on macOS, which changes the \
              auto traits of its Thread"
)]
fn handles_and_ids() {
    same_type_traits!(thread::Thread, std::thread::Thread);
    same_type_traits!(thread::ThreadId, std::thread::ThreadId);
    same_type_traits!(thread::Builder, std::thread::Builder);
}

#[test]
fn join_handles() {
    same_type_traits!(thread::JoinHandle<u8>, std::thread::JoinHandle<u8>);
    same_type_traits!(
        thread::JoinHandle<Rc<u8>>,
        std::thread::JoinHandle<Rc<u8>>
    );
    same_type_traits!(
        thread::JoinHandle<Cell<u8>>,
        std::thread::JoinHandle<Cell<u8>>
    );
}

#[test]
#[cfg_attr(
    all(miri, target_vendor = "apple"),
    ignore = "std parks with pthreads under Miri on macOS, which changes the \
              auto traits of its Thread"
)]
fn scopes() {
    same_type_traits!(
        thread::Scope<'static, 'static>,
        std::thread::Scope<'static, 'static>
    );
    same_type_traits!(
        thread::ScopedJoinHandle<'static, u8>,
        std::thread::ScopedJoinHandle<'static, u8>
    );
    same_type_traits!(
        thread::ScopedJoinHandle<'static, Rc<u8>>,
        std::thread::ScopedJoinHandle<'static, Rc<u8>>
    );
    same_type_traits!(
        thread::ScopedJoinHandle<'static, Cell<u8>>,
        std::thread::ScopedJoinHandle<'static, Cell<u8>>
    );
}

#[test]
fn local_keys() {
    same_type_traits!(thread::LocalKey<u8>, std::thread::LocalKey<u8>);
    same_type_traits!(
        thread::LocalKey<Cell<u8>>,
        std::thread::LocalKey<Cell<u8>>
    );
    same_type_traits!(thread::LocalKey<Rc<u8>>, std::thread::LocalKey<Rc<u8>>);
    same_type_traits!(thread::AccessError, std::thread::AccessError);
}

#[test]
fn result_is_std_result() {
    let lite: thread::Result<u8> = Ok(1);
    let std: std::thread::Result<u8> = lite;
    assert_eq!(std.ok(), Some(1));
}

/// Results that std lets callers ignore can be ignored without a warning.
#[test]
#[deny(unused_must_use)]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn ignorable_results_match_std() {
    let handle = thread::spawn(|| ());
    handle.is_finished();
    thread::scope(|s| {
        s.spawn(|| ()).is_finished();
    });
    handle.join().unwrap();
}
