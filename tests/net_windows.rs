//! Windows-specific behavior of `litestd::net` and the socket half of
//! `os::windows::io`, compared with std on loopback sockets: owned and raw
//! sockets, Winsock error codes and messages, timeouts, `connect_timeout`,
//! vectored I/O, handle inheritance and name resolution.

#![cfg(all(windows, feature = "net"))]
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    reason = "helpers, like tests, fail on errors"
)]

use core::{net::SocketAddr, ptr, time::Duration};
use std::{
    io::{self, Read as _, Write as _},
    net as snet,
    os::windows::io::{
        AsRawSocket as _, AsSocket as _, FromRawSocket as _, IntoRawSocket as _,
    },
    sync::Once,
    thread,
    time::Instant,
};

use litestd::{
    io::{self as lio, IoSlice, IoSliceMut, Read as _, Write as _},
    net::{self as lnet, Shutdown, ToSocketAddrs as _},
    os::windows::io::{
        AsRawSocket as _, AsSocket as _, BorrowedSocket, FromRawSocket as _,
        IntoRawSocket as _, OwnedSocket, RawSocket,
    },
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetHandleInformation, GetLastError, HANDLE_FLAG_INHERIT,
        WAIT_TIMEOUT,
    },
    Networking::WinSock::{
        AF_INET, IN_ADDR, IN_ADDR_0, INVALID_SOCKET, IPPROTO_TCP, SOCK_STREAM,
        SOCKADDR_IN, WSA_FLAG_OVERLAPPED, WSADATA, WSASocketW, WSAStartup,
        bind, listen,
    },
    System::IO::{CreateIoCompletionPort, GetQueuedCompletionStatus},
};

/// How long any test may block on a socket before it fails.
const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// Ends the test binary if the tests run far longer than they should, so
/// that a call that blocks forever shows as a failure.
fn watchdog() {
    static START: Once = Once::new();
    START.call_once(|| {
        thread::spawn(|| {
            thread::sleep(Duration::from_secs(120));
            eprintln!("net_windows: the tests took over two minutes");
            std::process::exit(101);
        });
    });
}

/// Asserts that litestd's error is std's: code, kind and message.
#[track_caller]
fn same_error(lite: &lio::Error, std: &io::Error) {
    assert_eq!(lite.raw_os_error(), std.raw_os_error(), "{lite} vs {std}");
    assert_eq!(format!("{:?}", lite.kind()), format!("{:?}", std.kind()));
    assert_eq!(lite.to_string(), std.to_string());
    assert_eq!(format!("{lite:?}"), format!("{std:?}"));
}

/// Asserts that the results agree: equal values, or the same error.
#[track_caller]
fn same<T: PartialEq + core::fmt::Debug>(
    lite: lio::Result<T>,
    std: io::Result<T>,
) {
    match (lite, std) {
        (Ok(lite), Ok(std)) => assert_eq!(lite, std),
        (Err(lite), Err(std)) => same_error(&lite, &std),
        (lite, std) => panic!("litestd {lite:?}, std {std:?}"),
    }
}

/// Whether child processes would inherit `socket`.
fn inheritable(socket: RawSocket) -> bool {
    let mut flags = 0;
    let handle = ptr::without_provenance_mut(usize::try_from(socket).unwrap());
    // SAFETY: the caller's socket is open; `flags` is valid for writes.
    let ok = unsafe { GetHandleInformation(handle, &raw mut flags) };
    assert_ne!(ok, 0, "{}", io::Error::last_os_error());
    flags & HANDLE_FLAG_INHERIT != 0
}

/// A connected pair: a litestd stream and the std stream it accepted, both
/// with timeouts.
fn lite_pair() -> (lnet::TcpStream, snet::TcpStream) {
    let listener = snet::TcpListener::bind("127.0.0.1:0").unwrap();
    let lite =
        lnet::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (std, _) = listener.accept().unwrap();
    lite.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    lite.set_write_timeout(Some(IO_TIMEOUT)).unwrap();
    std.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    std.set_write_timeout(Some(IO_TIMEOUT)).unwrap();
    (lite, std)
}

/// A connected pair of std streams, with timeouts.
fn std_pair() -> (snet::TcpStream, snet::TcpStream) {
    let listener = snet::TcpListener::bind("127.0.0.1:0").unwrap();
    let client =
        snet::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server, _) = listener.accept().unwrap();
    for s in [&client, &server] {
        s.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
        s.set_write_timeout(Some(IO_TIMEOUT)).unwrap();
    }
    (client, server)
}

/// Reads exactly `len` bytes from a std stream.
fn read_exact(stream: &mut snet::TcpStream, len: usize) -> Vec<u8> {
    let mut buf = vec![0; len];
    stream.read_exact(&mut buf).unwrap();
    buf
}

#[test]
fn owned_socket_try_clone() {
    watchdog();
    let listener = lnet::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let owned = OwnedSocket::from(listener);
    let clone = owned.try_clone().unwrap();
    let borrowed_clone = owned.as_socket().try_clone_to_owned().unwrap();
    assert_ne!(clone.as_raw_socket(), owned.as_raw_socket());
    assert_ne!(borrowed_clone.as_raw_socket(), owned.as_raw_socket());
    assert!(!inheritable(clone.as_raw_socket()));
    assert!(!inheritable(borrowed_clone.as_raw_socket()));
    drop(owned);
    drop(borrowed_clone);

    // The clone listens on the same address after the original is closed.
    let listener = lnet::TcpListener::from(clone);
    assert_eq!(listener.local_addr().unwrap(), addr);
    let mut client = snet::TcpStream::connect(addr).unwrap();
    client.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    let (mut accepted, peer) = listener.accept().unwrap();
    assert_eq!(peer, client.local_addr().unwrap());
    accepted.write_all(b"cloned").unwrap();
    assert_eq!(read_exact(&mut client, 6), b"cloned");

    // Streams and UDP sockets clone too.
    let stream = accepted.try_clone().unwrap();
    assert_ne!(stream.as_raw_socket(), accepted.as_raw_socket());
    drop(accepted);
    (&stream).write_all(b"again").unwrap();
    assert_eq!(read_exact(&mut client, 5), b"again");
    let udp = lnet::UdpSocket::bind("127.0.0.1:0").unwrap();
    let udp_clone = udp.try_clone().unwrap();
    assert_eq!(udp_clone.local_addr().unwrap(), udp.local_addr().unwrap());
}

#[test]
fn raw_socket_round_trips() {
    watchdog();
    // litestd to raw and back.
    let listener = lnet::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let raw = listener.as_raw_socket();
    assert_eq!(listener.as_socket().as_raw_socket(), raw);
    assert_eq!(listener.into_raw_socket(), raw);
    // SAFETY: `raw` was given up by the listener above.
    let listener = unsafe { lnet::TcpListener::from_raw_socket(raw) };
    assert_eq!(listener.local_addr().unwrap(), addr);

    // litestd to std: std accepts on litestd's listener.
    // SAFETY: `into_raw_socket` hands over ownership.
    let std_listener = unsafe {
        snet::TcpListener::from_raw_socket(listener.into_raw_socket())
    };
    let lite = lnet::TcpStream::connect(addr).unwrap();
    let (std_server, peer) = std_listener.accept().unwrap();
    assert_eq!(peer, lite.local_addr().unwrap());

    // std to litestd: litestd reads from std's stream.
    // SAFETY: as above.
    let mut lite_server = unsafe {
        lnet::TcpStream::from_raw_socket(std_server.into_raw_socket())
    };
    lite_server.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    (&lite).write_all(b"raw").unwrap();
    let mut buf = [0; 3];
    lite_server.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"raw");

    // Through `OwnedSocket`, for every type.
    let owned = OwnedSocket::from(lite_server);
    let raw = owned.as_raw_socket();
    let stream = lnet::TcpStream::from(owned);
    assert_eq!(stream.as_raw_socket(), raw);
    let udp = lnet::UdpSocket::bind("127.0.0.1:0").unwrap();
    let udp_addr = udp.local_addr().unwrap();
    let raw = udp.as_raw_socket();
    let udp = lnet::UdpSocket::from(OwnedSocket::from(udp));
    assert_eq!(
        (udp.as_raw_socket(), udp.local_addr().unwrap()),
        (raw, udp_addr)
    );
    // SAFETY: as above.
    let owned = unsafe { OwnedSocket::from_raw_socket(udp.into_raw_socket()) };
    assert_eq!(owned.into_raw_socket(), raw);
    // SAFETY: `raw` was given up above; this takes it back to close it.
    drop(unsafe { OwnedSocket::from_raw_socket(raw) });
}

#[test]
fn debug_matches_std() {
    watchdog();
    let (client, server) = std_pair();
    let listener = snet::TcpListener::bind("127.0.0.1:0").unwrap();
    let udp = snet::UdpSocket::bind("127.0.0.1:0").unwrap();
    let std_debug = [
        format!("{client:?}"),
        format!("{listener:?}"),
        format!("{udp:?}"),
    ];
    // SAFETY: each `from_raw_socket` takes over what `into_raw_socket`
    // gave up, and each socket goes back to std the same way.
    let lite_debug = unsafe {
        let client = lnet::TcpStream::from_raw_socket(client.into_raw_socket());
        let listener =
            lnet::TcpListener::from_raw_socket(listener.into_raw_socket());
        let udp = lnet::UdpSocket::from_raw_socket(udp.into_raw_socket());
        let debug = [
            format!("{client:?}"),
            format!("{listener:?}"),
            format!("{udp:?}"),
        ];
        drop(snet::TcpStream::from_raw_socket(client.into_raw_socket()));
        drop(snet::TcpListener::from_raw_socket(
            listener.into_raw_socket(),
        ));
        drop(snet::UdpSocket::from_raw_socket(udp.into_raw_socket()));
        debug
    };
    assert_eq!(lite_debug, std_debug);
    assert!(std_debug[0].contains("socket: "), "{}", std_debug[0]);

    let raw = server.as_raw_socket();
    // SAFETY: `server` keeps the socket open while the borrow lives.
    let lite = unsafe { BorrowedSocket::borrow_raw(raw) };
    assert_eq!(format!("{lite:?}"), format!("{:?}", server.as_socket()));
    let std_owned = std::os::windows::io::OwnedSocket::from(server);
    let std_debug = format!("{std_owned:?}");
    // SAFETY: as above.
    let lite_owned =
        unsafe { OwnedSocket::from_raw_socket(std_owned.into_raw_socket()) };
    assert_eq!(format!("{lite_owned:?}"), std_debug);
}

#[test]
fn wsa_errors_match_std() {
    watchdog();
    let codes = (10000..=10120).chain(11000..=11035);
    for code in codes {
        let lite = lio::Error::from_raw_os_error(code);
        let std = io::Error::from_raw_os_error(code);
        assert_eq!(lite.to_string(), std.to_string(), "code {code}");
        same_error(&lite, &std);
    }
    for (code, kind) in [
        (10013, lio::ErrorKind::PermissionDenied),
        (10022, lio::ErrorKind::InvalidInput),
        (10035, lio::ErrorKind::WouldBlock),
        (10048, lio::ErrorKind::AddrInUse),
        (10049, lio::ErrorKind::AddrNotAvailable),
        (10050, lio::ErrorKind::NetworkDown),
        (10051, lio::ErrorKind::NetworkUnreachable),
        (10053, lio::ErrorKind::ConnectionAborted),
        (10054, lio::ErrorKind::ConnectionReset),
        (10057, lio::ErrorKind::NotConnected),
        (10058, lio::ErrorKind::BrokenPipe),
        (10060, lio::ErrorKind::TimedOut),
        (10061, lio::ErrorKind::ConnectionRefused),
        (10065, lio::ErrorKind::HostUnreachable),
        (10069, lio::ErrorKind::QuotaExceeded),
    ] {
        assert_eq!(lio::Error::from_raw_os_error(code).kind(), kind);
    }
}

#[test]
fn socket_errors_match_std() {
    watchdog();
    // A closed port refuses connections.
    let addr = snet::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    same(
        lnet::TcpStream::connect(addr).map(drop),
        snet::TcpStream::connect(addr).map(drop),
    );

    // A nonblocking listener without connections would block.
    let lite = lnet::TcpListener::bind("127.0.0.1:0").unwrap();
    let std = snet::TcpListener::bind("127.0.0.1:0").unwrap();
    lite.set_nonblocking(true).unwrap();
    std.set_nonblocking(true).unwrap();
    same(lite.accept().map(|(_, a)| a), std.accept().map(|(_, a)| a));

    // Binding an address in use.
    let taken = lite.local_addr().unwrap();
    same(
        lnet::TcpListener::bind(taken).map(drop),
        snet::TcpListener::bind(taken).map(drop),
    );

    // A nonblocking read without data would block.
    let (lite, _peer) = lite_pair();
    let (std, _std_peer) = std_pair();
    lite.set_nonblocking(true).unwrap();
    std.set_nonblocking(true).unwrap();
    let mut buf = [0; 4];
    same((&lite).read(&mut buf), (&std).read(&mut buf));
    let (mut a, mut b) = ([0; 4], [0; 4]);
    let mut lite_bufs = [IoSliceMut::new(&mut a)];
    let mut std_bufs = [io::IoSliceMut::new(&mut b)];
    same(
        (&lite).read_vectored(&mut lite_bufs),
        (&std).read_vectored(&mut std_bufs),
    );
    same(
        lite.take_error().map(|e| e.is_none()),
        std.take_error().map(|e| e.is_none()),
    );
}

#[test]
fn shutdown_matches_std() {
    watchdog();
    let (lite, mut peer) = lite_pair();
    let (std, mut std_peer) = std_pair();

    // Reading after shutting down reading reads the end of the stream.
    lite.shutdown(Shutdown::Read).unwrap();
    std.shutdown(snet::Shutdown::Read).unwrap();
    let mut buf = [0; 4];
    same((&lite).read(&mut buf), (&std).read(&mut buf));
    let mut bufs = [IoSliceMut::new(&mut buf)];
    let lite_n = (&lite).read_vectored(&mut bufs);
    let mut buf = [0; 4];
    let mut bufs = [io::IoSliceMut::new(&mut buf)];
    same(lite_n, (&std).read_vectored(&mut bufs));

    // Writing after shutting down writing fails.
    lite.shutdown(Shutdown::Write).unwrap();
    std.shutdown(snet::Shutdown::Write).unwrap();
    same((&lite).write(b"late"), (&std).write(b"late"));
    same(
        (&lite).write_vectored(&[IoSlice::new(b"late")]),
        (&std).write_vectored(&[io::IoSlice::new(b"late")]),
    );
    // The peers see the end of the stream.
    let mut buf = [0; 4];
    assert_eq!(peer.read(&mut buf).unwrap(), 0);
    assert_eq!(std_peer.read(&mut buf).unwrap(), 0);
}

#[test]
fn timeouts_round_like_std() {
    watchdog();
    let durations = [
        Duration::from_nanos(1),
        Duration::from_micros(999),
        Duration::from_millis(1),
        Duration::from_millis(1) + Duration::from_nanos(1),
        Duration::from_micros(1500),
        Duration::from_secs(10),
        Duration::from_millis(u64::from(u32::MAX) - 1),
        Duration::from_millis(u64::from(u32::MAX)),
        Duration::from_millis(u64::from(u32::MAX)) + Duration::from_nanos(1),
        Duration::from_secs(u64::MAX),
        Duration::MAX,
    ];
    let (lite, _peer) = lite_pair();
    let (std, _std_peer) = std_pair();
    let lite_udp = lnet::UdpSocket::bind("127.0.0.1:0").unwrap();
    let std_udp = snet::UdpSocket::bind("127.0.0.1:0").unwrap();
    for dur in durations.into_iter().map(Some).chain([None]) {
        same(lite.set_read_timeout(dur), std.set_read_timeout(dur));
        same(lite.set_write_timeout(dur), std.set_write_timeout(dur));
        same(lite.read_timeout(), std.read_timeout());
        same(lite.write_timeout(), std.write_timeout());
        same(
            lite_udp.set_read_timeout(dur),
            std_udp.set_read_timeout(dur),
        );
        same(
            lite_udp.set_write_timeout(dur),
            std_udp.set_write_timeout(dur),
        );
        same(lite_udp.read_timeout(), std_udp.read_timeout());
        same(lite_udp.write_timeout(), std_udp.write_timeout());
    }
    assert_eq!(
        lite.read_timeout().unwrap(),
        None,
        "`None` clears the timeout"
    );
    lite.set_read_timeout(Some(Duration::from_nanos(1)))
        .unwrap();
    assert_eq!(lite.read_timeout().unwrap(), Some(Duration::from_millis(1)));

    // A zero timeout is an error, which leaves the timeout as it was.
    let zero = Some(Duration::ZERO);
    same(lite.set_read_timeout(zero), std.set_read_timeout(zero));
    same(lite.set_write_timeout(zero), std.set_write_timeout(zero));
    same(
        lite_udp.set_read_timeout(zero),
        std_udp.set_read_timeout(zero),
    );
    same(
        lite_udp.set_write_timeout(zero),
        std_udp.set_write_timeout(zero),
    );
    assert_eq!(lite.read_timeout().unwrap(), Some(Duration::from_millis(1)));
}

#[test]
fn read_timeout_expires() {
    watchdog();
    let (lite, _peer) = lite_pair();
    lite.set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    let start = Instant::now();
    let err = (&lite).read(&mut [0; 4]).unwrap_err();
    assert!(
        start.elapsed() >= Duration::from_millis(40),
        "{:?}",
        start.elapsed()
    );
    let (std, _std_peer) = std_pair();
    std.set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    same_error(&err, &(&std).read(&mut [0; 4]).unwrap_err());
}

/// A loopback listener with a backlog of one, which the returned
/// connections fill, so that the next attempt to connect waits, if the OS
/// drops the attempts it has no room for.
fn full_listener() -> (snet::TcpListener, Vec<snet::TcpStream>) {
    let mut data = WSADATA::default();
    // SAFETY: `data` is valid for writes; Winsock counts the calls.
    assert_eq!(unsafe { WSAStartup(0x0202, &raw mut data) }, 0);
    // SAFETY: no protocol info; the socket is closed by the listener below.
    let raw = unsafe {
        WSASocketW(
            AF_INET.into(),
            SOCK_STREAM,
            IPPROTO_TCP,
            ptr::null(),
            0,
            WSA_FLAG_OVERLAPPED,
        )
    };
    assert_ne!(raw, INVALID_SOCKET);
    // SAFETY: `raw` is a new socket that nothing else owns.
    let listener =
        unsafe { snet::TcpListener::from_raw_socket(raw as RawSocket) };
    let addr = SOCKADDR_IN {
        sin_family: AF_INET,
        sin_port: 0,
        sin_addr: IN_ADDR {
            S_un: IN_ADDR_0 {
                S_addr: u32::from_ne_bytes([127, 0, 0, 1]),
            },
        },
        sin_zero: [0; 8],
    };
    let len = i32::try_from(size_of::<SOCKADDR_IN>()).unwrap();
    // SAFETY: `addr` is a valid `SOCKADDR_IN` of `len` bytes.
    assert_eq!(unsafe { bind(raw, (&raw const addr).cast(), len) }, 0);
    // SAFETY: the socket is open and bound.
    assert_eq!(unsafe { listen(raw, 1) }, 0);
    let addr = listener.local_addr().unwrap();
    let mut queued = Vec::new();
    for _ in 0..16 {
        match snet::TcpStream::connect_timeout(
            &addr,
            Duration::from_millis(200),
        ) {
            Ok(stream) => queued.push(stream),
            Err(_) => break,
        }
    }
    (listener, queued)
}

#[test]
fn connect_timeout_expires() {
    watchdog();
    let (listener, _queued) = full_listener();
    let addr = listener.local_addr().unwrap();
    let timeout = Duration::from_millis(300);
    let start = Instant::now();
    let lite = lnet::TcpStream::connect_timeout(&addr, timeout);
    let elapsed = start.elapsed();
    let std = snet::TcpStream::connect_timeout(&addr, timeout);
    let (Err(lite), Err(std)) = (lite, std) else {
        panic!("a full backlog accepted a connection");
    };
    same_error(&lite, &std);
    if std.kind() == io::ErrorKind::TimedOut {
        assert_eq!(lite.to_string(), "connection timed out");
        assert!(elapsed >= Duration::from_millis(250), "{elapsed:?}");
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    }
}

#[test]
fn connect_timeout_matches_std() {
    watchdog();
    let listener = snet::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let stream = lnet::TcpStream::connect_timeout(&addr, IO_TIMEOUT).unwrap();
    let (mut accepted, _) = listener.accept().unwrap();
    accepted.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    // The stream is back in blocking mode.
    stream.set_write_timeout(Some(IO_TIMEOUT)).unwrap();
    (&stream).write_all(b"ok").unwrap();
    assert_eq!(read_exact(&mut accepted, 2), b"ok");

    // A zero timeout fails once the connection has to wait.
    same(
        lnet::TcpStream::connect_timeout(&addr, Duration::ZERO).map(drop),
        snet::TcpStream::connect_timeout(&addr, Duration::ZERO).map(drop),
    );
    // A closed port refuses the connection.
    drop(listener);
    same(
        lnet::TcpStream::connect_timeout(&addr, IO_TIMEOUT).map(drop),
        snet::TcpStream::connect_timeout(&addr, IO_TIMEOUT).map(drop),
    );
}

#[test]
fn vectored_io() {
    watchdog();
    let (lite, mut peer) = lite_pair();
    let bufs = [
        IoSlice::new(b"one "),
        IoSlice::new(b""),
        IoSlice::new(b"two"),
    ];
    assert_eq!((&lite).write_vectored(&bufs).unwrap(), 7);
    assert_eq!(read_exact(&mut peer, 7), b"one two");

    // More buffers than one call takes: each call sends a prefix, in order.
    let data: Vec<u8> = (0..=199).collect();
    let mut slices: Vec<IoSlice<'_>> =
        data.chunks(1).map(IoSlice::new).collect();
    let mut rest = &mut slices[..];
    let mut sent = 0;
    while !rest.is_empty() {
        let n = (&lite).write_vectored(rest).unwrap();
        assert!(n > 0);
        sent += n;
        IoSlice::advance_slices(&mut rest, n);
    }
    assert_eq!(sent, data.len());
    assert_eq!(read_exact(&mut peer, data.len()), data);

    // Reads fill the buffers in order.
    peer.write_all(b"abcdefgh").unwrap();
    let (mut a, mut b) = ([0; 3], [0; 5]);
    let mut got = 0;
    while got < 8 {
        let mut bufs = [IoSliceMut::new(&mut a), IoSliceMut::new(&mut b)];
        let mut bufs = &mut bufs[..];
        IoSliceMut::advance_slices(&mut bufs, got);
        let n = (&lite).read_vectored(bufs).unwrap();
        assert!(n > 0);
        got += n;
    }
    assert_eq!((&a, &b), (b"abc", b"defgh"));

    // Empty buffers take no room: data after a long run of them moves.
    let mut slices = vec![IoSlice::new(&[]); 100];
    slices.push(IoSlice::new(b"late"));
    assert_eq!((&lite).write_vectored(&slices).unwrap(), 4);
    assert_eq!(read_exact(&mut peer, 4), b"late");
    peer.write_all(b"data").unwrap();
    let mut empties = [[0u8; 0]; 100];
    let mut data = [0; 4];
    let mut bufs: Vec<IoSliceMut<'_>> =
        empties.iter_mut().map(|b| IoSliceMut::new(b)).collect();
    bufs.push(IoSliceMut::new(&mut data));
    let n = (&lite).read_vectored(&mut bufs).unwrap();
    drop(bufs);
    assert!(n > 0, "a read into empty buffers first reported the end");
    assert_eq!(&data[..n], &b"data"[..n]);
    let mut rest = [0; 4];
    (&lite).read_exact(&mut rest[n..]).unwrap();

    // No buffers at all, as std passes them on.
    let (std, _std_peer) = std_pair();
    lite.set_nonblocking(true).unwrap();
    std.set_nonblocking(true).unwrap();
    same(
        (&lite).read_vectored(&mut []),
        (&std).read_vectored(&mut []),
    );
    same((&lite).write_vectored(&[]), (&std).write_vectored(&[]));
}

#[test]
fn peek_and_options_match_std() {
    watchdog();
    let (lite, mut peer) = lite_pair();
    let (std, mut std_peer) = std_pair();
    peer.write_all(b"peek").unwrap();
    std_peer.write_all(b"peek").unwrap();
    let (mut a, mut b) = ([0; 4], [0; 4]);
    // Loopback data may take a moment to arrive; the timeouts bound it.
    same(lite.peek(&mut a), std.peek(&mut b));
    assert_eq!(a, b);
    same((&lite).read(&mut a), (&std).read(&mut b));

    for on in [true, false] {
        same(lite.set_nodelay(on), std.set_nodelay(on));
        same(lite.nodelay(), std.nodelay());
    }
    for ttl in [1, 64, 255] {
        same(lite.set_ttl(ttl), std.set_ttl(ttl));
        same(lite.ttl(), std.ttl());
    }
    same(lite.set_ttl(0), std.set_ttl(0));
    same(lite.ttl(), std.ttl());
    same(
        lite.peer_addr().map(|a| a.ip()),
        std.peer_addr().map(|a| a.ip()),
    );

    let lite = lnet::TcpListener::bind("127.0.0.1:0").unwrap();
    let std = snet::TcpListener::bind("127.0.0.1:0").unwrap();
    same(lite.set_ttl(99), std.set_ttl(99));
    same(lite.ttl(), std.ttl());
    assert!(lite.take_error().unwrap().is_none());
}

#[test]
fn udp_matches_std() {
    watchdog();
    let lite = lnet::UdpSocket::bind("127.0.0.1:0").unwrap();
    let std = snet::UdpSocket::bind("127.0.0.1:0").unwrap();
    lite.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    std.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    let lite_addr = lite.local_addr().unwrap();
    let std_addr = std.local_addr().unwrap();

    // Datagrams both ways, with peek.
    assert_eq!(lite.send_to(b"to std", std_addr).unwrap(), 6);
    let mut buf = [0; 16];
    assert_eq!(std.recv_from(&mut buf).unwrap(), (6, lite_addr));
    std.send_to(b"to lite", lite_addr).unwrap();
    assert_eq!(lite.peek_from(&mut buf).unwrap(), (7, std_addr));
    assert_eq!(lite.recv_from(&mut buf).unwrap(), (7, std_addr));
    assert_eq!(&buf[..7], b"to lite");

    // A datagram longer than the buffer fails, as in std.
    std.send_to(b"0123456789", lite_addr).unwrap();
    let other = snet::UdpSocket::bind("127.0.0.1:0").unwrap();
    other.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    std.send_to(b"0123456789", other.local_addr().unwrap())
        .unwrap();
    let mut small = [0; 4];
    same(
        lite.recv_from(&mut small).map(|(n, a)| (n, a.port())),
        other.recv_from(&mut small).map(|(n, a)| (n, a.port())),
    );

    // Connected sockets.
    same(lite.peer_addr(), other.peer_addr());
    lite.connect(std_addr).unwrap();
    std.connect(lite_addr).unwrap();
    assert_eq!(lite.peer_addr().unwrap(), std_addr);
    assert_eq!(lite.send(b"hi").unwrap(), 2);
    assert_eq!(std.recv(&mut buf).unwrap(), 2);
    std.send(b"yo").unwrap();
    assert_eq!(lite.peek(&mut buf).unwrap(), 2);
    assert_eq!(lite.recv(&mut buf).unwrap(), 2);
    assert_eq!(&buf[..2], b"yo");

    // Options.
    for on in [true, false] {
        same(lite.set_broadcast(on), other.set_broadcast(on));
        same(lite.broadcast(), other.broadcast());
        same(
            lite.set_multicast_loop_v4(on),
            other.set_multicast_loop_v4(on),
        );
        same(lite.multicast_loop_v4(), other.multicast_loop_v4());
    }
    for ttl in [0, 1, 32, 255] {
        same(
            lite.set_multicast_ttl_v4(ttl),
            other.set_multicast_ttl_v4(ttl),
        );
        same(lite.multicast_ttl_v4(), other.multicast_ttl_v4());
        same(lite.set_ttl(ttl.max(1)), other.set_ttl(ttl.max(1)));
        same(lite.ttl(), other.ttl());
    }
    same(lite.multicast_loop_v6(), other.multicast_loop_v6());
    let group = lnet::Ipv4Addr::new(239, 255, 43, 21);
    let any = lnet::Ipv4Addr::UNSPECIFIED;
    same(
        lite.join_multicast_v4(&group, &any),
        other.join_multicast_v4(&group, &any),
    );
    same(
        lite.leave_multicast_v4(&group, &any),
        other.leave_multicast_v4(&group, &any),
    );
    let group6 = lnet::Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 0x123);
    same(
        lite.join_multicast_v6(&group6, 0),
        other.join_multicast_v6(&group6, 0),
    );
    same(
        lite.take_error().map(|e| e.is_some()),
        other.take_error().map(|e| e.is_some()),
    );
}

#[test]
fn udp_v6_matches_std() {
    watchdog();
    let (lite, std) = match (
        lnet::UdpSocket::bind("[::1]:0"),
        snet::UdpSocket::bind("[::1]:0"),
    ) {
        (Ok(lite), Ok(std)) => (lite, std),
        (lite, std) => {
            // No IPv6 loopback here; both fail alike.
            same(lite.map(drop), std.map(drop));
            return;
        }
    };
    std.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    for on in [true, false] {
        same(
            lite.set_multicast_loop_v6(on),
            std.set_multicast_loop_v6(on),
        );
        same(lite.multicast_loop_v6(), std.multicast_loop_v6());
    }
    lite.send_to(b"six", std.local_addr().unwrap()).unwrap();
    let mut buf = [0; 3];
    let (n, from) = std.recv_from(&mut buf).unwrap();
    assert_eq!((n, from), (3, lite.local_addr().unwrap()));
    let group = lnet::Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 0x123);
    same(
        lite.join_multicast_v6(&group, 0),
        std.join_multicast_v6(&group, 0),
    );
    same(
        lite.leave_multicast_v6(&group, 0),
        std.leave_multicast_v6(&group, 0),
    );
}

#[test]
fn sockets_are_not_inherited() {
    watchdog();
    let listener = lnet::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let stream = lnet::TcpStream::connect(addr).unwrap();
    let timed = lnet::TcpStream::connect_timeout(&addr, IO_TIMEOUT).unwrap();
    let udp = lnet::UdpSocket::bind("127.0.0.1:0").unwrap();
    for raw in [
        listener.as_raw_socket(),
        stream.as_raw_socket(),
        timed.as_raw_socket(),
        udp.as_raw_socket(),
    ] {
        assert!(!inheritable(raw));
    }
    // An accepted socket takes after its listener as Winsock makes it, as
    // in std; Wine makes it inheritable, for std as well.
    let (accepted, _) = listener.accept().unwrap();
    let std_listener = snet::TcpListener::bind("127.0.0.1:0").unwrap();
    let _client = snet::TcpStream::connect(std_listener.local_addr().unwrap());
    let (std_accepted, _) = std_listener.accept().unwrap();
    assert_eq!(
        inheritable(accepted.as_raw_socket()),
        inheritable(std_accepted.as_raw_socket()),
    );
}

#[test]
fn lookup_matches_std() {
    watchdog();
    let lite: Vec<SocketAddr> =
        ("localhost", 8080).to_socket_addrs().unwrap().collect();
    let std: Vec<SocketAddr> =
        std::net::ToSocketAddrs::to_socket_addrs(&("localhost", 8080))
            .unwrap()
            .collect();
    assert_eq!(lite, std);
    assert!(
        lite.iter()
            .all(|a| a.port() == 8080 && a.ip().is_loopback())
    );
    same(
        "localhost:80".to_socket_addrs().map(Iterator::count),
        std::net::ToSocketAddrs::to_socket_addrs("localhost:80")
            .map(Iterator::count),
    );
    same(
        ("local\0host", 80).to_socket_addrs().map(|_| ()),
        std::net::ToSocketAddrs::to_socket_addrs(&("local\0host", 80))
            .map(|_| ()),
    );
    // Winsock answers an empty name with the local addresses, without a
    // query; Wine may refuse it. Either way both agree.
    same(
        ("", 80).to_socket_addrs().map(Iterator::collect::<Vec<_>>),
        std::net::ToSocketAddrs::to_socket_addrs(&("", 80))
            .map(Iterator::collect::<Vec<_>>),
    );
}

#[test]
fn tcp_v6_matches_std() {
    watchdog();
    let listener = match lnet::TcpListener::bind("[::1]:0") {
        Ok(listener) => listener,
        Err(lite) => {
            // No IPv6 loopback here; both fail alike.
            same_error(&lite, &snet::TcpListener::bind("[::1]:0").unwrap_err());
            return;
        }
    };
    let addr = listener.local_addr().unwrap();
    assert!(addr.is_ipv6() && addr.ip().is_loopback());
    let client = lnet::TcpStream::connect(addr).unwrap();
    let (accepted, peer) = listener.accept().unwrap();
    assert_eq!(peer, client.local_addr().unwrap());
    // std reads the same sockets' addresses alike.
    // SAFETY: each socket goes to std and back through its raw form.
    let (std_local, std_peer) = unsafe {
        let std = snet::TcpStream::from_raw_socket(accepted.into_raw_socket());
        let addrs = (std.local_addr().unwrap(), std.peer_addr().unwrap());
        drop(lnet::TcpStream::from_raw_socket(std.into_raw_socket()));
        addrs
    };
    assert_eq!((std_local, std_peer), (addr, peer));
    assert_eq!(client.peer_addr().unwrap(), addr);
}

#[test]
fn accepted_stream_works() {
    watchdog();
    let listener = lnet::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let mut client = snet::TcpStream::connect(addr).unwrap();
    client.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    let (mut accepted, peer) = listener.accept().unwrap();
    assert_eq!(peer, client.local_addr().unwrap());
    assert_eq!(accepted.peer_addr().unwrap(), peer);
    assert_eq!(accepted.local_addr().unwrap(), addr);
    accepted.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    client.write_all(b"ping").unwrap();
    let mut buf = [0; 4];
    accepted.read_exact(&mut buf).unwrap();
    accepted.write_all(&buf).unwrap();
    assert_eq!(read_exact(&mut client, 4), b"ping");
    let mut incoming = listener.incoming();
    let _second = snet::TcpStream::connect(addr).unwrap();
    assert!(incoming.next().unwrap().is_ok());
}

#[test]
fn socket_from_elsewhere() {
    watchdog();
    // An overlapped socket that other code bound to a completion port:
    // litestd's calls on it complete before they return, a timeout
    // included, and post nothing to the port.
    let listener = snet::TcpListener::bind("127.0.0.1:0").unwrap();
    let client =
        snet::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut server, _) = listener.accept().unwrap();
    server.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    let raw = client.into_raw_socket();
    let handle = ptr::without_provenance_mut(usize::try_from(raw).unwrap());
    // SAFETY: the socket is open; a new port is created for it.
    let port = unsafe { CreateIoCompletionPort(handle, ptr::null_mut(), 7, 0) };
    assert!(!port.is_null(), "{}", io::Error::last_os_error());
    // SAFETY: `into_raw_socket` gave up the socket.
    let lite = unsafe { lnet::TcpStream::from_raw_socket(raw) };
    lite.set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    lite.set_write_timeout(Some(IO_TIMEOUT)).unwrap();
    let mut buf = [0; 4];
    let err = (&lite).read(&mut buf).unwrap_err();
    assert_eq!(err.kind(), lio::ErrorKind::TimedOut);
    lite.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    server.write_all(b"data").unwrap();
    (&lite).read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"data");
    (&lite).write_all(b"back").unwrap();
    let mut bufs = [IoSliceMut::new(&mut buf)];
    server.write_all(b"more").unwrap();
    assert!((&lite).read_vectored(&mut bufs).unwrap() > 0);
    assert_eq!(read_exact(&mut server, 4), b"back");

    let (mut bytes, mut key, mut overlapped) = (0, 0, ptr::null_mut());
    // SAFETY: the port is open, and the outputs are valid for writes.
    let ok = unsafe {
        GetQueuedCompletionStatus(
            port,
            &raw mut bytes,
            &raw mut key,
            &raw mut overlapped,
            0,
        )
    };
    // SAFETY: `GetLastError` has no preconditions.
    let error = unsafe { GetLastError() };
    assert_eq!((ok, error, overlapped), (0, WAIT_TIMEOUT, ptr::null_mut()));
    drop(lite);
    // SAFETY: the port is open and owned here.
    assert_ne!(unsafe { CloseHandle(port) }, 0);
}
