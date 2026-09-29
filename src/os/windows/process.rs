//! Windows-specific extensions to primitives in the [`process`] module.
//!
//! [`Child`](process::Child), [`ChildStdin`](process::ChildStdin),
//! [`ChildStdout`](process::ChildStdout) and
//! [`ChildStderr`](process::ChildStderr) give access to their handles, and
//! a [`Stdio`](process::Stdio) can be made from a handle.

use crate::{
    ffi::OsStr,
    os::windows::io::{
        AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, IntoRawHandle,
        OwnedHandle, RawHandle,
    },
    process, sys,
};

mod private {
    /// Seals the extension traits so that, as in std, methods can be added.
    pub trait Sealed {}

    impl Sealed for crate::process::Command {}
    impl Sealed for crate::process::ExitStatus {}
}

impl FromRawHandle for process::Stdio {
    /// Takes ownership of `handle`, which each child spawned with the
    /// `Stdio` gets a duplicate of.
    unsafe fn from_raw_handle(handle: RawHandle) -> Self {
        // SAFETY: the caller passes an owned handle, as `OwnedHandle` needs.
        Self::from(unsafe { OwnedHandle::from_raw_handle(handle) })
    }
}

impl From<OwnedHandle> for process::Stdio {
    /// Takes ownership of a handle and returns a [`Stdio`](process::Stdio)
    /// that can attach a stream to it.
    fn from(handle: OwnedHandle) -> Self {
        Self(sys::process::Stdio::Handle(handle))
    }
}

impl AsRawHandle for process::Child {
    /// Returns the handle of the child process.
    #[inline]
    fn as_raw_handle(&self) -> RawHandle {
        self.handle.handle().as_raw_handle()
    }
}

impl AsHandle for process::Child {
    #[inline]
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.handle.handle().as_handle()
    }
}

impl IntoRawHandle for process::Child {
    /// Consumes the `Child`, returning the handle of the process, which the
    /// caller must close; the pipes to the child are closed.
    fn into_raw_handle(self) -> RawHandle {
        self.handle.into_handle().into_raw_handle()
    }
}

impl From<process::Child> for OwnedHandle {
    /// Takes ownership of a [`Child`](process::Child)'s process handle.
    fn from(child: process::Child) -> Self {
        child.handle.into_handle()
    }
}

macro_rules! impl_pipe {
    ($($t:ident)*) => {$(
        impl AsRawHandle for process::$t {
            #[inline]
            fn as_raw_handle(&self) -> RawHandle {
                self.inner.handle().as_raw_handle()
            }
        }

        impl AsHandle for process::$t {
            #[inline]
            fn as_handle(&self) -> BorrowedHandle<'_> {
                self.inner.handle().as_handle()
            }
        }

        impl IntoRawHandle for process::$t {
            fn into_raw_handle(self) -> RawHandle {
                self.inner.into_handle().into_raw_handle()
            }
        }

        impl From<process::$t> for OwnedHandle {
            #[doc = concat!(
                "Takes ownership of a [`", stringify!($t), "`](process::",
                stringify!($t), ")'s pipe handle."
            )]
            fn from(pipe: process::$t) -> Self {
                pipe.inner.into_handle()
            }
        }

        impl From<OwnedHandle> for process::$t {
            #[doc = concat!(
                "Creates a [`", stringify!($t), "`](process::",
                stringify!($t), ") from `handle`, which must be open for ",
                "overlapped I/O, as reading and writing use it."
            )]
            fn from(handle: OwnedHandle) -> Self {
                Self {
                    inner: sys::process::ChildPipe::from_handle(handle),
                }
            }
        }
    )*};
}

impl_pipe! { ChildStdin ChildStdout ChildStderr }

/// Windows-specific extensions to [`process::ExitStatus`]. This trait is
/// sealed: it cannot be implemented outside litestd.
pub trait ExitStatusExt: private::Sealed {
    /// Creates a new `ExitStatus` from the raw underlying `u32` return value
    /// of a process.
    fn from_raw(raw: u32) -> Self;
}

impl ExitStatusExt for process::ExitStatus {
    fn from_raw(raw: u32) -> Self {
        Self(sys::process::ExitStatus::from(raw))
    }
}

/// Windows-specific extensions to the [`process::Command`] builder. This
/// trait is sealed: it cannot be implemented outside litestd.
pub trait CommandExt: private::Sealed {
    /// Sets the [process creation flags][1] to be passed to `CreateProcess`.
    ///
    /// They are always combined with `CREATE_UNICODE_ENVIRONMENT`.
    ///
    /// [1]: https://docs.microsoft.com/en-us/windows/win32/procthread/process-creation-flags
    fn creation_flags(&mut self, flags: u32) -> &mut process::Command;

    /// Appends literal text to the command line without any quoting or
    /// escaping.
    ///
    /// This is useful for passing arguments to applications that don't
    /// follow the standard C run-time escaping rules, such as `cmd.exe /c`.
    ///
    /// # Batch files
    ///
    /// Note the `cmd /c` command line has slightly different escaping rules
    /// than batch files themselves. If possible, it may be better to write
    /// complex arguments to a temporary `.bat` file, with appropriate
    /// escaping, and simply run that using:
    ///
    /// ```no_run
    /// # use litestd::process::Command;
    /// # let temp_bat_file = "";
    /// # #[allow(unused)]
    /// let output = Command::new("cmd")
    ///     .args(["/c", &format!("\"{temp_bat_file}\"")])
    ///     .output();
    /// ```
    ///
    /// Raw text is never checked or escaped, even for a batch file: validate
    /// untrusted input before passing it here.
    fn raw_arg<S: AsRef<OsStr>>(
        &mut self,
        text_to_append_as_is: S,
    ) -> &mut process::Command;
}

impl CommandExt for process::Command {
    fn creation_flags(&mut self, flags: u32) -> &mut process::Command {
        self.as_inner_mut().creation_flags(flags);
        self
    }

    fn raw_arg<S: AsRef<OsStr>>(
        &mut self,
        text_to_append_as_is: S,
    ) -> &mut process::Command {
        self.as_inner_mut().raw_arg(text_to_append_as_is.as_ref());
        self
    }
}
