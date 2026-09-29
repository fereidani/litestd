//! `litestd::net::{TcpStream, TcpListener}` against std on loopback:
//! clients and servers of either library talking to each other, timeouts
//! that expire, nonblocking mode, refused connections, `take_error`,
//! `shutdown`, `peek`, the socket options, `Debug`, and the descriptor
//! traits. Miri cannot run sockets.

// WebAssembly has no sockets in litestd.
#![cfg(all(feature = "net", not(miri), not(target_family = "wasm")))]
#![allow(clippy::unwrap_used, reason = "a failure fails the test")]

mod net_support;

use core::{
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};
use std::{
    io::{Read as _, Write as _},
    thread,
    time::Instant,
};

use litestd::{
    io::{IoSlice, IoSliceMut, Read as LiteRead, Write as LiteWrite},
    net::{Shutdown, TcpListener, TcpStream},
};
use net_support::{
    IO_LIMIT, TIMEOUTS, same_error, same_result, same_timeout, timed,
};

/// A litestd listener on a free loopback port.
fn lite_listener() -> (TcpListener, SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    (listener, addr)
}

/// A std listener on a free loopback port.
fn std_listener() -> (std::net::TcpListener, SocketAddr) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    (listener, addr)
}

/// A loopback address that nothing listens on, most likely.
fn closed_addr() -> SocketAddr {
    std_listener().1
}

fn limit_lite(stream: &TcpStream) {
    stream.set_read_timeout(Some(IO_LIMIT)).unwrap();
    stream.set_write_timeout(Some(IO_LIMIT)).unwrap();
}

fn limit_std(stream: &std::net::TcpStream) {
    stream.set_read_timeout(Some(IO_LIMIT)).unwrap();
    stream.set_write_timeout(Some(IO_LIMIT)).unwrap();
}

/// A connected pair: a litestd stream and the std stream at the other end.
fn lite_std_pair() -> (TcpStream, std::net::TcpStream) {
    let (listener, addr) = std_listener();
    let lite = TcpStream::connect(addr).unwrap();
    let (std, peer) = listener.accept().unwrap();
    assert_eq!(peer, lite.local_addr().unwrap());
    limit_lite(&lite);
    limit_std(&std);
    (lite, std)
}

#[test]
fn litestd_client_talks_to_std_server() {
    timed(|| {
        let (listener, addr) = std_listener();
        let server = thread::spawn(move || {
            let (mut stream, peer) = listener.accept().unwrap();
            limit_std(&stream);
            let mut request = Vec::new();
            stream.read_to_end(&mut request).unwrap();
            stream.write_all(&request.repeat(2)).unwrap();
            (peer, request)
        });
        let mut client = TcpStream::connect(addr).unwrap();
        limit_lite(&client);
        assert_eq!(client.peer_addr().unwrap(), addr);
        let payload: Vec<u8> =
            (0..200_000u32).map(|i| i.to_le_bytes()[0]).collect();
        client.write_all(&payload).unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        let mut reply = Vec::new();
        client.read_to_end(&mut reply).unwrap();
        let (peer, request) = server.join().unwrap();
        assert_eq!(peer, client.local_addr().unwrap());
        assert_eq!(request, payload);
        assert_eq!(reply, payload.repeat(2));
    });
}

#[test]
fn std_client_talks_to_litestd_server() {
    timed(|| {
        let (listener, addr) = lite_listener();
        let client = thread::spawn(move || {
            let mut stream = std::net::TcpStream::connect(addr).unwrap();
            limit_std(&stream);
            stream.write_all(b"ping").unwrap();
            stream.shutdown(std::net::Shutdown::Write).unwrap();
            let mut reply = String::new();
            stream.read_to_string(&mut reply).unwrap();
            (stream.local_addr().unwrap(), reply)
        });
        let (mut stream, peer) = listener.accept().unwrap();
        limit_lite(&stream);
        let mut request = String::new();
        stream.read_to_string(&mut request).unwrap();
        assert_eq!(request, "ping");
        stream.write_all(b"pong").unwrap();
        drop(stream);
        let (client_addr, reply) = client.join().unwrap();
        assert_eq!(peer, client_addr);
        assert_eq!(reply, "pong");
    });
}

#[test]
fn ipv6_loopback_works_both_ways() {
    timed(|| {
        let Ok(listener) =
            std::net::TcpListener::bind((Ipv6Addr::LOCALHOST, 0))
        else {
            return; // No IPv6 on this host.
        };
        let addr = listener.local_addr().unwrap();
        let mut lite = TcpStream::connect(addr).unwrap();
        let (mut std, peer) = listener.accept().unwrap();
        assert_eq!(peer, lite.local_addr().unwrap());
        assert!(lite.peer_addr().unwrap().is_ipv6());
        lite.write_all(b"six").unwrap();
        let mut buf = [0; 3];
        std.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"six");

        let (listener, addr) = {
            let l = TcpListener::bind("[::1]:0").unwrap();
            let a = l.local_addr().unwrap();
            (l, a)
        };
        let std = std::net::TcpStream::connect(addr).unwrap();
        let (_lite, peer) = listener.accept().unwrap();
        assert_eq!(peer, std.local_addr().unwrap());
    });
}

#[test]
fn vectored_io_moves_every_buffer() {
    timed(|| {
        let (mut lite, mut std) = lite_std_pair();
        let parts: [&[u8]; 3] = [b"one ", b"two ", b"three"];
        let mut bufs = parts.map(IoSlice::new);
        let mut slices = &mut bufs[..];
        while !slices.is_empty() {
            let n = lite.write_vectored(slices).unwrap();
            assert_ne!(n, 0);
            IoSlice::advance_slices(&mut slices, n);
        }
        let mut got = [0; 13];
        std.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"one two three");

        std.write_all(&got).unwrap();
        let (mut a, mut b, mut c) = ([0; 4], [0; 4], [0; 5]);
        let mut bufs = [
            IoSliceMut::new(&mut a),
            IoSliceMut::new(&mut b),
            IoSliceMut::new(&mut c),
        ];
        let mut slices = &mut bufs[..];
        while !slices.is_empty() {
            let n = lite.read_vectored(slices).unwrap();
            assert_ne!(n, 0);
            IoSliceMut::advance_slices(&mut slices, n);
        }
        assert_eq!((&a, &b, &c), (b"one ", b"two ", b"three"));
    });
}

#[test]
fn reads_and_writes_through_shared_references() {
    timed(|| {
        let (lite, mut std) = lite_std_pair();
        (&lite).write_all(b"shared").unwrap();
        (&lite).flush().unwrap();
        let mut buf = [0; 6];
        std.read_exact(&mut buf).unwrap();
        std.write_all(b"back").unwrap();
        let mut got = [0; 4];
        (&lite).read_exact(&mut got).unwrap();
        assert_eq!(&got, b"back");
    });
}

/// `read_to_end` appends to what the vector holds, through a shared
/// reference too, and `read_to_string` rejects bad UTF-8 as std does.
#[test]
fn reading_to_the_end_keeps_the_buffer_and_checks_utf8() {
    timed(|| {
        let (lite, mut std) = lite_std_pair();
        let data: Vec<u8> =
            (0..70_000u32).map(|i| i.to_le_bytes()[1]).collect();
        std.write_all(&data).unwrap();
        std.shutdown(std::net::Shutdown::Write).unwrap();
        let mut got = b"kept".to_vec();
        assert_eq!((&lite).read_to_end(&mut got).unwrap(), data.len());
        assert_eq!((&got[..4], &got[4..]), (&b"kept"[..], &data[..]));

        let (mut lite, mut peer) = lite_std_pair();
        let (mut std, mut std_peer) = std_pair();
        for sender in [&mut peer, &mut std_peer] {
            sender.write_all(b"ok \xff").unwrap();
            sender.shutdown(std::net::Shutdown::Write).unwrap();
        }
        let (mut a, mut b) = (String::from("x"), String::from("x"));
        let lite = lite.read_to_string(&mut a).unwrap_err();
        let std = std.read_to_string(&mut b).unwrap_err();
        assert_eq!(
            (a.as_str(), lite.to_string()),
            (b.as_str(), std.to_string())
        );
    });
}

#[test]
fn connection_refused_matches_std() {
    timed(|| {
        let addr = closed_addr();
        same_error(
            &TcpStream::connect(addr).unwrap_err(),
            &std::net::TcpStream::connect(addr).unwrap_err(),
        );
        let timeout = Duration::from_secs(5);
        same_error(
            &TcpStream::connect_timeout(&addr, timeout).unwrap_err(),
            &std::net::TcpStream::connect_timeout(&addr, timeout).unwrap_err(),
        );
    });
}

/// `connect` tries each address in turn and reports the last error.
#[test]
fn connect_tries_every_address() {
    timed(|| {
        let (listener, open) = std_listener();
        let closed = closed_addr();
        let addrs = [closed, open];
        let lite = TcpStream::connect(&addrs[..]).unwrap();
        assert_eq!(lite.peer_addr().unwrap(), open);
        drop(listener.accept().unwrap());
        let addrs = [open, closed];
        drop(listener);
        same_error(
            &TcpStream::connect(&addrs[..]).unwrap_err(),
            &std::net::TcpStream::connect(&addrs[..]).unwrap_err(),
        );
    });
}

/// A listener whose accept queue is full: Linux drops further connection
/// requests, so connecting to it hangs until a timeout. macOS completes
/// them anyway, and Windows refuses them.
#[cfg(unix)]
struct FullListener {
    _listener: std::net::TcpListener,
    _queued: std::net::TcpStream,
    addr: SocketAddr,
}

#[cfg(unix)]
impl FullListener {
    fn new() -> Self {
        use std::os::fd::{AsRawFd, FromRawFd};
        // SAFETY: plain socket calls on a descriptor this function owns.
        let listener = unsafe {
            let fd = net_support::raw_tcp_socket();
            let listener = std::net::TcpListener::from_raw_fd(fd);
            let addr = net_support::loopback_v4(0);
            let len = libc::socklen_t::try_from(size_of::<libc::sockaddr_in>())
                .unwrap();
            assert_eq!(libc::bind(fd, (&raw const addr).cast(), len), 0);
            // A backlog of zero holds one connection.
            assert_eq!(libc::listen(fd, 0), 0);
            listener
        };
        let addr = listener.local_addr().unwrap();
        let queued = std::net::TcpStream::connect(addr).unwrap();
        // The listener is readable once the connection is in its queue.
        net_support::wait_for(listener.as_raw_fd(), libc::POLLIN);
        Self {
            _listener: listener,
            _queued: queued,
            addr,
        }
    }
}

// Connecting to a `FullListener` hangs on Linux only.
#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn connect_timeout_expires_like_std() {
    timed(|| {
        let full = FullListener::new();
        let timeout = Duration::from_millis(150);
        let start = Instant::now();
        let lite = TcpStream::connect_timeout(&full.addr, timeout).unwrap_err();
        let elapsed = start.elapsed();
        assert!(elapsed >= timeout, "returned after {elapsed:?}");
        assert!(elapsed < Duration::from_secs(10), "took {elapsed:?}");
        let std = std::net::TcpStream::connect_timeout(&full.addr, timeout)
            .unwrap_err();
        same_error(&lite, &std);
    });
}

#[cfg(unix)]
#[test]
fn connect_timeout_rejects_zero_like_std() {
    timed(|| {
        let full = FullListener::new();
        same_error(
            &TcpStream::connect_timeout(&full.addr, Duration::ZERO)
                .unwrap_err(),
            &std::net::TcpStream::connect_timeout(&full.addr, Duration::ZERO)
                .unwrap_err(),
        );
        // Below a millisecond still waits, and still times out where
        // connecting hangs.
        if cfg!(any(target_os = "linux", target_os = "android")) {
            let tiny = Duration::from_nanos(1);
            same_error(
                &TcpStream::connect_timeout(&full.addr, tiny).unwrap_err(),
                &std::net::TcpStream::connect_timeout(&full.addr, tiny)
                    .unwrap_err(),
            );
        }
    });
}

#[test]
fn connect_timeout_connects_in_blocking_mode() {
    timed(|| {
        let (listener, addr) = std_listener();
        let mut lite =
            TcpStream::connect_timeout(&addr, Duration::from_secs(5)).unwrap();
        let (mut std, _) = listener.accept().unwrap();
        // The stream is blocking again: this read waits for the data.
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            std.write_all(b"late").unwrap();
            std
        });
        let mut buf = [0; 4];
        lite.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"late");
        drop(writer.join().unwrap());
        let mut v6 = SocketAddr::from((Ipv6Addr::LOCALHOST, addr.port()));
        v6.set_port(closed_addr().port());
        let lite =
            TcpStream::connect_timeout(&v6, Duration::from_secs(5)).map(drop);
        let std =
            std::net::TcpStream::connect_timeout(&v6, Duration::from_secs(5))
                .map(drop);
        same_result(lite, std);
    });
}

#[test]
fn read_timeout_expires_like_std() {
    timed(|| {
        let (lite, std) = lite_std_pair();
        let timeout = Duration::from_millis(100);
        lite.set_read_timeout(Some(timeout)).unwrap();
        std.set_read_timeout(Some(timeout)).unwrap();
        let start = Instant::now();
        let lite_err = (&lite).read(&mut [0; 8]).unwrap_err();
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(90), "after {elapsed:?}");
        let std_err = (&std).read(&mut [0; 8]).unwrap_err();
        same_error(&lite_err, &std_err);
        same_error(
            &lite.peek(&mut [0; 8]).unwrap_err(),
            &std.peek(&mut [0; 8]).unwrap_err(),
        );
    });
}

/// Writes to `write` until it fails, which happens once nobody reads and
/// the buffers are full, after the write timeout.
fn fill<E>(mut write: impl FnMut(&[u8]) -> Result<usize, E>) -> E {
    let chunk = vec![7u8; 1 << 20];
    // Ends when a write fails; the buffers are finite, so one does.
    loop {
        if let Err(e) = write(&chunk) {
            return e;
        }
    }
}

#[test]
fn write_timeout_expires_like_std() {
    timed(|| {
        let timeout = Duration::from_millis(100);
        let (lite, _lite_peer) = lite_std_pair();
        lite.set_write_timeout(Some(timeout)).unwrap();
        let lite_err = fill(|buf| (&lite).write(buf));
        let (std, _std_peer) = {
            let (listener, addr) = std_listener();
            let std = std::net::TcpStream::connect(addr).unwrap();
            (std, listener.accept().unwrap().0)
        };
        std.set_write_timeout(Some(timeout)).unwrap();
        let std_err = fill(|buf| (&std).write(buf));
        same_error(&lite_err, &std_err);
    });
}

#[test]
fn timeouts_round_trip_like_std() {
    timed(|| {
        let (lite, std) = lite_std_pair();
        for dur in TIMEOUTS {
            same_result(lite.set_read_timeout(dur), std.set_read_timeout(dur));
            same_result(
                lite.set_write_timeout(dur),
                std.set_write_timeout(dur),
            );
            same_timeout(lite.read_timeout(), std.read_timeout());
            same_timeout(lite.write_timeout(), std.write_timeout());
        }
        let zero = Some(Duration::ZERO);
        same_result(lite.set_read_timeout(zero), std.set_read_timeout(zero));
        same_result(lite.set_write_timeout(zero), std.set_write_timeout(zero));
    });
}

/// litestd reads the timeouts that std sets as the kernel holds them.
#[cfg(unix)]
#[test]
fn timeouts_that_std_sets_read_back_through_litestd() {
    use std::os::fd::IntoRawFd as _;

    use litestd::os::fd::FromRawFd as _;
    use net_support::lite_reading_of;
    timed(|| {
        let (_lite, std) = lite_std_pair();
        let fd = std.try_clone().unwrap().into_raw_fd();
        // SAFETY: `fd` is a new descriptor for the socket, owned by nothing
        // else.
        let view = unsafe { TcpStream::from_raw_fd(fd) };
        for dur in TIMEOUTS {
            std.set_read_timeout(dur).unwrap();
            std.set_write_timeout(dur).unwrap();
            let read = lite_reading_of(std.read_timeout().unwrap());
            assert_eq!(view.read_timeout().unwrap(), read, "{dur:?}");
            let write = lite_reading_of(std.write_timeout().unwrap());
            assert_eq!(view.write_timeout().unwrap(), write, "{dur:?}");
        }
    });
}

#[test]
fn nonblocking_mode_would_block_like_std() {
    timed(|| {
        let (lite, std) = lite_std_pair();
        lite.set_nonblocking(true).unwrap();
        std.set_nonblocking(true).unwrap();
        same_error(
            &(&lite).read(&mut [0; 4]).unwrap_err(),
            &(&std).read(&mut [0; 4]).unwrap_err(),
        );
        same_error(
            &lite.peek(&mut [0; 4]).unwrap_err(),
            &std.peek(&mut [0; 4]).unwrap_err(),
        );
        lite.set_nonblocking(false).unwrap();

        let (listener, _) = lite_listener();
        let (std_listener, _) = std_listener();
        listener.set_nonblocking(true).unwrap();
        std_listener.set_nonblocking(true).unwrap();
        same_error(
            &listener.accept().unwrap_err(),
            &std_listener.accept().unwrap_err(),
        );
        same_error(
            &listener.incoming().next().unwrap().unwrap_err(),
            &std_listener.incoming().next().unwrap().unwrap_err(),
        );
    });
}

#[test]
fn peek_leaves_the_data_queued() {
    timed(|| {
        let (lite, mut std) = lite_std_pair();
        std.write_all(b"peekaboo").unwrap();
        let mut buf = [0; 8];
        let mut n = 0;
        while n < 8 {
            n = lite.peek(&mut buf).unwrap();
        }
        assert_eq!(&buf, b"peekaboo");
        let mut again = [0; 8];
        assert_eq!(lite.peek(&mut again).unwrap(), 8);
        let mut read = [0; 8];
        (&lite).read_exact(&mut read).unwrap();
        assert_eq!(&read, b"peekaboo");
    });
}

/// A new TCP socket that is neither bound nor connected.
#[cfg(unix)]
fn fresh_tcp_socket() -> litestd::os::fd::OwnedFd {
    let fd = net_support::raw_tcp_socket();
    // SAFETY: `socket` returned a new descriptor that nothing else owns.
    unsafe { litestd::os::fd::FromRawFd::from_raw_fd(fd) }
}

#[cfg(unix)]
fn fresh_std_tcp_socket() -> std::net::TcpStream {
    use std::os::fd::FromRawFd;
    let fd = litestd::os::fd::IntoRawFd::into_raw_fd(fresh_tcp_socket());
    // SAFETY: `fd` was just released by its owner.
    unsafe { std::net::TcpStream::from_raw_fd(fd) }
}

/// A connected pair of std streams, to compare litestd's with.
fn std_pair() -> (std::net::TcpStream, std::net::TcpStream) {
    let (listener, addr) = std_listener();
    let client = std::net::TcpStream::connect(addr).unwrap();
    let (server, _) = listener.accept().unwrap();
    limit_std(&client);
    limit_std(&server);
    (client, server)
}

#[test]
fn shutdown_behaves_like_std() {
    use std::net::Shutdown as S;
    timed(|| {
        let (lite, mut lite_peer) = lite_std_pair();
        let (std, mut std_peer) = std_pair();
        same_result(lite.shutdown(Shutdown::Write), std.shutdown(S::Write));
        let mut rest = Vec::new();
        lite_peer.read_to_end(&mut rest).unwrap();
        assert_eq!(rest, b"");
        std_peer.read_to_end(&mut rest).unwrap();
        same_result((&lite).write(b"x"), (&std).write(b"x"));
        same_result(lite.shutdown(Shutdown::Write), std.shutdown(S::Write));
        same_result(lite.shutdown(Shutdown::Read), std.shutdown(S::Read));
        same_result((&lite).read(&mut [0; 4]), (&std).read(&mut [0; 4]));
        same_result(lite.shutdown(Shutdown::Both), std.shutdown(S::Both));

        let (lite, lite_peer) = lite_std_pair();
        lite_peer.shutdown(S::Both).unwrap();
        assert_eq!((&lite).read(&mut [0; 4]).unwrap(), 0);
    });
}

/// A socket that is not connected fails alike.
#[cfg(unix)]
#[test]
fn unconnected_sockets_fail_like_std() {
    timed(|| {
        let lite = TcpStream::from(fresh_tcp_socket());
        let std = fresh_std_tcp_socket();
        for (a, b) in [
            (Shutdown::Read, std::net::Shutdown::Read),
            (Shutdown::Write, std::net::Shutdown::Write),
            (Shutdown::Both, std::net::Shutdown::Both),
        ] {
            same_result(lite.shutdown(a), std.shutdown(b));
        }
        same_result(lite.peer_addr(), std.peer_addr());
        same_result(lite.local_addr(), std.local_addr());
        same_result((&lite).read(&mut [0; 4]), (&std).read(&mut [0; 4]));
        same_result((&lite).write(b"x"), (&std).write(b"x"));
    });
}

#[test]
fn a_closed_peer_fails_writes_with_broken_pipe() {
    timed(|| {
        let (lite, std) = lite_std_pair();
        drop(std);
        // The first write may still succeed; the peer answers it with a
        // reset, after which writes fail.
        let err = loop {
            if let Err(e) = (&lite).write(b"after close") {
                break e;
            }
            thread::sleep(Duration::from_millis(5));
        };
        assert!(
            matches!(
                err.kind(),
                litestd::io::ErrorKind::BrokenPipe
                    | litestd::io::ErrorKind::ConnectionReset
                    | litestd::io::ErrorKind::ConnectionAborted
            ),
            "{err:?}"
        );
    });
}

/// In a process where `SIGPIPE` has its default action, as in a litestd
/// program, writing to a closed connection must fail rather than raise the
/// signal, which would kill the process.
#[cfg(unix)]
#[test]
fn writes_do_not_raise_sigpipe() {
    if net_support::is_child() {
        net_support::default_sigpipe();
        let (lite, std) = lite_std_pair();
        drop(std);
        for _ in 0..1000 {
            if (&lite).write(b"x").is_err() {
                let bufs = [IoSlice::new(b"y")];
                assert!((&lite).write_vectored(&bufs).is_err());
                println!("survived");
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("writes kept succeeding");
    }
    let out = net_support::run_child("writes_do_not_raise_sigpipe");
    assert!(out.status.success(), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("survived"),
        "{out:?}"
    );
}

#[test]
fn take_error_matches_std() {
    timed(|| {
        let (lite, std) = lite_std_pair();
        assert!(lite.take_error().unwrap().is_none());
        assert!(std.take_error().unwrap().is_none());
        let (listener, _) = lite_listener();
        assert!(listener.take_error().unwrap().is_none());
    });
}

/// A nonblocking connection to a closed port leaves its error in
/// `SO_ERROR`.
#[cfg(unix)]
#[test]
fn take_error_reports_a_refused_connection() {
    timed(|| {
        let addr = closed_addr();
        let pending = || -> i32 {
            let fd =
                litestd::os::fd::IntoRawFd::into_raw_fd(fresh_tcp_socket());
            let sa = net_support::loopback_v4(addr.port());
            let len = libc::socklen_t::try_from(size_of::<libc::sockaddr_in>())
                .unwrap();
            // SAFETY: `ioctl` reads the `int` it is given, and `sa` is
            // valid for reads of `len` bytes.
            unsafe {
                let mut on = 1;
                assert_eq!(libc::ioctl(fd, libc::FIONBIO, &raw mut on), 0);
                let _ = libc::connect(fd, (&raw const sa).cast(), len);
            }
            net_support::wait_for(fd, libc::POLLOUT);
            fd
        };
        // SAFETY: `pending` returns a descriptor that nothing else owns.
        let lite = unsafe {
            <TcpStream as litestd::os::fd::FromRawFd>::from_raw_fd(pending())
        };
        // SAFETY: as above.
        let std = unsafe {
            <std::net::TcpStream as std::os::fd::FromRawFd>::from_raw_fd(
                pending(),
            )
        };
        let lite_err = lite.take_error().unwrap().unwrap();
        let std_err = std.take_error().unwrap().unwrap();
        same_error(&lite_err, &std_err);
        assert!(lite.take_error().unwrap().is_none());
    });
}

#[test]
fn options_round_trip_like_std() {
    timed(|| {
        let (lite, std) = lite_std_pair();
        same_result(lite.nodelay(), std.nodelay());
        for on in [true, false, true] {
            same_result(lite.set_nodelay(on), std.set_nodelay(on));
            same_result(lite.nodelay(), std.nodelay());
        }
        same_result(lite.ttl(), std.ttl());
        for ttl in [1, 64, 255, 256, 0, u32::MAX, 17] {
            same_result(lite.set_ttl(ttl), std.set_ttl(ttl));
            same_result(lite.ttl(), std.ttl());
        }
        let (listener, _) = lite_listener();
        let (std_listener, _) = std_listener();
        for ttl in [1, 300, 99] {
            same_result(listener.set_ttl(ttl), std_listener.set_ttl(ttl));
            same_result(listener.ttl(), std_listener.ttl());
        }
    });
}

#[test]
fn try_clone_shares_the_stream() {
    timed(|| {
        let (lite, mut std) = lite_std_pair();
        let clone = lite.try_clone().unwrap();
        (&clone).write_all(b"from clone").unwrap();
        drop(clone);
        (&lite).write_all(b"!").unwrap();
        let mut buf = [0; 11];
        std.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"from clone!");
        let (listener, addr) = lite_listener();
        let listener2 = listener.try_clone().unwrap();
        drop(listener);
        let _client = std::net::TcpStream::connect(addr).unwrap();
        let (_s, peer) = listener2.accept().unwrap();
        assert_eq!(peer.ip(), Ipv4Addr::LOCALHOST);
    });
}

#[test]
fn incoming_yields_connections() {
    timed(|| {
        let (listener, addr) = lite_listener();
        let clients: Vec<_> = (0..3)
            .map(|_| std::net::TcpStream::connect(addr).unwrap())
            .collect();
        let mut incoming = listener.incoming();
        for client in &clients {
            let stream = incoming.next().unwrap().unwrap();
            assert_eq!(
                stream.peer_addr().unwrap(),
                client.local_addr().unwrap()
            );
        }
        let _: &dyn core::iter::FusedIterator<Item = _> = &incoming;
        assert!(
            format!("{incoming:?}")
                .starts_with("Incoming { listener: TcpListener {")
        );
    });
}

#[test]
fn listeners_bind_like_std() {
    timed(|| {
        let (_listener, addr) = std_listener();
        same_error(
            &TcpListener::bind(addr).unwrap_err(),
            &std::net::TcpListener::bind(addr).unwrap_err(),
        );
        // `SO_REUSEADDR`: a port whose listener just closed binds again.
        let (listener, addr) = lite_listener();
        let client = std::net::TcpStream::connect(addr).unwrap();
        let (server, _) = listener.accept().unwrap();
        drop(server);
        drop(client);
        drop(listener);
        TcpListener::bind(addr).unwrap();
        // Binding to an address this host does not have fails alike.
        let foreign = SocketAddr::from(([192, 0, 2, 1], 0));
        same_error(
            &TcpListener::bind(foreign).unwrap_err(),
            &std::net::TcpListener::bind(foreign).unwrap_err(),
        );
    });
}

/// With the same descriptor underneath, `Debug` prints what std prints.
#[cfg(unix)]
#[test]
fn debug_output_matches_std() {
    use core::mem::ManuallyDrop;
    use std::os::fd::AsRawFd;
    timed(|| {
        let (std_listener, addr) = std_listener();
        let std_stream = std::net::TcpStream::connect(addr).unwrap();
        let std_udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        // SAFETY: the litestd values borrow the std values' descriptors and
        // are never dropped, so each descriptor is closed once, by std.
        let (lite_listener, lite_stream, lite_udp) = unsafe {
            (
                ManuallyDrop::new(<TcpListener as litestd::os::fd::FromRawFd>::from_raw_fd(std_listener.as_raw_fd())),
                ManuallyDrop::new(<TcpStream as litestd::os::fd::FromRawFd>::from_raw_fd(std_stream.as_raw_fd())),
                ManuallyDrop::new(<litestd::net::UdpSocket as litestd::os::fd::FromRawFd>::from_raw_fd(std_udp.as_raw_fd())),
            )
        };
        assert_eq!(
            format!("{:?}", *lite_listener),
            format!("{std_listener:?}")
        );
        assert_eq!(format!("{:?}", *lite_stream), format!("{std_stream:?}"));
        assert_eq!(format!("{:?}", *lite_udp), format!("{std_udp:?}"));
        assert_eq!(format!("{:#?}", *lite_stream), format!("{std_stream:#?}"));
        // A socket without addresses leaves them out.
        let lite = TcpStream::from(fresh_tcp_socket());
        let std = fresh_std_tcp_socket();
        let lite_fd = litestd::os::fd::AsRawFd::as_raw_fd(&lite);
        let expected = format!("{std:?}").replace(
            &format!("fd: {}", std.as_raw_fd()),
            &format!("fd: {lite_fd}"),
        );
        assert_eq!(format!("{lite:?}"), expected);
    });
}

#[cfg(unix)]
#[test]
fn descriptor_traits_convert_like_std() {
    use litestd::os::fd::{AsFd, AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
    timed(|| {
        let (lite, mut std) = lite_std_pair();
        let raw = lite.as_raw_fd();
        assert_eq!(lite.as_fd().as_raw_fd(), raw);
        let owned = OwnedFd::from(lite);
        assert_eq!(owned.as_raw_fd(), raw);
        let lite = TcpStream::from(owned);
        let raw2 = lite.into_raw_fd();
        assert_eq!(raw2, raw);
        // SAFETY: `raw2` was just released by its owner.
        let mut lite = unsafe { TcpStream::from_raw_fd(raw2) };
        lite.write_all(b"fd").unwrap();
        let mut buf = [0; 2];
        std.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"fd");

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let listener = TcpListener::from(OwnedFd::from(listener));
        assert_eq!(listener.local_addr().unwrap(), addr);
        // SAFETY: the descriptor passes from one owner to the next.
        let listener =
            unsafe { TcpListener::from_raw_fd(listener.into_raw_fd()) };
        assert_eq!(listener.as_fd().as_raw_fd(), listener.as_raw_fd());
    });
}

#[test]
fn types_have_std_s_auto_traits() {
    use core::panic::{RefUnwindSafe, UnwindSafe};
    const fn check<T: Send + Sync + Unpin + UnwindSafe + RefUnwindSafe>() {}
    check::<TcpStream>();
    check::<TcpListener>();
    check::<litestd::net::Incoming<'static>>();
    check::<litestd::net::UdpSocket>();
    check::<Shutdown>();
}
