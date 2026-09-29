//! A module for working with processes.
//!
//! [`exit`], [`abort`] and [`id`] concern the current process; with the
//! `command` feature, [`Command`] spawns and manages child processes.
//!
//! # Differences from std
//!
//! - [`Child::wait_with_output`] and [`Command::output`] return an error where
//!   std panics because reading the child's output failed.
//! - [`Termination::report`] prints an `Err` only with the `stdio` feature.

#[cfg(feature = "command")]
pub(crate) mod args;
#[cfg(feature = "command")]
mod child;
#[cfg(feature = "command")]
mod command;
#[cfg(feature = "command")]
pub(crate) mod env;
mod exit_code;
#[cfg(feature = "command")]
mod status;
#[cfg(feature = "command")]
mod stdio;

#[cfg(feature = "command")]
pub(crate) use self::child::StdioPipes;
pub use self::exit_code::{ExitCode, Termination};
#[cfg(feature = "command")]
pub use self::{
    child::{Child, ChildStderr, ChildStdin, ChildStdout, Output},
    command::{Command, CommandArgs, CommandEnvs},
    status::ExitStatus,
    stdio::Stdio,
};
use crate::sys;

/// Terminates the process in an abnormal fashion.
///
/// No destructors or exit handlers run. The status is `SIGABRT` on Unix and
/// a fail-fast exception on Windows.
#[cold]
pub fn abort() -> ! {
    sys::os::abort()
}

/// Terminates the current process with the specified exit code.
///
/// No destructors run. On Unix, `atexit` handlers run, the parent sees only
/// the low eight bits of `code`, and a second thread calling this blocks
/// until the process ends; C code calling `exit` concurrently is still
/// undefined behavior, as with std. On Windows this calls `ExitProcess`.
pub fn exit(code: i32) -> ! {
    sys::os::exit(code)
}

/// Returns the OS-assigned process identifier associated with this process.
#[must_use]
pub fn id() -> u32 {
    sys::os::id()
}
