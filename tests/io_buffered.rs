//! `BufReader`, `BufWriter`, `LineWriter` and `IntoInnerError` against
//! std: random operation sequences over scripted readers and writers, with
//! every result, the buffer contents and the calls reaching the inner
//! reader or writer compared after each operation.

#![cfg(feature = "io")]

mod io_support;

use io_support::{
    Norm, Reader, Writer, chunks, noise, scripted, scripted_writer, scripts,
    text,
};

/// A deterministic stream of small numbers for building operation
/// sequences.
struct Dice {
    bytes: Vec<u8>,
    next: usize,
}

impl Dice {
    fn new(seed: u64) -> Self {
        Self {
            bytes: noise(4096, seed),
            next: 0,
        }
    }

    fn byte(&mut self) -> u8 {
        let byte = self.bytes[self.next % self.bytes.len()];
        self.next += 1;
        byte
    }

    fn roll(&mut self, sides: usize) -> usize {
        usize::from(self.byte()) % sides
    }

    /// A number in `-20..20`.
    fn offset(&mut self) -> i64 {
        i64::from(self.byte() % 40) - 20
    }

    /// A length, usually small, sometimes around `capacity` or larger.
    fn len(&mut self, capacity: usize) -> usize {
        match self.roll(8) {
            0 => capacity,
            1 => capacity.saturating_sub(1),
            2 => capacity + 1,
            3 => capacity * 2 + 3,
            _ => self.roll(12),
        }
    }
}

/// An operation on a `BufReader`.
#[derive(Clone, Copy, Debug)]
enum ReadOp {
    Read(usize),
    ReadExact(usize),
    ReadVectored(usize, usize),
    FillConsume(usize),
    ReadLine,
    ReadUntil(u8),
    SkipUntil(u8),
    SeekRelative(i64),
    SeekCurrent(i64),
    SeekStart(u64),
    StreamPosition,
}

fn read_ops(
    dice: &mut Dice,
    capacity: usize,
    count: usize,
    seek: bool,
) -> Vec<ReadOp> {
    (0..count)
        .map(|_| match dice.roll(if seek { 11 } else { 7 }) {
            0 => ReadOp::Read(dice.len(capacity)),
            1 => ReadOp::ReadExact(dice.len(capacity)),
            2 => ReadOp::ReadVectored(dice.len(capacity), dice.len(capacity)),
            3 => ReadOp::FillConsume(dice.len(capacity)),
            4 => ReadOp::ReadLine,
            5 => ReadOp::ReadUntil(b'a'),
            6 => ReadOp::SkipUntil(b'\n'),
            7 => ReadOp::SeekRelative(dice.offset()),
            8 => ReadOp::SeekCurrent(dice.offset()),
            9 => ReadOp::SeekStart(u64::from(dice.byte() % 100)),
            _ => ReadOp::StreamPosition,
        })
        .collect()
}

/// Runs `ops` on a `BufReader` over `inner` with both libraries and
/// compares every step.
fn check_reader(inner: &Reader, capacity: usize, ops: &[ReadOp]) {
    differential!(io => {
        use io::{BufRead, Read, Seek};
        let mut reader = io::BufReader::with_capacity(capacity, inner.clone());
        let mut steps = Vec::new();
        for &op in ops {
            let outcome = match op {
                ReadOp::Read(n) => {
                    let mut buf = vec![0; n];
                    let r = reader.read(&mut buf).norm();
                    format!("{r:?} {:?}", &buf[..*r.as_ref().unwrap_or(&0)])
                }
                ReadOp::ReadExact(n) => {
                    let mut buf = vec![0; n];
                    let r = reader.read_exact(&mut buf).norm();
                    format!("{r:?} {buf:?}")
                }
                ReadOp::ReadVectored(first_len, second_len) => {
                    let (mut first, mut second) = (vec![0; first_len], vec![0; second_len]);
                    let mut bufs = [io::IoSliceMut::new(&mut first), io::IoSliceMut::new(&mut second)];
                    let r = reader.read_vectored(&mut bufs).norm();
                    format!("{r:?} {first:?} {second:?}")
                }
                ReadOp::FillConsume(n) => {
                    let r = reader.fill_buf().map(<[u8]>::to_vec).norm();
                    reader.consume(n);
                    format!("{r:?}")
                }
                ReadOp::ReadLine => {
                    let mut line = String::new();
                    let r = reader.read_line(&mut line).norm();
                    format!("{r:?} {line:?}")
                }
                ReadOp::ReadUntil(delim) => {
                    let mut buf = Vec::new();
                    let r = reader.read_until(delim, &mut buf).norm();
                    format!("{r:?} {buf:?}")
                }
                ReadOp::SkipUntil(delim) => format!("{:?}", reader.skip_until(delim).norm()),
                ReadOp::SeekRelative(n) => format!("{:?}", reader.seek_relative(n).norm()),
                ReadOp::SeekCurrent(n) => {
                    format!("{:?}", reader.seek(io::SeekFrom::Current(n)).norm())
                }
                ReadOp::SeekStart(n) => {
                    format!("{:?}", reader.seek(io::SeekFrom::Start(n)).norm())
                }
                ReadOp::StreamPosition => format!("{:?}", reader.stream_position().norm()),
            };
            steps.push((op_name(op), outcome, reader.buffer().to_vec()));
        }
        let inner = reader.into_inner();
        let position = inner.position();
        (steps, inner.calls, inner.seeks, position)
    });
}

fn op_name(op: ReadOp) -> String {
    format!("{op:?}")
}

#[test]
fn bufreader_operations_match_std() {
    let sequences = if cfg!(miri) { 4 } else { 400 };
    let mut dice = Dice::new(1);
    for round in 0..sequences {
        let capacity = [0, 1, 2, 5, 16, 64][dice.roll(6)];
        let len = [0, 3, 40, 150, 700][dice.roll(5)];
        let data = text(len);
        let chunk = chunks()[dice.roll(chunks().len())];
        let all = scripts(len);
        let events = &all[dice.roll(all.len())];
        let seek = round % 2 == 0;
        let ops = read_ops(&mut dice, capacity, 24, seek);
        check_reader(&scripted(&data, chunk, events), capacity, &ops);
    }
}

#[test]
fn bufreader_seek_failures_match_std() {
    for op in [
        ReadOp::SeekRelative(-3),
        ReadOp::SeekRelative(100),
        ReadOp::SeekCurrent(1),
        ReadOp::SeekCurrent(i64::MIN),
        ReadOp::SeekCurrent(-100),
        ReadOp::SeekStart(2),
        ReadOp::StreamPosition,
    ] {
        // Fill the buffer, then seek, with and without a failing inner seek.
        for fail in [false, true] {
            let mut inner = Reader::new(text(64)).chunk(10);
            if fail {
                inner = inner.fail_seek();
            }
            check_reader(
                &inner,
                16,
                &[ReadOp::Read(4), op, ReadOp::Read(4), op],
            );
        }
    }
}

#[test]
#[should_panic = "overflow when subtracting remaining buffer size from inner stream position"]
fn bufreader_stream_position_panics_when_out_of_sync() {
    use litestd::io::{BufRead, BufReader, Seek, SeekFrom};
    let mut reader = BufReader::with_capacity(8, Reader::new(text(64)));
    reader.fill_buf().unwrap();
    // Move the inner reader back behind the buffered data.
    reader.get_mut().seek(SeekFrom::Start(2)).unwrap();
    let _ = reader.stream_position();
}

#[test]
fn bufreader_read_to_end_and_string_match_std() {
    let mut split = text(20);
    split.extend_from_slice("\u{4e2d}".as_bytes());
    let inputs = if cfg!(miri) {
        vec![text(70), split]
    } else {
        vec![text(0), text(70), noise(70, 2), split]
    };
    let (capacities, heads): (&[usize], &[usize]) = if cfg!(miri) {
        (&[1, 64], &[21])
    } else {
        (&[0, 1, 7, 64], &[0, 1, 21])
    };
    for data in &inputs {
        for &capacity in capacities {
            for &chunk in chunks() {
                for events in scripts(data.len()) {
                    for prefix in ["", "pre\u{e9} "] {
                        for &head in heads {
                            differential!(io => {
                                use io::Read;
                                let inner = scripted(data, chunk, &events);
                                let mut reader = io::BufReader::with_capacity(capacity, inner);
                                let mut first = vec![0; head];
                                let r0 = reader.read(&mut first).norm();
                                let mut bytes = prefix.as_bytes().to_vec();
                                let r1 = reader.read_to_end(&mut bytes).norm();
                                let inner = scripted(data, chunk, &events);
                                let mut reader = io::BufReader::with_capacity(capacity, inner);
                                let r2 = reader.read(&mut first).norm();
                                let mut string = String::from(prefix);
                                let r3 = reader.read_to_string(&mut string).norm();
                                (r0, r1, bytes, r2, r3, string, reader.buffer().to_vec())
                            });
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn bufreader_accessors_match_std() {
    differential!(io => {
        use io::BufRead;
        let mut reader = io::BufReader::new(Reader::new(text(10)));
        let capacity = reader.capacity();
        let filled = reader.fill_buf().map(<[u8]>::to_vec).norm();
        reader.consume(3);
        let debug = format!("{:?}", io::BufReader::with_capacity(5, &b"abc"[..]));
        let buffer = reader.buffer().to_vec();
        let position = reader.get_ref().position();
        let calls = reader.get_mut().calls.clone();
        let inner = reader.into_inner();
        (capacity, filled, debug, buffer, position, calls, inner.position())
    });
}

#[test]
fn bufreader_lines_match_std() {
    let capacities: &[usize] = if cfg!(miri) {
        &[3, 8192]
    } else {
        &[1, 3, 8, 8192]
    };
    for &capacity in capacities {
        for &chunk in chunks() {
            let data = text(if cfg!(miri) { 120 } else { 300 });
            for events in scripts(data.len()) {
                differential!(io => {
                    use io::BufRead;
                    let inner = scripted(&data, chunk, &events);
                    io::BufReader::with_capacity(capacity, inner)
                        .lines()
                        .map(Norm::norm)
                        .take(80)
                        .collect::<Vec<_>>()
                });
            }
        }
    }
}

/// An operation on a `BufWriter` or `LineWriter`.
#[derive(Clone, Copy, Debug)]
enum WriteOp {
    Write(usize),
    WriteAll(usize),
    WriteVectored(usize, usize, usize),
    WriteFmt(usize),
    Flush,
}

fn write_ops(dice: &mut Dice, capacity: usize, count: usize) -> Vec<WriteOp> {
    (0..count)
        .map(|_| match dice.roll(9) {
            0 | 1 => WriteOp::Write(dice.len(capacity)),
            2 | 3 => WriteOp::WriteAll(dice.len(capacity)),
            4 | 5 => WriteOp::WriteVectored(
                dice.len(capacity),
                dice.len(capacity),
                dice.len(capacity),
            ),
            6 | 7 => WriteOp::WriteFmt(dice.len(capacity)),
            _ => WriteOp::Flush,
        })
        .collect()
}

/// Runs `ops` on a `BufWriter` or `LineWriter` (the expression
/// `$make(capacity, inner)`) with both libraries, feeding it consecutive
/// pieces of `data`, and compares every step and the final state.
macro_rules! check_writer {
    ($make:ident, $data:expr, $inner:expr, $capacity:expr, $ops:expr) => {{
        let (data, inner, capacity, ops): (&[u8], &Writer, usize, &[WriteOp]) =
            ($data, $inner, $capacity, $ops);
        differential!(io => {
            use io::Write;
            let mut writer = io::$make::with_capacity(capacity, inner.clone());
            let mut source = data.iter().copied().cycle();
            let mut take = |n: usize| -> Vec<u8> { source.by_ref().take(n).collect() };
            let mut steps = Vec::new();
            for &op in ops {
                let outcome = match op {
                    WriteOp::Write(n) => format!("{:?}", writer.write(&take(n)).norm()),
                    WriteOp::WriteAll(n) => format!("{:?}", writer.write_all(&take(n)).norm()),
                    WriteOp::WriteVectored(a, b, c) => {
                        let (a, b, c) = (take(a), take(b), take(c));
                        let bufs = [io::IoSlice::new(&a), io::IoSlice::new(&b), io::IoSlice::new(&c)];
                        format!("{:?}", writer.write_vectored(&bufs).norm())
                    }
                    WriteOp::WriteFmt(n) => {
                        let piece = String::from_utf8_lossy(&take(n)).into_owned();
                        format!("{:?}", write!(writer, "<{piece}>{}", n).norm())
                    }
                    WriteOp::Flush => format!("{:?}", writer.flush().norm()),
                };
                let inner = writer.get_ref();
                steps.push((format!("{op:?}"), outcome, inner.data.len(), inner.calls.len()));
            }
            let debug = format!("{writer:?}").len();
            let inner = writer.get_ref().clone();
            let finished = writer.into_inner().map_err(|e| {
                let error = format!("{:?}", e.error());
                let (error2, writer) = e.into_parts();
                (error, format!("{error2}"), writer.get_ref().data.clone())
            });
            (steps, debug, inner.data, inner.calls, inner.flushes, finished.map(|w| (w.data, w.calls)))
        });
    }};
}

#[test]
fn bufwriter_operations_match_std() {
    let sequences = if cfg!(miri) { 4 } else { 500 };
    let mut dice = Dice::new(2);
    let data = text(997);
    for _ in 0..sequences {
        let capacity = [0, 1, 2, 5, 16, 64][dice.roll(6)];
        let chunk = chunks()[dice.roll(chunks().len())];
        let all = scripts(300);
        let events = &all[dice.roll(all.len())];
        let mut inner = scripted_writer(chunk, events);
        if dice.roll(4) == 0 {
            inner = inner.limit(dice.roll(200));
        }
        if dice.roll(4) == 0 {
            inner = inner.fail_flush();
        }
        let ops = write_ops(&mut dice, capacity, 20);
        check_writer!(BufWriter, &data, &inner, capacity, &ops);
    }
}

#[test]
fn linewriter_operations_match_std() {
    let sequences = if cfg!(miri) { 4 } else { 800 };
    let mut dice = Dice::new(3);
    // Short and long lines, and runs without newlines.
    let mut data = text(400);
    data.extend(core::iter::repeat_n(b'x', 150));
    data.extend_from_slice(b"\n\n\nabc\n");
    for _ in 0..sequences {
        let capacity = [0, 1, 2, 5, 16, 64][dice.roll(6)];
        let chunk = chunks()[dice.roll(chunks().len())];
        let all = scripts(300);
        let events = &all[dice.roll(all.len())];
        let mut inner = scripted_writer(chunk, events);
        if dice.roll(4) == 0 {
            inner = inner.limit(dice.roll(200));
        }
        let ops = write_ops(&mut dice, capacity, 20);
        check_writer!(LineWriter, &data, &inner, capacity, &ops);
    }
}

#[test]
fn bufwriter_drop_flushes_like_std() {
    for &chunk in chunks() {
        for events in scripts(20) {
            differential!(io => {
                use io::Write;
                let mut inner = scripted_writer(chunk, &events);
                {
                    let mut writer = io::BufWriter::with_capacity(16, &mut inner);
                    let r = writer.write_all(b"0123456789").norm();
                    assert!(r.is_ok());
                }
                let dropped = (inner.data.clone(), inner.calls.clone());
                let mut line_inner = scripted_writer(chunk, &events);
                let line_result = {
                    let mut writer = io::LineWriter::with_capacity(16, &mut line_inner);
                    writer.write_all(b"ab\ncdef").norm()
                };
                (dropped, line_result, line_inner.data, line_inner.calls)
            });
        }
    }
}

#[test]
fn bufwriter_into_parts_matches_std() {
    for limit in [0, 3, 100] {
        differential!(io => {
            use io::Write;
            let mut writer = io::BufWriter::with_capacity(8, Writer::new().limit(limit));
            let r = writer.write_all(b"0123456").norm();
            let f = writer.flush().norm();
            let (inner, buffered) = writer.into_parts();
            let buffered = buffered.map_err(|e| e.to_string());
            (r, f, inner.data, buffered)
        });
    }
}

#[test]
fn bufwriter_seek_matches_std() {
    differential!(io => {
        use io::{Seek, Write};
        let cursor = io::Cursor::new(vec![b'.'; 8]);
        let mut writer = io::BufWriter::with_capacity(4, cursor);
        let first = writer.write_all(b"ab").norm();
        let seek = writer.seek(io::SeekFrom::Start(5)).norm();
        let second = writer.write_all(b"cd").norm();
        let position = writer.stream_position().norm();
        let rewind = writer.rewind().norm();
        let third = writer.write(b"e").norm();
        let inner = writer.into_inner().map(io::Cursor::into_inner).map_err(|e| e.to_string());
        (first, seek, second, position, rewind, third, inner)
    });
}

#[test]
fn writer_debug_matches_std() {
    differential!(io => {
        use io::Write;
        let mut buf = io::BufWriter::with_capacity(10, Vec::<u8>::new());
        let a = buf.write(b"abc").norm();
        let mut line = io::LineWriter::new(Vec::<u8>::new());
        let b = line.write(b"no newline").norm();
        (a, b, format!("{buf:?}"), format!("{line:?}"), format!("{:?}", line.get_mut()))
    });
}

#[test]
fn into_inner_error_matches_std() {
    differential!(io => {
        use io::Write;
        // The inner writer takes one byte, so flushing the rest fails.
        let mut writer = io::BufWriter::with_capacity(4, Writer::new().limit(1));
        let written = writer.write(b"abc").norm();
        let err = writer.into_inner().unwrap_err();
        let display = err.to_string();
        let kind = format!("{:?}", err.error().kind());
        let debug = format!("{err:?}");
        let source = core::error::Error::source(&err).map(ToString::to_string);
        let buffered = err.into_inner().buffer().to_vec();

        let mut line = io::LineWriter::with_capacity(4, Writer::new().limit(0));
        let r = line.write(b"xy").norm();
        let (error, line) = line.into_inner().unwrap_err().into_parts();
        let line_data = line.get_ref().data.clone();

        let mut w = io::BufWriter::with_capacity(4, Writer::new().limit(0));
        let q = w.write(b"q").norm();
        let from = io::Error::from(w.into_inner().unwrap_err());
        let mut w = io::BufWriter::with_capacity(4, Writer::new().limit(0));
        let z = w.write(b"z").norm();
        let into_error = w.into_inner().unwrap_err().into_error();
        (
            (written, display, kind, debug, source, buffered),
            (r, error.to_string(), line_data),
            (q, from.to_string(), z, format!("{into_error:?}")),
        )
    });
}
