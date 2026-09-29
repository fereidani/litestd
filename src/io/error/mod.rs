//! [`Error`], [`ErrorKind`] and [`Result`].
//!
//! An error takes one of four `Form`s. `Repr` stores them in one word on
//! 64-bit targets, like std, and as a plain enum elsewhere; everything else
//! here goes through its `form`, `form_mut` and `into_form`.

mod kind;
#[cfg_attr(target_pointer_width = "64", path = "packed.rs")]
#[cfg_attr(not(target_pointer_width = "64"), path = "unpacked.rs")]
mod repr;

use core::{error, fmt, result};

use alloc_crate::{boxed::Box, collections::TryReserveError, ffi::NulError};

pub use self::kind::ErrorKind;
use self::repr::Repr;
use crate::sys;

/// A specialized [`Result`](result::Result) type for I/O operations.
pub type Result<T> = result::Result<T, Error>;

/// The error type for I/O and other operating system operations. An OS error
/// is just a code, and one with a static message allocates nothing.
pub struct Error {
    repr: Repr,
}

/// The forms of an error, with a custom error as `C`: borrowed, mutably
/// borrowed or owned.
enum Form<C> {
    Os(i32),
    Simple(ErrorKind),
    Message(&'static SimpleMessage),
    Custom(C),
}

/// A kind and a static message, for errors that allocate nothing.
pub(crate) struct SimpleMessage {
    pub(crate) kind: ErrorKind,
    pub(crate) message: &'static str,
}

struct Custom {
    kind: ErrorKind,
    error: Box<dyn error::Error + Send + Sync>,
}

/// Creates an [`Error`] with a static message, without allocating.
macro_rules! const_error {
    ($kind:expr, $message:expr $(,)?) => {
        $crate::io::Error::from_static_message(
            const {
                &$crate::io::SimpleMessage {
                    kind: $kind,
                    message: $message,
                }
            },
        )
    };
}
pub(crate) use const_error;

impl Error {
    /// Creates a new I/O error from a known kind and an arbitrary payload.
    pub fn new<E>(kind: ErrorKind, error: E) -> Self
    where
        E: Into<Box<dyn error::Error + Send + Sync>>,
    {
        Self::custom(kind, error.into())
    }

    /// Creates a new I/O error of kind [`ErrorKind::Other`] from a payload.
    pub fn other<E>(error: E) -> Self
    where
        E: Into<Box<dyn error::Error + Send + Sync>>,
    {
        Self::custom(ErrorKind::Other, error.into())
    }

    // Kept out of the generic constructors so that each payload type adds
    // only a conversion, not another copy of this code.
    fn custom(
        kind: ErrorKind,
        error: Box<dyn error::Error + Send + Sync>,
    ) -> Self {
        let custom = Box::new(Custom { kind, error });
        Self {
            repr: Repr::new(Form::Custom(custom)),
        }
    }

    /// Creates an error from a static message without allocating.
    pub(crate) const fn from_static_message(
        message: &'static SimpleMessage,
    ) -> Self {
        Self {
            repr: Repr::from_message(message),
        }
    }

    /// Returns the calling thread's last OS error: `errno` or `GetLastError`.
    #[must_use]
    #[inline]
    pub fn last_os_error() -> Self {
        Self::from_raw_os_error(sys::os::errno())
    }

    /// Creates a new [`Error`] from a particular OS error code.
    #[must_use]
    #[inline]
    #[allow(clippy::missing_const_for_fn, reason = "not `const` in std")]
    pub fn from_raw_os_error(code: i32) -> Self {
        Self {
            repr: Repr::new(Form::Os(code)),
        }
    }

    /// Returns the OS error that this error represents, if any.
    #[must_use]
    #[inline]
    pub fn raw_os_error(&self) -> Option<i32> {
        match self.repr.form() {
            Form::Os(code) => Some(code),
            _ => None,
        }
    }

    /// Returns a reference to the inner error wrapped by this error, if any.
    #[must_use]
    #[inline]
    pub fn get_ref(
        &self,
    ) -> Option<&(dyn error::Error + Send + Sync + 'static)> {
        match self.repr.form() {
            Form::Custom(custom) => Some(&*custom.error),
            _ => None,
        }
    }

    /// Returns a mutable reference to the inner error, if any.
    #[must_use]
    #[inline]
    pub fn get_mut(
        &mut self,
    ) -> Option<&mut (dyn error::Error + Send + Sync + 'static)> {
        match self.repr.form_mut() {
            Form::Custom(custom) => Some(&mut *custom.error),
            _ => None,
        }
    }

    /// Consumes the error, returning its inner error, if any.
    #[must_use = "`self` will be dropped if the result is not used"]
    pub fn into_inner(self) -> Option<Box<dyn error::Error + Send + Sync>> {
        match self.repr.into_form() {
            Form::Custom(custom) => Some(custom.error),
            _ => None,
        }
    }

    /// Attempts to downcast the custom boxed error to `E`.
    ///
    /// # Errors
    ///
    /// Returns `self` unchanged if it does not wrap an `E`.
    pub fn downcast<E>(self) -> result::Result<E, Self>
    where
        E: error::Error + Send + Sync + 'static,
    {
        match self.repr.form() {
            Form::Custom(custom) if custom.error.is::<E>() => {}
            _ => return Err(self),
        }
        // `is` has just seen an `E` in the box, so neither fallback runs.
        match self.repr.into_form() {
            Form::Custom(custom) => {
                let Custom { kind, error } = *custom;
                error
                    .downcast()
                    .map(|error| *error)
                    .map_err(|error| Self::custom(kind, error))
            }
            form => Err(Self {
                repr: Repr::new(form),
            }),
        }
    }

    /// Returns the corresponding [`ErrorKind`] for this error.
    // Not inlined: the table that decodes OS codes would come along.
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        match self.repr.form() {
            Form::Os(code) => sys::os::decode_error_kind(code),
            Form::Simple(kind) => kind,
            Form::Message(message) => message.kind,
            Form::Custom(custom) => custom.kind,
        }
    }

    /// Returns whether this error is of kind [`ErrorKind::Interrupted`], which
    /// the I/O helpers retry. An OS error is compared with the one code that
    /// means it, without the table that decodes every code.
    #[inline]
    pub(crate) fn is_interrupted(&self) -> bool {
        let kind = match self.repr.form() {
            Form::Os(code) => return sys::os::is_interrupted(code),
            Form::Simple(kind) => kind,
            Form::Message(message) => message.kind,
            Form::Custom(custom) => custom.kind,
        };
        kind == ErrorKind::Interrupted
    }
}

/// The errors that the I/O traits and types report.
impl Error {
    pub(crate) const INVALID_UTF8: Self = const_error!(
        ErrorKind::InvalidData,
        "stream did not contain valid UTF-8"
    );
    pub(crate) const READ_EXACT_EOF: Self =
        const_error!(ErrorKind::UnexpectedEof, "failed to fill whole buffer");
    pub(crate) const WRITE_ALL_EOF: Self =
        const_error!(ErrorKind::WriteZero, "failed to write whole buffer");
}

impl From<ErrorKind> for Error {
    /// Converts an [`ErrorKind`] into an [`Error`] without allocating.
    #[inline]
    fn from(kind: ErrorKind) -> Self {
        Self {
            repr: Repr::new(Form::Simple(kind)),
        }
    }
}

impl From<NulError> for Error {
    /// Converts a [`NulError`] into an [`ErrorKind::InvalidInput`] error.
    fn from(_: NulError) -> Self {
        const_error!(
            ErrorKind::InvalidInput,
            "data provided contains a nul byte"
        )
    }
}

impl From<TryReserveError> for Error {
    /// Converts a [`TryReserveError`] into an [`ErrorKind::OutOfMemory`] error.
    fn from(_: TryReserveError) -> Self {
        ErrorKind::OutOfMemory.into()
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.repr.form() {
            Form::Os(code) => debug_struct(
                f,
                "Os",
                &[
                    ("code", &code),
                    ("kind", &sys::os::decode_error_kind(code)),
                    ("message", &OsMessage(code)),
                ],
            ),
            Form::Simple(kind) => f.debug_tuple("Kind").field(&kind).finish(),
            Form::Message(message) => debug_struct(
                f,
                "Error",
                &[("kind", &message.kind), ("message", &message.message)],
            ),
            Form::Custom(custom) => debug_struct(
                f,
                "Custom",
                &[("kind", &custom.kind), ("error", &custom.error)],
            ),
        }
    }
}

/// Formats a struct as `f.debug_struct(name)` with `fields` does. One copy
/// serves every form, where each builder chain would inline its own.
#[inline(never)]
fn debug_struct(
    f: &mut fmt::Formatter<'_>,
    name: &str,
    fields: &[(&str, &dyn fmt::Debug)],
) -> fmt::Result {
    let mut builder = f.debug_struct(name);
    for (name, value) in fields {
        builder.field(name, value);
    }
    builder.finish()
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.repr.form() {
            Form::Os(code) => {
                sys::os::fmt_error_message(code, f)?;
                write!(f, " (os error {code})")
            }
            Form::Simple(kind) => f.write_str(kind.as_str()),
            // Like std, a static message honors width and precision, and a
            // custom error gets them; kinds and OS errors ignore them.
            Form::Message(message) => crate::ffi::pad(message.message, f),
            Form::Custom(custom) => fmt::Display::fmt(&custom.error, f),
        }
    }
}

/// The OS's description of an error code, which `Debug` shows as std does:
/// between quotes, unescaped.
struct OsMessage(i32);

impl fmt::Debug for OsMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("\"")?;
        sys::os::fmt_error_message(self.0, f)?;
        f.write_str("\"")
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self.repr.form() {
            Form::Custom(custom) => custom.error.source(),
            _ => None,
        }
    }
}
