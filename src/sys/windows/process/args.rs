//! The command line that `CreateProcessW` passes to the child: arguments
//! quoted so that the C runtime's parser recovers them, or, for a batch
//! file, escaped so that `cmd.exe` neither splits nor expands them.

use core::iter;

use alloc_crate::vec::Vec;

use crate::{
    ffi::OsStr, io, os::windows::ffi::OsStrExt, process::args::ArgsIter,
};

pub(super) const QUOTE: u16 = b'"' as u16;
const BACKSLASH: u16 = b'\\' as u16;
const SPACE: u16 = b' ' as u16;
const PERCENT: u16 = b'%' as u16;

/// std's error for an argument, variable or directory containing a NUL.
pub(super) const NUL_ERROR: io::Error = io::const_error!(
    io::ErrorKind::InvalidInput,
    "nul byte found in provided data",
);

/// Fails with std's error if `s` contains a NUL, which WTF-8 encodes as a
/// zero byte and nothing else as one.
pub(super) fn ensure_no_nuls(s: &OsStr) -> io::Result<()> {
    if s.as_encoded_bytes().contains(&0) {
        Err(NUL_ERROR)
    } else {
        Ok(())
    }
}

/// Appends `s` in UTF-16.
pub(super) fn push_wide(out: &mut Vec<u16>, s: &OsStr) {
    // WTF-8 never has fewer bytes than UTF-16 has units.
    out.reserve(s.len());
    out.extend(s.encode_wide());
}

/// Pairs each of `args` with whether `raw_arg` added it, which `raw` tells
/// by the indices of such arguments among the program and its arguments.
pub(super) fn tag<'a>(
    args: ArgsIter<'a>,
    raw: &'a [usize],
) -> impl Iterator<Item = (&'a OsStr, bool)> {
    let mut raw = raw.iter().peekable();
    args.enumerate()
        .map(move |(i, arg)| (arg, raw.next_if_eq(&&(i + 1)).is_some()))
}

/// The units needed for `args` if none needs escaping, plus `extra`.
fn capacity(args: &ArgsIter<'_>, extra: usize) -> usize {
    args.clone()
        .fold(extra, |n, arg| n.saturating_add(arg.len() + 3))
}

/// Returns the NUL-terminated command line that runs `program` with `args`,
/// which the C runtime of the child splits back into `program` and `args`.
/// `program` must not contain a NUL.
pub(super) fn command_line(
    program: &OsStr,
    args: ArgsIter<'_>,
    raw: &[usize],
) -> io::Result<Vec<u16>> {
    debug_assert!(!program.as_encoded_bytes().contains(&0));
    let mut cmd = Vec::with_capacity(capacity(&args, program.len() + 3));
    // The program is always quoted, but not escaped: file names cannot
    // contain quotes, and the parser takes backslashes literally in it.
    cmd.push(QUOTE);
    push_wide(&mut cmd, program);
    cmd.push(QUOTE);
    for (arg, raw) in tag(args, raw) {
        cmd.push(SPACE);
        append_arg(&mut cmd, arg, raw)?;
    }
    cmd.push(0);
    Ok(cmd)
}

/// Appends `arg` for the C runtime's parser. A regular argument is quoted
/// if it is empty or contains a space or tab; backslashes are doubled
/// before a quote, which is escaped, and before a closing quote. A raw one
/// is appended as it is.
fn append_arg(cmd: &mut Vec<u16>, s: &OsStr, raw: bool) -> io::Result<()> {
    ensure_no_nuls(s)?;
    if raw {
        push_wide(cmd, s);
        return Ok(());
    }
    let bytes = s.as_encoded_bytes();
    let quote =
        bytes.is_empty() || bytes.iter().any(|&b| matches!(b, b' ' | b'\t'));
    if quote {
        cmd.push(QUOTE);
    }
    // Backslashes go out as they come; `backslashes` counts the run so far.
    let mut backslashes = 0;
    for unit in s.encode_wide() {
        if unit == BACKSLASH {
            backslashes += 1;
        } else {
            if unit == QUOTE {
                // 2n + 1 backslashes and the quote stand for n and a quote.
                cmd.extend(iter::repeat_n(BACKSLASH, backslashes + 1));
            }
            backslashes = 0;
        }
        cmd.push(unit);
    }
    if quote {
        // 2n backslashes before the closing quote stand for n.
        cmd.extend(iter::repeat_n(BACKSLASH, backslashes));
        cmd.push(QUOTE);
    }
    Ok(())
}

/// Returns the NUL-terminated command line that makes `cmd.exe` run the
/// batch file `script` (NUL-terminated) with `args`, escaped against
/// injection as std does since CVE-2024-24576: regular arguments with a
/// line break are refused, since `cmd.exe` would end the command there.
pub(super) fn batch_command_line(
    script: &[u16],
    args: ArgsIter<'_>,
    raw: &[usize],
) -> io::Result<Vec<u16>> {
    // `/e:ON` enables the substring syntax that the escape of `%` uses, and
    // `/v:OFF` disables delayed expansion. The whole command is quoted, as
    // `/c` requires; the first quote opens it, the second the script name.
    const PREFIX: &[u8] = b"cmd.exe /e:ON /v:OFF /d /c \"\"";
    let script = script.strip_suffix(&[0]).unwrap_or(script);
    if script.contains(&QUOTE) || script.last() == Some(&BACKSLASH) {
        return Err(io::const_error!(
            io::ErrorKind::InvalidInput,
            "Windows file names may not contain `\"` or end with `\\`",
        ));
    }
    let extra = PREFIX.len() + script.len() + 2;
    let mut cmd = Vec::with_capacity(capacity(&args, extra));
    cmd.extend(PREFIX.iter().map(|&b| u16::from(b)));
    cmd.extend_from_slice(script);
    cmd.push(QUOTE);
    for (arg, raw) in tag(args, raw) {
        cmd.push(SPACE);
        if raw {
            // Raw arguments are the caller's responsibility.
            append_arg(&mut cmd, arg, true)?;
            continue;
        }
        let bytes = arg.as_encoded_bytes();
        if bytes.iter().any(|&b| matches!(b, b'\r' | b'\n')) {
            return Err(io::const_error!(
                io::ErrorKind::InvalidInput,
                "batch file arguments are invalid",
            ));
        }
        append_batch_arg(&mut cmd, arg)?;
    }
    cmd.push(QUOTE);
    cmd.push(0);
    Ok(cmd)
}

/// Appends `arg` for `cmd.exe`. It is quoted if it is empty, ends in a
/// backslash (which would escape the closing quote of `"%~1"`), or contains
/// a control character or ASCII punctuation other than `#$*+-./:?@\_`.
/// Backslashes are doubled before a quote, and a quote is doubled. Each `%`
/// is preceded by `%%cd:~,`, an empty substring of `%cd%` that keeps
/// `cmd.exe` from expanding a variable name that follows.
fn append_batch_arg(cmd: &mut Vec<u16>, arg: &OsStr) -> io::Result<()> {
    const UNQUOTED: &[u8] = br"#$*+-./:?@\_";
    const PERCENT_ESCAPE: [u16; 7] = [
        PERCENT,
        PERCENT,
        b'c' as u16,
        b'd' as u16,
        b':' as u16,
        b'~' as u16,
        b',' as u16,
    ];
    ensure_no_nuls(arg)?;
    let bytes = arg.as_encoded_bytes();
    let mut quote = matches!(bytes.last(), None | Some(b'\\'));
    let mut prev = 0;
    for &b in bytes {
        let special = b.is_ascii()
            && !(b.is_ascii_alphanumeric() || UNQUOTED.contains(&b));
        // The C1 controls U+0080 to U+009F are `C2 80` to `C2 9F` in WTF-8,
        // where `C2` is always a leading byte.
        let c1_control = prev == 0xC2 && b <= 0x9F;
        quote |= special || c1_control;
        prev = b;
    }
    if quote {
        cmd.push(QUOTE);
    }
    let mut backslashes = 0;
    for unit in arg.encode_wide() {
        if unit == BACKSLASH {
            backslashes += 1;
        } else {
            if unit == QUOTE {
                // 2n backslashes, and a quote that escapes this one.
                cmd.extend(iter::repeat_n(BACKSLASH, backslashes));
                cmd.push(QUOTE);
            } else if unit == PERCENT {
                cmd.extend_from_slice(&PERCENT_ESCAPE);
            }
            backslashes = 0;
        }
        cmd.push(unit);
    }
    if quote {
        cmd.extend(iter::repeat_n(BACKSLASH, backslashes));
        cmd.push(QUOTE);
    }
    Ok(())
}
