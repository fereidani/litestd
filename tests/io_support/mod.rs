//! Shared helpers for the differential I/O tests.
//!
//! [`Reader`] and [`Writer`] implement both std's and litestd's I/O traits
//! and misbehave on a script: short transfers, interruptions and failures
//! at chosen byte offsets. Offsets rather than call counts keep a script
//! meaningful when std and litestd split the same work into different
//! calls. [`differential!`] runs a scenario once against each library and
//! compares the results.

#![allow(dead_code, reason = "each test file uses a subset")]
#![allow(
    clippy::redundant_pub_crate,
    reason = "`pub` would trip `unreachable_pub`"
)]

use core::fmt::Debug;

/// Runs `$body` with `$io` naming `litestd::io` and then `std::io`, and
/// asserts that both runs produce equal values. With `$fs` as well, it
/// names the matching `fs` module.
#[macro_export]
macro_rules! differential {
    ($io:ident => $body:expr) => {{
        let lite = {
            #[allow(unused_imports)]
            use litestd::io as $io;
            $body
        };
        let std = {
            #[allow(unused_imports)]
            use std::io as $io;
            $body
        };
        assert_eq!(lite, std, "litestd (left) differs from std (right)");
    }};
    ($io:ident, $fs:ident => $body:expr) => {{
        let lite = {
            #[allow(unused_imports)]
            use litestd::{fs as $fs, io as $io};
            $body
        };
        let std = {
            #[allow(unused_imports)]
            use std::{fs as $fs, io as $io};
            $body
        };
        assert_eq!(lite, std, "litestd (left) differs from std (right)");
    }};
}

/// An I/O error, reduced to what std and litestd must agree on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ErrorSummary {
    pub(crate) kind: String,
    pub(crate) display: String,
    pub(crate) debug: String,
}

/// Turns a std or litestd I/O result into a comparable value.
pub(crate) trait Norm {
    type Output: Debug + PartialEq;

    fn norm(self) -> Self::Output;
}

impl<T: Debug + PartialEq> Norm for std::io::Result<T> {
    type Output = Result<T, ErrorSummary>;

    fn norm(self) -> Self::Output {
        self.map_err(|e| ErrorSummary {
            kind: format!("{:?}", e.kind()),
            display: e.to_string(),
            debug: format!("{e:?}"),
        })
    }
}

impl<T: Debug + PartialEq> Norm for litestd::io::Result<T> {
    type Output = Result<T, ErrorSummary>;

    fn norm(self) -> Self::Output {
        self.map_err(|e| ErrorSummary {
            kind: format!("{:?}", e.kind()),
            display: e.to_string(),
            debug: format!("{e:?}"),
        })
    }
}

/// What a scripted reader or writer does instead of transferring data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Event {
    /// Fails with an error of kind `Interrupted`.
    Interrupt,
    /// Fails with a custom error of kind `Other`.
    Fail,
    /// Transfers nothing: `Ok(0)`.
    Zero,
}

impl Event {
    fn std(self) -> std::io::Error {
        match self {
            Self::Interrupt => std::io::ErrorKind::Interrupted.into(),
            _ => std::io::Error::other("scripted failure"),
        }
    }

    fn lite(self) -> litestd::io::Error {
        match self {
            Self::Interrupt => litestd::io::ErrorKind::Interrupted.into(),
            _ => litestd::io::Error::other("scripted failure"),
        }
    }
}

/// Events by byte offset; each fires once, when the stream is at its offset.
#[derive(Clone, Debug, Default)]
struct Events(Vec<(usize, Event)>);

impl Events {
    fn add(&mut self, at: usize, event: Event) {
        self.0.push((at, event));
    }

    /// Removes and returns the first event at `pos`.
    fn take(&mut self, pos: usize) -> Option<Event> {
        let i = self.0.iter().position(|&(at, _)| at == pos)?;
        Some(self.0.remove(i).1)
    }

    /// The bytes that can pass before the next event after `pos`.
    fn room(&self, pos: usize) -> usize {
        self.0
            .iter()
            .filter(|&&(at, _)| at > pos)
            .map(|&(at, _)| at - pos)
            .min()
            .unwrap_or(usize::MAX)
    }
}

/// A scripted source of bytes: a `Read`, `BufRead` and `Seek`.
#[derive(Clone, Debug)]
pub(crate) struct Reader {
    data: Vec<u8>,
    pos: usize,
    /// The most bytes one call transfers.
    chunk: usize,
    events: Events,
    /// The bytes that `fill_buf` returned and `consume` has not taken.
    window: usize,
    /// Whether the next seek fails.
    fail_seek: bool,
    /// The buffer length of every read, and `usize::MAX` for `fill_buf`.
    pub(crate) calls: Vec<usize>,
    /// Every seek target, as an offset from the start.
    pub(crate) seeks: Vec<i128>,
}

impl Reader {
    pub(crate) fn new(data: impl Into<Vec<u8>>) -> Self {
        Self {
            data: data.into(),
            pos: 0,
            chunk: usize::MAX,
            events: Events::default(),
            window: 0,
            fail_seek: false,
            calls: Vec::new(),
            seeks: Vec::new(),
        }
    }

    /// Limits each call to `chunk` bytes.
    #[must_use]
    pub(crate) const fn chunk(mut self, chunk: usize) -> Self {
        self.chunk = chunk;
        self
    }

    /// Makes the reader do `event` when it reaches byte `at`.
    #[must_use]
    pub(crate) fn event(mut self, at: usize, event: Event) -> Self {
        self.events.add(at, event);
        self
    }

    /// Makes the next seek fail.
    #[must_use]
    pub(crate) const fn fail_seek(mut self) -> Self {
        self.fail_seek = true;
        self
    }

    pub(crate) const fn position(&self) -> usize {
        self.pos
    }

    /// Returns how many bytes the next transfer of at most `want` bytes
    /// moves, or the event that happens instead.
    fn step(&mut self, want: usize) -> Result<usize, Event> {
        self.calls.push(want);
        if want == 0 {
            return Ok(0);
        }
        if let Some(event) = self.events.take(self.pos) {
            return Err(event);
        }
        let left = self.data.len().saturating_sub(self.pos);
        Ok(want
            .min(self.chunk)
            .min(left)
            .min(self.events.room(self.pos)))
    }

    fn read_into(&mut self, buf: &mut [u8]) -> Result<usize, Event> {
        let n = match self.step(buf.len()) {
            Err(Event::Zero) => 0,
            other => other?,
        };
        if n > 0 {
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        }
        self.pos += n;
        self.window = self.window.saturating_sub(n);
        Ok(n)
    }

    fn fill(&mut self) -> Result<&[u8], Event> {
        if self.window == 0 {
            self.window = match self.step(usize::MAX) {
                Err(Event::Zero) => 0,
                other => other?,
            };
        }
        Ok(self
            .data
            .get(self.pos..self.pos + self.window)
            .unwrap_or_default())
    }

    fn take(&mut self, amount: usize) {
        let amount = amount.min(self.window);
        self.window -= amount;
        self.pos += amount;
    }

    /// Moves to `target`, or fails if it is negative or a failure is due.
    fn seek_to(&mut self, target: i128) -> Result<u64, Option<Event>> {
        self.seeks.push(target);
        if core::mem::take(&mut self.fail_seek) {
            return Err(Some(Event::Fail));
        }
        let Ok(target) = u64::try_from(target) else {
            return Err(None);
        };
        self.pos = usize::try_from(target).unwrap_or(usize::MAX);
        self.window = 0;
        Ok(target)
    }

    fn std_target(&self, pos: std::io::SeekFrom) -> i128 {
        match pos {
            std::io::SeekFrom::Start(n) => i128::from(n),
            std::io::SeekFrom::End(n) => {
                self.data.len() as i128 + i128::from(n)
            }
            std::io::SeekFrom::Current(n) => self.pos as i128 + i128::from(n),
        }
    }

    fn lite_target(&self, pos: litestd::io::SeekFrom) -> i128 {
        match pos {
            litestd::io::SeekFrom::Start(n) => i128::from(n),
            litestd::io::SeekFrom::End(n) => {
                self.data.len() as i128 + i128::from(n)
            }
            litestd::io::SeekFrom::Current(n) => {
                self.pos as i128 + i128::from(n)
            }
        }
    }
}

impl std::io::Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.read_into(buf).map_err(Event::std)
    }
}

impl litestd::io::Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> litestd::io::Result<usize> {
        self.read_into(buf).map_err(Event::lite)
    }
}

impl std::io::BufRead for Reader {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        self.fill().map_err(Event::std)
    }

    fn consume(&mut self, amount: usize) {
        self.take(amount);
    }
}

impl litestd::io::BufRead for Reader {
    fn fill_buf(&mut self) -> litestd::io::Result<&[u8]> {
        self.fill().map_err(Event::lite)
    }

    fn consume(&mut self, amount: usize) {
        self.take(amount);
    }
}

impl std::io::Seek for Reader {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        let target = self.std_target(pos);
        self.seek_to(target).map_err(|e| {
            e.map_or_else(
                || std::io::ErrorKind::InvalidInput.into(),
                Event::std,
            )
        })
    }
}

impl litestd::io::Seek for Reader {
    fn seek(&mut self, pos: litestd::io::SeekFrom) -> litestd::io::Result<u64> {
        let target = self.lite_target(pos);
        self.seek_to(target).map_err(|e| {
            e.map_or_else(
                || litestd::io::ErrorKind::InvalidInput.into(),
                Event::lite,
            )
        })
    }
}

/// A scripted sink of bytes.
#[derive(Clone, Debug)]
pub(crate) struct Writer {
    pub(crate) data: Vec<u8>,
    /// The most bytes one call accepts.
    chunk: usize,
    /// The total the writer accepts before it accepts nothing more.
    limit: usize,
    events: Events,
    /// Whether the next flush fails.
    fail_flush: bool,
    /// The buffer length of every write.
    pub(crate) calls: Vec<usize>,
    pub(crate) flushes: usize,
}

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
}

impl Writer {
    pub(crate) const fn new() -> Self {
        Self {
            data: Vec::new(),
            chunk: usize::MAX,
            limit: usize::MAX,
            events: Events(Vec::new()),
            fail_flush: false,
            calls: Vec::new(),
            flushes: 0,
        }
    }

    /// Limits each call to `chunk` bytes.
    #[must_use]
    pub(crate) const fn chunk(mut self, chunk: usize) -> Self {
        self.chunk = chunk;
        self
    }

    /// Accepts nothing after `limit` bytes in total.
    #[must_use]
    pub(crate) const fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// Makes the writer do `event` once it holds `at` bytes.
    #[must_use]
    pub(crate) fn event(mut self, at: usize, event: Event) -> Self {
        self.events.add(at, event);
        self
    }

    /// Makes the next flush fail.
    #[must_use]
    pub(crate) const fn fail_flush(mut self) -> Self {
        self.fail_flush = true;
        self
    }

    fn step(&mut self, buf: &[u8]) -> Result<usize, Event> {
        self.calls.push(buf.len());
        if buf.is_empty() {
            return Ok(0);
        }
        let len = self.data.len();
        match self.events.take(len) {
            Some(Event::Zero) => return Ok(0),
            Some(event) => return Err(event),
            None => {}
        }
        let n = buf
            .len()
            .min(self.chunk)
            .min(self.limit.saturating_sub(len))
            .min(self.events.room(len));
        self.data.extend_from_slice(&buf[..n]);
        Ok(n)
    }

    fn flush_now(&mut self) -> Result<(), Event> {
        self.flushes += 1;
        if core::mem::take(&mut self.fail_flush) {
            return Err(Event::Fail);
        }
        Ok(())
    }
}

impl std::io::Write for Writer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.step(buf).map_err(Event::std)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.flush_now().map_err(Event::std)
    }
}

impl litestd::io::Write for Writer {
    fn write(&mut self, buf: &[u8]) -> litestd::io::Result<usize> {
        self.step(buf).map_err(Event::lite)
    }

    fn flush(&mut self) -> litestd::io::Result<()> {
        self.flush_now().map_err(Event::lite)
    }
}

/// Deterministic test data: `len` bytes of a repeating pattern that
/// includes newlines, carriage returns and non-ASCII UTF-8.
pub(crate) fn text(len: usize) -> Vec<u8> {
    const PATTERN: &[u8] =
        "line \u{e9}\u{4e2d}\r\nab\nc\n\nlonger line of text 0123456789\n"
            .as_bytes();
    PATTERN.iter().copied().cycle().take(len).collect()
}

/// Deterministic pseudo-random bytes.
pub(crate) fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[3]
        })
        .collect()
}

/// Sizes around the boundaries the implementations care about, fewer under
/// Miri.
pub(crate) const fn sizes() -> &'static [usize] {
    if cfg!(miri) {
        &[0, 1, 32, 33, 100]
    } else {
        &[
            0, 1, 2, 5, 31, 32, 33, 64, 100, 1000, 4096, 8191, 8192, 8193,
            20_000, 70_000,
        ]
    }
}

/// Chunk limits for short transfers, fewer under Miri.
pub(crate) const fn chunks() -> &'static [usize] {
    if cfg!(miri) {
        &[7, usize::MAX]
    } else {
        &[1, 3, 7, 32, 100, 4096, 8192, usize::MAX]
    }
}

/// Event scripts for a stream of `len` bytes: none, and each event at the
/// start, in the middle and at the end. Under Miri, fewer.
pub(crate) fn scripts(len: usize) -> Vec<Vec<(usize, Event)>> {
    if cfg!(miri) {
        return vec![
            Vec::new(),
            vec![(len / 2, Event::Interrupt)],
            vec![(len / 2, Event::Fail)],
            vec![(len / 2, Event::Zero)],
        ];
    }
    let mut scripts = vec![Vec::new()];
    for event in [Event::Interrupt, Event::Fail, Event::Zero] {
        for at in [0, len / 2, len] {
            scripts.push(vec![(at, event)]);
        }
    }
    scripts.push(vec![
        (0, Event::Interrupt),
        (len / 3, Event::Interrupt),
        (len, Event::Interrupt),
    ]);
    scripts.push(vec![(len / 3, Event::Interrupt), (len / 2, Event::Fail)]);
    scripts
}

/// A reader over `data` with the given chunk limit and events.
pub(crate) fn scripted(
    data: &[u8],
    chunk: usize,
    events: &[(usize, Event)],
) -> Reader {
    events
        .iter()
        .fold(Reader::new(data).chunk(chunk), |r, &(at, e)| r.event(at, e))
}

/// A writer with the given chunk limit and events.
pub(crate) fn scripted_writer(
    chunk: usize,
    events: &[(usize, Event)],
) -> Writer {
    events
        .iter()
        .fold(Writer::new().chunk(chunk), |w, &(at, e)| w.event(at, e))
}
