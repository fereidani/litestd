//! The representation of `litestd::io::Error`: its size and auto traits
//! against std's, and each of its forms through every accessor.

#![cfg(feature = "io")]
#![allow(
    clippy::incompatible_msrv,
    reason = "the tests compare against the std of the toolchain that runs them"
)]
#![allow(clippy::unwrap_used, reason = "helpers, like tests, fail on errors")]

extern crate alloc;

use alloc::sync::Arc;
use core::{
    error::Error as _,
    fmt,
    hash::Hash,
    hint::black_box,
    mem::size_of,
    panic::{RefUnwindSafe, UnwindSafe},
    sync::atomic::{AtomicUsize, Ordering},
};
use std::thread;

use litestd::{
    ffi::CString,
    io::{Error, ErrorKind, Result},
};

/// `ENOENT` on Unix and `ERROR_FILE_NOT_FOUND` on Windows.
const NOT_FOUND: i32 = 2;

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

/// Whether `$ty` is `Send`, `Sync`, `Unpin`, `UnwindSafe` and
/// `RefUnwindSafe`.
macro_rules! auto_traits {
    ($ty:ty) => {
        [
            implements!($ty: Send),
            implements!($ty: Sync),
            implements!($ty: Unpin),
            implements!($ty: UnwindSafe),
            implements!($ty: RefUnwindSafe),
        ]
    };
}

/// Pairs kinds with std's of the same name.
macro_rules! kind_pairs {
    ($($kind:ident),* $(,)?) => {
        [$((ErrorKind::$kind, std::io::ErrorKind::$kind)),*]
    };
}

/// Every kind but `Uncategorized`, which std has not stabilized.
const KINDS: [(ErrorKind, std::io::ErrorKind); 39] = kind_pairs![
    NotFound,
    PermissionDenied,
    ConnectionRefused,
    ConnectionReset,
    HostUnreachable,
    NetworkUnreachable,
    ConnectionAborted,
    NotConnected,
    AddrInUse,
    AddrNotAvailable,
    NetworkDown,
    BrokenPipe,
    AlreadyExists,
    WouldBlock,
    NotADirectory,
    IsADirectory,
    DirectoryNotEmpty,
    ReadOnlyFilesystem,
    StaleNetworkFileHandle,
    InvalidInput,
    InvalidData,
    TimedOut,
    WriteZero,
    StorageFull,
    NotSeekable,
    QuotaExceeded,
    FileTooLarge,
    ResourceBusy,
    ExecutableFileBusy,
    Deadlock,
    CrossesDevices,
    TooManyLinks,
    InvalidFilename,
    ArgumentListTooLong,
    Interrupted,
    Unsupported,
    UnexpectedEof,
    OutOfMemory,
    Other,
];

#[derive(Debug, PartialEq, Eq)]
struct Payload(u32);

impl fmt::Display for Payload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "payload {}", self.0)
    }
}

impl core::error::Error for Payload {}

/// A payload that counts its drops.
#[derive(Debug)]
struct Counted(Arc<AtomicUsize>);

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

impl fmt::Display for Counted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("counted")
    }
}

impl core::error::Error for Counted {}

/// One error of each form, with std's equivalent: an OS code, a kind, a
/// static message and a custom payload.
fn every_form() -> [(Error, std::io::Error); 4] {
    [
        (
            Error::from_raw_os_error(NOT_FOUND),
            std::io::Error::from_raw_os_error(NOT_FOUND),
        ),
        (
            Error::from(ErrorKind::TimedOut),
            std::io::Error::from(std::io::ErrorKind::TimedOut),
        ),
        (
            Error::from(CString::new("a\0b").unwrap_err()),
            std::io::Error::from(std::ffi::CString::new("a\0b").unwrap_err()),
        ),
        (
            Error::new(ErrorKind::InvalidData, Payload(4)),
            std::io::Error::new(std::io::ErrorKind::InvalidData, Payload(4)),
        ),
    ]
}

#[test]
fn sizes_match_std() {
    assert_eq!(size_of::<Error>(), size_of::<std::io::Error>());
    assert_eq!(size_of::<Result<()>>(), size_of::<std::io::Result<()>>());
    assert_eq!(
        size_of::<Option<Error>>(),
        size_of::<Option<std::io::Error>>()
    );
    assert_eq!(
        size_of::<Result<usize>>(),
        size_of::<std::io::Result<usize>>()
    );
}

/// One word, whose null value holds `Ok(())` and `None`.
#[cfg(target_pointer_width = "64")]
#[test]
fn one_word_on_64_bit_targets() {
    assert_eq!(size_of::<Error>(), 8);
    assert_eq!(size_of::<Result<()>>(), 8);
    assert_eq!(size_of::<Option<Error>>(), 8);
    assert_eq!(size_of::<Result<usize>>(), 16);
}

/// An enum of two words, whose unused tags hold `Ok` and `None`.
#[cfg(target_pointer_width = "32")]
#[test]
fn two_words_on_32_bit_targets() {
    assert_eq!(size_of::<Error>(), 8);
    assert_eq!(size_of::<Result<()>>(), 8);
    assert_eq!(size_of::<Option<Error>>(), 8);
    assert_eq!(size_of::<Result<usize>>(), 8);
}

#[test]
fn auto_traits_match_std() {
    fn kind<T: Copy + Eq + Ord + Hash + fmt::Debug + fmt::Display>() {}
    fn error<T: core::error::Error + 'static>() {}
    kind::<ErrorKind>();
    error::<Error>();

    assert!(!implements!(core::cell::Cell<i32>: Sync));
    assert_eq!(auto_traits!(Error), auto_traits!(std::io::Error));
    assert_eq!(auto_traits!(ErrorKind), auto_traits!(std::io::ErrorKind));
    // A custom payload need not be unwind safe.
    assert_eq!(auto_traits!(Error), [true, true, true, false, false]);
    assert_eq!(auto_traits!(ErrorKind), [true; 5]);
}

#[test]
fn every_kind_round_trips() {
    for (lite, std) in KINDS {
        let err = Error::from(lite);
        let std_err = std::io::Error::from(std);
        assert_eq!(err.kind(), lite);
        assert_eq!(err.raw_os_error(), None);
        assert_eq!(err.to_string(), std_err.to_string());
        assert_eq!(format!("{err:?}"), format!("{std_err:?}"));
        assert_eq!(format!("{lite:?}"), format!("{std:?}"));
    }
    let err = Error::from(ErrorKind::Uncategorized);
    assert_eq!(err.kind(), ErrorKind::Uncategorized);
    assert_eq!(err.to_string(), "uncategorized error");
    assert_eq!(format!("{err:?}"), "Kind(Uncategorized)");
}

/// The packed form keeps the code in 32 bits: its sign and every bit.
#[test]
fn os_codes_round_trip() {
    #[allow(clippy::cast_possible_wrap, reason = "codes keep their bits")]
    let codes = [
        0,
        1,
        NOT_FOUND,
        -1,
        i32::MIN,
        i32::MIN + 1,
        i32::MAX,
        0x5555_5555,
        0xAAAA_AAAA_u32 as i32,
    ];
    for code in codes {
        let err = Error::from_raw_os_error(code);
        assert_eq!(err.raw_os_error(), Some(code));
        let std_err = std::io::Error::from_raw_os_error(code);
        assert_eq!(err.to_string(), std_err.to_string(), "code {code}");
        let err = err.downcast::<Payload>().unwrap_err();
        assert_eq!(err.raw_os_error(), Some(code));
    }
}

#[test]
fn every_form_formats_like_std() {
    for (err, std_err) in every_form() {
        assert_eq!(err.to_string(), std_err.to_string());
        assert_eq!(format!("{err:?}"), format!("{std_err:?}"));
        assert_eq!(format!("{err:#?}"), format!("{std_err:#?}"));
        assert_eq!(
            format!("{:?}", err.kind()),
            format!("{:?}", std_err.kind())
        );
        assert_eq!(err.raw_os_error(), std_err.raw_os_error());
        assert_eq!(format!("[{err:>60.5}]"), format!("[{std_err:>60.5}]"));
    }
    // A custom error receives the width and precision.
    let (err, std_err) = (Error::other("text"), std::io::Error::other("text"));
    assert_eq!(format!("[{err:>8.2}]"), format!("[{std_err:>8.2}]"));
    assert_eq!(format!("[{err:>8.2}]"), "[      te]");
}

/// Asserts that `$lite` and `$std` format alike with each format string.
macro_rules! same_formats {
    ($lite:expr, $std:expr, $($spec:literal),* $(,)?) => {{
        let (lite, std) = (&$lite, &$std);
        $(assert_eq!(format!($spec, lite), format!($spec, std), "{}", $spec);)*
    }};
}

/// Asserts that `lite` and `std` format alike with and without the flags
/// that `Display` and `Debug` may honor.
#[allow(
    clippy::literal_string_with_formatting_args,
    reason = "`same_formats!` passes each string to `format!`"
)]
fn formats_like_std(lite: &Error, std: &std::io::Error) {
    same_formats!(
        lite,
        std,
        "{}",
        "{:>20}",
        "{:<8.3}",
        "{:^30.10}",
        "{:*<40}",
        "{:.0}",
        "{:5}",
        "{:?}",
        "{:#?}",
        "{:>20?}",
        "{:x?}",
        "{:#X?}",
        "{:+?}",
        "{:08?}",
    );
}

/// Static messages, which only litestd's own errors carry.
// `Write` for `&mut [u8]` takes a `&mut &mut [u8]` receiver.
#[allow(clippy::mut_mut)]
fn static_messages() -> [(Error, std::io::Error); 4] {
    use std::io::{Read as _, Write as _};

    let mut buf = [0; 1];
    let read_exact = litestd::io::Read::read_exact(&mut &b""[..], &mut buf);
    let std_read_exact = (&mut &b""[..]).read_exact(&mut buf);
    let mut text = String::new();
    let invalid =
        litestd::io::Read::read_to_string(&mut &b"\xFF"[..], &mut text);
    let std_invalid = (&mut &b"\xFF"[..]).read_to_string(&mut text);
    let mut full = [0; 0];
    let write_all = litestd::io::Write::write_all(&mut &mut full[..], b"a");
    let std_write_all = (&mut &mut full[..]).write_all(b"a");
    [
        (
            Error::from(CString::new("a\0b").unwrap_err()),
            std::io::Error::from(std::ffi::CString::new("a\0b").unwrap_err()),
        ),
        (read_exact.unwrap_err(), std_read_exact.unwrap_err()),
        (invalid.unwrap_err(), std_invalid.unwrap_err()),
        (write_all.unwrap_err(), std_write_all.unwrap_err()),
    ]
}

#[test]
fn every_form_formats_like_std_with_every_flag() {
    for (err, std_err) in every_form().into_iter().chain(static_messages()) {
        formats_like_std(&err, &std_err);
    }
    for (lite, std) in KINDS {
        formats_like_std(&Error::from(lite), &std::io::Error::from(std));
    }
    // Custom payloads get the flags; a quoted one is escaped, and a nested
    // error is indented when pretty-printed.
    let payloads = ["te\"xt\n\u{7f}'", ""];
    for payload in payloads {
        formats_like_std(
            &Error::other(payload),
            &std::io::Error::other(payload),
        );
    }
    formats_like_std(
        &Error::other(Error::from_raw_os_error(NOT_FOUND)),
        &std::io::Error::other(std::io::Error::from_raw_os_error(NOT_FOUND)),
    );
}

/// The kind that litestd gives `code` as a newer std does, where the std
/// the tests run with says `Uncategorized`: std 1.99 decodes `ENOTSUP` as
/// `Unsupported` also where it differs from `EOPNOTSUPP`, as on macOS, and
/// std 1.100 makes Windows' `ERROR_NEGATIVE_SEEK` `InvalidInput` and decodes
/// WASI's codes with the Unix table, which has `ENOTEMPTY`.
const fn newer_kind(code: i32) -> Option<&'static str> {
    if cfg!(windows) && code == 131 {
        Some("InvalidInput")
    } else if cfg!(target_os = "wasi") && code == 55 {
        Some("DirectoryNotEmpty")
    } else if is_enotsup(code) {
        Some("Unsupported")
    } else {
        None
    }
}

/// Whether `code` is the OS's `ENOTSUP`.
const fn is_enotsup(code: i32) -> bool {
    #[cfg(any(unix, target_os = "wasi"))]
    return code == libc::ENOTSUP;
    #[cfg(not(any(unix, target_os = "wasi")))]
    {
        let _ = code;
        false
    }
}

/// The name of the kind that litestd gives `code`, where std names `kind`.
/// Nightly std has an `InputOutputError` kind that no stable std has yet,
/// which litestd reports as `Uncategorized`, as stable std does.
fn litestd_kind_name(code: i32, kind: &str) -> &str {
    if let (Some(newer), "Uncategorized") = (newer_kind(code), kind) {
        newer
    } else if kind == "InputOutputError" {
        "Uncategorized"
    } else {
        kind
    }
}

/// std's `Debug` of the OS error `code`, pretty if `pretty`, with the kind
/// named as litestd names it.
fn std_os_debug(code: i32, pretty: bool) -> String {
    let err = std::io::Error::from_raw_os_error(code);
    let debug = if pretty {
        format!("{err:#?}")
    } else {
        format!("{err:?}")
    };
    let kind = format!("{:?}", err.kind());
    let name = litestd_kind_name(code, &kind);
    debug.replacen(&format!("kind: {kind}"), &format!("kind: {name}"), 1)
}

/// OS codes whose messages the system has, and some it has not: the
/// messages of Windows in particular hold quotes and line breaks, which
/// `Debug` shows unescaped, as std does.
#[test]
fn os_messages_format_like_std() {
    let last = match (cfg!(miri), cfg!(windows)) {
        (true, _) => 3,
        (false, false) => 140,
        (false, true) => 1400,
    };
    #[allow(clippy::cast_possible_wrap, reason = "codes keep their bits")]
    let unusual = [
        -1,
        4096,
        i32::MAX,
        0x8007_0002_u32 as i32,
        0xD000_0022_u32 as i32,
    ];
    for code in (0..=last).chain(unusual) {
        let (err, std_err) = (
            Error::from_raw_os_error(code),
            std::io::Error::from_raw_os_error(code),
        );
        assert_eq!(err.to_string(), std_err.to_string(), "code {code}");
        let std_kind = format!("{:?}", std_err.kind());
        if litestd_kind_name(code, &std_kind) == std_kind {
            let (kind, std_kind) = (err.kind(), std_err.kind());
            assert_eq!(kind.to_string(), std_kind.to_string(), "code {code}");
        }
        assert_eq!(format!("{err:?}"), std_os_debug(code, false));
        assert_eq!(format!("{err:#?}"), std_os_debug(code, true));
    }
}

#[test]
fn only_custom_errors_have_an_inner_error() {
    for (mut err, std_err) in every_form() {
        let custom = std_err.get_ref().is_some();
        assert_eq!(err.get_ref().is_some(), custom);
        assert_eq!(err.get_mut().is_some(), custom);
        assert!(err.source().is_none());
        let inner = err.into_inner();
        assert_eq!(inner.is_some(), custom);
        if let Some(inner) = inner {
            assert_eq!(*inner.downcast::<Payload>().unwrap(), Payload(4));
        }
    }
}

#[test]
fn get_mut_changes_the_payload() {
    let mut err = Error::new(ErrorKind::InvalidData, Payload(1));
    let payload = err.get_mut().unwrap().downcast_mut::<Payload>().unwrap();
    payload.0 = 2;
    assert_eq!(err.to_string(), "payload 2");
    assert_eq!(err.kind(), ErrorKind::InvalidData);
    let payload = err.get_ref().unwrap().downcast_ref::<Payload>();
    assert_eq!(payload, Some(&Payload(2)));
}

#[test]
fn downcast_takes_the_payload_or_changes_nothing() {
    for (err, _) in every_form() {
        let before = format!("{err:?}");
        let err = err.downcast::<fmt::Error>().unwrap_err();
        assert_eq!(format!("{err:?}"), before);
    }
    let err = Error::new(ErrorKind::InvalidData, Payload(5));
    let err = err.downcast::<fmt::Error>().unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidData);
    assert_eq!(err.downcast::<Payload>().unwrap(), Payload(5));
}

#[test]
fn custom_payloads_drop_once() {
    let drops = Arc::new(AtomicUsize::new(0));
    let counted = || Counted(Arc::clone(&drops));
    let dropped = || drops.load(Ordering::Relaxed);

    drop(Error::other(counted()));
    assert_eq!(dropped(), 1);
    let err = Error::other(counted()).downcast::<Payload>().unwrap_err();
    assert_eq!(dropped(), 1);
    let payload = err.downcast::<Counted>().unwrap();
    assert_eq!(dropped(), 1);
    drop(payload);
    assert_eq!(dropped(), 2);
    let inner = Error::other(counted()).into_inner();
    assert_eq!(dropped(), 2);
    drop(inner);
    assert_eq!(dropped(), 3);
    let mut err = Error::other(counted());
    *err.get_mut().unwrap().downcast_mut::<Counted>().unwrap() = counted();
    assert_eq!(dropped(), 4);
    drop(err);
    assert_eq!(dropped(), 5);
}

#[derive(Debug)]
struct Empty;

impl fmt::Display for Empty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("empty")
    }
}

impl core::error::Error for Empty {}

#[derive(Debug)]
#[repr(align(64))]
struct Aligned(u8);

impl fmt::Display for Aligned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "aligned {}", self.0)
    }
}

impl core::error::Error for Aligned {}

/// A zero-sized payload, whose box allocates nothing, and one aligned
/// beyond the tag bits.
#[test]
fn unusual_payloads() {
    let err = Error::other(Empty);
    assert_eq!(err.to_string(), "empty");
    assert!(err.get_ref().unwrap().is::<Empty>());
    assert!(err.downcast::<Empty>().is_ok());

    let err = Error::new(ErrorKind::Unsupported, Aligned(9));
    let inner = err.get_ref().unwrap().downcast_ref::<Aligned>().unwrap();
    assert_eq!(core::ptr::from_ref(inner).addr() % 64, 0);
    assert_eq!(err.to_string(), "aligned 9");
    assert_eq!(err.downcast::<Aligned>().unwrap().0, 9);
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn errors_cross_threads() {
    let errors = every_form().map(|(err, _)| err);
    let debug = errors.each_ref().map(|err| format!("{err:?}"));
    thread::scope(|s| {
        for _ in 0..2 {
            s.spawn(|| {
                for (err, debug) in errors.iter().zip(&debug) {
                    assert_eq!(&format!("{err:?}"), debug);
                }
            });
        }
    });
    let errors = thread::spawn(move || errors).join().unwrap();
    for (err, debug) in errors.iter().zip(&debug) {
        assert_eq!(&format!("{err:?}"), debug);
    }
    // Dropped by a thread other than the one that created them.
    thread::spawn(move || drop(errors)).join().unwrap();
}

#[test]
fn conversions_and_niches() {
    fn fails(kind: Option<ErrorKind>) -> Result<()> {
        if let Some(kind) = kind {
            Err(kind)?;
        }
        Ok(())
    }
    assert!(fails(None).is_ok());
    let err = fails(Some(ErrorKind::BrokenPipe)).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::BrokenPipe);

    let reserve = Vec::<u8>::new().try_reserve(usize::MAX).unwrap_err();
    let (err, std_err) =
        (Error::from(reserve.clone()), std::io::Error::from(reserve));
    assert_eq!(format!("{err:?}"), format!("{std_err:?}"));

    // Through memory, as the null word that means `None` and `Ok(())`.
    for (err, _) in every_form() {
        let debug = format!("{err:?}");
        let err = black_box(Some(err)).unwrap();
        let result: Result<()> = black_box(Err(err));
        assert_eq!(format!("{:?}", result.unwrap_err()), debug);
        assert!(black_box(None::<Error>).is_none());
        assert!(black_box(Result::Ok(())).is_ok());
    }
}
