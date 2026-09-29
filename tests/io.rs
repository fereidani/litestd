//! `litestd::io::Error` against std's documented behavior.

#![cfg(feature = "io")]

use core::{error::Error as _, fmt};

use litestd::{
    ffi::CString,
    io::{Error, ErrorKind, Result},
};

#[cfg(any(unix, target_os = "wasi"))]
const NOT_FOUND: i32 = libc::ENOENT;
#[cfg(windows)]
const NOT_FOUND: i32 = 2; // ERROR_FILE_NOT_FOUND
/// wasm32-unknown-unknown has no OS, and so no codes of its own.
#[cfg(target_os = "unknown")]
const NOT_FOUND: i32 = 2;

#[cfg(any(unix, target_os = "wasi"))]
const INTERRUPTED: i32 = libc::EINTR;

#[derive(Debug)]
struct MyError(u32);

impl fmt::Display for MyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "my error {}", self.0)
    }
}

impl core::error::Error for MyError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        None
    }
}

#[derive(Debug)]
struct Wrapper(MyError);

impl fmt::Display for Wrapper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("wrapper")
    }
}

impl core::error::Error for Wrapper {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        Some(&self.0)
    }
}

#[test]
fn os_errors_match_std() {
    let lite = Error::from_raw_os_error(NOT_FOUND);
    let std = std::io::Error::from_raw_os_error(NOT_FOUND);
    assert_eq!(lite.raw_os_error(), Some(NOT_FOUND));
    // Without an OS, std decodes no code.
    if cfg!(not(target_os = "unknown")) {
        assert_eq!(lite.kind(), ErrorKind::NotFound);
    }
    assert_eq!(format!("{:?}", lite.kind()), format!("{:?}", std.kind()));
    assert_eq!(lite.to_string(), std.to_string());
    assert_eq!(format!("{lite:?}"), format!("{std:?}"));
    assert!(lite.get_ref().is_none());
    assert!(lite.source().is_none());
}

/// OS error codes beyond the common range: negative and unknown codes, and
/// on Windows `HRESULT`s and `NTSTATUS` codes tagged with `FACILITY_NT_BIT`,
/// whose messages std looks up in `ntdll`.
#[allow(clippy::cast_possible_wrap, reason = "codes keep their bit pattern")]
const UNUSUAL_OS_CODES: &[i32] = &[
    -1,
    i32::MIN,
    i32::MAX,
    0x1000_0000,
    0x8007_0005_u32 as i32,
    0xD000_0022_u32 as i32,
    0xD000_0034_u32 as i32,
    0xD000_00BB_u32 as i32,
];

#[test]
fn every_os_code_formats_like_std() {
    // The messages come from the same C library or system table as std's.
    // Windows has thousands of codes, some with messages of hundreds of
    // characters.
    let common = match (cfg!(miri), cfg!(windows)) {
        (true, _) => 40,
        (false, true) => 16_000,
        (false, false) => 200,
    };
    for code in (0..common).chain(UNUSUAL_OS_CODES.iter().copied()) {
        let lite = Error::from_raw_os_error(code);
        let std = std::io::Error::from_raw_os_error(code);
        assert_eq!(lite.to_string(), std.to_string(), "code {code}");
        // The kinds differ only where std uses an unstable kind, see
        // `os_error_kinds_match_std`.
        if format!("{:?}", lite.kind()) == format!("{:?}", std.kind()) {
            assert_eq!(format!("{lite:?}"), format!("{std:?}"), "code {code}");
        }
    }
}

#[test]
fn display_honors_the_formatter_like_std() {
    let lite = Error::from(CString::new("a\0b").unwrap_err());
    let std = std::io::Error::from(std::ffi::CString::new("a\0b").unwrap_err());
    // A static message is padded and truncated like a `str`.
    assert_eq!(format!("[{lite:>40}]"), format!("[{std:>40}]"));
    assert_eq!(format!("[{lite:.4}]"), format!("[{std:.4}]"));
    // Kinds and OS errors ignore width and precision, as with std.
    let pairs = [
        (
            Error::from(ErrorKind::TimedOut),
            std::io::Error::from(std::io::ErrorKind::TimedOut),
        ),
        (
            Error::from_raw_os_error(NOT_FOUND),
            std::io::Error::from_raw_os_error(NOT_FOUND),
        ),
    ];
    for (lite, std) in pairs {
        assert_eq!(format!("[{lite:>80.3}]"), format!("[{std:>80.3}]"));
        assert_eq!(
            format!("[{:>30}]", lite.kind()),
            format!("[{:>30}]", std.kind())
        );
    }
}

#[test]
fn unknown_os_code() {
    let lite = Error::from_raw_os_error(i32::MAX);
    assert_eq!(lite.kind(), ErrorKind::Uncategorized);
    assert!(
        lite.to_string()
            .ends_with(&format!("(os error {})", i32::MAX))
    );
    assert_eq!(Error::from_raw_os_error(-1).raw_os_error(), Some(-1));
}

#[cfg(any(unix, target_os = "wasi"))]
#[test]
fn os_error_kinds_match_std() {
    for code in 0..200 {
        let lite = Error::from_raw_os_error(code).kind();
        let std = std::io::Error::from_raw_os_error(code).kind();
        // Nightly std decodes `EIO` to `InputOutputError`, which no stable
        // std has yet; litestd follows stable std.
        let std =
            format!("{std:?}").replace("InputOutputError", "Uncategorized");
        // Newer std decodes WASI's codes with the Unix table, `ENOTEMPTY`
        // included, as litestd does; stable std 1.98 says `Uncategorized`.
        #[cfg(target_os = "wasi")]
        let std = if code == libc::ENOTEMPTY {
            std.replace("Uncategorized", "DirectoryNotEmpty")
        } else {
            std
        };
        // std 1.99 and later decode `ENOTSUP` as `Unsupported`, as litestd
        // does, also where it differs from `EOPNOTSUPP`, as on macOS; std
        // 1.98 says `Uncategorized` there.
        let std = if code == libc::ENOTSUP {
            std.replace("Uncategorized", "Unsupported")
        } else {
            std
        };
        assert_eq!(format!("{lite:?}"), std, "code {code}");
    }
    assert_eq!(
        Error::from_raw_os_error(INTERRUPTED).kind(),
        ErrorKind::Interrupted
    );
}

#[cfg(any(unix, target_os = "wasi"))]
#[test]
fn last_os_error_reads_errno() {
    // SAFETY: closing an invalid descriptor only sets `errno` to `EBADF`.
    let r = unsafe { libc::close(-1) };
    assert_eq!(r, -1);
    let err = Error::last_os_error();
    assert_eq!(err.raw_os_error(), Some(libc::EBADF));
}

#[test]
fn simple_kinds() {
    let err = Error::from(ErrorKind::TimedOut);
    assert_eq!(err.kind(), ErrorKind::TimedOut);
    assert_eq!(err.raw_os_error(), None);
    assert_eq!(err.to_string(), "timed out");
    assert_eq!(format!("{err:?}"), "Kind(TimedOut)");
    assert_eq!(
        format!("{err:?}"),
        format!("{:?}", std::io::Error::from(std::io::ErrorKind::TimedOut))
    );
    assert!(err.into_inner().is_none());
}

#[test]
fn kind_display_matches_std() {
    let pairs = [
        (ErrorKind::NotFound, std::io::ErrorKind::NotFound),
        (
            ErrorKind::PermissionDenied,
            std::io::ErrorKind::PermissionDenied,
        ),
        (ErrorKind::WouldBlock, std::io::ErrorKind::WouldBlock),
        (ErrorKind::InvalidInput, std::io::ErrorKind::InvalidInput),
        (ErrorKind::InvalidData, std::io::ErrorKind::InvalidData),
        (ErrorKind::TimedOut, std::io::ErrorKind::TimedOut),
        (ErrorKind::WriteZero, std::io::ErrorKind::WriteZero),
        (ErrorKind::Interrupted, std::io::ErrorKind::Interrupted),
        (ErrorKind::Unsupported, std::io::ErrorKind::Unsupported),
        (ErrorKind::UnexpectedEof, std::io::ErrorKind::UnexpectedEof),
        (ErrorKind::OutOfMemory, std::io::ErrorKind::OutOfMemory),
        (ErrorKind::Other, std::io::ErrorKind::Other),
        (ErrorKind::QuotaExceeded, std::io::ErrorKind::QuotaExceeded),
        (
            ErrorKind::CrossesDevices,
            std::io::ErrorKind::CrossesDevices,
        ),
    ];
    for (lite, std) in pairs {
        assert_eq!(lite.to_string(), std.to_string());
        assert_eq!(format!("{lite:?}"), format!("{std:?}"));
    }
}

#[test]
fn custom_errors() {
    let err = Error::new(ErrorKind::InvalidData, MyError(7));
    assert_eq!(err.kind(), ErrorKind::InvalidData);
    assert_eq!(err.to_string(), "my error 7");
    assert_eq!(
        format!("{err:?}"),
        "Custom { kind: InvalidData, error: MyError(7) }"
    );
    assert!(err.get_ref().unwrap().is::<MyError>());

    let err = Error::other("boxed message");
    assert_eq!(err.kind(), ErrorKind::Other);
    assert_eq!(err.to_string(), "boxed message");

    let err = Error::new(ErrorKind::Other, String::from("owned"));
    assert_eq!(err.into_inner().unwrap().to_string(), "owned");
}

#[test]
fn get_mut_and_source() {
    let mut err = Error::new(ErrorKind::Other, MyError(1));
    err.get_mut().unwrap().downcast_mut::<MyError>().unwrap().0 = 2;
    assert_eq!(err.to_string(), "my error 2");

    // `source` skips the wrapped error itself, as with std.
    let err = Error::new(ErrorKind::Other, Wrapper(MyError(3)));
    assert_eq!(err.source().unwrap().to_string(), "my error 3");
    let err = Error::new(ErrorKind::Other, MyError(4));
    assert!(err.source().is_none());
}

#[test]
fn downcast() {
    let err = Error::new(ErrorKind::Other, MyError(5));
    let Err(err) = err.downcast::<fmt::Error>() else {
        panic!("wrong type");
    };
    assert_eq!(err.kind(), ErrorKind::Other);
    assert_eq!(err.downcast::<MyError>().unwrap().0, 5);

    let os = Error::from_raw_os_error(NOT_FOUND);
    let os = os.downcast::<MyError>().unwrap_err();
    assert_eq!(os.raw_os_error(), Some(NOT_FOUND));
}

#[test]
fn conversions() {
    let nul = CString::new("a\0b").unwrap_err();
    let err = Error::from(nul);
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
    assert_eq!(err.to_string(), "data provided contains a nul byte");
    assert_eq!(
        format!("{err:?}"),
        "Error { kind: InvalidInput, \
         message: \"data provided contains a nul byte\" }"
    );

    let mut v: Vec<u8> = Vec::new();
    let reserve = v.try_reserve(usize::MAX).unwrap_err();
    let err = Error::from(reserve);
    assert_eq!(err.kind(), ErrorKind::OutOfMemory);
}

#[test]
fn result_alias_and_question_mark() {
    fn fails() -> Result<u8> {
        Err(ErrorKind::NotFound)?;
        Ok(1)
    }
    assert_eq!(fails().unwrap_err().kind(), ErrorKind::NotFound);
}

/// The provided methods retry after every form of an interrupted call: an
/// OS error, a bare kind and a custom error, as std does. Other errors end
/// them.
#[test]
fn provided_methods_retry_every_interrupted_error() {
    use litestd::io::{Read, Write};

    /// Fails once with each error of `errors`, then serves one byte of
    /// `data`, or accepts one byte, per call.
    struct Flaky {
        errors: Vec<Error>,
        data: Vec<u8>,
        written: Vec<u8>,
    }

    impl Flaky {
        fn new(errors: Vec<Error>) -> Self {
            Self {
                errors,
                data: b"abc".to_vec(),
                written: Vec::new(),
            }
        }
    }

    impl Read for Flaky {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
            if let Some(e) = self.errors.pop() {
                return Err(e);
            }
            match (buf.first_mut(), self.data.is_empty()) {
                (Some(slot), false) => {
                    *slot = self.data.remove(0);
                    Ok(1)
                }
                _ => Ok(0),
            }
        }
    }

    impl Write for Flaky {
        fn write(&mut self, buf: &[u8]) -> Result<usize> {
            if let Some(e) = self.errors.pop() {
                return Err(e);
            }
            self.written.extend(buf.first());
            Ok(buf.len().min(1))
        }

        fn flush(&mut self) -> Result<()> {
            Ok(())
        }
    }

    let interrupted = || {
        vec![
            Error::from(ErrorKind::Interrupted),
            Error::new(ErrorKind::Interrupted, "custom"),
            #[cfg(any(unix, target_os = "wasi"))]
            Error::from_raw_os_error(INTERRUPTED),
        ]
    };
    let mut buf = [0; 3];
    Flaky::new(interrupted()).read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"abc");
    let mut all = Vec::new();
    Flaky::new(interrupted()).read_to_end(&mut all).unwrap();
    assert_eq!(all, b"abc");
    let mut sink = Flaky::new(interrupted());
    sink.write_all(b"xyz").unwrap();
    assert_eq!(sink.written, b"xyz");
    let copied =
        litestd::io::copy(&mut Flaky::new(interrupted()), &mut Vec::new());
    assert_eq!(copied.unwrap(), 3);
    // Any other error, an OS one included, ends the call.
    let fatal = Error::from_raw_os_error(NOT_FOUND);
    let err = Flaky::new(vec![fatal]).read_exact(&mut buf).unwrap_err();
    assert_eq!(err.raw_os_error(), Some(NOT_FOUND));
}
