//! The standard stream handles have std's auto traits, trait
//! implementations, formatting, descriptors and handles.

#![cfg(all(feature = "stdio", feature = "io"))]

use core::{
    fmt,
    panic::{RefUnwindSafe, UnwindSafe},
};

/// Evaluates to whether `$ty` implements `$trait`, on stable Rust.
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

#[test]
fn auto_traits_match_std() {
    use litestd::io as lite;
    assert_same_auto_traits!(lite::Stdin, std::io::Stdin);
    assert_same_auto_traits!(
        lite::StdinLock<'static>,
        std::io::StdinLock<'static>
    );
    assert_same_auto_traits!(lite::Stdout, std::io::Stdout);
    assert_same_auto_traits!(
        lite::StdoutLock<'static>,
        std::io::StdoutLock<'static>
    );
    assert_same_auto_traits!(lite::Stderr, std::io::Stderr);
    assert_same_auto_traits!(
        lite::StderrLock<'static>,
        std::io::StderrLock<'static>
    );
}

#[test]
fn trait_impls_match_std() {
    use litestd::io::{self as lite, BufRead, IsTerminal, Read, Write};
    fn read<T: Read>() {}
    fn buf_read<T: BufRead>() {}
    fn write<T: Write>() {}
    fn is_terminal<T: IsTerminal>() {}
    fn debug<T: fmt::Debug>() {}
    read::<lite::Stdin>();
    read::<&lite::Stdin>();
    read::<lite::StdinLock<'static>>();
    buf_read::<lite::StdinLock<'static>>();
    write::<lite::Stdout>();
    write::<&lite::Stdout>();
    write::<lite::StdoutLock<'static>>();
    write::<lite::Stderr>();
    write::<&lite::Stderr>();
    write::<lite::StderrLock<'static>>();
    is_terminal::<lite::Stdin>();
    is_terminal::<lite::StdinLock<'_>>();
    is_terminal::<lite::Stdout>();
    is_terminal::<lite::StdoutLock<'_>>();
    is_terminal::<lite::Stderr>();
    is_terminal::<lite::StderrLock<'_>>();
    debug::<lite::Stdin>();
    debug::<lite::StdinLock<'_>>();
    debug::<lite::StdoutLock<'_>>();
    debug::<lite::StderrLock<'_>>();
    // The locks are `'static`, as std's are.
    let _: fn(&lite::Stdin) -> lite::StdinLock<'static> = lite::Stdin::lock;
    let _: fn(&lite::Stdout) -> lite::StdoutLock<'static> = lite::Stdout::lock;
    let _: fn(&lite::Stderr) -> lite::StderrLock<'static> = lite::Stderr::lock;
    let _: fn(lite::Stdin) -> lite::Lines<lite::StdinLock<'static>> =
        lite::Stdin::lines;
}

#[cfg(all(unix, any(feature = "fs", feature = "net")))]
#[test]
fn descriptors_match_std() {
    use litestd::os::fd::{AsFd, AsRawFd};
    let lite = [
        litestd::io::stdin().as_raw_fd(),
        litestd::io::stdout().as_raw_fd(),
        litestd::io::stderr().as_raw_fd(),
        litestd::io::stdout().lock().as_fd().as_raw_fd(),
        litestd::io::stderr().lock().as_fd().as_raw_fd(),
    ];
    let std = {
        use std::os::fd::{AsFd, AsRawFd};
        [
            std::io::stdin().as_raw_fd(),
            std::io::stdout().as_raw_fd(),
            std::io::stderr().as_raw_fd(),
            std::io::stdout().lock().as_fd().as_raw_fd(),
            std::io::stderr().lock().as_fd().as_raw_fd(),
        ]
    };
    assert_eq!(lite, std);
}

#[cfg(all(windows, any(feature = "fs", feature = "net")))]
#[test]
fn handles_match_std() {
    use litestd::os::windows::io::{AsHandle, AsRawHandle};
    let lite = [
        litestd::io::stdin().as_raw_handle(),
        litestd::io::stdout().as_raw_handle(),
        litestd::io::stderr().as_handle().as_raw_handle(),
        litestd::io::stdout().lock().as_raw_handle(),
    ];
    let std = {
        use std::os::windows::io::{AsHandle, AsRawHandle};
        [
            std::io::stdin().as_raw_handle(),
            std::io::stdout().as_raw_handle(),
            std::io::stderr().as_handle().as_raw_handle(),
            std::io::stdout().lock().as_raw_handle(),
        ]
    };
    assert_eq!(lite.map(<*mut _>::addr), std.map(<*mut _>::addr));
}

#[test]
fn debug_matches_std() {
    assert_eq!(
        format!("{:?}", litestd::io::stdin()),
        format!("{:?}", std::io::stdin())
    );
    assert_eq!(
        format!("{:?}", litestd::io::stdout()),
        format!("{:?}", std::io::stdout())
    );
    assert_eq!(
        format!("{:?}", litestd::io::stderr()),
        format!("{:?}", std::io::stderr())
    );
    assert_eq!(
        format!("{:?}", litestd::io::stdout().lock()),
        format!("{:?}", std::io::stdout().lock())
    );
    assert_eq!(
        format!("{:?}", litestd::io::stderr().lock()),
        format!("{:?}", std::io::stderr().lock())
    );
    // Only the litestd lock: std's would wait for the harness's reader.
    assert_eq!(
        format!("{:?}", litestd::io::stdin().lock()),
        "StdinLock { .. }"
    );
}
