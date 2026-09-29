//! The `os::unix::ffi` and `os::windows::ffi` extension traits against std.

#![cfg(feature = "path")]

#[cfg(unix)]
mod unix {
    use std::{ffi, os::unix::ffi::OsStrExt as _};

    use litestd::{
        ffi::{OsStr, OsString},
        os::unix::ffi::{OsStrExt, OsStringExt},
    };

    /// Runs the same code against both crates.
    macro_rules! api_log {
        ($krate:ident) => {{
            use $krate::{
                ffi::{OsStr, OsString},
                os::unix::ffi::{OsStrExt, OsStringExt},
            };

            let bytes: &[u8] = b"f\xffo\x80o";
            let os_str = OsStr::from_bytes(bytes);
            let os_string = OsString::from_vec(bytes.to_vec());
            vec![
                format!("{os_str:?}"),
                format!("{:?}", os_str.as_bytes()),
                format!("{os_string:?}"),
                format!("{:?}", os_string.clone().into_vec()),
                format!("{:?}", os_str.to_string_lossy()),
                format!("{}", os_str.display()),
                format!("{:?}", OsStr::from_bytes(b"").as_bytes()),
            ]
        }};
    }

    #[test]
    fn api_matches_std() {
        assert_eq!(api_log!(std), api_log!(litestd));
    }

    #[test]
    fn any_bytes_round_trip() {
        let all: Vec<u8> = (0..=255).collect();
        let os_str = OsStr::from_bytes(&all);
        assert_eq!(os_str.as_bytes(), &all[..]);
        assert_eq!(os_str.as_encoded_bytes(), &all[..]);
        let os_string = OsString::from_vec(all.clone());
        assert_eq!(os_string.as_os_str(), os_str);
        assert_eq!(os_string.into_vec(), all);
        assert_eq!(
            format!("{:?}", OsStr::from_bytes(&all)),
            format!("{:?}", ffi::OsStr::from_bytes(&all))
        );
    }

    #[test]
    fn from_bytes_borrows() {
        let bytes = b"borrowed";
        assert_eq!(
            OsStr::from_bytes(bytes).as_bytes().as_ptr(),
            bytes.as_ptr()
        );
        let vec = b"moved".to_vec();
        let ptr = vec.as_ptr();
        let back = OsString::from_vec(vec).into_vec();
        assert_eq!(back.as_ptr(), ptr);
    }
}

#[cfg(windows)]
mod windows {
    use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};

    use litestd::{
        ffi::{OsStr, OsString},
        os::windows::ffi::{EncodeWide, OsStrExt, OsStringExt},
    };

    /// Runs the same code against both crates.
    macro_rules! api_log {
        ($krate:ident) => {{
            use $krate::{
                ffi::{OsStr, OsString},
                os::windows::ffi::{OsStrExt, OsStringExt},
            };

            let wide: &[u16] =
                &[0x66, 0xd800, 0x6f, 0xd83d, 0xde00, 0xdc00, 0xe9, 0x0a];
            let os_string = OsString::from_wide(wide);
            let os_str: &OsStr = &os_string;
            let mut encode = os_str.encode_wide();
            let first = encode.next();
            let fused = OsStr::new("").encode_wide().next();
            vec![
                format!("{os_str:?}"),
                format!("{:?}", os_str.to_string_lossy()),
                format!("{}", os_str.display()),
                format!("{:?}", os_str.to_str()),
                format!("{:?}", os_str.encode_wide().collect::<Vec<u16>>()),
                format!("{:?}", os_str.encode_wide().size_hint()),
                format!("{first:?} {:?}", encode.size_hint()),
                format!("{encode:?}"),
                format!("{:#?}", OsStr::new("a\n").encode_wide()),
                format!("{fused:?}"),
                format!("{:?}", os_string.clone().into_string()),
                format!("{:?}", os_str.as_encoded_bytes()),
            ]
        }};
    }

    #[test]
    fn api_matches_std() {
        assert_eq!(api_log!(std), api_log!(litestd));
    }

    /// Every unpaired, paired and reversed surrogate arrangement of up to
    /// four units drawn from a small alphabet.
    #[test]
    fn wide_strings_round_trip_like_std() {
        const ALPHABET: [u16; 7] =
            [0x61, 0xe9, 0xd800, 0xdbff, 0xdc00, 0xdfff, 0xfffd];
        let mut units = Vec::new();
        for len in 0..=4u32 {
            for mut n in 0..ALPHABET.len().pow(len) {
                units.clear();
                for _ in 0..len {
                    units.push(ALPHABET[n % ALPHABET.len()]);
                    n /= ALPHABET.len();
                }
                let s = std::ffi::OsString::from_wide(&units);
                let t = OsString::from_wide(&units);
                assert_eq!(
                    s.as_encoded_bytes(),
                    t.as_encoded_bytes(),
                    "{units:x?}"
                );
                let back: Vec<u16> = t.encode_wide().collect();
                assert_eq!(back, units);
                assert_eq!(format!("{s:?}"), format!("{t:?}"));
                assert_eq!(s.to_string_lossy(), t.to_string_lossy());
                assert_eq!(
                    s.clone().into_string().ok(),
                    t.clone().into_string().ok()
                );
                let (sl, sh) = s.encode_wide().size_hint();
                assert_eq!((sl, sh), t.encode_wide().size_hint());
                assert!(
                    sl <= units.len() && sh.is_some_and(|h| h >= units.len())
                );
            }
        }
    }

    /// Pushing joins a leading surrogate at the end with a trailing one at
    /// the start, as concatenating the UTF-16 does.
    #[test]
    fn push_pairs_surrogates() {
        let mut s = OsString::from_wide(&[0x61, 0xd83d]);
        s.push(OsString::from_wide(&[0xde00, 0x62]));
        assert_eq!(s, "a\u{1f600}b");
        assert_eq!(s.clone().into_string().as_deref(), Ok("a\u{1f600}b"));
        let mut s = OsString::from_wide(&[0xd83d]);
        s.push(OsString::from_wide(&[0xd83d]));
        assert_eq!(s.encode_wide().collect::<Vec<_>>(), [0xd83d, 0xd83d]);
        s.push(OsString::from_wide(&[0xde00]));
        assert_eq!(
            s.encode_wide().collect::<Vec<_>>(),
            [0xd83d, 0xd83d, 0xde00]
        );
        assert_eq!(s.to_string_lossy(), "\u{fffd}\u{1f600}");
        // A trailing surrogate first, then a leading one, stay unpaired.
        let mut s = OsString::from_wide(&[0xde00]);
        s.push(OsString::from_wide(&[0xd83d]));
        assert_eq!(s.encode_wide().collect::<Vec<_>>(), [0xde00, 0xd83d]);
        assert!(s.into_string().is_err());
    }

    /// A string that becomes UTF-8 through pairing still converts.
    #[test]
    fn paired_string_converts_to_string() {
        let mut s = OsString::new();
        s.push(OsString::from_wide(&[0xd83d]));
        assert!(s.clone().into_string().is_err());
        s.push(OsString::from_wide(&[0xde00]));
        assert_eq!(s.into_string().as_deref(), Ok("\u{1f600}"));
        let mut s = OsString::from("x");
        s.push(OsString::from_wide(&[0xdc00]));
        assert!(s.clone().into_string().is_err());
        s.clear();
        s.push("y");
        assert_eq!(s.into_string().as_deref(), Ok("y"));
    }

    #[test]
    fn encode_wide_is_fused_and_clonable() {
        let s = OsString::from_wide(&[0x61, 0xd83d, 0xde00]);
        let mut it: EncodeWide<'_> = s.encode_wide();
        let copy = it.clone();
        assert_eq!(it.by_ref().collect::<Vec<_>>(), [0x61, 0xd83d, 0xde00]);
        assert_eq!(it.next(), None);
        assert_eq!(it.next(), None);
        assert_eq!(copy.count(), 3);
    }

    #[test]
    fn ascii_case_keeps_surrogates() {
        let mut s = OsString::from_wide(&[0x41, 0xd800, 0x62]);
        s.make_ascii_lowercase();
        assert_eq!(s.encode_wide().collect::<Vec<_>>(), [0x61, 0xd800, 0x62]);
        assert_eq!(
            s.to_ascii_uppercase().encode_wide().collect::<Vec<_>>(),
            [0x41, 0xd800, 0x42]
        );
        assert!(
            s.eq_ignore_ascii_case(OsString::from_wide(&[0x41, 0xd800, 0x42]))
        );
        assert!(
            !s.eq_ignore_ascii_case(OsString::from_wide(&[0x41, 0xd801, 0x42]))
        );
    }

    #[test]
    fn display_pads_only_without_surrogates() {
        let plain = OsStr::new("ab");
        let std_plain = std::ffi::OsStr::new("ab");
        assert_eq!(
            format!("[{:>5}]", plain.display()),
            format!("[{:>5}]", std_plain.display())
        );
        let s = OsString::from_wide(&[0x61, 0xd800]);
        let std_s = std::ffi::OsString::from_wide(&[0x61, 0xd800]);
        assert_eq!(
            format!("[{:>5}]", s.display()),
            format!("[{:>5}]", std_s.display())
        );
    }
}
