//! `copy`, `empty`, `sink`, `repeat`, `Chain`, `Take`, the prelude, and
//! the auto traits and `Debug` output of every I/O type, against std.

#![cfg(feature = "io")]

mod io_support;

use core::{cell::Cell, marker::PhantomData};

use io_support::{
    Event, Norm, Reader, Writer, chunks, noise, scripted, scripted_writer,
    scripts, sizes, text,
};

#[test]
fn copy_matches_std() {
    for &len in sizes() {
        let data = noise(len, 21);
        for &chunk in chunks() {
            for events in scripts(len) {
                let writer_scripts = [
                    vec![],
                    vec![(len / 2, Event::Fail)],
                    vec![(1, Event::Interrupt)],
                    vec![(len / 3, Event::Zero)],
                ];
                let kept = if cfg!(miri) { 2 } else { writer_scripts.len() };
                for writer_events in writer_scripts.into_iter().take(kept) {
                    differential!(io => {
                        let mut reader = scripted(&data, chunk, &events);
                        let mut writer = scripted_writer(chunk, &writer_events);
                        let result = io::copy(&mut reader, &mut writer).norm();
                        let position = reader.position();
                        (result, writer.data, writer.calls, reader.calls, position)
                    });
                }
            }
        }
    }
}

#[test]
fn copy_between_std_types_matches_std() {
    let data = text(if cfg!(miri) { 300 } else { 30_000 });
    for capacity in [1, 100, 8192, 20_000] {
        differential!(io => {
            use io::Write;
            let mut source = &data[..];
            let mut vec = Vec::new();
            let to_vec = io::copy(&mut source, &mut vec).norm();
            let mut reader = io::BufReader::with_capacity(capacity, Reader::new(data.clone()).chunk(777));
            let mut writer = io::BufWriter::with_capacity(capacity, Writer::new().chunk(999));
            let buffered = io::copy(&mut reader, &mut writer).norm();
            let flushed = writer.flush().norm();
            let dyn_reader: &mut dyn io::Read = &mut &data[..10];
            let dyn_writer: &mut dyn io::Write = &mut io::sink();
            let dynamic = io::copy(dyn_reader, dyn_writer).norm();
            let mut deque = litestd::collections::VecDeque::from(data.clone());
            let mut cursor = io::Cursor::new(Vec::new());
            let to_cursor = io::copy(&mut deque, &mut cursor).norm();
            (
                (to_vec, vec == data),
                (buffered, flushed, writer.get_ref().data == data),
                (dynamic, to_cursor, cursor.into_inner() == data),
            )
        });
    }
}

#[test]
fn empty_matches_std() {
    differential!(io => {
        use io::{BufRead, Read, Seek, Write};
        let mut e = io::empty();
        let mut buf = [7; 3];
        let reads = (
            e.read(&mut buf).norm(),
            e.read_exact(&mut []).norm(),
            e.read_exact(&mut buf).norm(),
            e.read_vectored(&mut [io::IoSliceMut::new(&mut buf)]).norm(),
            e.read_to_end(&mut vec![1]).norm(),
            e.read_to_string(&mut String::from("s")).norm(),
        );
        let buffered = (
            e.fill_buf().map(<[u8]>::to_vec).norm(),
            e.read_until(b'\n', &mut Vec::new()).norm(),
            e.skip_until(b'\n').norm(),
            e.read_line(&mut String::new()).norm(),
            e.lines().count(),
        );
        e.consume(5);
        let seeks = (
            e.seek(io::SeekFrom::End(-5)).norm(),
            e.seek(io::SeekFrom::Current(5)).norm(),
            e.stream_position().norm(),
            e.rewind().norm(),
            e.seek_relative(-1).norm(),
        );
        let writes = (
            e.write(b"abc").norm(),
            e.write_vectored(&[io::IoSlice::new(b"ab"), io::IoSlice::new(b"c")]).norm(),
            e.write_all(b"abc").norm(),
            write!(e, "{}", 5).norm(),
            e.flush().norm(),
            (&e).write(b"ab").norm(),
            write!(&e, "{}", 5).norm(),
            (&e).flush().norm(),
        );
        let copy = e;
        let default = io::Empty::default();
        (reads, buffered, seeks, writes, buf, format!("{copy:?} {default:?} {:?}", e.clone()))
    });
}

#[test]
fn sink_matches_std() {
    differential!(io => {
        use io::Write;
        let mut s = io::sink();
        let bufs = [io::IoSlice::new(b"ab"), io::IoSlice::new(b""), io::IoSlice::new(b"cde")];
        let writes = (
            s.write(b"abc").norm(),
            s.write_vectored(&bufs).norm(),
            s.write_all(b"abc").norm(),
            write!(s, "{}", 5).norm(),
            s.flush().norm(),
            (&s).write(b"ab").norm(),
            (&s).write_vectored(&bufs).norm(),
            (&s).write_all(b"").norm(),
            write!(&s, "x").norm(),
            (&s).flush().norm(),
        );
        let default = io::Sink::default();
        (writes, format!("{s:?} {default:?} {:?}", s.clone()))
    });
}

#[test]
fn repeat_matches_std() {
    differential!(io => {
        use io::Read;
        let mut r = io::repeat(b'z');
        let mut a = [0; 5];
        let mut b = [0; 3];
        let mut c = [0; 0];
        let reads = (
            r.read(&mut a).norm(),
            r.read_exact(&mut b).norm(),
            r.read_vectored(&mut [io::IoSliceMut::new(&mut c), io::IoSliceMut::new(&mut a)]).norm(),
            r.read_to_end(&mut Vec::new()).norm(),
            r.read_to_string(&mut String::new()).norm(),
        );
        let mut limited = Vec::new();
        let taken = io::repeat(1).take(10).read_to_end(&mut limited).norm();
        #[allow(clippy::unbuffered_bytes, reason = "the data is in memory")]
        let bytes: Vec<_> = io::repeat(2).bytes().take(3).map(Norm::norm).collect();
        (reads, a, b, taken, limited, bytes, format!("{r:?}"))
    });
}

/// An operation on a `Chain` or `Take`.
#[derive(Clone, Copy, Debug)]
enum Op {
    Read(usize),
    ReadVectored(usize, usize),
    ReadToEnd,
    ReadToString,
    FillConsume(usize),
    ReadUntil(u8),
    SkipUntil(u8),
    ReadLine,
}

fn ops(seed: u64, count: usize) -> Vec<Op> {
    noise(count * 2, seed)
        .chunks(2)
        .map(|pair| {
            let n = usize::from(pair[1] % 13);
            match pair[0] % 9 {
                0 | 1 => Op::Read(n),
                2 => Op::ReadVectored(n / 2, n),
                3 => Op::ReadToEnd,
                4 => Op::ReadToString,
                5 => Op::FillConsume(n),
                6 => Op::ReadUntil(b'\n'),
                7 => Op::SkipUntil(b'b'),
                _ => Op::ReadLine,
            }
        })
        .collect()
}

/// Whether the calls that `ops` make on an inner reader are the same in
/// every std version: `read_to_end` sizes its reads differently in older
/// ones.
fn comparable(ops: &[Op]) -> bool {
    !ops.iter()
        .any(|op| matches!(op, Op::ReadToEnd | Op::ReadToString))
}

/// Runs `ops` on `$reader` with both libraries and returns every outcome.
macro_rules! run_ops {
    ($io:ident, $reader:expr, $ops:expr) => {{
        use $io::{BufRead, Read};
        let reader = $reader;
        let mut outcomes = Vec::new();
        for &op in $ops {
            outcomes.push(match op {
                Op::Read(n) => {
                    let mut buf = vec![0; n];
                    format!("{:?} {buf:?}", reader.read(&mut buf).norm())
                }
                Op::ReadVectored(a, b) => {
                    let (mut x, mut y) = (vec![0; a], vec![0; b]);
                    let mut bufs = [
                        $io::IoSliceMut::new(&mut x),
                        $io::IoSliceMut::new(&mut y),
                    ];
                    format!(
                        "{:?} {x:?} {y:?}",
                        reader.read_vectored(&mut bufs).norm()
                    )
                }
                Op::ReadToEnd => {
                    let mut buf = Vec::new();
                    format!("{:?} {buf:?}", reader.read_to_end(&mut buf).norm())
                }
                Op::ReadToString => {
                    let mut buf = String::new();
                    format!(
                        "{:?} {buf:?}",
                        reader.read_to_string(&mut buf).norm()
                    )
                }
                Op::FillConsume(n) => {
                    let filled = reader.fill_buf().map(<[u8]>::to_vec).norm();
                    let n = filled.as_ref().map_or(0, |b| n.min(b.len()));
                    reader.consume(n);
                    format!("{filled:?}")
                }
                Op::ReadUntil(delim) => {
                    let mut buf = Vec::new();
                    format!(
                        "{:?} {buf:?}",
                        reader.read_until(delim, &mut buf).norm()
                    )
                }
                Op::SkipUntil(delim) => {
                    format!("{:?}", reader.skip_until(delim).norm())
                }
                Op::ReadLine => {
                    let mut line = String::new();
                    format!("{:?} {line:?}", reader.read_line(&mut line).norm())
                }
            });
        }
        outcomes
    }};
}

#[test]
fn chain_matches_std() {
    let rounds = if cfg!(miri) { 8 } else { 300 };
    for round in 0..rounds {
        let seed = round as u64;
        let first = text(usize::from(noise(1, seed)[0] % 40));
        let mut second = text(usize::from(noise(1, seed + 1)[0] % 40));
        second.reverse();
        let all_first = scripts(first.len());
        let all_second = scripts(second.len());
        let first_events = &all_first[round % all_first.len()];
        let second_events = &all_second[(round / 3) % all_second.len()];
        let chunk = chunks()[round % chunks().len()];
        let ops = ops(seed, 12);
        differential!(io => {
            use io::Read;
            let a = scripted(&first, chunk, first_events);
            let b = scripted(&second, chunk, second_events);
            let mut chain = a.chain(b);
            let outcomes = run_ops!(io, &mut chain, &ops);
            let debug = format!("{chain:?}");
            let (a, b) = chain.get_ref();
            let refs = (a.position(), b.position());
            let (a, _) = chain.get_mut();
            let calls = a.calls.clone();
            let (a, b) = chain.into_inner();
            // `read_to_end` sizes its reads differently in older std.
            let calls = comparable(&ops).then_some((debug, calls, a.calls, b.calls));
            (outcomes, refs, calls)
        });
    }
}

#[test]
fn chain_empty_reads_do_not_switch_like_std() {
    differential!(io => {
        use io::Read;
        let mut chain = (&b"ab"[..]).chain(&b"cd"[..]);
        let mut empty = [];
        let mut buf = [0; 4];
        let zero = chain.read(&mut empty).norm();
        let first = chain.read(&mut buf).norm();
        let zero_vectored = chain.read_vectored(&mut [io::IoSliceMut::new(&mut empty)]).norm();
        let second = chain.read(&mut buf).norm();
        (zero, first, zero_vectored, second, buf)
    });
}

#[test]
fn take_matches_std() {
    let rounds = if cfg!(miri) { 8 } else { 300 };
    for round in 0..rounds {
        let seed = round as u64 + 1000;
        let data = text(usize::from(noise(1, seed)[0] % 60));
        let all = scripts(data.len());
        let events = &all[round % all.len()];
        let chunk = chunks()[round % chunks().len()];
        let limit = [0, 1, 5, 17, 100, u64::MAX][round % 6];
        let ops = ops(seed, 10);
        differential!(io => {
            use io::Read;
            let mut take = scripted(&data, chunk, events).take(limit);
            let outcomes = run_ops!(io, &mut take, &ops);
            let remaining = take.limit();
            take.set_limit(3);
            let more = run_ops!(io, &mut take, &ops[..3]);
            let debug = format!("{take:?}");
            let position = take.get_ref().position();
            let calls = take.get_mut().calls.clone();
            let limit = take.limit();
            let inner = take.into_inner().calls;
            let calls = comparable(&ops).then_some((debug, calls, inner));
            (outcomes, remaining, more, limit, position, calls)
        });
    }
}

#[test]
fn take_seek_matches_std() {
    let targets = [
        (0, 0),
        (0, 3),
        (0, 10),
        (0, 11),
        (1, 0),
        (1, -4),
        (1, 1),
        (1, -11),
        (2, 2),
        (2, -2),
        (2, 20),
        (2, i64::MIN),
        (2, i64::MAX),
    ];
    for fail in [false, true] {
        for &(whence, offset) in &targets {
            differential!(io => {
                use io::{Read, Seek};
                let mut inner = Reader::new(text(40));
                if fail {
                    inner = inner.fail_seek();
                }
                let mut take = inner.take(10);
                let mut buf = [0; 4];
                let read = take.read(&mut buf).norm();
                let pos = match whence {
                    0 => io::SeekFrom::Start(offset.unsigned_abs()),
                    1 => io::SeekFrom::End(offset),
                    _ => io::SeekFrom::Current(offset),
                };
                let seek = take.seek(pos).norm();
                let position = take.stream_position().norm();
                let relative = take.seek_relative(-1).norm();
                let rewind = take.rewind().norm();
                let mut rest = Vec::new();
                let tail = take.read_to_end(&mut rest).norm();
                (read, seek, position, relative, rewind, tail, rest, take.limit(), take.into_inner().seeks)
            });
        }
    }
}

#[test]
fn take_seek_big_offsets_match_std() {
    differential!(io => {
        use io::{Read, Seek};
        let mut take = Reader::new(Vec::new()).take(u64::MAX);
        let far = take.seek(io::SeekFrom::Start(u64::MAX)).norm();
        let position = take.stream_position().norm();
        let near = take.seek(io::SeekFrom::Start(1)).norm();
        let end = take.seek(io::SeekFrom::End(0)).norm();
        let past = take.seek(io::SeekFrom::End(1)).norm();
        (far, position, near, end, past, take.limit(), take.into_inner().seeks)
    });
}

#[test]
fn bytes_lines_split_debug_match_std() {
    differential!(io => {
        use io::{BufRead, Read};
        let data = &b"a\nb"[..];
        (
            format!("{:?}", data.bytes()),
            format!("{:?}", BufRead::lines(data)),
            format!("{:?}", BufRead::split(data, b'\n')),
            format!("{:?}", data.chain(data)),
            format!("{:?}", data.take(2)),
            format!("{:?}", io::SeekFrom::End(-1)),
            format!("{:?}", io::Cursor::new(data)),
            format!("{:?}", io::BufReader::new(data)),
            format!("{:?}", io::IoSlice::new(data)),
        )
    });
}

#[test]
fn prelude_brings_the_traits() {
    use litestd::io::prelude::*;
    let mut reader = litestd::io::BufReader::new(&b"x\ny"[..]);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let mut rest = String::new();
    reader.read_to_string(&mut rest).unwrap();
    let mut out = litestd::io::Cursor::new(Vec::new());
    out.write_all(line.as_bytes()).unwrap();
    out.rewind().unwrap();
    assert_eq!(
        (line.as_str(), rest.as_str(), out.stream_position().unwrap()),
        ("x\n", "y", 0)
    );
}

#[test]
fn unsized_buffered_types_work() {
    use litestd::io::{BufRead, BufReader, BufWriter, LineWriter, Read, Write};
    let mut reader: Box<BufReader<dyn Read>> =
        Box::new(BufReader::new(&b"one\ntwo"[..]));
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert_eq!((line.as_str(), reader.buffer()), ("one\n", &b"two"[..]));
    let mut writer: Box<BufWriter<dyn Write>> =
        Box::new(BufWriter::new(litestd::io::sink()));
    writer.write_all(b"abc").unwrap();
    assert_eq!(writer.buffer(), b"abc");
    writer.flush().unwrap();
    let mut line_writer: Box<LineWriter<dyn Write>> =
        Box::new(LineWriter::new(Vec::new()));
    line_writer.write_all(b"a\nb").unwrap();
    assert_eq!(reader.fill_buf().unwrap(), b"two");
}

/// Whether `$t` implements `$bound`, decided where the type is concrete:
/// the inherent constant exists only if the bound holds, and takes priority
/// over the trait's fallback.
macro_rules! implements {
    ($t:ty: $bound:path) => {{
        struct Probe<T: ?Sized>(PhantomData<T>);
        #[allow(dead_code)]
        trait Fallback {
            const YES: bool = false;
        }
        impl<T: ?Sized> Fallback for Probe<T> {}
        #[allow(dead_code)]
        impl<T: ?Sized + $bound> Probe<T> {
            const YES: bool = true;
        }
        <Probe<$t>>::YES
    }};
}

/// The auto traits of `$t`: `Send`, `Sync`, `Unpin`, `UnwindSafe` and
/// `RefUnwindSafe`.
macro_rules! auto_traits {
    ($t:ty) => {
        [
            implements!($t: Send),
            implements!($t: Sync),
            implements!($t: Unpin),
            implements!($t: core::panic::UnwindSafe),
            implements!($t: core::panic::RefUnwindSafe),
        ]
    };
}

/// An inner reader or writer that is neither `Send` nor `Sync`.
struct NotSend(PhantomData<*const ()>);
/// One that is not `Sync` nor `RefUnwindSafe`.
struct Shared(#[allow(dead_code)] Cell<u8>);
/// One that is not `Unpin`.
struct Pinned(core::marker::PhantomPinned);

/// Makes the probe types writers for both libraries, as `BufWriter` and
/// `LineWriter` require.
macro_rules! writer {
    ($($t:ty),*) => {$(
        impl std::io::Write for $t {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                Ok(buf.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        impl litestd::io::Write for $t {
            fn write(&mut self, buf: &[u8]) -> litestd::io::Result<usize> {
                Ok(buf.len())
            }

            fn flush(&mut self) -> litestd::io::Result<()> {
                Ok(())
            }
        }
    )*};
}

writer!(NotSend, Shared, Pinned);

#[test]
fn auto_traits_match_std() {
    macro_rules! same {
        ($([$($t:tt)*])*) => {$(
            assert_eq!(
                auto_traits!(litestd::io::$($t)*),
                auto_traits!(std::io::$($t)*),
                stringify!($($t)*),
            );
        )*};
    }
    same! {
        [IoSlice<'static>]
        [IoSliceMut<'static>]
        [SeekFrom]
        [Error]
        [ErrorKind]
        [Empty]
        [Sink]
        [Repeat]
        [WriterPanicked]
        [Cursor<Vec<u8>>]
        [Cursor<NotSend>]
        [Cursor<Shared>]
        [Cursor<Pinned>]
        [BufReader<&'static [u8]>]
        [BufReader<NotSend>]
        [BufReader<Shared>]
        [BufReader<Pinned>]
        [BufWriter<Vec<u8>>]
        [BufWriter<NotSend>]
        [BufWriter<Shared>]
        [BufWriter<Pinned>]
        [LineWriter<Vec<u8>>]
        [LineWriter<NotSend>]
        [LineWriter<Shared>]
        [LineWriter<Pinned>]
        [IntoInnerError<Vec<u8>>]
        [IntoInnerError<NotSend>]
        [IntoInnerError<Shared>]
        [Chain<&'static [u8], NotSend>]
        [Chain<Shared, Pinned>]
        [Take<NotSend>]
        [Take<Shared>]
        [Take<Pinned>]
        [Bytes<NotSend>]
        [Bytes<Shared>]
        [Lines<Shared>]
        [Lines<Pinned>]
        [Split<NotSend>]
        [Split<Shared>]
    }
}

#[test]
fn traits_are_dyn_compatible() {
    use litestd::io::{BufRead, Read, Seek, Write};
    let mut cursor = litestd::io::Cursor::new(b"dyn".to_vec());
    let read: &mut dyn Read = &mut cursor;
    let mut buf = [0; 1];
    assert_eq!(read.read(&mut buf).unwrap(), 1);
    let bufread: &mut dyn BufRead = &mut cursor;
    assert_eq!(bufread.fill_buf().unwrap(), b"yn");
    let seek: &mut dyn Seek = &mut cursor;
    assert_eq!(seek.stream_position().unwrap(), 1);
    let write: &mut dyn Write = &mut cursor;
    write.write_all(b"!").unwrap();
    assert_eq!(cursor.into_inner(), b"d!n");
}
