//! `litestd::path` against std over a generated corpus of tricky paths.
//!
//! Every path is built from raw platform units (bytes on Unix, UTF-16 code
//! units on Windows) with both crates, then every operation runs on both and
//! the results are compared: encoded bytes, `Debug` and `Display` output,
//! hash inputs and orderings. The corpus holds every short combination of
//! the core atoms, random longer ones, and the inputs of std's own tests.

#![cfg(feature = "path")]
#![allow(
    clippy::incompatible_msrv,
    reason = "the tests compare against the std of the toolchain that runs them"
)]
#![allow(
    clippy::unnecessary_debug_formatting,
    reason = "failures show paths escaped, bytes that are not UTF-8 included"
)]
// The helper modules are private to this test, as is everything else.
#![allow(unreachable_pub)]

extern crate alloc;

use alloc::fmt;
use core::{
    cmp::Ordering,
    hash::{Hash, Hasher},
};
use std::path::{Path as StdPath, PathBuf as StdPathBuf};

use litestd::path::{Path as LitePath, PathBuf as LitePathBuf};

#[cfg(not(windows))]
type Unit = u8;
#[cfg(windows)]
type Unit = u16;

#[cfg(any(unix, target_os = "wasi"))]
mod raw {
    #[cfg(unix)]
    use std::os::unix::ffi::OsStrExt as _;
    #[cfg(target_os = "wasi")]
    use std::os::wasi::ffi::OsStrExt as _;

    #[cfg(unix)]
    use litestd::os::unix::ffi::OsStrExt as _;
    #[cfg(target_os = "wasi")]
    use litestd::os::wasi::ffi::OsStrExt as _;

    pub fn std(units: &[u8]) -> std::path::PathBuf {
        std::path::PathBuf::from(std::ffi::OsStr::from_bytes(units))
    }

    pub fn lite(units: &[u8]) -> litestd::path::PathBuf {
        litestd::path::PathBuf::from(litestd::ffi::OsStr::from_bytes(units))
    }

    /// Units that only make sense on this platform: bytes that are not
    /// UTF-8.
    pub fn extra_atoms() -> Vec<Vec<u8>> {
        [
            &b"\xff"[..],
            b"\x80",
            b"\xc3",
            b"\xed\xa0\x80",
            b"\xf0\x9f\x98",
            b"a\xc0\xaf",
        ]
        .iter()
        .map(|a| a.to_vec())
        .collect()
    }

    pub fn core_atoms() -> Vec<Vec<u8>> {
        ["/", "//", ".", "..", "a", "b.c", ".d", "..e", "\u{e9}", "~"]
            .iter()
            .map(|a| a.as_bytes().to_vec())
            .chain([b"\xff".to_vec()])
            .collect()
    }

    pub fn units(s: &str) -> Vec<u8> {
        s.as_bytes().to_vec()
    }
}

/// wasm32-unknown-unknown has no byte-string extensions, so its corpus is
/// UTF-8.
#[cfg(target_os = "unknown")]
mod raw {
    #[allow(clippy::unwrap_used, reason = "every atom here is UTF-8")]
    fn text(units: &[u8]) -> &str {
        core::str::from_utf8(units).unwrap()
    }

    pub fn std(units: &[u8]) -> std::path::PathBuf {
        std::path::PathBuf::from(text(units))
    }

    pub fn lite(units: &[u8]) -> litestd::path::PathBuf {
        litestd::path::PathBuf::from(text(units))
    }

    pub const fn extra_atoms() -> Vec<Vec<u8>> {
        Vec::new()
    }

    pub fn core_atoms() -> Vec<Vec<u8>> {
        ["/", "//", ".", "..", "a", "b.c", ".d", "..e", "\u{e9}", "~"]
            .iter()
            .map(|a| a.as_bytes().to_vec())
            .collect()
    }

    pub fn units(s: &str) -> Vec<u8> {
        s.as_bytes().to_vec()
    }
}

#[cfg(windows)]
mod raw {
    use std::os::windows::ffi::OsStringExt as _;

    use litestd::os::windows::ffi::OsStringExt as _;

    pub fn std(units: &[u16]) -> std::path::PathBuf {
        std::path::PathBuf::from(std::ffi::OsString::from_wide(units))
    }

    pub fn lite(units: &[u16]) -> litestd::path::PathBuf {
        litestd::path::PathBuf::from(litestd::ffi::OsString::from_wide(units))
    }

    /// Units that only make sense on this platform: prefixes, and
    /// surrogates that are unpaired or split across atoms.
    pub fn extra_atoms() -> Vec<Vec<u16>> {
        let mut atoms: Vec<Vec<u16>> = [
            r"\\?\",
            r"\\?\UNC\",
            r"\\?\UNC/",
            r"\\?\C:",
            r"\\?\c:\",
            r"\\?\C:/",
            r"\\.\",
            r"\\./",
            "//./",
            r"\\",
            "//",
            "C:",
            "c:",
            "Z:",
            "1:",
            r"\\server\share",
            "//server/share",
            r"\\?/",
            "//?/",
            r"\\?\UNC\srv\shr",
            r"\/",
            r"/\",
            "UNC",
            "unc",
            "COM1",
        ]
        .iter()
        .map(|a| units(a))
        .collect();
        atoms.extend([
            vec![0xd800],
            vec![0xdc00],
            vec![0xd83d, 0xde00],
            vec![0xde00, 0xd83d],
        ]);
        atoms
    }

    pub fn core_atoms() -> Vec<Vec<u16>> {
        [
            "\\", "/", ".", "..", "a", "b.c", "..e", "C:", r"\\?\", r"\\.\",
            r"\\", "UNC",
        ]
        .iter()
        .map(|a| units(a))
        .chain([vec![0xd800]])
        .collect()
    }

    pub fn units(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }
}

/// Atoms common to both platforms.
fn common_atoms() -> Vec<Vec<Unit>> {
    [
        "/",
        "//",
        "\\",
        ".",
        "..",
        "...",
        "a",
        "b",
        "foo",
        "bar",
        ".a",
        "..b",
        "a.",
        "a.b",
        "a.b.c",
        ".a.b",
        "a..b",
        "tar.gz",
        "x.tar.gz",
        "\u{e9}",
        "\u{e9}.\u{fc}",
        "\u{65e5}\u{672c}.txt",
        " ",
        "a b",
        "~",
        ":",
        "?",
        "*",
        "\"",
        "'",
        "\n",
        "\t",
        "\u{301}",
        "\u{feff}",
        "\u{ff9e}",
    ]
    .iter()
    .map(|a| raw::units(a))
    .collect()
}

/// Inputs from std's own path tests.
const STD_SEEDS: &[&str] = &[
    "",
    ".",
    "..",
    "../",
    "./",
    "./.",
    "/",
    "/..",
    "\\",
    r"\\.\",
    r"\\?\",
    "./a",
    r"\a",
    r"\\?\a\b\",
    "a/./b",
    "a//b",
    "a/b",
    "a/b/c",
    r"a\b\c",
    "a/.foo",
    "a/.rustfmt.toml",
    "a/.x.y.z",
    r"\\?\bar",
    r"\\?\bar\foo.txt",
    "c:",
    "c:/",
    r"c:\",
    r"\\?\C:",
    r"\\?\C:\",
    r"\\?\C:\.foo",
    "//././../././../?/C:/foo/bar",
    "//./?/C:/foo/bar",
    r"\\?\C:/foo/bar",
    r"c:\foo.txt",
    "//?/C:/foo.txt",
    r"//?/C:\foo.txt",
    r"\\?\C:\foo.txt",
    r"\\?\C:\.foo.txt.zip",
    r"\\?\C:\foo.txt.zip",
    ".foo",
    "///foo///",
    "/foo",
    "/foo/",
    r"\\.\foo",
    "foo",
    "foo.",
    "foo/",
    "foo/.",
    "foo/..",
    "foo/../",
    "foo/./",
    r"//./foo\bar",
    r"//.\foo/bar",
    "///foo///bar",
    "/foo/bar",
    r"\\./foo/bar",
    r"\\.\foo/bar",
    r"\\.\foo\bar",
    r"\\?\foo/bar",
    "foo.bar.",
    "foo/../bar",
    "foo/./bar",
    "foo/bar",
    r"\\.\foo\bar/baz",
    "foo.bar.txt",
    "foo.txt",
    r"\\server",
    r"\\server\share",
    "//server/share/foo.txt",
    r"//server/share\foo.txt",
    r"\\server\share\foo.txt",
    r"\\?\UNC\",
    r"\\?\UNC\server",
    "//?/UNC/server/share/foo.txt",
    r"//?/UNC/server\share/foo.txt",
    r"\\?\UNC\server/share\foo.txt",
    r"\\?\UNC\server\share\foo.txt",
    "x",
    "..x.y.z",
    ".x.y.z",
    r"C:\a\b\c",
    "C:d",
    r"C:a\b\c",
    r"\\?\C:a\b",
    r"\\?\A:\x\y",
    r"\\?\UNC\server\share",
    r"\\?\UNC\\share",
    "C:.",
    "C:./",
    r"C:.\x",
    "c:..",
    r"\\?\C:\.",
    r"\\?\a\.\b",
];

/// A small deterministic generator, so that failures reproduce.
struct Rng(u64);

impl Rng {
    const fn next(&mut self) -> u64 {
        // splitmix64
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    #[allow(
        clippy::cast_possible_truncation,
        reason = "the remainder is below `n`, a `usize`"
    )]
    const fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// The corpus: std's inputs, every combination of up to `depth` core
/// atoms, and `random` concatenations of up to eight atoms of all kinds.
fn corpus(depth: u32, random: usize) -> Vec<Vec<Unit>> {
    let mut corpus: Vec<Vec<Unit>> =
        STD_SEEDS.iter().map(|s| raw::units(s)).collect();
    let core = raw::core_atoms();
    for len in 1..=depth {
        for mut n in 0..core.len().pow(len) {
            let mut path = Vec::new();
            for _ in 0..len {
                path.extend_from_slice(&core[n % core.len()]);
                n /= core.len();
            }
            corpus.push(path);
        }
    }
    let mut atoms = common_atoms();
    atoms.extend(raw::extra_atoms());
    let mut rng = Rng(0x9a7b);
    for _ in 0..random {
        let mut path = Vec::new();
        for _ in 0..=rng.below(8) {
            // Separators come up often, as in real paths.
            let atom = if rng.below(3) == 0 {
                rng.below(3)
            } else {
                rng.below(atoms.len())
            };
            path.extend_from_slice(&atoms[atom]);
        }
        corpus.push(path);
    }
    corpus
}

fn full_corpus() -> Vec<Vec<Unit>> {
    if cfg!(miri) {
        corpus(1, 10).into_iter().step_by(5).collect()
    } else {
        corpus(3, 6000)
    }
}

/// Whether std parses `\\?\C:/` as a verbatim disk, as it did before
/// Rust 1.100. litestd follows the newer rule, a plain verbatim prefix.
fn std_has_old_verbatim_disk() -> bool {
    cfg!(windows)
        && matches!(
            StdPath::new(r"\\?\C:/x").components().next(),
            Some(std::path::Component::Prefix(p))
                if matches!(p.kind(), std::path::Prefix::VerbatimDisk(_))
        )
}

/// Whether the old and new std parse a path with these encoded bytes
/// differently: `\\?\C:/` starts it.
const fn differs_across_std_versions(bytes: &[u8]) -> bool {
    matches!(bytes, [b'\\', b'\\', b'?', b'\\', drive, b':', b'/', ..] if drive.is_ascii_alphabetic())
}

/// Whether litestd hashes a path with these encoded bytes differently from
/// std: a drive followed by a `.` component, as in `C:.` or `C:./x`, which
/// std hashes although `components` drops it. See `Hash for Path`.
fn std_hash_ignores_eq(bytes: &[u8]) -> bool {
    cfg!(windows)
        && matches!(bytes, [drive, b':', b'.', rest @ ..]
            if drive.is_ascii_alphabetic()
                && rest.first().is_none_or(|&b| b == b'/' || b == b'\\'))
}

/// The paths of the corpus that both std versions agree on, with how many
/// were skipped.
fn comparable_corpus() -> (Vec<Vec<Unit>>, usize) {
    let corpus = full_corpus();
    if !std_has_old_verbatim_disk() {
        return (corpus, 0);
    }
    let total = corpus.len();
    let kept: Vec<_> = corpus
        .into_iter()
        .filter(|u| !differs_across_std_versions(bytes_lite(&raw::lite(u))))
        .collect();
    let skipped = total - kept.len();
    (kept, skipped)
}

fn bytes_std(p: &StdPath) -> &[u8] {
    p.as_os_str().as_encoded_bytes()
}

fn bytes_lite(p: &LitePath) -> &[u8] {
    p.as_os_str().as_encoded_bytes()
}

fn opt_std(s: Option<&std::ffi::OsStr>) -> Option<&[u8]> {
    s.map(std::ffi::OsStr::as_encoded_bytes)
}

fn opt_lite(s: Option<&litestd::ffi::OsStr>) -> Option<&[u8]> {
    s.map(litestd::ffi::OsStr::as_encoded_bytes)
}

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

/// A component as comparable data: its `Debug` form and its bytes.
fn std_component(c: std::path::Component<'_>) -> (String, Vec<u8>) {
    (format!("{c:?}"), c.as_os_str().as_encoded_bytes().to_vec())
}

fn lite_component(c: litestd::path::Component<'_>) -> (String, Vec<u8>) {
    (format!("{c:?}"), c.as_os_str().as_encoded_bytes().to_vec())
}

/// Formats `value` with the specifications the tests compare.
fn formats(value: &dyn fmt::Display) -> Vec<String> {
    vec![
        format!("{value}"),
        format!("[{value:>24}]"),
        format!("[{value:^9.3}]"),
        format!("[{value:.0}]"),
        format!("[{value:+<6.1}]"),
    ]
}

/// Iterates both ends in a pattern set by `steps`, comparing each item and
/// what `as_path` leaves.
fn compare_interleaved(s: &StdPath, t: &LitePath, steps: u32) {
    let mut sc = s.components();
    let mut tc = t.components();
    for i in 0..32 {
        let back = steps >> (i % 16) & 1 == 1;
        let (a, b) = if back {
            (
                sc.next_back().map(std_component),
                tc.next_back().map(lite_component),
            )
        } else {
            (sc.next().map(std_component), tc.next().map(lite_component))
        };
        assert_eq!(a, b, "{s:?} step {i}");
        assert_eq!(
            bytes_std(sc.as_path()),
            bytes_lite(tc.as_path()),
            "{s:?} step {i}"
        );
        // `Debug` parses what `as_path` leaves again, which can start with
        // a prefix that the std versions read differently.
        if !(std_has_old_verbatim_disk()
            && differs_across_std_versions(bytes_lite(tc.as_path())))
        {
            assert_eq!(format!("{sc:?}"), format!("{tc:?}"), "{s:?} step {i}");
        }
        if a.is_none() {
            break;
        }
    }
}

fn compare_components(s: &StdPath, t: &LitePath) {
    let forward: Vec<_> = s.components().map(std_component).collect();
    assert_eq!(
        forward,
        t.components().map(lite_component).collect::<Vec<_>>(),
        "{s:?}"
    );
    let backward: Vec<_> = s.components().rev().map(std_component).collect();
    assert_eq!(
        backward,
        t.components().rev().map(lite_component).collect::<Vec<_>>(),
        "{s:?}"
    );
    let iter: Vec<_> =
        s.iter().map(|c| c.as_encoded_bytes().to_vec()).collect();
    assert_eq!(
        iter,
        t.iter()
            .map(|c| c.as_encoded_bytes().to_vec())
            .collect::<Vec<_>>()
    );
    let iter_back: Vec<_> = s
        .iter()
        .rev()
        .map(|c| c.as_encoded_bytes().to_vec())
        .collect();
    assert_eq!(
        iter_back,
        t.iter()
            .rev()
            .map(|c| c.as_encoded_bytes().to_vec())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        format!("{:?}", s.components()),
        format!("{:?}", t.components())
    );
    assert_eq!(format!("{:?}", s.iter()), format!("{:?}", t.iter()));
    assert_eq!(
        bytes_std(s.components().as_path()),
        bytes_lite(t.components().as_path())
    );
    for steps in [0, u32::MAX, 0b1010_1010, 0b0110, 0b1001_1100_0111] {
        compare_interleaved(s, t, steps);
    }
}

fn compare_queries(s: &StdPath, t: &LitePath) {
    assert_eq!(
        s.parent().map(bytes_std),
        t.parent().map(bytes_lite),
        "{s:?}"
    );
    assert_eq!(opt_std(s.file_name()), opt_lite(t.file_name()), "{s:?}");
    assert_eq!(opt_std(s.file_stem()), opt_lite(t.file_stem()), "{s:?}");
    assert_eq!(opt_std(s.file_prefix()), opt_lite(t.file_prefix()), "{s:?}");
    assert_eq!(opt_std(s.extension()), opt_lite(t.extension()), "{s:?}");
    assert_eq!(
        (
            s.has_root(),
            s.is_absolute(),
            s.is_relative(),
            s.as_os_str().is_empty()
        ),
        (t.has_root(), t.is_absolute(), t.is_relative(), t.is_empty()),
        "{s:?}"
    );
    assert_eq!(s.to_str(), t.to_str());
    assert_eq!(s.to_string_lossy(), t.to_string_lossy());
    assert_eq!(format!("{s:?}"), format!("{t:?}"));
    assert_eq!(format!("{:?}", s.display()), format!("{:?}", t.display()));
    assert_eq!(formats(&s.display()), formats(&t.display()), "{s:?}");
    let ancestors: Vec<_> = s.ancestors().map(bytes_std).collect();
    assert_eq!(
        ancestors,
        t.ancestors().map(bytes_lite).collect::<Vec<_>>(),
        "{s:?}"
    );
    assert_eq!(
        format!("{:?}", s.ancestors()),
        format!("{:?}", t.ancestors())
    );
    if !std_hash_ignores_eq(bytes_lite(t)) {
        assert_eq!(hash_record(s), hash_record(t), "{s:?}");
        assert_eq!(
            hash_record(&s.to_path_buf()),
            hash_record(&t.to_path_buf())
        );
    }
    let components: Vec<_> = s.components().map(|c| hash_record(&c)).collect();
    assert_eq!(
        components,
        t.components().map(|c| hash_record(&c)).collect::<Vec<_>>()
    );
}

/// Extensions to set and add, some meant to catch pairing surrogates across
/// the dot on Windows.
fn extensions() -> Vec<Vec<Unit>> {
    let list = ["", "x", "tar.gz", ".", "..", ".x", "x.", "\u{e9}"]
        .iter()
        .map(|s| raw::units(s));
    // Units that are not Unicode, where the platform can make them.
    #[cfg(any(unix, target_os = "wasi"))]
    let list = list.chain([b"\xff".to_vec()]);
    #[cfg(windows)]
    let list = list.chain([vec![0xdc00], vec![0xd800], vec![0xde00, 0x61]]);
    list.collect()
}

/// File names to set, separators and prefixes included.
fn file_names() -> Vec<Vec<Unit>> {
    ["", "n", "n.e", ".", "..", "a/b", "/abs", r"\x", "C:y"]
        .iter()
        .map(|s| raw::units(s))
        .collect()
}

fn compare_modifications(units: &[Unit]) {
    let (s, t) = (raw::std(units), raw::lite(units));

    // `pop` down to the root.
    let (mut sp, mut tp) = (s.clone(), t.clone());
    for _ in 0..=units.len() {
        let (a, b) = (sp.pop(), tp.pop());
        assert_eq!(a, b, "{s:?}");
        assert_eq!(bytes_std(&sp), bytes_lite(&tp), "{s:?}");
        if !a {
            break;
        }
    }

    for name in file_names() {
        let (mut sn, mut tn) = (s.clone(), t.clone());
        let (sname, tname) = (raw::std(&name), raw::lite(&name));
        sn.set_file_name(&sname);
        tn.set_file_name(&tname);
        assert_eq!(bytes_std(&sn), bytes_lite(&tn), "{s:?} {sname:?}");
        let (sw, tw) = (s.with_file_name(&sname), t.with_file_name(&tname));
        assert_eq!(bytes_std(&sw), bytes_lite(&tw), "{s:?} {sname:?}");
    }

    for ext in extensions() {
        let (sext, text) = (raw::std(&ext), raw::lite(&ext));
        let (sext, text) = (sext.as_os_str(), text.as_os_str());
        let (mut se, mut te) = (s.clone(), t.clone());
        assert_eq!(
            se.set_extension(sext),
            te.set_extension(text),
            "{s:?} {sext:?}"
        );
        assert_eq!(bytes_std(&se), bytes_lite(&te), "{s:?} set {sext:?}");
        let (mut sa, mut ta) = (s.clone(), t.clone());
        assert_eq!(
            sa.add_extension(sext),
            ta.add_extension(text),
            "{s:?} {sext:?}"
        );
        assert_eq!(bytes_std(&sa), bytes_lite(&ta), "{s:?} add {sext:?}");
        let (sw, tw) = (s.with_extension(sext), t.with_extension(text));
        assert_eq!(bytes_std(&sw), bytes_lite(&tw), "{s:?} with {sext:?}");
        let (sw, tw) =
            (s.with_added_extension(sext), t.with_added_extension(text));
        assert_eq!(
            bytes_std(&sw),
            bytes_lite(&tw),
            "{s:?} with added {sext:?}"
        );
    }

    let (mut sm, mut tm) = (s.clone(), t.clone());
    sm.as_mut_os_str().make_ascii_uppercase();
    tm.as_mut_os_str().make_ascii_uppercase();
    assert_eq!(bytes_std(&sm), bytes_lite(&tm));
    assert_eq!(bytes_lite(&t.clone().into_boxed_path()), bytes_lite(&t));
    assert_eq!(s.into_string().ok(), t.into_string().ok());
}

fn compare_pair(a: &[Unit], b: &[Unit]) {
    let (sa, sb) = (raw::std(a), raw::std(b));
    let (ta, tb) = (raw::lite(a), raw::lite(b));
    assert_eq!(
        bytes_std(&sa.join(&sb)),
        bytes_lite(&ta.join(&tb)),
        "{sa:?} join {sb:?}"
    );
    let (mut sp, mut tp) = (sa.clone(), ta.clone());
    sp.push(&sb);
    tp.push(&tb);
    assert_eq!(bytes_std(&sp), bytes_lite(&tp), "{sa:?} push {sb:?}");
    assert_eq!(
        sa.strip_prefix(&sb).map(bytes_std).ok(),
        ta.strip_prefix(&tb).map(bytes_lite).ok(),
        "{sa:?} strip {sb:?}"
    );
    assert_eq!(
        sa.starts_with(&sb),
        ta.starts_with(&tb),
        "{sa:?} starts {sb:?}"
    );
    assert_eq!(sa.ends_with(&sb), ta.ends_with(&tb), "{sa:?} ends {sb:?}");
    assert_eq!(sa == sb, ta == tb, "{sa:?} == {sb:?}");
    assert_eq!(sa.cmp(&sb), ta.cmp(&tb), "{sa:?} cmp {sb:?}");
    assert_eq!(sa.partial_cmp(&sb), ta.partial_cmp(&tb));
    assert_eq!(
        sa.as_path() == sb.as_os_str(),
        ta.as_path() == tb.as_os_str()
    );
    if ta == tb {
        assert_eq!(
            hash_record(&ta),
            hash_record(&tb),
            "{ta:?} and {tb:?} hash"
        );
    }
}

/// Compares `Components` after moving each end of both some steps, which
/// exercises their comparisons outside the fast paths.
fn compare_partial_components(a: &[Unit], b: &[Unit], rng: &mut Rng) {
    let (sa, sb) = (raw::std(a), raw::std(b));
    let (ta, tb) = (raw::lite(a), raw::lite(b));
    let (mut sca, mut scb) = (sa.components(), sb.components());
    let (mut tca, mut tcb) = (ta.components(), tb.components());
    for _ in 0..rng.below(3) {
        sca.next();
        tca.next();
    }
    for _ in 0..rng.below(3) {
        scb.next_back();
        tcb.next_back();
    }
    // `Components` is also an iterator, whose `cmp` consumes it: name the
    // `Ord` and `PartialOrd` methods.
    assert_eq!(sca == scb, tca == tcb, "{sa:?} {sb:?}");
    assert_eq!(Ord::cmp(&sca, &scb), Ord::cmp(&tca, &tcb), "{sa:?} {sb:?}");
    assert_eq!(
        PartialOrd::partial_cmp(&sca, &scb),
        PartialOrd::partial_cmp(&tca, &tcb)
    );
}

#[test]
fn components_match_std() {
    let (corpus, _) = comparable_corpus();
    for units in &corpus {
        compare_components(&raw::std(units), &raw::lite(units));
    }
}

#[test]
fn queries_match_std() {
    let (corpus, _) = comparable_corpus();
    for units in &corpus {
        compare_queries(&raw::std(units), &raw::lite(units));
    }
}

#[test]
fn modifications_match_std() {
    let (corpus, _) = comparable_corpus();
    for units in &corpus {
        compare_modifications(units);
    }
}

#[test]
fn binary_operations_match_std() {
    let (corpus, _) = comparable_corpus();
    let mut rng = Rng(0x0b1a);
    let partners = if cfg!(miri) { 2 } else { 8 };
    for a in &corpus {
        for _ in 0..partners {
            let b = &corpus[rng.below(corpus.len())];
            compare_pair(a, b);
            compare_partial_components(a, b, &mut rng);
        }
        // Each path against itself and its neighbours in the corpus, which
        // share long prefixes.
        compare_pair(a, a);
    }
    for pair in corpus.windows(2) {
        compare_pair(&pair[0], &pair[1]);
    }
}

#[test]
fn sorting_matches_std() {
    let (corpus, _) = comparable_corpus();
    let mut by_std: Vec<usize> = (0..corpus.len()).collect();
    let mut by_lite = by_std.clone();
    let std_paths: Vec<StdPathBuf> =
        corpus.iter().map(|u| raw::std(u)).collect();
    let lite_paths: Vec<LitePathBuf> =
        corpus.iter().map(|u| raw::lite(u)).collect();
    by_std.sort_by(|&i, &j| std_paths[i].cmp(&std_paths[j]).then(i.cmp(&j)));
    by_lite.sort_by(|&i, &j| lite_paths[i].cmp(&lite_paths[j]).then(i.cmp(&j)));
    assert_eq!(by_std, by_lite);
    // Neighbours in sorted order are the most similar paths.
    for pair in by_lite.windows(2) {
        let (i, j) = (pair[0], pair[1]);
        let equal = lite_paths[i] == lite_paths[j];
        assert_eq!(equal, std_paths[i] == std_paths[j]);
        assert_eq!(equal, lite_paths[i].cmp(&lite_paths[j]) == Ordering::Equal);
        if equal {
            assert_eq!(
                hash_record(&lite_paths[i]),
                hash_record(&lite_paths[j])
            );
        }
    }
}

#[test]
fn corpus_is_large_and_skips_little() {
    let (corpus, skipped) = comparable_corpus();
    let size = corpus.len();
    println!("paths compared: {size}, skipped for older std: {skipped}");
    if !cfg!(miri) {
        assert!(size > 5000, "{size}");
    }
    assert!(skipped * 20 < size, "{skipped}");
}
