//! The provided methods of `Read`, `Write`, `BufRead` and `Seek`, the
//! forwarding implementations, and `IoSlice`/`IoSliceMut`, against std.

#![cfg(feature = "io")]

mod io_support;

use io_support::{
    Event, Norm, Reader, Writer, chunks, noise, scripted, scripted_writer,
    scripts, sizes, text,
};

#[test]
fn read_to_end_matches_std() {
    for &len in sizes() {
        let data = noise(len, len as u64);
        for &chunk in chunks() {
            for events in scripts(len) {
                for prefix in [&b""[..], b"prefix"] {
                    differential!(io => {
                        use io::Read;
                        let mut reader = scripted(&data, chunk, &events);
                        let mut buf = prefix.to_vec();
                        let result = reader.read_to_end(&mut buf).norm();
                        (result, buf, reader.position())
                    });
                }
            }
        }
    }
}

#[test]
fn read_to_end_does_not_grow_an_exact_fit() {
    for len in [0, 1, 3, 31, 32, 33, 100, 5000] {
        let data = noise(len, 1);
        differential!(io => {
            use io::Read;
            let mut buf = Vec::with_capacity(len);
            let n = Reader::new(data.clone()).chunk(7).read_to_end(&mut buf).norm();
            (n, buf.capacity() == len, buf == data)
        });
    }
    differential!(io => {
        use io::Read;
        let mut buf = Vec::new();
        let n = Reader::new(Vec::new()).read_to_end(&mut buf).norm();
        (n, buf.capacity())
    });
}

#[test]
fn read_to_string_matches_std() {
    let mut inputs = vec![text(0), text(300), noise(64, 3)];
    if !cfg!(miri) {
        inputs.extend([text(1), text(9), text(20_000), noise(9000, 4)]);
    }
    // Valid UTF-8 except for a truncated character at the end.
    let mut truncated = text(40);
    truncated.extend_from_slice(&"\u{e9}".as_bytes()[..1]);
    inputs.push(truncated);
    for data in &inputs {
        for &chunk in chunks() {
            for events in scripts(data.len()) {
                for prefix in ["", "d\u{e9}j\u{e0} "] {
                    differential!(io => {
                        use io::Read;
                        let mut reader = scripted(data, chunk, &events);
                        let mut buf = String::from(prefix);
                        let result = reader.read_to_string(&mut buf).norm();
                        (result, buf, reader.position())
                    });
                }
            }
        }
    }
}

#[test]
fn read_to_string_error_inside_a_character() {
    // The read fails after the first byte of a two-byte character: the
    // bytes read so far are not valid UTF-8, so nothing is kept.
    let data = "a\u{e9}".as_bytes();
    for at in 0..=data.len() {
        differential!(io => {
            use io::Read;
            let mut reader = Reader::new(data).chunk(1).event(at, Event::Fail);
            let mut buf = String::from("x");
            let result = reader.read_to_string(&mut buf).norm();
            (result, buf)
        });
    }
}

#[test]
fn read_exact_matches_std() {
    for &len in sizes() {
        let data = noise(len, 7);
        for want in [0, 1, len / 2, len, len + 1] {
            for &chunk in chunks() {
                for events in scripts(len) {
                    differential!(io => {
                        use io::Read;
                        let mut reader = scripted(&data, chunk, &events);
                        let mut buf = vec![0xAA; want];
                        let result = reader.read_exact(&mut buf).norm();
                        (result, buf, reader.position())
                    });
                }
            }
        }
    }
}

#[test]
fn read_vectored_reads_the_first_nonempty_buffer() {
    for events in scripts(10) {
        differential!(io => {
            use io::Read;
            let mut reader = scripted(b"0123456789", 4, &events);
            let (mut a, mut b, mut c) = ([0; 0], [0; 3], [0; 5]);
            let mut bufs = [
                io::IoSliceMut::new(&mut a),
                io::IoSliceMut::new(&mut b),
                io::IoSliceMut::new(&mut c),
            ];
            let first = reader.read_vectored(&mut bufs).norm();
            let second = reader.read_vectored(&mut bufs[2..]).norm();
            let empty = reader.read_vectored(&mut []).norm();
            (first, second, empty, b, c, reader.calls.clone())
        });
    }
}

#[test]
fn bytes_matches_std() {
    for &len in sizes().iter().take(6) {
        let data = noise(len, 9);
        for events in scripts(len) {
            differential!(io => {
                use io::Read;
                let reader = scripted(&data, 3, &events);
                reader.bytes().map(Norm::norm).take(len + 4).collect::<Vec<_>>()
            });
        }
    }
}

#[test]
fn by_ref_borrows() {
    differential!(io => {
        use io::{Read, Write};
        let mut reader = Reader::new(&b"abcdef"[..]);
        let mut head = Vec::new();
        reader.by_ref().take(2).read_to_end(&mut head).norm().unwrap();
        let mut rest = String::new();
        reader.read_to_string(&mut rest).norm().unwrap();
        let mut writer = Writer::new();
        writer.by_ref().write_all(b"xy").norm().unwrap();
        (head, rest, writer.data)
    });
}

#[test]
// `differential!` runs the body twice; only the second clone is redundant.
#[allow(clippy::redundant_clone)]
fn read_to_string_function_matches_std() {
    for data in [text(50), noise(50, 5)] {
        differential!(io => {
            io::read_to_string(Reader::new(data.clone()).chunk(7)).norm()
        });
    }
}

/// The data, chunk limit and events of a scripted reader.
type Case = (Vec<u8>, usize, Vec<(usize, Event)>);

/// Scripted `BufRead`s over `len` bytes whose `fill_buf` returns short
/// windows.
fn buf_reads(len: usize) -> Vec<Case> {
    let data = text(len);
    let mut cases = Vec::new();
    for &chunk in chunks() {
        for events in scripts(len) {
            cases.push((data.clone(), chunk, events));
        }
    }
    cases
}

#[test]
fn read_until_and_skip_until_match_std() {
    for &len in sizes().iter().take(8) {
        for (data, chunk, events) in buf_reads(len) {
            let delims: &[u8] = if cfg!(miri) {
                &[b'\n', 0xE9]
            } else {
                &[b'\n', b'x', 0xE9]
            };
            for &delim in delims {
                differential!(io => {
                    use io::BufRead;
                    let mut reader = scripted(&data, chunk, &events);
                    let mut results = Vec::new();
                    for _ in 0..8 {
                        let mut buf = b"old".to_vec();
                        let r = reader.read_until(delim, &mut buf).norm();
                        results.push((r, buf));
                        results.push((reader.skip_until(delim).norm(), Vec::new()));
                    }
                    (results, reader.position())
                });
            }
        }
    }
}

#[test]
fn read_line_matches_std() {
    let mut inputs = vec![text(0), text(10), text(77), noise(40, 11)];
    // A line with an invalid byte between valid ones.
    inputs.push(b"good\nb\xffd\nafter\n".to_vec());
    for data in &inputs {
        for &chunk in chunks() {
            for events in scripts(data.len()) {
                differential!(io => {
                    use io::BufRead;
                    let mut reader = scripted(data, chunk, &events);
                    let mut lines = Vec::new();
                    let mut buf = String::from("keep ");
                    for _ in 0..12 {
                        let r = reader.read_line(&mut buf).norm();
                        lines.push((r, buf.clone()));
                    }
                    lines
                });
            }
        }
    }
}

#[test]
fn lines_and_split_match_std() {
    let mut inputs = vec![
        text(0),
        text(60),
        b"a\r\n\r\nb\rc\n\n\r".to_vec(),
        b"no newline".to_vec(),
        b"bad \xff line\nok\n".to_vec(),
    ];
    inputs.push(noise(30, 2));
    for data in &inputs {
        for events in scripts(data.len()) {
            differential!(io => {
                use io::BufRead;
                let lines = scripted(data, 5, &events)
                    .lines()
                    .map(Norm::norm)
                    .take(40)
                    .collect::<Vec<_>>();
                let split = scripted(data, 5, &events)
                    .split(b'\n')
                    .map(Norm::norm)
                    .take(40)
                    .collect::<Vec<_>>();
                (lines, split)
            });
        }
    }
}

#[test]
fn write_all_matches_std() {
    for &len in sizes() {
        let data = noise(len, 13);
        for &chunk in chunks() {
            for events in scripts(len) {
                differential!(io => {
                    use io::Write;
                    let mut writer = scripted_writer(chunk, &events);
                    let result = writer.write_all(&data).norm();
                    (result, writer.data, writer.calls)
                });
            }
        }
    }
}

#[test]
fn write_all_stops_when_the_writer_is_full() {
    differential!(io => {
        use io::Write;
        let mut writer = Writer::new().chunk(3).limit(5);
        let result = writer.write_all(b"0123456789").norm();
        (result, writer.data, writer.calls)
    });
}

#[test]
fn write_fmt_matches_std() {
    for &chunk in chunks() {
        for events in scripts(12) {
            differential!(io => {
                use io::Write;
                let mut writer = scripted_writer(chunk, &events);
                let formatted = write!(writer, "{}-{:>4}|{:?}", 12, "ab", 'c').norm();
                let literal = write!(writer, "literal").norm();
                let line = writeln!(writer).norm();
                (formatted, literal, line, writer.data)
            });
        }
    }
}

#[test]
fn write_fmt_reports_a_failing_formatter() {
    struct Broken;

    impl core::fmt::Display for Broken {
        fn fmt(&self, _: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            Err(core::fmt::Error)
        }
    }

    // std panics here; litestd returns an error.
    use litestd::io::Write;
    let mut out = Vec::new();
    let err = write!(out, "a{Broken}b").unwrap_err();
    assert_eq!(err.to_string(), "formatter error");
    assert_eq!(out, b"a");
    // An error from the writer wins over the formatter's.
    let mut storage = [0_u8; 0];
    let mut full = &mut storage[..];
    let err = write!(full, "{}", 1).unwrap_err();
    assert_eq!(err.kind(), litestd::io::ErrorKind::WriteZero);
}

#[test]
fn write_vectored_writes_the_first_nonempty_buffer() {
    for events in scripts(8) {
        differential!(io => {
            use io::Write;
            let mut writer = scripted_writer(4, &events);
            let bufs = [
                io::IoSlice::new(b""),
                io::IoSlice::new(b"abc"),
                io::IoSlice::new(b"defgh"),
            ];
            let first = writer.write_vectored(&bufs).norm();
            let second = writer.write_vectored(&bufs[2..]).norm();
            let empty = writer.write_vectored(&[]).norm();
            let flushed = writer.flush().norm();
            (first, second, empty, flushed, writer.data, writer.calls)
        });
    }
}

#[test]
fn seek_provided_methods_match_std() {
    for fail in [false, true] {
        differential!(io => {
            use io::Seek;
            let mut reader = Reader::new(text(100));
            if fail {
                reader = reader.fail_seek();
            }
            let forward = reader.seek_relative(10).norm();
            let position = reader.stream_position().norm();
            let back = reader.seek_relative(-3).norm();
            let after_back = reader.stream_position().norm();
            let negative = reader.seek_relative(-50).norm();
            let rewind = reader.rewind().norm();
            let end = reader.seek(io::SeekFrom::End(-1)).norm();
            (forward, position, back, after_back, negative, rewind, end, reader.seeks)
        });
    }
}

#[test]
fn seek_from_matches_std() {
    use litestd::io::SeekFrom;
    let pairs = [
        (SeekFrom::Start(3), std::io::SeekFrom::Start(3)),
        (SeekFrom::End(-2), std::io::SeekFrom::End(-2)),
        (
            SeekFrom::Current(i64::MIN),
            std::io::SeekFrom::Current(i64::MIN),
        ),
    ];
    for (lite, std) in pairs {
        assert_eq!(format!("{lite:?}"), format!("{std:?}"));
        assert_eq!(lite, lite.clone());
    }
    assert_ne!(SeekFrom::Start(0), SeekFrom::Current(0));
}

#[test]
// The forwarding impls for `&mut R` take `&mut &mut R` receivers.
#[allow(clippy::mut_mut)]
fn forwarding_impls_match_std() {
    for &chunk in chunks().iter().take(3) {
        // The calls name the forwarding implementations for `&mut T` and
        // `Box<T>` explicitly; method syntax would pick `T`'s.
        differential!(io => {
            use io::{BufRead, Read, Seek, Write};
            let mut reader = Reader::new(text(50)).chunk(chunk);
            let mut by_ref = &mut reader;
            let mut buf = Vec::new();
            let mut boxed: Box<Reader> = Box::new(Reader::new(text(20)).chunk(chunk));
            let reads = (
                Read::read(&mut by_ref, &mut [0; 3]).norm(),
                Read::read_to_end(&mut boxed, &mut buf).norm(),
                Read::read_vectored(&mut by_ref, &mut [io::IoSliceMut::new(&mut [0; 2])]).norm(),
                Read::read_exact(&mut boxed, &mut [0; 4]).norm(),
            );
            let mut text_buf = String::new();
            let to_string = Read::read_to_string(&mut by_ref, &mut text_buf).norm();
            let mut dynamic: Box<dyn BufRead> = Box::new(Reader::new(text(40)).chunk(chunk));
            let mut line = String::new();
            let mut until = Vec::new();
            let buffered = (
                BufRead::read_line(&mut dynamic, &mut line).norm(),
                BufRead::skip_until(&mut &mut dynamic, b'\n').norm(),
                BufRead::read_until(&mut dynamic, b'a', &mut until).norm(),
                BufRead::fill_buf(&mut &mut dynamic).map(<[u8]>::to_vec).norm(),
            );
            BufRead::consume(&mut dynamic, 1);
            let mut writer: Box<dyn Write> = Box::new(Writer::new().chunk(chunk));
            let written = (
                Write::write_all(&mut writer, b"forwarded").norm(),
                write!(&mut &mut writer, "{}", 42).norm(),
                Write::write_vectored(&mut &mut writer, &[io::IoSlice::new(b"v")]).norm(),
                Write::write(&mut writer, b"w").norm(),
                Write::flush(&mut &mut writer).norm(),
            );
            let mut seeker: Box<dyn Seek> = Box::new(Reader::new(text(10)));
            let seeks = (
                Seek::seek_relative(&mut seeker, 4).norm(),
                Seek::stream_position(&mut &mut seeker).norm(),
                Seek::rewind(&mut &mut seeker).norm(),
                Seek::seek(&mut seeker, io::SeekFrom::End(-2)).norm(),
            );
            (reads, to_string, buffered, written, seeks, (buf, text_buf, line, until))
        });
    }
}

#[test]
fn io_slice_advance_matches_std() {
    let data: Vec<u8> = (0..10).collect();
    for n in 0..=10 {
        let mut lite = litestd::io::IoSlice::new(&data);
        let mut std = std::io::IoSlice::new(&data);
        lite.advance(n);
        std.advance(n);
        assert_eq!(&*lite, &*std);
        assert_eq!(format!("{lite:?}"), format!("{std:?}"));

        let mut lite_buf = data.clone();
        let mut std_buf = data.clone();
        let mut lite = litestd::io::IoSliceMut::new(&mut lite_buf);
        let mut std = std::io::IoSliceMut::new(&mut std_buf);
        lite.advance(n);
        std.advance(n);
        assert_eq!(&*lite, &*std);
        assert_eq!(format!("{lite:?}"), format!("{std:?}"));
        if let Some(first) = lite.first_mut() {
            *first = 0xFF;
        }
        assert_eq!(lite_buf.get(n), if n < 10 { Some(&0xFF) } else { None });
    }
}

#[test]
fn io_slice_advance_slices_matches_std() {
    let parts: [&[u8]; 6] = [b"", b"abc", b"", b"", b"defgh", b"i"];
    for n in 0..=9 {
        let mut lite: Vec<_> =
            parts.iter().map(|p| litestd::io::IoSlice::new(p)).collect();
        let mut std: Vec<_> =
            parts.iter().map(|p| std::io::IoSlice::new(p)).collect();
        let mut lite = &mut lite[..];
        let mut std = &mut std[..];
        litestd::io::IoSlice::advance_slices(&mut lite, n);
        std::io::IoSlice::advance_slices(&mut std, n);
        assert_eq!(format!("{lite:?}"), format!("{std:?}"), "n = {n}");

        let mut lite_store: Vec<Vec<u8>> =
            parts.iter().map(|p| p.to_vec()).collect();
        let mut std_store = lite_store.clone();
        let mut lite: Vec<_> = lite_store
            .iter_mut()
            .map(|p| litestd::io::IoSliceMut::new(p))
            .collect();
        let mut std: Vec<_> = std_store
            .iter_mut()
            .map(|p| std::io::IoSliceMut::new(p))
            .collect();
        let mut lite = &mut lite[..];
        let mut std = &mut std[..];
        litestd::io::IoSliceMut::advance_slices(&mut lite, n);
        std::io::IoSliceMut::advance_slices(&mut std, n);
        assert_eq!(format!("{lite:?}"), format!("{std:?}"), "n = {n}");
    }
}

#[test]
fn io_slice_is_an_iovec() {
    use core::mem::{align_of, size_of};
    assert_eq!(
        size_of::<litestd::io::IoSlice<'_>>(),
        2 * size_of::<usize>()
    );
    assert_eq!(
        size_of::<litestd::io::IoSliceMut<'_>>(),
        2 * size_of::<usize>()
    );
    assert_eq!(align_of::<litestd::io::IoSlice<'_>>(), align_of::<usize>());
    #[cfg(unix)]
    {
        let data = *b"iovec";
        let slices = [
            litestd::io::IoSlice::new(&data[..2]),
            litestd::io::IoSlice::new(&data[2..]),
        ];
        // SAFETY: `IoSlice` is `repr(transparent)` over `iovec`.
        let iovecs = unsafe {
            core::slice::from_raw_parts(
                slices.as_ptr().cast::<libc::iovec>(),
                2,
            )
        };
        assert_eq!(iovecs[0].iov_base.cast_const(), data.as_ptr().cast());
        assert_eq!(iovecs[0].iov_len, 2);
        assert_eq!(iovecs[1].iov_len, 3);
    }
    // std guarantees that both are ABI compatible with Winsock's `WSABUF`,
    // whose 32-bit length comes first.
    #[cfg(windows)]
    {
        #[repr(C)]
        struct WsaBuf {
            len: u32,
            buf: *mut u8,
        }

        let mut data = *b"wsabuf";
        let slices = [
            litestd::io::IoSlice::new(&data[..2]),
            litestd::io::IoSlice::new(&data[2..]),
        ];
        // SAFETY: `IoSlice` is `repr(transparent)` over `WSABUF`.
        let bufs = unsafe {
            core::slice::from_raw_parts(slices.as_ptr().cast::<WsaBuf>(), 2)
        };
        assert_eq!((bufs[0].len, bufs[1].len), (2, 4));
        assert_eq!(bufs[0].buf.cast_const(), data.as_ptr());
        let base = data.as_mut_ptr();
        let slice_mut = litestd::io::IoSliceMut::new(&mut data[1..]);
        // SAFETY: `IoSliceMut` is `repr(transparent)` over `WSABUF`.
        let buf = unsafe { &*(&raw const slice_mut).cast::<WsaBuf>() };
        assert_eq!((buf.len, buf.buf), (5, base.wrapping_add(1)));
    }
}

#[test]
#[should_panic = "advancing IoSlice beyond its length"]
fn io_slice_advance_past_the_end_panics() {
    litestd::io::IoSlice::new(b"ab").advance(3);
}

#[test]
#[should_panic = "advancing IoSliceMut beyond its length"]
fn io_slice_mut_advance_past_the_end_panics() {
    litestd::io::IoSliceMut::new(&mut [0; 2]).advance(3);
}

#[test]
#[should_panic = "advancing io slices beyond their length"]
// `advance_slices` takes the slices as `&mut &mut [IoSlice]`.
#[allow(clippy::mut_mut)]
fn advance_slices_past_the_end_panics() {
    let mut slices = [
        litestd::io::IoSlice::new(b"ab"),
        litestd::io::IoSlice::new(b""),
    ];
    litestd::io::IoSlice::advance_slices(&mut &mut slices[..], 3);
}

#[test]
#[should_panic = "advancing io slices beyond their length"]
// As above, with `&mut &mut [IoSliceMut]`.
#[allow(clippy::mut_mut)]
fn advance_slices_mut_past_the_end_panics() {
    let mut a = [0; 2];
    let mut slices = [litestd::io::IoSliceMut::new(&mut a)];
    litestd::io::IoSliceMut::advance_slices(&mut &mut slices[..], 3);
}

#[test]
fn reader_contract_violations_are_clamped() {
    /// Claims to have read more than the buffer holds.
    struct Liar;

    impl litestd::io::Read for Liar {
        fn read(&mut self, buf: &mut [u8]) -> litestd::io::Result<usize> {
            buf.fill(b'z');
            Ok(buf.len() + 5)
        }
    }

    impl litestd::io::Write for Liar {
        fn write(&mut self, buf: &[u8]) -> litestd::io::Result<usize> {
            Ok(buf.len() + 5)
        }

        fn flush(&mut self) -> litestd::io::Result<()> {
            Ok(())
        }
    }

    use litestd::io::{BufRead, BufReader, Read, Write};
    let mut buf = [0; 4];
    Liar.read_exact(&mut buf).unwrap();
    assert_eq!(buf, *b"zzzz");
    let mut limited = Liar.take(6);
    assert_eq!(limited.read(&mut [0; 4]).unwrap(), 4);
    assert_eq!(limited.limit(), 2);
    let mut reader = BufReader::with_capacity(3, Liar);
    assert_eq!(reader.fill_buf().unwrap(), b"zzz");
    reader.consume(usize::MAX);
    Liar.write_all(b"abc").unwrap();
    // Reading to the end never ends with this reader; a bounded one shows
    // the lie does not get past the vector's length.
    let mut out = Vec::new();
    Liar.take(40).read_to_end(&mut out).unwrap();
    assert_eq!(out, [b'z'; 40]);
}
