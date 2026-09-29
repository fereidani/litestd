//! `litestd::path` against std.
//!
//! The same code runs against both crates, so any difference in signatures
//! or trait implementations fails to compile, and every result is compared.
//! `tests/path_corpus.rs` compares the operations over generated paths.

#![cfg(feature = "path")]
#![allow(
    clippy::incompatible_msrv,
    reason = "the tests compare against the std of the toolchain that runs them"
)]

extern crate alloc;

use alloc::{borrow::Cow, rc::Rc, sync::Arc};
use core::{
    borrow::Borrow,
    error::Error,
    fmt::Debug,
    hash::{Hash, Hasher},
    ptr,
};
#[cfg(unix)]
use std::os::unix::ffi as std_ffi;
#[cfg(target_os = "wasi")]
use std::os::wasi::ffi as std_ffi;

/// The byte-string extensions of the platform: Unix's, or WASI's.
#[cfg(unix)]
use litestd::os::unix::ffi as lite_ffi;
#[cfg(target_os = "wasi")]
use litestd::os::wasi::ffi as lite_ffi;

/// A hasher that records every write, to compare what values feed it.
#[derive(Default)]
struct Recorder(Vec<Vec<u8>>);

impl Hasher for Recorder {
    fn finish(&self) -> u64 {
        0
    }

    fn write(&mut self, bytes: &[u8]) {
        self.0.push(bytes.to_vec());
    }
}

fn hash_record<T: Hash + ?Sized>(value: &T) -> Vec<Vec<u8>> {
    let mut recorder = Recorder::default();
    value.hash(&mut recorder);
    recorder.0
}

/// Appends the `Debug` form of `value` to the log.
fn rec(log: &mut Vec<String>, value: impl Debug) {
    log.push(format!("{value:?}"));
}

/// Logs `==` and `partial_cmp` both ways between two values.
macro_rules! cmp_both {
    ($l:ident, $a:expr, $b:expr) => {
        rec(
            &mut $l,
            ($a == $b, $b == $a, $a.partial_cmp(&$b), $b.partial_cmp(&$a)),
        )
    };
}

/// Logs `==` both ways between two values.
macro_rules! eq_both {
    ($l:ident, $a:expr, $b:expr) => {
        rec(&mut $l, ($a == $b, $b == $a))
    };
}

/// Exercises the whole API of one crate's `path` module, returning a log of
/// every result.
macro_rules! api_log {
    ($krate:ident) => {{
        use $krate::{
            ffi::{OsStr, OsString},
            path::{
                self, Ancestors, Component, Components, Display, Iter,
                MAIN_SEPARATOR, MAIN_SEPARATOR_STR, Path, PathBuf, Prefix,
                StripPrefixError,
            },
        };

        let mut l = Vec::new();
        rec(&mut l, (MAIN_SEPARATOR, MAIN_SEPARATOR_STR));
        rec(
            &mut l,
            ['/', '\\', 'a', '\u{e9}', ':'].map(path::is_separator),
        );

        // Inspection.
        let p = Path::new("/tmp/./foo//bar.tar.gz/");
        rec(&mut l, p);
        rec(&mut l, (p.as_os_str(), p.to_str(), p.to_string_lossy()));
        rec(&mut l, p.to_path_buf());
        rec(&mut l, (p.is_absolute(), p.is_relative(), p.has_root()));
        rec(&mut l, (p.is_empty(), Path::new("").is_empty()));
        rec(&mut l, (p.parent(), p.file_name(), p.file_stem()));
        rec(&mut l, (p.file_prefix(), p.extension()));
        let ancestors: Ancestors<'_> = p.ancestors();
        rec(&mut l, ancestors);
        rec(&mut l, ancestors.collect::<Vec<_>>());
        let components: Components<'_> = p.components();
        rec(&mut l, &components);
        rec(&mut l, components.clone().collect::<Vec<Component<'_>>>());
        rec(&mut l, components.clone().rev().collect::<Vec<_>>());
        let iter: Iter<'_> = p.iter();
        rec(&mut l, &iter);
        rec(&mut l, iter.clone().collect::<Vec<&OsStr>>());
        rec(&mut l, iter.clone().rev().collect::<Vec<_>>());
        let mut c = p.components();
        c.next();
        c.next_back();
        rec(&mut l, (c.as_path(), AsRef::<Path>::as_ref(&c)));
        rec(&mut l, AsRef::<OsStr>::as_ref(&c));
        rec(&mut l, (c.next(), c.next(), c.next(), c.next(), c.next()));
        let mut i = p.iter();
        i.next();
        rec(&mut l, (i.as_path(), AsRef::<Path>::as_ref(&i)));
        rec(&mut l, AsRef::<OsStr>::as_ref(&i));
        rec(&mut l, (i.next_back(), i.next(), i.next(), i.next()));
        let display: Display<'_> = p.display();
        rec(&mut l, &display);
        rec(
            &mut l,
            format!("{display}|{display:>40}|{display:.4}|{display:-^9.3}"),
        );

        // Prefix matching.
        rec(&mut l, p.strip_prefix("/tmp"));
        rec(&mut l, p.strip_prefix(PathBuf::from("/tmp/foo/")));
        rec(&mut l, p.strip_prefix(p));
        let error: StripPrefixError = p.strip_prefix("tmp").unwrap_err();
        rec(
            &mut l,
            (&error, error.clone() == error, error.source().is_none()),
        );
        rec(&mut l, format!("{error}|{error:>20}|{error:.6}"));
        rec(&mut l, (p.starts_with("/tmp"), p.starts_with("/t")));
        rec(&mut l, (p.starts_with(OsStr::new("/")), p.starts_with("")));
        rec(&mut l, (p.ends_with("bar.tar.gz"), p.ends_with("gz")));
        rec(&mut l, p.ends_with(PathBuf::from("foo/bar.tar.gz")));

        // Building new paths.
        rec(
            &mut l,
            (p.join("x"), p.join("/abs"), p.join(""), p.join(".")),
        );
        rec(
            &mut l,
            (p.with_file_name("n"), p.with_file_name(OsString::from("m"))),
        );
        rec(&mut l, (p.with_extension("zip"), p.with_extension("")));
        rec(&mut l, p.with_extension(OsStr::new("a.b")));
        rec(
            &mut l,
            (p.with_added_extension("zst"), p.with_added_extension("")),
        );
        rec(&mut l, Path::new("..x").with_extension("y"));
        rec(&mut l, Path::new("/").with_added_extension("gz"));

        // `PathBuf`.
        let mut b = PathBuf::new();
        rec(&mut l, (&b, b.capacity()));
        b.push("a");
        b.push(String::from("b/"));
        b.push(Path::new("c"));
        b.push(OsStr::new("d.txt"));
        rec(&mut l, &b);
        rec(&mut l, (b.pop(), b.clone(), b.pop(), b.clone()));
        b.set_file_name("name.txt");
        rec(&mut l, &b);
        rec(&mut l, (b.set_extension("md"), b.clone()));
        rec(&mut l, (b.add_extension("gz"), b.clone()));
        rec(
            &mut l,
            (
                b.set_extension(""),
                b.clone(),
                b.add_extension(""),
                b.clone(),
            ),
        );
        let mut root = PathBuf::from("/");
        rec(
            &mut l,
            (root.set_extension("x"), root.add_extension("x"), root.pop()),
        );
        root.set_file_name("f");
        rec(&mut l, &root);
        b.as_mut_os_string().push("-raw");
        rec(&mut l, &b);
        b.as_mut_os_str().make_ascii_uppercase();
        rec(&mut l, &b);
        rec(&mut l, b.as_path());
        rec(&mut l, b.clone().into_os_string());
        rec(&mut l, b.clone().into_string());
        rec(&mut l, b.clone().into_boxed_path());
        let leaked: &'static mut Path = b.clone().leak();
        leaked.as_mut_os_str().make_ascii_lowercase();
        rec(&mut l, &*leaked);
        // SAFETY: `leak` returned the whole allocation of a clone, which has
        // no spare capacity, so it is the allocation of a `Box<Path>` of the
        // same length.
        drop(unsafe { Box::from_raw(ptr::from_mut(leaked)) });
        let mut cap = PathBuf::with_capacity(16);
        rec(&mut l, cap.capacity() >= 16);
        cap.reserve(40);
        rec(&mut l, cap.capacity() >= 40);
        cap.reserve_exact(64);
        rec(&mut l, cap.capacity() >= 64);
        rec(
            &mut l,
            (cap.try_reserve(8).is_ok(), cap.try_reserve_exact(8).is_ok()),
        );
        rec(&mut l, cap.try_reserve(usize::MAX).is_err());
        cap.push("abc");
        cap.shrink_to(10);
        rec(&mut l, cap.capacity());
        cap.shrink_to_fit();
        rec(&mut l, cap.capacity());
        cap.clear();
        rec(&mut l, (&cap, cap.capacity()));

        // Components.
        rec(&mut l, Component::RootDir.as_os_str());
        rec(
            &mut l,
            (
                Component::CurDir.as_os_str(),
                Component::ParentDir.as_os_str(),
            ),
        );
        rec(&mut l, Component::Normal(OsStr::new("n")).as_os_str());
        rec(&mut l, AsRef::<Path>::as_ref(&Component::CurDir));
        rec(&mut l, AsRef::<OsStr>::as_ref(&Component::ParentDir));
        rec(&mut l, Component::CurDir.cmp(&Component::ParentDir));
        rec(
            &mut l,
            Component::Normal(OsStr::new("a")) < Component::RootDir,
        );
        rec(&mut l, hash_record(&Component::Normal(OsStr::new("h"))));
        let prefixes = [
            Prefix::Verbatim(OsStr::new("v")),
            Prefix::VerbatimUNC(OsStr::new("s"), OsStr::new("h")),
            Prefix::VerbatimDisk(b'C'),
            Prefix::DeviceNS(OsStr::new("COM1")),
            Prefix::UNC(OsStr::new("s"), OsStr::new("h")),
            Prefix::Disk(b'D'),
        ];
        for prefix in prefixes {
            rec(&mut l, (prefix, prefix.is_verbatim(), hash_record(&prefix)));
            rec(&mut l, prefixes.map(|other| prefix.cmp(&other)));
        }

        // Conversions.
        rec(&mut l, PathBuf::from("s"));
        rec(&mut l, PathBuf::from(String::from("string")));
        rec(&mut l, PathBuf::from(OsString::from("os")));
        rec(&mut l, PathBuf::from(OsStr::new("os str")));
        rec(&mut l, PathBuf::from(&PathBuf::from("ref")));
        rec(&mut l, PathBuf::from(Box::<Path>::from(Path::new("box"))));
        rec(&mut l, PathBuf::from(Cow::Borrowed(Path::new("cow"))));
        rec(
            &mut l,
            PathBuf::from(Cow::<Path>::Owned(PathBuf::from("cow2"))),
        );
        rec(&mut l, OsString::from(PathBuf::from("back")));
        let mut for_mut = PathBuf::from("mut");
        rec(&mut l, Box::<Path>::from(Path::new("r")));
        rec(&mut l, Box::<Path>::from(&mut *for_mut));
        rec(&mut l, Box::<Path>::from(Cow::Borrowed(Path::new("c"))));
        rec(
            &mut l,
            Box::<Path>::from(Cow::<Path>::Owned(PathBuf::from("o"))),
        );
        rec(&mut l, Box::<Path>::from(PathBuf::from("pb")));
        let boxed = Box::<Path>::from(Path::new("boxed"));
        rec(&mut l, (boxed.clone(), boxed.into_path_buf()));
        rec(&mut l, Arc::<Path>::from(PathBuf::from("arc")));
        rec(&mut l, Arc::<Path>::from(Path::new("arc ref")));
        rec(&mut l, Arc::<Path>::from(&mut *for_mut));
        rec(&mut l, Rc::<Path>::from(PathBuf::from("rc")));
        rec(&mut l, Rc::<Path>::from(Path::new("rc ref")));
        rec(&mut l, Rc::<Path>::from(&mut *for_mut));
        let owned_buf = PathBuf::from("cow ref");
        let cows: [Cow<'_, Path>; 3] = [
            Cow::from(Path::new("b")),
            Cow::from(PathBuf::from("o")),
            Cow::from(&owned_buf),
        ];
        rec(&mut l, cows.clone().map(|c| matches!(c, Cow::Borrowed(_))));
        rec(&mut l, "parsed".parse::<PathBuf>());
        rec(&mut l, PathBuf::from_iter(["/tmp", "foo", "bar"]));
        rec(&mut l, ["a", "b", "/c", "d"].iter().collect::<PathBuf>());
        let mut extended = PathBuf::from("/tmp");
        extended.extend(["foo", "bar"]);
        extended.extend([Path::new("baz")]);
        rec(&mut l, &extended);
        rec(&mut l, (PathBuf::default(), Path::new("x").to_owned()));
        let mut target = PathBuf::from("a much longer path than the source");
        Path::new("src").clone_into(&mut target);
        rec(&mut l, &target);
        target.clone_from(&PathBuf::from("from"));
        rec(&mut l, &target);
        let borrowed: &Path = extended.borrow();
        rec(&mut l, (borrowed, &*extended));
        (*extended).as_mut_os_str().make_ascii_uppercase();
        rec(&mut l, &extended);
        let cow_os_str: Cow<'_, OsStr> = Cow::Borrowed(OsStr::new("c"));
        let os_string_s = OsString::from("s");
        let string = String::from("string");
        let buf = PathBuf::from("buf");
        let cur_dir = Component::CurDir;
        let as_ref_paths: [&Path; 8] = [
            Path::new("p").as_ref(),
            OsStr::new("o").as_ref(),
            AsRef::<Path>::as_ref(&cow_os_str),
            AsRef::<Path>::as_ref(&os_string_s),
            "str".as_ref(),
            AsRef::<Path>::as_ref(&string),
            AsRef::<Path>::as_ref(&buf),
            AsRef::<Path>::as_ref(&cur_dir),
        ];
        rec(&mut l, as_ref_paths);
        let as_ref_os: [&OsStr; 2] =
            [Path::new("p").as_ref(), AsRef::<OsStr>::as_ref(&buf)];
        rec(&mut l, as_ref_os);
        let into_iter: Vec<&OsStr> = (&extended).into_iter().collect();
        rec(&mut l, into_iter);
        let mut via_for = Vec::new();
        for part in Path::new("x/y") {
            via_for.push(part);
        }
        rec(&mut l, via_for);

        // Comparison and hashing.
        let pb = PathBuf::from("/a/b");
        let path = Path::new("/a//b/.");
        let os = OsStr::new("/a/b/");
        let os_string = OsString::from("/a/c");
        let cow_path: Cow<'_, Path> = Cow::Borrowed(Path::new("/a"));
        let cow_os: Cow<'_, OsStr> = Cow::Borrowed(OsStr::new("/a/b"));
        rec(
            &mut l,
            (
                pb == pb.clone(),
                *path == *path,
                pb.cmp(&pb),
                path.cmp(path),
            ),
        );
        eq_both!(l, pb, *"/a/b");
        eq_both!(l, pb, String::from("/a/b"));
        eq_both!(l, *path, *"/a/b");
        eq_both!(l, *path, String::from("/a/b/"));
        cmp_both!(l, pb, *path);
        cmp_both!(l, pb, path);
        cmp_both!(l, cow_path, *path);
        cmp_both!(l, cow_path, path);
        cmp_both!(l, cow_path, pb);
        cmp_both!(l, pb, *os);
        cmp_both!(l, pb, os);
        cmp_both!(l, pb, cow_os);
        cmp_both!(l, pb, os_string);
        cmp_both!(l, *path, *os);
        cmp_both!(l, *path, os);
        cmp_both!(l, *path, cow_os);
        cmp_both!(l, *path, os_string);
        cmp_both!(l, path, *os);
        cmp_both!(l, path, cow_os);
        cmp_both!(l, path, os_string);
        cmp_both!(l, cow_path, *os);
        cmp_both!(l, cow_path, os);
        cmp_both!(l, cow_path, os_string);
        rec(
            &mut l,
            (hash_record(&pb), hash_record(path), hash_record(&cow_path)),
        );
        rec(&mut l, pb.components() == path.components());
        rec(&mut l, pb.components().cmp(Path::new("/a/c").components()));
        rec(&mut l, pb.components().partial_cmp(path.components()));

        #[cfg(windows)]
        {
            use $krate::path::PrefixComponent;

            for s in [
                r"C:\x",
                r"c:y",
                r"\\?\C:\x",
                r"\\?\UNC\server\share\x",
                r"\\?\pictures\kittens",
                r"\\.\COM1\x",
                r"\\server\share\x",
                r"//server/share",
            ] {
                let p = Path::new(s);
                if let Some(Component::Prefix(pc)) = p.components().next() {
                    let pc: PrefixComponent<'_> = pc;
                    rec(&mut l, (pc, pc.kind(), pc.as_os_str()));
                    rec(&mut l, (hash_record(&pc), pc == pc, pc.cmp(&pc)));
                }
                rec(&mut l, (p.components().collect::<Vec<_>>(), p.has_root()));
                rec(&mut l, (p.is_absolute(), p.parent(), p.file_name()));
            }
        }
        l
    }};
}

#[test]
fn api_matches_std() {
    let std_log = api_log!(std);
    let lite_log = api_log!(litestd);
    assert_eq!(std_log.len(), lite_log.len());
    for (i, (s, t)) in std_log.iter().zip(&lite_log).enumerate() {
        assert_eq!(s, t, "entry {i}");
    }
}

#[test]
fn auto_traits_match_std() {
    use core::panic::{RefUnwindSafe, UnwindSafe};

    use litestd::path::*;

    fn check<T: Send + Sync + Unpin + UnwindSafe + RefUnwindSafe>() {}
    fn check_unsized<
        T: ?Sized + Send + Sync + Unpin + UnwindSafe + RefUnwindSafe,
    >() {
    }
    fn copy<T: Copy>() {}

    check_unsized::<Path>();
    check::<PathBuf>();
    check::<Component<'static>>();
    check::<Components<'static>>();
    check::<Iter<'static>>();
    check::<Ancestors<'static>>();
    check::<Display<'static>>();
    check::<Prefix<'static>>();
    check::<PrefixComponent<'static>>();
    check::<StripPrefixError>();
    copy::<Component<'static>>();
    copy::<Prefix<'static>>();
    copy::<PrefixComponent<'static>>();
    copy::<Ancestors<'static>>();
}

#[test]
fn new_is_const() {
    const EMPTY: litestd::path::PathBuf = litestd::path::PathBuf::new();
    assert!(EMPTY.as_os_str().is_empty());
}

#[test]
fn iterators_are_fused() {
    let path = litestd::path::Path::new("/a");
    let mut components = path.components();
    assert_eq!(components.by_ref().count(), 2);
    assert_eq!(components.next(), None);
    assert_eq!(components.next_back(), None);
    let mut iter = path.iter();
    assert_eq!(iter.by_ref().count(), 2);
    assert_eq!(iter.next(), None);
    let mut ancestors = path.ancestors();
    assert_eq!(ancestors.by_ref().count(), 2);
    assert_eq!(ancestors.next(), None);
}

fn fuse_check<I: core::iter::FusedIterator>(_: I) {}

#[test]
fn iterators_implement_fused_iterator() {
    let path = litestd::path::Path::new("x");
    fuse_check(path.components());
    fuse_check(path.iter());
    fuse_check(path.ancestors());
}

/// Every method that takes an extension panics, like std, when it holds a
/// separator.
mod extension_separators {
    use litestd::path::{Path, PathBuf};

    #[test]
    #[should_panic = "extension cannot contain path separators"]
    fn set_extension() {
        PathBuf::from("a.b").set_extension("c/d");
    }

    #[test]
    #[should_panic = "extension cannot contain path separators"]
    fn set_extension_without_file_name() {
        PathBuf::from("/").set_extension("/");
    }

    #[test]
    #[should_panic = "extension cannot contain path separators"]
    fn add_extension() {
        PathBuf::from("a").add_extension("/");
    }

    #[test]
    #[should_panic = "extension cannot contain path separators"]
    fn with_extension() {
        let _ = Path::new("a.b").with_extension("x/");
    }

    #[test]
    #[should_panic = "extension cannot contain path separators"]
    fn with_extension_of_dot_dot_name() {
        let _ = Path::new("..b").with_extension("/");
    }

    #[test]
    #[should_panic = "extension cannot contain path separators"]
    fn with_added_extension() {
        let _ = Path::new("a").with_added_extension("x/y");
    }

    #[cfg(windows)]
    #[test]
    #[should_panic = "extension cannot contain path separators"]
    fn backslash_on_windows() {
        PathBuf::from("a").set_extension(r"x\y");
    }

    #[cfg(unix)]
    #[test]
    fn backslash_is_no_separator_on_unix() {
        let mut path = PathBuf::from("a");
        assert!(path.set_extension(r"x\y"));
        assert_eq!(path, Path::new(r"a.x\y"));
    }
}

/// `with_extension` of a file name made of `..` and an extension returns the
/// `..` alone, as std does.
#[test]
fn with_extension_of_dot_dot_names() {
    use litestd::path::Path;

    for (input, output) in [
        ("..b", ".."),
        ("x/..b", "x/.."),
        ("..b/", "..y"),
        ("x/..bc/", "x/..y"),
        ("...b", "...y"),
        ("a.b", "a.y"),
    ] {
        let lite = Path::new(input).with_extension("y");
        let std = std::path::Path::new(input).with_extension("y");
        assert_eq!(lite.as_os_str().as_encoded_bytes(), output.as_bytes());
        assert_eq!(std.as_os_str().as_encoded_bytes(), output.as_bytes());
    }
}

#[cfg(any(unix, target_os = "wasi"))]
#[test]
fn non_utf8_paths() {
    use litestd::{ffi::OsStr, path::Path};

    use crate::{lite_ffi::OsStrExt, std_ffi::OsStrExt as _};

    let bytes = b"/tmp/\xff\xfe.\x80/x.\xc3";
    let lite = Path::new(OsStr::from_bytes(bytes));
    let std = std::path::Path::new(std::ffi::OsStr::from_bytes(bytes));
    assert_eq!(format!("{lite:?}"), format!("{std:?}"));
    assert_eq!(format!("{}", lite.display()), format!("{}", std.display()));
    assert_eq!(
        format!("[{:>30.8}]", lite.display()),
        format!("[{:>30.8}]", std.display())
    );
    assert_eq!(lite.to_string_lossy(), std.to_string_lossy());
    assert_eq!(lite.to_str(), None);
    assert_eq!(
        lite.extension().map(OsStrExt::as_bytes),
        std.extension().map(std::ffi::OsStr::as_bytes)
    );
}

/// std 1.100 and later take `\\?\C:` for a verbatim disk only when a
/// backslash or nothing follows the colon.
#[cfg(windows)]
#[test]
fn verbatim_disk_needs_a_backslash() {
    use litestd::{
        ffi::OsStr,
        path::{Component, Path, Prefix},
    };

    fn prefix(s: &str) -> Option<Prefix<'_>> {
        match Path::new(s).components().next() {
            Some(Component::Prefix(p)) => Some(p.kind()),
            _ => None,
        }
    }

    assert_eq!(prefix(r"\\?\C:"), Some(Prefix::VerbatimDisk(b'C')));
    assert_eq!(prefix(r"\\?\c:\x"), Some(Prefix::VerbatimDisk(b'C')));
    assert_eq!(
        prefix(r"\\?\C:/x"),
        Some(Prefix::Verbatim(OsStr::new("C:/x")))
    );
    assert_eq!(
        prefix(r"\\?\C:x"),
        Some(Prefix::Verbatim(OsStr::new("C:x")))
    );
    let components: Vec<_> =
        Path::new(r"\\?\C:/foo\bar").components().collect();
    assert_eq!(
        components,
        [
            Component::Prefix(
                match Path::new(r"\\?\C:/foo").components().next() {
                    Some(Component::Prefix(p)) => p,
                    _ => panic!(),
                }
            ),
            Component::RootDir,
            Component::Normal(OsStr::new("bar")),
        ]
    );
}

#[test]
fn hash_is_consistent_with_eq() {
    use litestd::path::Path;

    let equal = [
        ["a/b", "a//b", "a/./b", "a/b/", "a/b/.", "./a/b/./"],
        ["/", "//", "/.", "/./", "///./", "/"],
        ["", "", "", "", "", ""],
        [".", "./", ".//", "./.", "./", "."],
    ];
    for group in equal {
        let first = Path::new(group[0]);
        for other in group {
            let other = Path::new(other);
            if first == other {
                assert_eq!(hash_record(first), hash_record(other), "{other:?}");
            }
        }
    }
    assert_eq!(
        hash_record(Path::new("a/b")),
        hash_record(Path::new("a//b/"))
    );
    assert_ne!(
        hash_record(Path::new("a/bc")),
        hash_record(Path::new("ab/c"))
    );
}

/// std hashes `C:.` and `C:` differently although they are equal; litestd
/// keeps `Hash` consistent with `Eq`.
#[cfg(windows)]
#[test]
fn drive_relative_dot_hashes_like_the_drive() {
    use litestd::path::Path;

    for (a, b) in [
        ("C:.", "C:"),
        ("C:./x", "C:x"),
        (r"c:.\x", "C:x"),
        ("C:.//x/.", "c:x"),
        ("C:..", "C:.."),
    ] {
        assert_eq!(Path::new(a), Path::new(b));
        assert_eq!(hash_record(Path::new(a)), hash_record(Path::new(b)));
    }
    assert_ne!(Path::new("C:.."), Path::new("C:"));
}
