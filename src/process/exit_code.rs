//! [`ExitCode`] and the [`Termination`] trait.

use core::fmt;

/// This type represents the status code the current process can return to
/// its parent under normal termination.
///
/// It provides the platform's canonical [`SUCCESS`](ExitCode::SUCCESS) and
/// [`FAILURE`](ExitCode::FAILURE) codes, and `From<u8>` for any other; like
/// std's, it offers `PartialEq` but no `Eq`, `Hash` or access to the raw
/// value, since a platform may know several failure codes.
#[derive(Clone, Copy, PartialEq)]
#[allow(clippy::derive_partial_eq_without_eq, reason = "not `Eq` in std")]
pub struct ExitCode(u8);

impl ExitCode {
    /// The canonical `ExitCode` for successful termination on this platform.
    pub const SUCCESS: Self = Self(0);

    /// The canonical `ExitCode` for unsuccessful termination on this
    /// platform.
    pub const FAILURE: Self = Self(1);
}

impl Default for ExitCode {
    /// Returns [`ExitCode::SUCCESS`].
    fn default() -> Self {
        Self::SUCCESS
    }
}

impl From<u8> for ExitCode {
    /// Constructs an `ExitCode` from an arbitrary u8 value.
    fn from(code: u8) -> Self {
        Self(code)
    }
}

impl fmt::Debug for ExitCode {
    /// Formats the code as std does on each platform.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        /// The platform code, named as std's backends name it.
        struct Raw(u8);

        impl fmt::Debug for Raw {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let name = if cfg!(unix) {
                    "unix_exit_status"
                } else {
                    "ExitCode"
                };
                f.debug_tuple(name).field(&self.0).finish()
            }
        }

        f.debug_tuple("ExitCode").field(&Raw(self.0)).finish()
    }
}

/// A trait for implementing arbitrary return types in the `main` function.
///
/// litestd has no runtime that calls `main`, so nothing calls
/// [`report`](Termination::report) implicitly; the trait and std's
/// implementations exist so that code written against std compiles.
pub trait Termination {
    /// Is called to get the representation of the value as status code.
    /// This status code is returned to the operating system.
    fn report(self) -> ExitCode;
}

impl Termination for () {
    #[inline]
    fn report(self) -> ExitCode {
        ExitCode::SUCCESS
    }
}

mod never {
    /// Names a function pointer's return type, the only place where stable
    /// Rust accepts `!`.
    pub trait FnOutput {
        type Output;
    }

    impl<R> FnOutput for fn() -> R {
        type Output = R;
    }
}

/// The never type `!`, which std implements [`Termination`] for.
type Never = <fn() -> ! as never::FnOutput>::Output;

impl Termination for Never {
    fn report(self) -> ExitCode {
        self
    }
}

impl Termination for ExitCode {
    #[inline]
    fn report(self) -> ExitCode {
        self
    }
}

impl<T: Termination, E: fmt::Debug> Termination for Result<T, E> {
    /// Reports `Ok` values as they report themselves. An `Err` is printed to
    /// the standard error as `Error: {err:?}`, as in std, when the `stdio`
    /// feature is enabled, and reports [`ExitCode::FAILURE`].
    fn report(self) -> ExitCode {
        match self {
            Ok(val) => val.report(),
            Err(err) => {
                #[cfg(feature = "stdio")]
                #[allow(clippy::used_underscore_items, reason = "`eprintln!`")]
                crate::_eprintln(format_args!("Error: {err:?}"));
                #[cfg(not(feature = "stdio"))]
                let _ = err;
                ExitCode::FAILURE
            }
        }
    }
}
