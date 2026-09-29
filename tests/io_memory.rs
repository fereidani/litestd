//! `Cursor` over every buffer type std supports, and the I/O traits of
//! `&[u8]`, `&mut [u8]`, `Vec<u8>` and `VecDeque<u8>`, against std.

#![cfg(feature = "io")]

mod io_support;

use io_support::{Norm, noise, text};
use litestd::collections::VecDeque;

/// A deterministic stream of small numbers for building operation
/// sequences.
struct Dice {
    bytes: Vec<u8>,
    next: usize,
}

impl Dice {
    fn new(seed: u64) -> Self {
        Self {
            bytes: noise(8192, seed),
            next: 0,
        }
    }

    fn roll(&mut self, sides: usize) -> usize {
        let byte = self.bytes[self.next % self.bytes.len()];
        self.next += 1;
        usize::from(byte) % sides
    }
}

/// An operation on a readable cursor.
#[derive(Clone, Copy, Debug)]
enum ReadOp {
    Read(usize),
    ReadExact(usize),
    ReadVectored(usize, usize),
    ReadToEnd,
    ReadToString,
    FillConsume(usize),
    ReadLine,
    Seek(usize, i64),
    SetPosition(u64),
    StreamPosition,
}

fn read_ops(dice: &mut Dice, count: usize) -> Vec<ReadOp> {
    (0..count)
        .map(|_| match dice.roll(12) {
            0 | 1 => ReadOp::Read(dice.roll(20)),
            2 => ReadOp::ReadExact(dice.roll(20)),
            3 => ReadOp::ReadVectored(dice.roll(9), dice.roll(9)),
            4 => ReadOp::ReadToEnd,
            5 => ReadOp::ReadToString,
            6 => ReadOp::FillConsume(dice.roll(12)),
            7 => ReadOp::ReadLine,
            8 | 9 => {
                let offset = [0, 1, -1, 5, -5, 60, -60, i64::MAX, i64::MIN]
                    [dice.roll(9)];
                ReadOp::Seek(dice.roll(3), offset)
            }
            10 => ReadOp::SetPosition([0, 3, 50, u64::MAX][dice.roll(4)]),
            _ => ReadOp::StreamPosition,
        })
        .collect()
}

/// Runs `$ops` on `Cursor::new($inner)` with both libraries and compares
/// every step.
macro_rules! check_read_cursor {
    ($inner:expr, $ops:expr) => {{
        let ops: &[ReadOp] = $ops;
        differential!(io => {
            use io::{BufRead, Read, Seek};
            let mut cursor = io::Cursor::new($inner);
            let mut steps = Vec::new();
            for &op in ops {
                let outcome = match op {
                    ReadOp::Read(n) => {
                        let mut buf = vec![0; n];
                        let r = cursor.read(&mut buf).norm();
                        format!("{r:?} {buf:?}")
                    }
                    ReadOp::ReadExact(n) => {
                        let mut buf = vec![0; n];
                        let r = cursor.read_exact(&mut buf).norm();
                        format!("{r:?} {buf:?}")
                    }
                    ReadOp::ReadVectored(a, b) => {
                        let (mut x, mut y) = (vec![0; a], vec![0; b]);
                        let mut bufs = [io::IoSliceMut::new(&mut x), io::IoSliceMut::new(&mut y)];
                        let r = cursor.read_vectored(&mut bufs).norm();
                        format!("{r:?} {x:?} {y:?}")
                    }
                    ReadOp::ReadToEnd => {
                        let mut buf = b"x".to_vec();
                        let r = cursor.read_to_end(&mut buf).norm();
                        format!("{r:?} {buf:?}")
                    }
                    ReadOp::ReadToString => {
                        let mut buf = String::from("x");
                        let r = cursor.read_to_string(&mut buf).norm();
                        format!("{r:?} {buf:?}")
                    }
                    ReadOp::FillConsume(n) => {
                        let r = cursor.fill_buf().map(<[u8]>::to_vec).norm();
                        if r.as_ref().is_ok_and(|b| n <= b.len()) {
                            cursor.consume(n);
                        }
                        format!("{r:?}")
                    }
                    ReadOp::ReadLine => {
                        let mut line = String::new();
                        let r = cursor.read_line(&mut line).norm();
                        format!("{r:?} {line:?}")
                    }
                    ReadOp::Seek(whence, n) => {
                        let pos = match whence {
                            0 => io::SeekFrom::Start(n.unsigned_abs()),
                            1 => io::SeekFrom::End(n),
                            _ => io::SeekFrom::Current(n),
                        };
                        format!("{:?}", cursor.seek(pos).norm())
                    }
                    ReadOp::SetPosition(pos) => {
                        cursor.set_position(pos);
                        String::new()
                    }
                    ReadOp::StreamPosition => format!("{:?}", cursor.stream_position().norm()),
                };
                steps.push((format!("{op:?}"), outcome, cursor.position()));
            }
            steps
        });
    }};
}

#[test]
fn cursor_reads_match_std() {
    let sequences = if cfg!(miri) { 3 } else { 300 };
    let mut dice = Dice::new(1);
    for _ in 0..sequences {
        let mut data = text(dice.roll(70));
        if dice.roll(4) == 0 {
            data.push(0xFF);
        }
        let ops = read_ops(&mut dice, 16);
        check_read_cursor!(data.clone(), &ops);
        check_read_cursor!(&data[..], &ops);
        check_read_cursor!(&data, &ops);
        check_read_cursor!(data.clone().into_boxed_slice(), &ops);
        check_read_cursor!(String::from_utf8_lossy(&data).into_owned(), &ops);
    }
    let ops = read_ops(&mut dice, 40);
    check_read_cursor!(*b"fixed\narray", &ops);
    let mut backing = b"mutable\nslice".to_vec();
    check_read_cursor!(&mut backing[..], &ops);
}

/// An operation on a writable cursor.
#[derive(Clone, Copy, Debug)]
enum WriteOp {
    Write(usize),
    WriteAll(usize),
    WriteVectored(usize, usize, usize),
    WriteFmt(usize),
    SetPosition(u64),
    Seek(i64),
    Flush,
}

fn write_ops(dice: &mut Dice, count: usize, far: u64) -> Vec<WriteOp> {
    (0..count)
        .map(|_| match dice.roll(9) {
            0 | 1 => WriteOp::Write(dice.roll(12)),
            2 => WriteOp::WriteAll(dice.roll(12)),
            3 | 4 => {
                WriteOp::WriteVectored(dice.roll(6), dice.roll(6), dice.roll(6))
            }
            5 => WriteOp::WriteFmt(dice.roll(50)),
            6 => WriteOp::SetPosition([0, 2, 9, 30, far][dice.roll(5)]),
            7 => WriteOp::Seek([-3, 0, 4, -100][dice.roll(4)]),
            _ => WriteOp::Flush,
        })
        .collect()
}

/// Runs `$ops` on `Cursor::new($inner)` with both libraries, writing
/// consecutive pieces of a text, and compares every step and the final
/// buffer, read back with `$view`.
macro_rules! check_write_cursor {
    ([$($setup:tt)*] $inner:expr, $ops:expr, |$c:ident| $view:expr) => {{
        let ops: &[WriteOp] = $ops;
        differential!(io => {
            use io::{Seek, Write};
            $($setup)*
            let mut cursor = io::Cursor::new($inner);
            // More than the 16 operations of a sequence take.
            let mut source = text(256).into_iter();
            let mut take = |n: usize| -> Vec<u8> { source.by_ref().take(n).collect() };
            let mut steps = Vec::new();
            for &op in ops {
                let outcome = match op {
                    WriteOp::Write(n) => format!("{:?}", cursor.write(&take(n)).norm()),
                    WriteOp::WriteAll(n) => format!("{:?}", cursor.write_all(&take(n)).norm()),
                    WriteOp::WriteVectored(a, b, c) => {
                        let (a, b, c) = (take(a), take(b), take(c));
                        let bufs = [io::IoSlice::new(&a), io::IoSlice::new(&b), io::IoSlice::new(&c)];
                        format!("{:?}", cursor.write_vectored(&bufs).norm())
                    }
                    WriteOp::WriteFmt(n) => format!("{:?}", write!(cursor, "{n}:{n:>5}").norm()),
                    WriteOp::SetPosition(pos) => {
                        cursor.set_position(pos);
                        String::new()
                    }
                    WriteOp::Seek(n) => format!("{:?}", cursor.seek(io::SeekFrom::Current(n)).norm()),
                    WriteOp::Flush => format!("{:?}", cursor.flush().norm()),
                };
                let $c = &cursor;
                steps.push((format!("{op:?}"), outcome, cursor.position(), $view));
            }
            steps
        });
    }};
}

#[test]
fn cursor_writes_match_std() {
    let sequences = if cfg!(miri) { 2 } else { 300 };
    let mut dice = Dice::new(2);
    for _ in 0..sequences {
        // Vectors grow to any position, so keep positions small for them.
        let ops = write_ops(&mut dice, 16, 70);
        check_write_cursor!([] Vec::<u8>::new(), &ops, |c| c.get_ref().clone());
        check_write_cursor!([] b"existing".to_vec(), &ops, |c| c.get_ref().clone());
        check_write_cursor!(
            [let mut backing = b"backing vector".to_vec();] &mut backing, &ops,
            |c| Vec::clone(c.get_ref())
        );
        // Fixed buffers stop at their end, wherever the position is.
        let ops = write_ops(&mut dice, 16, u64::MAX);
        check_write_cursor!([] [b'.'; 13], &ops, |c| c.get_ref().to_vec());
        check_write_cursor!(
            [] vec![b'-'; 21].into_boxed_slice(), &ops, |c| c.get_ref().to_vec()
        );
        check_write_cursor!(
            [let mut backing = [b'_'; 9];] &mut backing[..], &ops,
            |c| c.get_ref().to_vec()
        );
    }
}

#[test]
fn cursor_write_past_usize_matches_std() {
    // A position beyond `usize::MAX` cannot index a vector.
    if usize::BITS < 64 {
        differential!(io => {
            use io::Write;
            let mut cursor = io::Cursor::new(Vec::new());
            cursor.set_position(u64::MAX);
            (cursor.write(b"x").norm(), cursor.get_ref().clone())
        });
    }
}

#[test]
fn cursor_basics_match_std() {
    differential!(io => {
        use io::Read;
        const CURSOR: io::Cursor<&[u8]> = io::Cursor::new(b"const");
        const POSITION: u64 = CURSOR.position();
        let mut a = io::Cursor::new(vec![1_u8, 2, 3]);
        a.set_position(2);
        let mut b = a.clone();
        let mut buf = [0; 4];
        let n = b.read(&mut buf).norm();
        let mut c = io::Cursor::<Vec<u8>>::default();
        c.clone_from(&a);
        *c.get_mut() = vec![9];
        let debug = format!("{a:?} {c:?} {CURSOR:?}");
        (POSITION, CURSOR.get_ref().len(), n, buf, a == b, a == a.clone(), debug, c.into_inner())
    });
}

#[test]
fn slice_reads_match_std() {
    let sequences = if cfg!(miri) { 6 } else { 400 };
    let mut dice = Dice::new(3);
    for _ in 0..sequences {
        let mut data = text(dice.roll(60));
        if dice.roll(3) == 0 {
            data.insert(dice.roll(data.len() + 1), 0xC3);
        }
        let ops: Vec<(usize, usize)> =
            (0..12).map(|_| (dice.roll(7), dice.roll(15))).collect();
        differential!(io => {
            use io::{BufRead, Read};
            let mut slice = &data[..];
            let mut steps = Vec::new();
            for &(kind, n) in &ops {
                let outcome = match kind {
                    0 => {
                        let mut buf = vec![0; n];
                        let r = slice.read(&mut buf).norm();
                        format!("{r:?} {buf:?}")
                    }
                    1 => {
                        let mut buf = vec![0; n];
                        let r = slice.read_exact(&mut buf).norm();
                        format!("{r:?} {buf:?}")
                    }
                    2 => {
                        let (mut x, mut y) = (vec![0; n / 2], vec![0; n]);
                        let mut bufs = [io::IoSliceMut::new(&mut x), io::IoSliceMut::new(&mut y)];
                        let r = slice.read_vectored(&mut bufs).norm();
                        format!("{r:?} {x:?} {y:?}")
                    }
                    3 => {
                        let mut buf = b"x".to_vec();
                        let r = slice.read_to_end(&mut buf).norm();
                        format!("{r:?} {buf:?}")
                    }
                    4 => {
                        let mut buf = String::from("x");
                        let r = slice.read_to_string(&mut buf).norm();
                        format!("{r:?} {buf:?}")
                    }
                    5 => {
                        let mut line = String::new();
                        let r = slice.read_line(&mut line).norm();
                        format!("{r:?} {line:?}")
                    }
                    _ => {
                        let r = slice.fill_buf().map(<[u8]>::to_vec).norm();
                        slice.consume(n.min(slice.len()));
                        format!("{r:?}")
                    }
                };
                steps.push((outcome, slice.to_vec()));
            }
            steps
        });
    }
}

#[test]
fn slice_and_vec_writes_match_std() {
    let sequences = if cfg!(miri) { 5 } else { 400 };
    let mut dice = Dice::new(4);
    for _ in 0..sequences {
        let ops: Vec<(usize, usize)> =
            (0..12).map(|_| (dice.roll(5), dice.roll(9))).collect();
        let size = dice.roll(40);
        differential!(io => {
            use io::Write;
            let mut storage = vec![b'.'; size];
            let mut slice = &mut storage[..];
            let mut vec = b"start".to_vec();
            // More than the 12 operations of a sequence take.
            let mut source = text(128).into_iter();
            let mut steps = Vec::new();
            for &(kind, n) in &ops {
                let piece: Vec<u8> = source.by_ref().take(n).collect();
                let bufs = [io::IoSlice::new(&piece[..n / 2]), io::IoSlice::new(&piece[n / 2..])];
                let outcome = match kind {
                    0 => format!("{:?} {:?}", slice.write(&piece).norm(), vec.write(&piece).norm()),
                    1 => format!("{:?} {:?}", slice.write_all(&piece).norm(), vec.write_all(&piece).norm()),
                    2 => format!(
                        "{:?} {:?}",
                        slice.write_vectored(&bufs).norm(),
                        vec.write_vectored(&bufs).norm()
                    ),
                    3 => format!("{:?} {:?}", write!(slice, "{n}").norm(), write!(vec, "{n:03}").norm()),
                    _ => format!("{:?} {:?}", slice.flush().norm(), vec.flush().norm()),
                };
                steps.push((outcome, slice.len(), vec.clone()));
            }
            (steps, storage)
        });
    }
}

/// A deque whose contents wrap around the end of its buffer, so that
/// `as_slices` returns two nonempty halves.
fn wrapped(data: &[u8], split: usize) -> VecDeque<u8> {
    let mut deque = VecDeque::with_capacity(data.len() + 1);
    let split = split.min(data.len());
    deque.extend(core::iter::repeat_n(0, data.len() + 1 - split));
    deque.drain(..data.len() + 1 - split);
    deque.extend(data);
    deque
}

#[test]
fn vecdeque_matches_std() {
    let sequences = if cfg!(miri) { 6 } else { 400 };
    let mut dice = Dice::new(5);
    for _ in 0..sequences {
        let mut data = text(dice.roll(50));
        if dice.roll(3) == 0 {
            data.insert(dice.roll(data.len() + 1), 0xE4);
        }
        let split = dice.roll(data.len() + 1);
        let ops: Vec<(usize, usize)> =
            (0..10).map(|_| (dice.roll(9), dice.roll(20))).collect();
        differential!(io => {
            use io::{BufRead, Read, Write};
            let mut deque = wrapped(&data, split);
            let mut steps = Vec::new();
            for &(kind, n) in &ops {
                let outcome = match kind {
                    0 => {
                        let mut buf = vec![0; n];
                        let r = deque.read(&mut buf).norm();
                        format!("{r:?} {buf:?}")
                    }
                    1 => {
                        let mut buf = vec![0; n];
                        let r = deque.read_exact(&mut buf).norm();
                        format!("{r:?} {buf:?}")
                    }
                    2 => {
                        let mut buf = b"x".to_vec();
                        let r = deque.read_to_end(&mut buf).norm();
                        format!("{r:?} {buf:?}")
                    }
                    3 => {
                        let mut buf = String::from("x");
                        let r = deque.read_to_string(&mut buf).norm();
                        format!("{r:?} {buf:?}")
                    }
                    4 => {
                        let r = deque.fill_buf().map(<[u8]>::to_vec).norm();
                        let n = n.min(deque.as_slices().0.len());
                        deque.consume(n);
                        format!("{r:?}")
                    }
                    5 => {
                        let mut line = String::new();
                        let r = deque.read_line(&mut line).norm();
                        format!("{r:?} {line:?}")
                    }
                    6 => format!("{:?}", deque.write(&text(n)).norm()),
                    7 => {
                        let piece = text(n);
                        let bufs = [io::IoSlice::new(&piece), io::IoSlice::new(b"|")];
                        format!("{:?} {:?}", deque.write_vectored(&bufs).norm(), deque.flush().norm())
                    }
                    _ => format!("{:?}", deque.write_all(&text(n)).norm()),
                };
                steps.push((outcome, deque.clone()));
            }
            steps
        });
    }
}
