//! `litestd::env` against `std::env`.
//!
//! Tests that change the environment or the current directory run this
//! binary again with `LITESTD_ENV_CHILD` naming what the child checks, so
//! that the tests running in parallel here never see the change. The
//! directory tests run in a child whose current directory is a scratch
//! directory. Miri cannot spawn processes, so it runs only the others.

// wasm32-unknown-unknown has no environment: `tests/unsupported.rs`
// compares what litestd and std do there.
#![cfg(all(feature = "env", not(target_os = "unknown")))]
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    reason = "helpers, like tests, fail on errors"
)]

use core::panic::{RefUnwindSafe, UnwindSafe};
#[cfg(unix)]
use std::os::unix::ffi as std_ffi;
#[cfg(target_os = "wasi")]
use std::os::wasi::ffi as std_ffi;
use std::path::Path;
#[cfg(not(miri))]
use std::process::{Command, Output};

use litestd::env as lenv;
/// The byte-string extensions of the platform: Unix's, or WASI's.
#[cfg(unix)]
use litestd::os::unix::ffi as lite_ffi;
#[cfg(target_os = "wasi")]
use litestd::os::wasi::ffi as lite_ffi;

/// Set in the children, naming what they check.
#[cfg(not(miri))]
const CHILD: &str = "LITESTD_ENV_CHILD";

/// The variable a child's check sets and removes.
#[cfg(not(miri))]
const KEY: &str = "LITESTD_ENV_TEST_KEY";

/// The units of a litestd OS string: bytes on Unix, UTF-16 on Windows.
fn lite_units(s: &litestd::ffi::OsStr) -> Vec<u32> {
    #[cfg(not(windows))]
    let units = crate::lite_ffi::OsStrExt::as_bytes(s)
        .iter()
        .map(|&b| u32::from(b))
        .collect();
    #[cfg(windows)]
    let units = litestd::os::windows::ffi::OsStrExt::encode_wide(s)
        .map(u32::from)
        .collect();
    units
}

/// The units of a std OS string, as [`lite_units`] gives them.
fn std_units(s: &std::ffi::OsStr) -> Vec<u32> {
    #[cfg(not(windows))]
    let units = crate::std_ffi::OsStrExt::as_bytes(s)
        .iter()
        .map(|&b| u32::from(b))
        .collect();
    #[cfg(windows)]
    let units = std::os::windows::ffi::OsStrExt::encode_wide(s)
        .map(u32::from)
        .collect();
    units
}

/// Converts std's OS string to litestd's.
fn to_lite(s: &std::ffi::OsStr) -> litestd::ffi::OsString {
    #[cfg(not(windows))]
    let s = crate::lite_ffi::OsStringExt::from_vec(
        crate::std_ffi::OsStrExt::as_bytes(s).to_vec(),
    );
    #[cfg(windows)]
    let s = litestd::os::windows::ffi::OsStringExt::from_wide(
        &std::os::windows::ffi::OsStrExt::encode_wide(s).collect::<Vec<_>>(),
    );
    s
}

/// Asserts that litestd's and std's optional paths or strings agree.
fn same_path(
    what: &str,
    lite: Option<&litestd::path::Path>,
    real: Option<&Path>,
) {
    let lite = lite.map(|p| lite_units(p.as_os_str()));
    let real = real.map(|p| std_units(p.as_os_str()));
    assert_eq!(lite, real, "{what}");
}

/// Asserts that litestd's and std's `var` agree for `key`.
fn same_var(key: &std::ffi::OsStr) {
    let lite_key = to_lite(key);
    let lite = lenv::var(&lite_key);
    match std::env::var(key) {
        Ok(real) => assert_eq!(lite.as_deref(), Ok(real.as_str()), "{key:?}"),
        Err(std::env::VarError::NotPresent) => {
            assert_eq!(lite, Err(lenv::VarError::NotPresent), "{key:?}");
        }
        Err(std::env::VarError::NotUnicode(real)) => match lite {
            Err(lenv::VarError::NotUnicode(lite)) => {
                assert_eq!(lite_units(&lite), std_units(&real), "{key:?}");
            }
            other => panic!("{key:?}: {other:?}"),
        },
    }
    let lite_os = lenv::var_os(&lite_key);
    let real_os = std::env::var_os(key);
    assert_eq!(
        lite_os.as_deref().map(lite_units),
        real_os.as_deref().map(std_units),
        "{key:?}"
    );
}

/// Asserts that the whole environment, every variable in it and the names
/// that cannot be set agree with std's.
fn same_environment() {
    let lite: Vec<_> = lenv::vars_os()
        .map(|(k, v)| (lite_units(&k), lite_units(&v)))
        .collect();
    let real: Vec<_> = std::env::vars_os()
        .map(|(k, v)| (std_units(&k), std_units(&v)))
        .collect();
    assert_eq!(lite, real);
    let vars = lenv::vars_os();
    assert_eq!(vars.size_hint(), (real.len(), Some(real.len())));
    assert_eq!(format!("{vars:?}"), format!("{:?}", std::env::vars_os()));
    for (key, _) in std::env::vars_os() {
        same_var(&key);
    }
    // glibc's `getenv` finds `LITESTD_EQUALS=a` in `LITESTD_EQUALS=a=b=c`.
    let odd = [
        "",
        "=",
        "A=B",
        "=A",
        "NUL\0",
        "LITESTD_EQUALS=a",
        "LITESTD_E",
    ];
    for key in odd {
        same_var(key.as_ref());
    }
}

#[test]
fn environment_matches_std() {
    same_environment();
}

#[test]
fn unicode_vars_match_std() {
    // Vars panics on non-Unicode data; such data lives only in children.
    if std::env::vars_os()
        .all(|(k, v)| k.to_str().is_some() && v.to_str().is_some())
    {
        let lite: Vec<_> = lenv::vars().collect();
        let real: Vec<_> = std::env::vars().collect();
        assert_eq!(lite, real);
        assert_eq!(
            format!("{:?}", lenv::vars()),
            format!("{:?}", std::env::vars())
        );
    }
}

#[test]
fn partly_consumed_vars_print_the_rest() {
    let left = std::env::vars_os().count().saturating_sub(1);
    let mut lite = lenv::vars_os();
    let mut real = std::env::vars_os();
    lite.next();
    real.next();
    // Exact, where std's Windows iterator reports `(0, None)`.
    assert_eq!(lite.size_hint(), (left, Some(left)));
    assert_eq!(format!("{lite:?}"), format!("{real:?}"));
}

#[test]
fn var_errors_match_std() {
    let real = std::env::VarError::NotPresent;
    let lite = lenv::VarError::NotPresent;
    assert_eq!(lite.to_string(), real.to_string());
    assert_eq!(format!("{lite:?}"), format!("{real:?}"));
    let bytes = "caf\u{e9} \"x\"\n";
    let real = std::env::VarError::NotUnicode(bytes.into());
    let lite = lenv::VarError::NotUnicode(bytes.into());
    assert_eq!(lite.to_string(), real.to_string());
    assert_eq!(format!("{lite:?}"), format!("{real:?}"));
    assert_eq!(lite.clone(), lite);
    assert_ne!(lite, lenv::VarError::NotPresent);
    let _: &dyn core::error::Error = &lite;
}

/// The arguments of this process. Miri emulates std's, but litestd reads
/// the host's `/proc`, so under Miri they only have to agree with
/// themselves.
#[test]
#[cfg_attr(
    all(miri, target_os = "freebsd"),
    ignore = "Miri emulates no sysctl on FreeBSD"
)]
fn args_match_std() {
    let lite: Vec<_> = lenv::args_os().map(|a| lite_units(&a)).collect();
    let rev: Vec<_> = lenv::args_os().rev().map(|a| lite_units(&a)).collect();
    assert!(rev.iter().eq(lite.iter().rev()));
    assert_eq!(lenv::args_os().len(), lite.len());
    assert_ne!(lite.len(), 0);
    // The arguments of a test run are Unicode, so `args` yields them too.
    let strings: Vec<String> = lenv::args().collect();
    let back: Vec<String> = lenv::args().rev().collect();
    assert!(back.iter().eq(strings.iter().rev()));
    let units: Vec<Vec<u32>> = strings
        .iter()
        .map(|s| s.bytes().map(u32::from).collect())
        .collect();
    // Windows units are UTF-16, not the UTF-8 of the strings.
    assert!(cfg!(windows) || units == lite);
    if !cfg!(miri) {
        let real: Vec<_> = std::env::args_os().map(|a| std_units(&a)).collect();
        assert_eq!(lite, real);
        assert_eq!(
            format!("{:?}", lenv::args()),
            format!("{:?}", std::env::args())
        );
    }
}

#[test]
#[cfg_attr(
    all(miri, target_os = "freebsd"),
    ignore = "Miri emulates no sysctl on FreeBSD"
)]
fn directories_match_std() {
    same_path(
        "current_dir",
        lenv::current_dir().ok().as_deref(),
        std::env::current_dir().ok().as_deref(),
    );
    same_path(
        "current_exe",
        lenv::current_exe().ok().as_deref(),
        std::env::current_exe().ok().as_deref(),
    );
    same_path(
        "home_dir",
        lenv::home_dir().as_deref(),
        std::env::home_dir().as_deref(),
    );
}

#[test]
#[cfg_attr(target_os = "wasi", ignore = "std panics here on WASI")]
fn temp_dir_matches_std() {
    same_path(
        "temp_dir",
        Some(&lenv::temp_dir()),
        Some(&std::env::temp_dir()),
    );
}

#[test]
fn consts_match_std() {
    use std::env::consts as real;

    use litestd::env::consts as lite;
    assert_eq!(lite::ARCH, real::ARCH);
    assert_eq!(lite::FAMILY, real::FAMILY);
    assert_eq!(lite::OS, real::OS);
    assert_eq!(lite::DLL_PREFIX, real::DLL_PREFIX);
    assert_eq!(lite::DLL_SUFFIX, real::DLL_SUFFIX);
    assert_eq!(lite::DLL_EXTENSION, real::DLL_EXTENSION);
    assert_eq!(lite::EXE_SUFFIX, real::EXE_SUFFIX);
    assert_eq!(lite::EXE_EXTENSION, real::EXE_EXTENSION);
}

/// Path lists for both platforms, with every separator and quoting case.
const PATH_LISTS: &[&str] = &[
    "",
    ":",
    ";",
    "::",
    ";;",
    "/bin",
    "/bin:/usr/bin",
    "/bin:",
    ":/bin",
    r"c:\foo;c:\bar",
    r#"c:\foo;c:\som"e;di"r;c:\bar"#,
    r#""c:\a;b";c:\c"#,
    r#"""#,
    r#"a"b"c;d"#,
    r#"";"#,
    "\u{e9}t\u{e9};caf\u{e9}:\u{1f600}",
    " ; : ",
];

/// Asserts that litestd splits `list` as std does.
fn same_split(list: &std::ffi::OsStr) {
    let lite_list = to_lite(list);
    let lite: Vec<_> = lenv::split_paths(&lite_list)
        .map(|p| lite_units(p.as_os_str()))
        .collect();
    let real: Vec<_> = std::env::split_paths(list)
        .map(|p| std_units(p.as_os_str()))
        .collect();
    assert_eq!(lite, real, "{list:?}");
    let (low, high) = lenv::split_paths(&lite_list).size_hint();
    assert!(low <= real.len() && high.is_none_or(|h| h >= real.len()));
    assert_eq!(
        format!("{:?}", lenv::split_paths(&lite_list)),
        format!("{:?}", std::env::split_paths(list)),
    );
}

#[test]
#[cfg_attr(target_os = "wasi", ignore = "std panics here on WASI")]
fn split_paths_matches_std() {
    for list in PATH_LISTS {
        same_split(list.as_ref());
    }
    #[cfg(not(windows))]
    {
        use crate::std_ffi::OsStrExt;
        same_split(std::ffi::OsStr::from_bytes(b"/a\xff:/b\x80:"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        // Removing the quote joins the surrogates into one code point.
        let q = u16::from(b'"');
        let semi = u16::from(b';');
        for units in [
            &[0xD83D, q, 0xDE00, semi, 0xD800][..],
            &[0xDC00, semi, q, 0xD800, q, 0xDC00],
        ] {
            same_split(&std::ffi::OsString::from_wide(units));
        }
    }
}

#[test]
fn join_paths_matches_std() {
    let cases: &[&[&str]] = &[
        &[],
        &[""],
        &["", ""],
        &["/bin", "/usr/bin"],
        &["/bin:/sbin"],
        &[r"c:\a;b", r"c:\c"],
        &[r#"c:\"quoted""#],
        &["caf\u{e9}", "\u{1f600}"],
    ];
    for paths in cases {
        let lite = lenv::join_paths(paths.iter());
        let real = std::env::join_paths(paths.iter());
        match (lite, real) {
            (Ok(lite), Ok(real)) => {
                assert_eq!(lite_units(&lite), std_units(&real), "{paths:?}");
            }
            (Err(lite), Err(real)) => {
                assert_eq!(lite.to_string(), real.to_string());
                assert_eq!(format!("{lite:?}"), format!("{real:?}"));
                assert_eq!(format!("{lite:#?}"), format!("{real:#?}"));
            }
            (lite, real) => panic!("{paths:?}: {lite:?} but std {real:?}"),
        }
    }
}

#[test]
#[cfg_attr(target_os = "wasi", ignore = "WASI has no lists of paths")]
fn round_trips_through_split_paths() {
    let paths = ["/a b", "c", "", "\u{e9}"];
    let joined = lenv::join_paths(paths).unwrap();
    let split: Vec<_> = lenv::split_paths(&joined).collect();
    let back: Vec<_> = split.iter().map(|p| p.to_str().unwrap()).collect();
    assert_eq!(back, paths);
}

/// Compiles only if `T` is `Send`, `Sync`, `Unpin` and unwind-safe.
const fn thread_safe<T: Send + Sync + Unpin + UnwindSafe + RefUnwindSafe>() {}

/// Compiles only if `T` is `Unpin` and unwind-safe.
const fn unwind_safe<T: Unpin + UnwindSafe + RefUnwindSafe>() {}

/// Implemented twice for `Send` types, so that naming its item for one is
/// ambiguous and does not compile.
trait AmbiguousIfSend<A> {
    fn check() {}
}
impl<T: ?Sized> AmbiguousIfSend<()> for T {}
impl<T: ?Sized + Send> AmbiguousIfSend<u8> for T {}

/// As [`AmbiguousIfSend`], for `Sync`.
trait AmbiguousIfSync<A> {
    fn check() {}
}
impl<T: ?Sized> AmbiguousIfSync<()> for T {}
impl<T: ?Sized + Sync> AmbiguousIfSync<u8> for T {}

#[test]
fn auto_traits_match_std() {
    // std's iterators over the arguments and variables are neither `Send`
    // nor `Sync`; the rest is.
    <std::env::Args as AmbiguousIfSend<_>>::check();
    <std::env::VarsOs as AmbiguousIfSync<_>>::check();
    thread_safe::<std::env::SplitPaths<'static>>();
    <lenv::Args as AmbiguousIfSend<_>>::check();
    <lenv::Args as AmbiguousIfSync<_>>::check();
    <lenv::ArgsOs as AmbiguousIfSend<_>>::check();
    <lenv::ArgsOs as AmbiguousIfSync<_>>::check();
    <lenv::Vars as AmbiguousIfSend<_>>::check();
    <lenv::Vars as AmbiguousIfSync<_>>::check();
    <lenv::VarsOs as AmbiguousIfSend<_>>::check();
    <lenv::VarsOs as AmbiguousIfSync<_>>::check();
    unwind_safe::<lenv::Args>();
    unwind_safe::<lenv::ArgsOs>();
    unwind_safe::<lenv::Vars>();
    unwind_safe::<lenv::VarsOs>();
    thread_safe::<lenv::SplitPaths<'static>>();
    thread_safe::<lenv::JoinPathsError>();
    thread_safe::<lenv::VarError>();
}

/// Compiles only if the iterators have std's item types and traits.
#[test]
#[cfg_attr(
    all(miri, target_os = "freebsd"),
    ignore = "Miri emulates no sysctl on FreeBSD"
)]
fn iterator_traits_match_std() {
    fn args<I>()
    where
        I: Iterator
            + ExactSizeIterator
            + DoubleEndedIterator
            + core::fmt::Debug,
    {
    }
    fn vars<I: Iterator + core::fmt::Debug>() {}
    fn error<E: core::error::Error + core::fmt::Display>() {}
    args::<lenv::Args>();
    args::<lenv::ArgsOs>();
    vars::<lenv::Vars>();
    vars::<lenv::VarsOs>();
    vars::<lenv::SplitPaths<'static>>();
    error::<lenv::JoinPathsError>();
    error::<lenv::VarError>();
    let _: Option<String> = lenv::args().next();
    let _: Option<litestd::ffi::OsString> = lenv::args_os().next();
    let _: Option<(String, String)> = lenv::vars().next();
    let _: Option<(litestd::ffi::OsString, litestd::ffi::OsString)> =
        lenv::vars_os().next();
}

/// Keeps the children that tests start, some of which panic on purpose,
/// from writing core files. The children inherit this process's limit.
#[cfg(all(unix, not(miri)))]
fn forbid_core_files() {
    let limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a valid `rlimit` for the duration of the call.
    let r = unsafe { libc::setrlimit(libc::RLIMIT_CORE, &raw const limit) };
    assert_eq!(r, 0);
}

/// Runs `child` in a new process with `CHILD=mode`, after `setup` adjusts
/// its command.
#[cfg(not(miri))]
fn run_child(mode: &str, setup: impl FnOnce(&mut Command)) -> Output {
    #[cfg(unix)]
    forbid_core_files();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "child", "--nocapture", "--test-threads=1", "-q"])
        .env(CHILD, mode);
    setup(&mut command);
    command.output().unwrap()
}

/// Runs `child` and asserts that its check passed.
#[cfg(not(miri))]
fn child_passes(mode: &str, setup: impl FnOnce(&mut Command)) {
    let out = run_child(mode, setup);
    assert!(
        out.status.success(),
        "{mode}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The child side of the tests that change the environment or the current
/// directory; a no-op in the parent.
#[test]
fn child() {
    #[cfg(not(miri))]
    if let Ok(mode) = std::env::var(CHILD) {
        run_check(&mode);
    }
}

#[cfg(not(miri))]
fn run_check(mode: &str) {
    match mode {
        "environment" => same_environment(),
        "home" => same_path(
            "home_dir",
            lenv::home_dir().as_deref(),
            std::env::home_dir().as_deref(),
        ),
        "temp" => same_path(
            "temp_dir",
            Some(&lenv::temp_dir()),
            Some(&std::env::temp_dir()),
        ),
        "set-remove" => set_and_remove(),
        "vars-panics" => {
            lenv::vars().for_each(drop);
        }
        "current-dir" => current_dir_follows_set_current_dir(),
        #[cfg(unix)]
        "deleted-dir" => deleted_current_dir(),
        other => {
            let Some((api, case)) = other.split_once(':') else {
                panic!("unknown child mode {other}");
            };
            invalid_change(api, case);
        }
    }
}

/// Values of every length class: empty, Unicode, and past the stack
/// buffers of both backends.
#[cfg(not(miri))]
fn values() -> Vec<String> {
    vec![
        String::new(),
        "x".into(),
        "caf\u{e9} \u{1f600} a=b".into(),
        "v".repeat(383),
        "w".repeat(384),
        "y".repeat(5000),
    ]
}

/// Sets and removes variables with litestd, checking each change with std
/// and each change std makes with litestd.
#[cfg(not(miri))]
fn set_and_remove() {
    let long_key = "K".repeat(600);
    for key in [KEY, long_key.as_str()] {
        for value in values() {
            // SAFETY: the child runs this check alone, on its only thread.
            unsafe { lenv::set_var(key, &value) };
            assert_eq!(std::env::var(key).as_deref(), Ok(value.as_str()));
            assert_eq!(lenv::var(key).as_deref(), Ok(value.as_str()));
            same_environment();
        }
        // SAFETY: as above.
        unsafe { lenv::remove_var(key) };
        assert_eq!(std::env::var_os(key), None);
        assert_eq!(lenv::var_os(key), None);
        // SAFETY: as above; removing an unset variable succeeds.
        unsafe { lenv::remove_var(key) };
        // SAFETY: as above.
        unsafe { std::env::set_var(key, "from std") };
        assert_eq!(lenv::var(key).as_deref(), Ok("from std"));
        // SAFETY: as above.
        unsafe { std::env::remove_var(key) };
        assert_eq!(lenv::var_os(key), None);
    }
}

/// Calls `set_var` or `remove_var` with litestd or std on an invalid name
/// or value; the parent compares how the two children end.
#[cfg(not(miri))]
fn invalid_change(api: &str, case: &str) {
    let (key, value) = match case {
        "empty" => ("", "v"),
        "equals" => ("A=B", "v"),
        "equals-first" => ("=A", "v"),
        "nul-key" => ("A\0B", "v"),
        "nul-value" => (KEY, "v\0w"),
        other => panic!("unknown case {other}"),
    };
    // SAFETY: the child runs this check alone, on its only thread.
    unsafe {
        match api {
            "lite-set" => lenv::set_var(key, value),
            "std-set" => std::env::set_var(key, value),
            "lite-remove" => lenv::remove_var(key),
            "std-remove" => std::env::remove_var(key),
            other => panic!("unknown api {other}"),
        }
    }
}

/// The cases of [`invalid_change`].
#[cfg(not(miri))]
const INVALID_CASES: &[&str] =
    &["empty", "equals", "equals-first", "nul-key", "nul-value"];

/// A scratch directory of its own for one test.
#[cfg(not(miri))]
struct Scratch(std::path::PathBuf);

#[cfg(not(miri))]
impl Scratch {
    fn new(name: &str) -> Self {
        use core::sync::atomic::{AtomicUsize, Ordering};
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let n = COUNT.fetch_add(1, Ordering::Relaxed);
        // WASI has no process ids or temporary directory in std; its runner
        // gives each program a `/tmp` of its own.
        let (base, id) = if cfg!(target_os = "wasi") {
            (std::path::PathBuf::from("/tmp"), 0)
        } else {
            (std::env::temp_dir(), std::process::id())
        };
        let path = base.join(format!("litestd-env-{name}-{id}-{n}"));
        if path.exists() {
            std::fs::remove_dir_all(&path).unwrap();
        }
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

#[cfg(not(miri))]
impl Drop for Scratch {
    fn drop(&mut self) {
        let result = std::fs::remove_dir_all(&self.0);
        assert!(result.is_ok() || std::thread::panicking(), "{result:?}");
    }
}

/// Changes directories with litestd in a child whose current directory is
/// a scratch directory, and checks every step with std.
#[cfg(not(miri))]
fn current_dir_follows_set_current_dir() {
    let start = std::env::current_dir().unwrap();
    same_path(
        "start",
        lenv::current_dir().ok().as_deref(),
        Some(start.as_path()),
    );
    std::fs::create_dir("sub").unwrap();
    lenv::set_current_dir("sub").unwrap();
    assert_eq!(std::env::current_dir().unwrap(), start.join("sub"));
    same_path(
        "sub",
        lenv::current_dir().ok().as_deref(),
        std::env::current_dir().ok().as_deref(),
    );
    lenv::set_current_dir("..").unwrap();
    assert_eq!(std::env::current_dir().unwrap(), start);
    let missing = lenv::set_current_dir("missing").unwrap_err();
    let real = std::env::set_current_dir("missing").unwrap_err();
    assert_eq!(missing.raw_os_error(), real.raw_os_error());
    let nul = lenv::set_current_dir("sub\0").unwrap_err();
    assert_eq!(nul.kind(), litestd::io::ErrorKind::InvalidInput);
    #[cfg(unix)]
    assert_eq!(
        nul.to_string(),
        std::env::set_current_dir("sub\0").unwrap_err().to_string()
    );
    assert_eq!(std::env::current_dir().unwrap(), start);
    // Past the stack buffers, and past `MAX_PATH`, which Windows may refuse.
    let deep = start.join("a".repeat(200)).join("b".repeat(200));
    std::fs::create_dir_all(&deep).unwrap();
    let lite_deep = litestd::path::Path::new(deep.to_str().unwrap());
    match lenv::set_current_dir(lite_deep) {
        Ok(()) => same_path(
            "deep",
            lenv::current_dir().ok().as_deref(),
            std::env::current_dir().ok().as_deref(),
        ),
        Err(lite) => {
            let real = std::env::set_current_dir(&deep).unwrap_err();
            assert_eq!(lite.raw_os_error(), real.raw_os_error());
        }
    }
    lenv::set_current_dir(litestd::path::Path::new(start.to_str().unwrap()))
        .unwrap();
}

/// Removes the child's own scratch directory while it is current: both
/// crates then fail alike.
#[cfg(all(unix, not(miri)))]
fn deleted_current_dir() {
    let here = std::env::current_dir().unwrap();
    std::fs::create_dir("gone").unwrap();
    std::env::set_current_dir("gone").unwrap();
    std::fs::remove_dir(here.join("gone")).unwrap();
    let lite = lenv::current_dir().unwrap_err();
    let real = std::env::current_dir().unwrap_err();
    assert_eq!(lite.raw_os_error(), real.raw_os_error());
    std::env::set_current_dir(&here).unwrap();
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn chosen_environment_matches_std() {
    child_passes("environment", |command| {
        command
            .env("LITESTD_EMPTY", "")
            .env("LITESTD_UNICODE", "caf\u{e9} \u{1f600}")
            .env("LITESTD_EQUALS", "a=b=c")
            .env("LITESTD_LONG", "z".repeat(40_000));
        for i in 0..300 {
            command.env(format!("LITESTD_MANY_{i}"), i.to_string());
        }
        #[cfg(not(windows))]
        {
            use crate::std_ffi::OsStrExt;
            let odd = std::ffi::OsStr::from_bytes(b"\xff\xfe");
            command.env("LITESTD_NOT_UNICODE", odd);
            command.env(odd, "value");
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            let odd = std::ffi::OsString::from_wide(&[0xD800, 0x61]);
            command.env("LITESTD_NOT_UNICODE", &odd);
        }
    });
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn empty_environment_matches_std() {
    child_passes("environment", |command| {
        command.env_clear().env(CHILD, "environment");
    });
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn vars_panics_on_non_unicode() {
    let out = run_child("vars-panics", |command| {
        #[cfg(not(windows))]
        {
            use crate::std_ffi::OsStrExt;
            command.env(
                "LITESTD_NOT_UNICODE",
                std::ffi::OsStr::from_bytes(b"\xff"),
            );
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            command.env(
                "LITESTD_NOT_UNICODE",
                std::ffi::OsString::from_wide(&[0xDC00]),
            );
        }
    });
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        stderr.contains("an environment variable is not valid Unicode"),
        "{stderr}"
    );
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn set_and_remove_var_match_std() {
    child_passes("set-remove", |_| {});
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn invalid_changes_fail_as_in_std() {
    for case in INVALID_CASES {
        for (lite, real) in
            [("lite-set", "std-set"), ("lite-remove", "std-remove")]
        {
            let lite_out = run_child(&format!("{lite}:{case}"), |_| {});
            let real_out = run_child(&format!("{real}:{case}"), |_| {});
            let stderr = String::from_utf8_lossy(&lite_out.stderr);
            let failed = !lite_out.status.success();
            assert_eq!(failed, !real_out.status.success(), "{lite}:{case}");
            let expected = if lite == "lite-set" {
                "failed to set environment variable"
            } else {
                "failed to remove environment variable"
            };
            assert!(!failed || stderr.contains(expected), "{stderr}");
        }
    }
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn home_dir_falls_back_as_in_std() {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    child_passes("home", |command| {
        command.env_remove(var);
    });
    child_passes("home", |command| {
        command.env(var, "");
    });
    child_passes("home", |command| {
        command.env(var, "/some/where");
    });
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn temp_dir_follows_the_environment_as_in_std() {
    // Windows reports a one-unit path for an empty `TMP`, different on each
    // call, so std and litestd cannot agree on it.
    let values: &[&str] = if cfg!(windows) {
        &["/elsewhere", "relative"]
    } else {
        &["", "/elsewhere", "relative"]
    };
    for &value in values {
        child_passes("temp", |command| {
            if cfg!(windows) {
                command.env("TMP", value).env("TEMP", value);
            } else {
                command.env("TMPDIR", value);
            }
        });
    }
    child_passes("temp", |command| {
        command
            .env_remove("TMPDIR")
            .env_remove("TMP")
            .env_remove("TEMP");
    });
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn set_current_dir_matches_std() {
    let scratch = Scratch::new("cwd");
    child_passes("current-dir", |command| {
        command.current_dir(&scratch.0);
    });
}

#[cfg(all(unix, not(miri)))]
#[test]
fn deleted_current_dir_fails_as_in_std() {
    let scratch = Scratch::new("deleted");
    child_passes("deleted-dir", |command| {
        command.current_dir(&scratch.0);
    });
}
