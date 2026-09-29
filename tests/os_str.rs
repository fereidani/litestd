//! `litestd::ffi::{OsStr, OsString}` against std.
//!
//! The same code runs against both crates, so any difference in signatures
//! or trait implementations fails to compile, and every result is compared.
//! A generated corpus of strings, invalid ones included, then compares
//! conversions, formatting, concatenation, ordering and hashing.

// wasm32-unknown-unknown has no byte-string extensions to build strings
// that are not UTF-8 with; its `OsStr` is Unix's code, which runs here
// elsewhere.
#![cfg(all(feature = "path", not(target_os = "unknown")))]
#![allow(
    clippy::incompatible_msrv,
    reason = "the tests compare against the std of the toolchain that runs them"
)]
// The helper modules are private to this test, as is everything else.
#![allow(unreachable_pub)]

extern crate alloc;

use alloc::{borrow::Cow, rc::Rc, sync::Arc};
use core::{
    borrow::Borrow,
    fmt::{self, Debug, Write as _},
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

/// Exercises the whole API of one crate's `OsStr` and `OsString` on
/// Unicode data, returning a log of every result.
macro_rules! api_log {
    ($krate:ident) => {{
        use $krate::ffi::{OsStr, OsString};

        let mut l = Vec::new();

        // Construction and capacity.
        let mut s = OsString::new();
        rec(&mut l, (&s, s.capacity(), s.is_empty(), s.len()));
        s.push("abc");
        s.push(String::from("def"));
        s.push(OsStr::new("ghi"));
        s.push(OsString::from("jkl"));
        s.push(&OsString::from("mno"));
        rec(&mut l, (&s, s.len()));
        let mut c = OsString::with_capacity(10);
        rec(&mut l, c.capacity() >= 10);
        c.push("0123456789");
        c.reserve(100);
        rec(&mut l, c.capacity() >= 110);
        c.shrink_to(20);
        rec(&mut l, c.capacity());
        c.shrink_to_fit();
        rec(&mut l, c.capacity());
        c.reserve_exact(5);
        rec(&mut l, c.capacity() >= 15);
        rec(&mut l, c.try_reserve(7).is_ok());
        rec(&mut l, c.try_reserve_exact(3).is_ok());
        rec(&mut l, c.try_reserve(usize::MAX).is_err());
        rec(&mut l, c.try_reserve_exact(usize::MAX).is_err());
        c.clear();
        rec(&mut l, (&c, c.is_empty()));

        // Conversions to and from Rust strings and bytes.
        let from_string = OsString::from(String::from("h\u{e9}llo"));
        rec(&mut l, from_string.to_str());
        rec(&mut l, from_string.clone().into_string());
        rec(&mut l, from_string.as_os_str().to_string_lossy());
        rec(&mut l, from_string.clone().into_encoded_bytes());
        let bytes = from_string.as_encoded_bytes().to_vec();
        // SAFETY: the bytes come from `as_encoded_bytes`.
        let back = unsafe { OsString::from_encoded_bytes_unchecked(bytes) };
        rec(&mut l, &back);
        let slice =
            // SAFETY: the bytes come from `as_encoded_bytes`, split next to
            // ASCII.
            unsafe { OsStr::from_encoded_bytes_unchecked(&back.as_encoded_bytes()[1..]) };
        rec(&mut l, slice);
        rec(&mut l, <&str>::try_from(OsStr::new("ok")));
        rec(&mut l, "x".parse::<OsString>());
        rec(&mut l, OsStr::new("abc").to_os_string());

        // ASCII operations.
        let mixed = OsStr::new("Gr\u{dc}\u{df}e, J\u{fc}rgen \u{2764} ABC xyz");
        rec(&mut l, mixed.to_ascii_lowercase());
        rec(&mut l, mixed.to_ascii_uppercase());
        rec(&mut l, (mixed.is_ascii(), OsStr::new("abc").is_ascii()));
        rec(&mut l, mixed.eq_ignore_ascii_case("gR\u{fc}\u{df}E, j\u{dc}RGEN \u{2764} abc XYZ"));
        rec(&mut l, mixed.eq_ignore_ascii_case("Gr\u{dc}\u{df}e, J\u{fc}rgen \u{2764} ABC xy"));
        let mut owned = mixed.to_os_string();
        owned.make_ascii_lowercase();
        rec(&mut l, &owned);
        owned.make_ascii_uppercase();
        rec(&mut l, &owned);

        // Formatting.
        let text = OsStr::new("tab\there \"quoted\" 'single' \\ \u{7f} \u{301}\u{e9}\u{200b}");
        rec(&mut l, text);
        rec(&mut l, OsString::from("x\n"));
        rec(&mut l, text.display());
        rec(&mut l, format!("{}", text.display()));
        rec(&mut l, format!("[{:>8}]", OsStr::new("ab").display()));
        rec(&mut l, format!("[{:*^9.3}]", OsStr::new("abcdef").display()));
        rec(&mut l, format!("[{:<6.0}]", OsStr::new("abc").display()));

        // Boxes, shared pointers and `Cow`.
        let boxed: Box<OsStr> = OsString::from("boxed").into_boxed_os_str();
        rec(&mut l, &boxed);
        rec(&mut l, boxed.clone().into_os_string());
        rec(&mut l, Box::<OsStr>::default());
        rec(&mut l, Box::<OsStr>::from(OsStr::new("from ref")));
        let mut for_mut = OsString::from("from mut");
        rec(&mut l, Box::<OsStr>::from(for_mut.as_mut_os_str_compat()));
        rec(&mut l, Box::<OsStr>::from(Cow::Borrowed(OsStr::new("cow"))));
        rec(&mut l, Box::<OsStr>::from(Cow::<OsStr>::Owned(OsString::from("cow2"))));
        rec(&mut l, OsString::from(Box::<OsStr>::from(OsStr::new("unbox"))));
        rec(&mut l, Box::<OsStr>::from(OsString::from("box")));
        rec(&mut l, Arc::<OsStr>::from(OsString::from("arc")));
        rec(&mut l, Arc::<OsStr>::from(OsStr::new("arc ref")));
        rec(&mut l, Arc::<OsStr>::from(for_mut.as_mut_os_str_compat()));
        rec(&mut l, Rc::<OsStr>::from(OsString::from("rc")));
        rec(&mut l, Rc::<OsStr>::from(OsStr::new("rc ref")));
        rec(&mut l, Rc::<OsStr>::from(for_mut.as_mut_os_str_compat()));
        let cow: Cow<'_, OsStr> = Cow::from(OsString::from("owned"));
        rec(&mut l, &cow);
        let cow: Cow<'_, OsStr> = Cow::from(OsStr::new("borrowed"));
        rec(&mut l, matches!(cow, Cow::Borrowed(_)));
        let base = OsString::from("base");
        let cow: Cow<'_, OsStr> = Cow::from(&base);
        rec(&mut l, matches!(cow, Cow::Borrowed(_)));
        rec(&mut l, OsString::from(cow));
        let leaked: &'static mut OsStr = OsString::from("leak").leak();
        leaked.make_ascii_uppercase();
        rec(&mut l, &*leaked);
        // SAFETY: `leak` returned the whole allocation of a string copied
        // from a literal, which has no spare capacity, so it is the
        // allocation of a `Box<OsStr>` of the same length.
        drop(unsafe { Box::from_raw(ptr::from_mut(leaked)) });

        // Defaults, clones, borrows and `ToOwned`.
        rec(&mut l, OsString::default());
        rec(&mut l, <&OsStr>::default());
        let mut target = OsString::from("a much longer string than the source");
        target.clone_from(&OsString::from("src"));
        rec(&mut l, &target);
        OsStr::new("into").clone_into(&mut target);
        rec(&mut l, &target);
        let borrowed: &OsStr = base.borrow();
        rec(&mut l, borrowed.to_owned());
        rec(&mut l, (&base[..], &*base));
        let mut index_mut = OsString::from("idx");
        index_mut[..].make_ascii_uppercase();
        (*index_mut).make_ascii_lowercase();
        rec(&mut l, &index_mut);
        let string = String::from("string");
        let as_refs: [&OsStr; 4] = [
            OsStr::new("x").as_ref(),
            base.as_ref(),
            "str".as_ref(),
            string.as_ref(),
        ];
        rec(&mut l, as_refs);

        // `fmt::Write`, `Extend` and `FromIterator`.
        let mut written = OsString::new();
        write!(written, "{}-{}", 1, "two").unwrap();
        rec(&mut l, &written);
        written.extend([OsString::from("a"), OsString::from("b")]);
        written.extend([OsStr::new("c"), OsStr::new("d")]);
        written.extend([Cow::Borrowed(OsStr::new("e")), Cow::Owned(OsString::from("f"))]);
        rec(&mut l, &written);
        rec(&mut l, [OsString::from("x"), OsString::from("y")].into_iter().collect::<OsString>());
        rec(&mut l, Vec::<OsString>::new().into_iter().collect::<OsString>());
        rec(&mut l, [OsStr::new("x"), OsStr::new("y")].into_iter().collect::<OsString>());
        rec(&mut l, [Cow::Borrowed(OsStr::new("x")), Cow::Owned(OsString::from("y"))].into_iter().collect::<OsString>());
        rec(&mut l, [Cow::Owned(OsString::from("x")), Cow::Borrowed(OsStr::new("y"))].into_iter().collect::<OsString>());
        rec(&mut l, Vec::<Cow<'_, OsStr>>::new().into_iter().collect::<OsString>());

        // Comparisons across types.
        let a = OsString::from("apple");
        let b = OsStr::new("banana");
        let cow_a: Cow<'_, OsStr> = Cow::Borrowed(OsStr::new("apple"));
        rec(&mut l, (a == *"apple", *"apple" == a, a == "apple", "apple" == a));
        rec(&mut l, (a == *b, *b == a, a == b, b == a));
        rec(&mut l, (cow_a == *b, *b == cow_a, cow_a == b, b == cow_a, cow_a == a, a == cow_a));
        rec(&mut l, (*b == *"banana", *"banana" == *b));
        let b_string = OsString::from("b");
        rec(&mut l, (a.partial_cmp(&b_string), a.partial_cmp("apple")));
        rec(&mut l, (a.partial_cmp(b), b.partial_cmp(&a), a.partial_cmp(&b), (&b).partial_cmp(&a)));
        rec(&mut l, (cow_a.partial_cmp(b), b.partial_cmp(&cow_a), cow_a.partial_cmp(&b)));
        rec(&mut l, ((&b).partial_cmp(&cow_a), cow_a.partial_cmp(&a), a.partial_cmp(&cow_a)));
        let app = OsString::from("app");
        rec(&mut l, (b.partial_cmp("banana"), b.cmp(OsStr::new("band")), a.cmp(&app)));
        rec(&mut l, (a < b_string, b > OsStr::new("a")));
        rec(&mut l, hash_record(&a));
        rec(&mut l, hash_record(b));
        l
    }};
}

/// `OsStr` has no `as_mut_os_str`; this reborrows the same way on both
/// crates.
trait AsMutOsStrCompat {
    type Target: ?Sized;
    fn as_mut_os_str_compat(&mut self) -> &mut Self::Target;
}

impl AsMutOsStrCompat for std::ffi::OsString {
    type Target = std::ffi::OsStr;
    fn as_mut_os_str_compat(&mut self) -> &mut std::ffi::OsStr {
        self
    }
}

impl AsMutOsStrCompat for litestd::ffi::OsString {
    type Target = litestd::ffi::OsStr;
    fn as_mut_os_str_compat(&mut self) -> &mut litestd::ffi::OsStr {
        self
    }
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

/// The units a platform string is made of: bytes off Windows, and UTF-16
/// code units on Windows.
#[cfg(not(windows))]
type Unit = u8;
#[cfg(windows)]
type Unit = u16;

/// Creates both crates' strings from raw units, and reads the units back.
#[cfg(not(windows))]
mod raw {
    use super::{Lite, Std};
    use crate::{
        lite_ffi::{OsStrExt as _, OsStringExt as _},
        std_ffi::{OsStrExt as _, OsStringExt as _},
    };

    pub fn std(units: &[u8]) -> Std {
        Std::from_vec(units.to_vec())
    }

    pub fn lite(units: &[u8]) -> Lite {
        let owned = Lite::from_vec(units.to_vec());
        assert_eq!(litestd::ffi::OsStr::from_bytes(units), &owned);
        assert_eq!(owned.as_bytes(), units);
        owned
    }

    pub fn std_units(s: &std::ffi::OsStr) -> Vec<u8> {
        s.as_bytes().to_vec()
    }

    pub fn lite_units(s: &litestd::ffi::OsStr) -> Vec<u8> {
        s.as_bytes().to_vec()
    }

    /// Pieces that strings are generated from, including bytes that are
    /// not UTF-8.
    pub const ATOMS: &[&[u8]] = &[
        b"a",
        b"Z",
        b"hello",
        b" ",
        b"\t",
        b"\n",
        b"\0",
        b"\"",
        b"'",
        b"\\",
        b"\x7f",
        b"/",
        b".",
        b"~",
        "\u{e9}".as_bytes(),
        "\u{65e5}\u{672c}".as_bytes(),
        "\u{2764}".as_bytes(),
        "\u{301}".as_bytes(),
        "\u{200b}".as_bytes(),
        "\u{feff}".as_bytes(),
        "\u{ff9e}".as_bytes(),
        "\u{e000}".as_bytes(),
        "\u{10ffff}".as_bytes(),
        "\u{1f600}".as_bytes(),
        b"\x80",
        b"\xbf",
        b"\xc3",
        b"\xc0\xaf",
        b"\xe2\x82",
        b"\xed\xa0\x80",
        b"\xed\xbf\xbf",
        b"\xf0\x9f\x98",
        b"\xf4\x90\x80\x80",
        b"\xf8\x88\x80\x80\x80",
        b"\xfe",
        b"\xff",
    ];
}

/// Creates both crates' strings from raw units, and reads the units back.
#[cfg(windows)]
mod raw {
    use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};

    use litestd::os::windows::ffi::{OsStrExt as _, OsStringExt as _};

    use super::{Lite, Std};

    pub fn std(units: &[u16]) -> Std {
        Std::from_wide(units)
    }

    pub fn lite(units: &[u16]) -> Lite {
        Lite::from_wide(units)
    }

    pub fn std_units(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().collect()
    }

    pub fn lite_units(s: &litestd::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().collect()
    }

    /// Pieces that strings are generated from, including unpaired
    /// surrogates and pairs split across pieces.
    pub const ATOMS: &[&[u16]] = &[
        &[0x61],
        &[0x5a],
        &[0x68, 0x65, 0x6c, 0x6c, 0x6f],
        &[0x20],
        &[0x09],
        &[0x0a],
        &[0x00],
        &[0x22],
        &[0x27],
        &[0x5c],
        &[0x7f],
        &[0x2f],
        &[0x2e],
        &[0xe9],
        &[0x65e5, 0x672c],
        &[0x2764],
        &[0x301],
        &[0x200b],
        &[0xfeff],
        &[0xff9e],
        &[0xe000],
        &[0xdbff, 0xdfff],
        &[0xd83d, 0xde00],
        &[0xd800],
        &[0xdbff],
        &[0xdc00],
        &[0xdfff],
        &[0xd83d],
        &[0xde00],
        &[0xde00, 0xd83d],
        &[0xd800, 0xd800],
        &[0xdc00, 0xdc00],
        &[0xfffd],
        &[0xffff],
    ];
}

type Std = std::ffi::OsString;
type Lite = litestd::ffi::OsString;

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

/// Every atom alone and, outside Miri, in pairs, then random
/// concatenations.
fn corpus(random: usize) -> Vec<Vec<Unit>> {
    let atoms = raw::ATOMS;
    let mut corpus: Vec<Vec<Unit>> = vec![Vec::new()];
    for a in atoms {
        corpus.push(a.to_vec());
        if cfg!(miri) {
            continue;
        }
        for b in atoms {
            corpus.push([*a, *b].concat());
        }
    }
    let mut rng = Rng(0x05_5712);
    for _ in 0..random {
        let len = rng.below(8);
        let s: Vec<Unit> = (0..len)
            .flat_map(|_| atoms[rng.below(atoms.len())].to_vec())
            .collect();
        corpus.push(s);
    }
    corpus
}

const fn corpus_size() -> usize {
    if cfg!(miri) { 10 } else { 3000 }
}

/// Formats `value` with the specifications the tests compare.
fn formats(value: &dyn fmt::Display) -> Vec<String> {
    vec![
        format!("{value}"),
        format!("[{value:>12}]"),
        format!("[{value:<7}]"),
        format!("[{value:^9}]"),
        format!("[{value:*^11.4}]"),
        format!("[{value:.2}]"),
        format!("[{value:.0}]"),
        format!("[{value:#>5.1}]"),
    ]
}

/// Compares every unary operation on one string.
fn compare_one(units: &[Unit]) {
    let s = raw::std(units);
    let t = raw::lite(units);
    assert_eq!(s.as_encoded_bytes(), t.as_encoded_bytes(), "{s:?}");
    assert_eq!(raw::std_units(&s), raw::lite_units(&t), "{s:?}");
    assert_eq!(s.to_str(), t.to_str(), "{s:?}");
    assert_eq!(s.to_string_lossy(), t.to_string_lossy(), "{s:?}");
    assert_eq!(
        matches!(s.to_string_lossy(), Cow::Borrowed(_)),
        matches!(t.to_string_lossy(), Cow::Borrowed(_)),
    );
    assert_eq!(format!("{s:?}"), format!("{t:?}"));
    assert_eq!(format!("{:?}", s.display()), format!("{:?}", t.display()));
    assert_eq!(formats(&s.display()), formats(&t.display()), "{s:?}");
    assert_eq!(
        (s.len(), s.is_empty(), s.is_ascii()),
        (t.len(), t.is_empty(), t.is_ascii())
    );
    assert_eq!(
        s.to_ascii_uppercase().as_encoded_bytes(),
        t.to_ascii_uppercase().as_encoded_bytes()
    );
    assert_eq!(
        s.to_ascii_lowercase().as_encoded_bytes(),
        t.to_ascii_lowercase().as_encoded_bytes()
    );
    let upper = t.to_ascii_uppercase();
    assert!(t.eq_ignore_ascii_case(&upper));
    assert_eq!(hash_record(&*s), hash_record(&*t));
    assert_eq!(hash_record(&s), hash_record(&t));
    assert_eq!(
        s.into_string().map_err(Std::into_encoded_bytes),
        t.clone().into_string().map_err(Lite::into_encoded_bytes),
    );
    let boxed = t.clone().into_boxed_os_str();
    assert_eq!(&*boxed, &*t);
    assert_eq!(boxed.into_os_string(), t);
    assert_eq!(&*Rc::<litestd::ffi::OsStr>::from(t.as_os_str()), &*t);
    assert_eq!(&*Arc::<litestd::ffi::OsStr>::from(t.clone()), &*t);
}

#[test]
fn unary_operations_match_std() {
    for units in corpus(corpus_size()) {
        compare_one(&units);
    }
}

#[test]
fn concatenation_and_comparison_match_std() {
    let corpus = corpus(corpus_size());
    let mut rng = Rng(0x00c0_ffee);
    let rounds = if cfg!(miri) { 40 } else { 40_000 };
    for _ in 0..rounds {
        let a = &corpus[rng.below(corpus.len())];
        let b = &corpus[rng.below(corpus.len())];
        let (mut sa, sb) = (raw::std(a), raw::std(b));
        let (mut ta, tb) = (raw::lite(a), raw::lite(b));
        assert_eq!(sa.cmp(&sb), ta.cmp(&tb));
        assert_eq!(sa == sb, ta == tb);
        assert_eq!(sa.eq_ignore_ascii_case(&sb), ta.eq_ignore_ascii_case(&tb));
        // Pushing pairs surrogates across the seam on Windows, exactly as
        // concatenating the UTF-16 does.
        sa.push(&sb);
        ta.push(&tb);
        assert_eq!(sa.as_encoded_bytes(), ta.as_encoded_bytes(), "{sa:?}");
        let joined: Vec<Unit> = [a.as_slice(), b.as_slice()].concat();
        assert_eq!(raw::lite(&joined), ta);
        assert_eq!(ta.to_str(), sa.to_str());
        assert_eq!(
            sa.into_string()
                .map_err(std::ffi::OsString::into_encoded_bytes),
            ta.into_string()
                .map_err(litestd::ffi::OsString::into_encoded_bytes),
        );
    }
}

#[test]
fn collecting_matches_std() {
    let corpus = corpus(corpus_size());
    let mut rng = Rng(0xfeed);
    let rounds = if cfg!(miri) { 20 } else { 3000 };
    for _ in 0..rounds {
        let picks: Vec<&Vec<Unit>> = (0..rng.below(5))
            .map(|_| &corpus[rng.below(corpus.len())])
            .collect();
        let s: std::ffi::OsString = picks.iter().map(|u| raw::std(u)).collect();
        let t: litestd::ffi::OsString =
            picks.iter().map(|u| raw::lite(u)).collect();
        assert_eq!(s.as_encoded_bytes(), t.as_encoded_bytes());
        let lite_parts: Vec<litestd::ffi::OsString> =
            picks.iter().map(|u| raw::lite(u)).collect();
        let by_ref: litestd::ffi::OsString = lite_parts
            .iter()
            .map(litestd::ffi::OsString::as_os_str)
            .collect();
        assert_eq!(by_ref, t);
        let by_cow: litestd::ffi::OsString = lite_parts
            .iter()
            .map(|p| Cow::Borrowed(p.as_os_str()))
            .collect();
        assert_eq!(by_cow, t);
        let mut extended = litestd::ffi::OsString::new();
        extended
            .extend(lite_parts.iter().map(litestd::ffi::OsString::as_os_str));
        assert_eq!(extended, t);
    }
}

#[test]
fn equal_strings_hash_equally() {
    let corpus = corpus(if cfg!(miri) { 20 } else { 500 });
    for units in &corpus {
        let a = raw::lite(units);
        let b = raw::lite(&raw::lite_units(&a));
        assert_eq!(a, b);
        assert_eq!(hash_record(&a), hash_record(&b));
        assert_eq!(hash_record(&a), hash_record(a.as_encoded_bytes()));
    }
}

#[test]
fn from_encoded_bytes_round_trips() {
    for units in corpus(if cfg!(miri) { 20 } else { 500 }) {
        let t = raw::lite(&units);
        let bytes = t.clone().into_encoded_bytes();
        // SAFETY: the bytes come from `into_encoded_bytes`.
        let back = unsafe {
            litestd::ffi::OsString::from_encoded_bytes_unchecked(bytes)
        };
        assert_eq!(back, t);
        // SAFETY: the bytes come from `as_encoded_bytes`.
        let slice = unsafe {
            litestd::ffi::OsStr::from_encoded_bytes_unchecked(
                t.as_encoded_bytes(),
            )
        };
        assert_eq!(slice, &*t);
    }
}

#[test]
fn clone_into_and_clone_from_replace_the_contents() {
    for units in corpus(if cfg!(miri) { 10 } else { 200 }) {
        let t = raw::lite(&units);
        let mut target = litestd::ffi::OsString::from("previous contents");
        t.as_os_str().clone_into(&mut target);
        assert_eq!(target, t);
        let mut other = litestd::ffi::OsString::from("x");
        other.clone_from(&t);
        assert_eq!(other, t);
        assert_eq!(target.into_string().is_ok(), t.to_str().is_some());
    }
}

#[test]
fn auto_traits_match_std() {
    use core::panic::{RefUnwindSafe, UnwindSafe};

    fn owned<T: Send + Sync + Unpin + UnwindSafe + RefUnwindSafe>() {}
    fn unsized_<
        T: ?Sized + Send + Sync + Unpin + UnwindSafe + RefUnwindSafe,
    >() {
    }

    owned::<litestd::ffi::OsString>();
    unsized_::<litestd::ffi::OsStr>();
    owned::<litestd::ffi::os_str::Display<'static>>();
    owned::<&litestd::ffi::OsStr>();
}

#[test]
fn string_formatting_has_no_width_for_debug() {
    // `Debug` ignores width, like `str`'s.
    let t = litestd::ffi::OsString::from("ab");
    let s = std::ffi::OsString::from("ab");
    assert_eq!(format!("{t:>10?}"), format!("{s:>10?}"));
    assert_eq!(format!("{t:#?}"), format!("{s:#?}"));
}

#[test]
fn new_is_const() {
    const EMPTY: litestd::ffi::OsString = litestd::ffi::OsString::new();
    assert!(EMPTY.is_empty());
}

#[test]
fn display_writes_through_formatter() {
    // `Display` works in `write!`, and `OsString` implements `fmt::Write`.
    let mut out = String::new();
    write!(out, "{}", litestd::ffi::OsStr::new("x").display()).unwrap();
    let mut os = litestd::ffi::OsString::new();
    write!(os, "{out}{}", 1).unwrap();
    assert_eq!(os, "x1");
}
