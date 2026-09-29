//! `litestd::net::UdpSocket` against std on loopback: datagrams both ways,
//! connected sockets, `peek`, timeouts, nonblocking mode, errors, the
//! broadcast and multicast options, and the descriptor traits. Miri cannot
//! run sockets.

// WebAssembly has no sockets in litestd.
#![cfg(all(feature = "net", not(miri), not(target_family = "wasm")))]
#![allow(clippy::unwrap_used, reason = "a failure fails the test")]

mod net_support;

use core::{
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};
use std::time::Instant;

use litestd::net::UdpSocket;
use net_support::{
    IO_LIMIT, TIMEOUTS, same_error, same_result, same_timeout, timed,
};

fn lite_socket() -> (UdpSocket, SocketAddr) {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.set_read_timeout(Some(IO_LIMIT)).unwrap();
    let addr = socket.local_addr().unwrap();
    (socket, addr)
}

fn std_socket() -> (std::net::UdpSocket, SocketAddr) {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.set_read_timeout(Some(IO_LIMIT)).unwrap();
    let addr = socket.local_addr().unwrap();
    (socket, addr)
}

#[test]
fn datagrams_flow_both_ways() {
    timed(|| {
        let (lite, lite_addr) = lite_socket();
        let (std, std_addr) = std_socket();
        assert_eq!(lite.send_to(b"hello", std_addr).unwrap(), 5);
        let mut buf = [0; 16];
        let (n, from) = std.recv_from(&mut buf).unwrap();
        assert_eq!((&buf[..n], from), (&b"hello"[..], lite_addr));

        assert_eq!(std.send_to(b"world!", lite_addr).unwrap(), 6);
        let (n, from) = lite.peek_from(&mut buf).unwrap();
        assert_eq!((&buf[..n], from), (&b"world!"[..], std_addr));
        let (n, from) = lite.recv_from(&mut buf).unwrap();
        assert_eq!((&buf[..n], from), (&b"world!"[..], std_addr));

        // Addresses resolve like every other: a string, a tuple, a slice.
        lite.send_to(b"a", format!("127.0.0.1:{}", std_addr.port()))
            .unwrap();
        lite.send_to(b"b", ("127.0.0.1", std_addr.port())).unwrap();
        lite.send_to(b"c", &[std_addr][..]).unwrap();
        for expected in [b"a", b"b", b"c"] {
            let (n, _) = std.recv_from(&mut buf).unwrap();
            assert_eq!(&buf[..n], expected);
        }
        // An empty datagram is a datagram.
        lite.send_to(&[], std_addr).unwrap();
        assert_eq!(std.recv_from(&mut buf).unwrap(), (0, lite_addr));
    });
}

/// A datagram longer than the buffer is cut off on Unix and fails with
/// `WSAEMSGSIZE` on Windows, for std and litestd alike.
#[test]
fn a_short_buffer_gets_part_of_the_datagram() {
    timed(|| {
        let (lite, lite_addr) = lite_socket();
        let (std, std_addr) = std_socket();
        let (sender, _) = std_socket();
        for payload in [&b"0123456789"[..], b"abcdef"] {
            sender.send_to(payload, lite_addr).unwrap();
            sender.send_to(payload, std_addr).unwrap();
            let (mut a, mut b) = ([0; 4], [0; 4]);
            same_result(
                lite.recv_from(&mut a).map(|(n, _)| n),
                std.recv_from(&mut b).map(|(n, _)| n),
            );
            assert_eq!(a, b);
        }
    });
}

#[test]
fn connected_sockets_send_and_receive() {
    timed(|| {
        let (lite, lite_addr) = lite_socket();
        let (std, std_addr) = std_socket();
        lite.connect(std_addr).unwrap();
        std.connect(lite_addr).unwrap();
        assert_eq!(lite.peer_addr().unwrap(), std_addr);
        assert_eq!(lite.send(b"ping").unwrap(), 4);
        let mut buf = [0; 8];
        assert_eq!(std.recv(&mut buf).unwrap(), 4);
        std.send(b"pong").unwrap();
        assert_eq!(lite.peek(&mut buf).unwrap(), 4);
        assert_eq!(lite.recv(&mut buf).unwrap(), 4);
        assert_eq!(&buf[..4], b"pong");
        // A string address connects too, and each address is tried.
        lite.connect(format!("127.0.0.1:{}", std_addr.port()))
            .unwrap();
        let none: &[SocketAddr] = &[];
        same_error(
            &lite.connect(none).unwrap_err(),
            &std.connect(none).unwrap_err(),
        );
    });
}

#[test]
fn errors_match_std() {
    timed(|| {
        let (lite, _) = lite_socket();
        let (std, _) = std_socket();
        same_result(lite.peer_addr(), std.peer_addr());
        let none: &[SocketAddr] = &[];
        same_error(
            &lite.send_to(b"x", none).unwrap_err(),
            &std.send_to(b"x", none).unwrap_err(),
        );
        same_result(lite.send(b"x"), std.send(b"x"));
        // A datagram of an IPv4 socket to an IPv6 address fails alike.
        let v6 = SocketAddr::from((Ipv6Addr::LOCALHOST, 9));
        same_result(lite.send_to(b"x", v6), std.send_to(b"x", v6));
        // Too large for any datagram.
        let huge = vec![0; 70_000];
        let (_, to) = std_socket();
        same_result(lite.send_to(&huge, to), std.send_to(&huge, to));
        let (_other, addr) = std_socket();
        same_error(
            &UdpSocket::bind(addr).unwrap_err(),
            &std::net::UdpSocket::bind(addr).unwrap_err(),
        );
    });
}

/// A datagram to a closed port comes back as an ICMP error, which a
/// connected socket reports on its next receive and in `SO_ERROR`.
#[test]
fn refused_datagrams_match_std() {
    timed(|| {
        let closed = std_socket().1;
        let (lite, _) = lite_socket();
        let (std, _) = std_socket();
        lite.connect(closed).unwrap();
        std.connect(closed).unwrap();
        // Linux reports the error at once; where no ICMP error arrives,
        // as under Wine, both time out alike.
        let wait = Some(Duration::from_secs(1));
        lite.set_read_timeout(wait).unwrap();
        std.set_read_timeout(wait).unwrap();
        lite.send(b"anyone?").unwrap();
        std.send(b"anyone?").unwrap();
        let mut buf = [0; 8];
        same_error(
            &lite.recv(&mut buf).unwrap_err(),
            &std.recv(&mut buf).unwrap_err(),
        );
        lite.send(b"again").unwrap();
        std.send(b"again").unwrap();
        // Give the ICMP errors time to arrive.
        std::thread::sleep(Duration::from_millis(50));
        // Reading `SO_ERROR` clears it on Linux; both libraries see the
        // same on every platform.
        for _ in 0..2 {
            same_result(
                lite.take_error().map(|e| e.map(|e| e.raw_os_error())),
                std.take_error().map(|e| e.map(|e| e.raw_os_error())),
            );
        }
        #[cfg(target_os = "linux")]
        assert!(lite.take_error().unwrap().is_none());
    });
}

#[test]
fn read_timeout_expires_like_std() {
    timed(|| {
        let (lite, _) = lite_socket();
        let (std, _) = std_socket();
        let timeout = Duration::from_millis(80);
        lite.set_read_timeout(Some(timeout)).unwrap();
        std.set_read_timeout(Some(timeout)).unwrap();
        let start = Instant::now();
        let err = lite.recv_from(&mut [0; 4]).unwrap_err();
        assert!(start.elapsed() >= Duration::from_millis(70));
        same_error(&err, &std.recv_from(&mut [0; 4]).unwrap_err());
        same_error(
            &lite.peek_from(&mut [0; 4]).unwrap_err(),
            &std.peek_from(&mut [0; 4]).unwrap_err(),
        );
        let own = [Some(Duration::from_micros(10)), Some(Duration::new(3, 1))];
        for dur in own.into_iter().chain(TIMEOUTS) {
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

#[test]
fn nonblocking_mode_would_block_like_std() {
    timed(|| {
        let (lite, lite_addr) = lite_socket();
        let (std, std_addr) = std_socket();
        lite.set_nonblocking(true).unwrap();
        std.set_nonblocking(true).unwrap();
        let mut buf = [0; 4];
        same_error(
            &lite.recv_from(&mut buf).unwrap_err(),
            &std.recv_from(&mut buf).unwrap_err(),
        );
        lite.connect(std_addr).unwrap();
        std.connect(lite_addr).unwrap();
        same_error(
            &lite.recv(&mut buf).unwrap_err(),
            &std.recv(&mut buf).unwrap_err(),
        );
        same_error(
            &lite.peek(&mut buf).unwrap_err(),
            &std.peek(&mut buf).unwrap_err(),
        );
        lite.set_nonblocking(false).unwrap();
    });
}

#[test]
fn options_round_trip_like_std() {
    timed(|| {
        let (lite, _) = lite_socket();
        let (std, _) = std_socket();
        for on in [true, false] {
            same_result(lite.set_broadcast(on), std.set_broadcast(on));
            same_result(lite.broadcast(), std.broadcast());
            same_result(
                lite.set_multicast_loop_v4(on),
                std.set_multicast_loop_v4(on),
            );
            same_result(lite.multicast_loop_v4(), std.multicast_loop_v4());
        }
        same_result(lite.multicast_ttl_v4(), std.multicast_ttl_v4());
        for ttl in [0, 1, 42, 255, 256, u32::MAX] {
            let result = lite.set_multicast_ttl_v4(ttl);
            if ttl <= 255 {
                same_result(result, std.set_multicast_ttl_v4(ttl));
            } else {
                // std 1.100 and later reject what does not fit a byte before
                // asking the OS, as litestd does; older std passes it on,
                // where -1 (`u32::MAX`) resets the value.
                let err = result.unwrap_err();
                assert_eq!(format!("{err:?}"), "Kind(InvalidInput)");
                assert_eq!(err.raw_os_error(), None);
            }
            same_result(lite.multicast_ttl_v4(), std.multicast_ttl_v4());
            same_result(lite.set_ttl(ttl), std.set_ttl(ttl));
            same_result(lite.ttl(), std.ttl());
        }
        // IPv6 options on an IPv4 socket fail alike.
        same_result(
            lite.set_multicast_loop_v6(false),
            std.set_multicast_loop_v6(false),
        );
        same_result(lite.multicast_loop_v6(), std.multicast_loop_v6());

        let (Ok(lite6), Ok(std6)) = (
            UdpSocket::bind("[::1]:0"),
            std::net::UdpSocket::bind("[::1]:0"),
        ) else {
            return; // No IPv6 on this host.
        };
        for on in [false, true] {
            same_result(
                lite6.set_multicast_loop_v6(on),
                std6.set_multicast_loop_v6(on),
            );
            same_result(lite6.multicast_loop_v6(), std6.multicast_loop_v6());
        }
    });
}

#[test]
fn multicast_membership_matches_std() {
    timed(|| {
        let lite = UdpSocket::bind("0.0.0.0:0").unwrap();
        let std = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
        let group = Ipv4Addr::new(239, 255, 42, 99);
        let any = Ipv4Addr::UNSPECIFIED;
        let local = Ipv4Addr::LOCALHOST;
        same_result(
            lite.join_multicast_v4(&group, &local),
            std.join_multicast_v4(&group, &local),
        );
        same_result(
            lite.join_multicast_v4(&group, &local),
            std.join_multicast_v4(&group, &local),
        );
        same_result(
            lite.leave_multicast_v4(&group, &local),
            std.leave_multicast_v4(&group, &local),
        );
        same_result(
            lite.leave_multicast_v4(&group, &local),
            std.leave_multicast_v4(&group, &local),
        );
        same_result(
            lite.join_multicast_v4(&group, &any),
            std.join_multicast_v4(&group, &any),
        );
        // Not a multicast address.
        same_result(
            lite.join_multicast_v4(&local, &any),
            std.join_multicast_v4(&local, &any),
        );

        let (Ok(lite6), Ok(std6)) = (
            UdpSocket::bind("[::]:0"),
            std::net::UdpSocket::bind("[::]:0"),
        ) else {
            return; // No IPv6 on this host.
        };
        let group6 = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 0x4242);
        for interface in [0, 1, 99_999] {
            same_result(
                lite6.join_multicast_v6(&group6, interface),
                std6.join_multicast_v6(&group6, interface),
            );
            same_result(
                lite6.leave_multicast_v6(&group6, interface),
                std6.leave_multicast_v6(&group6, interface),
            );
        }
        same_result(
            lite6.join_multicast_v6(&Ipv6Addr::LOCALHOST, 0),
            std6.join_multicast_v6(&Ipv6Addr::LOCALHOST, 0),
        );
        // IPv6 membership on an IPv4 socket, and the reverse.
        same_result(
            lite.join_multicast_v6(&group6, 0),
            std.join_multicast_v6(&group6, 0),
        );
        same_result(
            lite6.join_multicast_v4(&group, &any),
            std6.join_multicast_v4(&group, &any),
        );
    });
}

#[test]
fn try_clone_shares_the_socket() {
    timed(|| {
        let (lite, lite_addr) = lite_socket();
        let (std, _) = std_socket();
        let clone = lite.try_clone().unwrap();
        assert_eq!(clone.local_addr().unwrap(), lite_addr);
        drop(lite);
        clone.send_to(b"clone", std.local_addr().unwrap()).unwrap();
        let mut buf = [0; 8];
        assert_eq!(std.recv_from(&mut buf).unwrap(), (5, lite_addr));
        assert!(
            format!("{clone:?}").starts_with("UdpSocket { addr: 127.0.0.1:")
        );
    });
}

#[cfg(unix)]
#[test]
fn descriptor_traits_convert() {
    use litestd::os::fd::{AsFd, AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
    timed(|| {
        let (socket, addr) = lite_socket();
        let raw = socket.as_raw_fd();
        assert_eq!(socket.as_fd().as_raw_fd(), raw);
        let socket = UdpSocket::from(OwnedFd::from(socket));
        assert_eq!(socket.local_addr().unwrap(), addr);
        // SAFETY: the descriptor passes from one owner to the next.
        let socket = unsafe { UdpSocket::from_raw_fd(socket.into_raw_fd()) };
        assert_eq!(socket.as_raw_fd(), raw);
    });
}

#[test]
fn ipv6_datagrams_flow() {
    timed(|| {
        let (Ok(lite), Ok(std)) = (
            UdpSocket::bind("[::1]:0"),
            std::net::UdpSocket::bind("[::1]:0"),
        ) else {
            return; // No IPv6 on this host.
        };
        std.set_read_timeout(Some(IO_LIMIT)).unwrap();
        lite.set_read_timeout(Some(IO_LIMIT)).unwrap();
        let std_addr = std.local_addr().unwrap();
        lite.send_to(b"v6", std_addr).unwrap();
        let mut buf = [0; 4];
        let (n, from) = std.recv_from(&mut buf).unwrap();
        assert_eq!((n, from), (2, lite.local_addr().unwrap()));
        std.send_to(b"back", from).unwrap();
        assert_eq!(lite.recv_from(&mut buf).unwrap(), (4, std_addr));
    });
}
