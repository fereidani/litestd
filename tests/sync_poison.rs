//! The poison types behave and format exactly as std's.
#![cfg(feature = "sync")]

use core::error::Error;

use litestd::sync::{PoisonError, TryLockError};

#[test]
fn poison_error_accessors() {
    let mut e = PoisonError::new(vec![1]);
    e.get_mut().push(2);
    assert_eq!(e.get_ref(), &[1, 2]);
    assert_eq!(e.into_inner(), [1, 2]);
}

#[test]
#[cfg_attr(
    panic = "abort",
    ignore = "std's PoisonError::new panics without unwinding"
)]
fn poison_error_formats_like_std() {
    let ours = PoisonError::new(5);
    let theirs = std::sync::PoisonError::new(5);
    assert_eq!(format!("{ours:?}"), format!("{theirs:?}"));
    assert_eq!(format!("{ours}"), format!("{theirs}"));
    assert_eq!(format!("{ours:>50}"), format!("{theirs:>50}"));
    assert!(ours.source().is_none());
}

#[test]
#[cfg_attr(
    panic = "abort",
    ignore = "std's PoisonError::new panics without unwinding"
)]
fn try_lock_error_formats_like_std() {
    let ours: TryLockError<()> = TryLockError::WouldBlock;
    let theirs: std::sync::TryLockError<()> =
        std::sync::TryLockError::WouldBlock;
    assert_eq!(format!("{ours:?}"), format!("{theirs:?}"));
    assert_eq!(format!("{ours}"), format!("{theirs}"));
    assert_eq!(format!("{ours:>60}"), format!("{theirs:>60}"));

    let ours = TryLockError::from(PoisonError::new(1));
    let theirs = std::sync::TryLockError::from(std::sync::PoisonError::new(1));
    assert!(matches!(ours, TryLockError::Poisoned(_)));
    assert_eq!(format!("{ours:?}"), format!("{theirs:?}"));
    assert_eq!(format!("{ours}"), format!("{theirs}"));
}

#[test]
#[allow(deprecated)]
#[cfg_attr(
    panic = "abort",
    ignore = "std's PoisonError::new panics without unwinding"
)]
fn try_lock_error_sources_like_std() {
    let ours = TryLockError::from(PoisonError::new(1));
    let theirs = std::sync::TryLockError::from(std::sync::PoisonError::new(1));
    assert_eq!(ours.source().is_some(), theirs.source().is_some());
    assert_eq!(
        ours.cause().map(ToString::to_string),
        theirs.cause().map(ToString::to_string),
    );
    let ours: TryLockError<()> = TryLockError::WouldBlock;
    assert!(ours.source().is_none());
    assert!(ours.cause().is_none());
}
