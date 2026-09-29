//! `litestd::net::ToSocketAddrs` against `std::net::ToSocketAddrs`: every
//! implementation, numeric parsing without name resolution, the resolver,
//! and the errors, compared kind, OS code, message and `Debug` output.
//!
//! Miri runs the numeric tests; it cannot call the system resolver.

#![cfg(feature = "net")]

extern crate alloc;

mod net_support;

use core::net::{
    IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6,
};

use litestd::net::ToSocketAddrs as LiteToSocketAddrs;
use net_support::{same_error, same_result};

/// Resolves `addr` with litestd and std and asserts that both agree.
#[track_caller]
fn same<A>(addr: &A)
where
    A: LiteToSocketAddrs + std::net::ToSocketAddrs + ?Sized,
{
    let lite = LiteToSocketAddrs::to_socket_addrs(addr)
        .map(Iterator::collect::<Vec<_>>);
    let std = std::net::ToSocketAddrs::to_socket_addrs(addr)
        .map(Iterator::collect::<Vec<_>>);
    same_result(lite, std);
}

#[test]
fn socket_addresses_are_the_identity() {
    let v4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 443);
    let v6 = SocketAddrV6::new(Ipv6Addr::LOCALHOST, 8080, 7, 3);
    same(&SocketAddr::V4(v4));
    same(&SocketAddr::V6(v6));
    same(&v4);
    same(&v6);
    same(&&v4);
    same(&(IpAddr::V4(Ipv4Addr::BROADCAST), 0));
    same(&(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 65535));
    same(&(Ipv4Addr::new(10, 1, 2, 3), 1));
    same(&(Ipv6Addr::LOCALHOST, 2));
    let addrs = [SocketAddr::V4(v4), SocketAddr::V6(v6), SocketAddr::V4(v4)];
    same(&&addrs[..]);
    same(&&addrs[..0]);
}

/// The iterator types are std's, so code written against std compiles.
#[test]
fn iterator_types_are_std_s() {
    let v4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 1);
    let _: core::option::IntoIter<SocketAddr> =
        LiteToSocketAddrs::to_socket_addrs(&v4).unwrap();
    let _: alloc::vec::IntoIter<SocketAddr> =
        LiteToSocketAddrs::to_socket_addrs("127.0.0.1:1").unwrap();
    let _: alloc::vec::IntoIter<SocketAddr> =
        LiteToSocketAddrs::to_socket_addrs(&("::1", 1)).unwrap();
    let slice = [SocketAddr::V4(v4)];
    let _: core::iter::Cloned<core::slice::Iter<'_, SocketAddr>> =
        LiteToSocketAddrs::to_socket_addrs(&&slice[..]).unwrap();
}

/// Strings that parse as addresses need no resolver, so Miri runs these.
#[test]
fn numeric_strings_parse_without_the_resolver() {
    for addr in [
        "127.0.0.1:80",
        "0.0.0.0:0",
        "255.255.255.255:65535",
        "[::1]:443",
        "[::]:0",
        "[fe80::1%5]:22",
        "[2001:db8::1]:8080",
        "[::ffff:1.2.3.4]:9",
    ] {
        same(addr);
        same(&addr.to_owned());
    }
    for (host, port) in [
        ("127.0.0.1", 80),
        ("::1", 443),
        ("::", 0),
        ("2001:db8::7", 65535),
        ("::ffff:10.0.0.1", 1),
    ] {
        same(&(host, port));
        same(&(host.to_owned(), port));
    }
}

/// Malformed strings fail before any resolution, with std's errors.
#[test]
fn malformed_strings_fail_like_std() {
    for addr in [
        "127.0.0.1",
        "localhost",
        "",
        "127.0.0.1:",
        "127.0.0.1:x",
        "127.0.0.1:65536",
        "127.0.0.1:-1",
        "[::1]:99999",
        "host:port",
        ":+80x",
    ] {
        same(addr);
    }
}

#[cfg(not(miri))]
#[test]
fn localhost_resolves_like_std() {
    same("localhost:80");
    same("localhost:0");
    same(&("localhost", 443));
    same(&("localhost".to_owned(), 443));
    same(&"localhost:1234".to_owned());
}

/// The host part of a string that is not a socket address goes to the
/// resolver as it is: an IPv6 address without brackets, or an IPv4 address
/// in the short forms `inet_aton` accepts.
#[cfg(not(miri))]
#[test]
fn near_numeric_hosts_resolve_like_std() {
    same("::1:80");
    same("1.2.3:80");
    same("1.2:80");
    same("0x7f.1:80");
    same(&("1.2.3", 80));
    same(&("127.1", 7));
}

#[cfg(not(miri))]
#[test]
fn resolver_failures_match_std() {
    same("nonexistent.invalid:80");
    same(&("nonexistent.invalid", 80));
    same(":80");
    same(&("", 80));
    same(&("a b", 80));
    // Longer than any DNS name, and than the stack buffer for host names.
    let long = "x".repeat(300);
    same(&(long.as_str(), 80));
    same(&format!("{long}:80"));
}

#[test]
fn nul_bytes_fail_like_std() {
    // Parsing fails first for strings that are not addresses; a NUL then
    // fails the conversion to a C string, before the resolver runs, so
    // Miri runs this too.
    same(&("a\0b", 80));
    same("a\0b:80");
    let long = format!("{}\0", "y".repeat(400));
    same(&(long.as_str(), 1));
}

#[test]
fn shutdown_matches_std() {
    use std::net::Shutdown as S;

    use litestd::net::Shutdown as L;
    const fn copy<T: Copy + Eq + core::fmt::Debug + Send + Sync + Unpin>() {}
    for (lite, std) in
        [(L::Read, S::Read), (L::Write, S::Write), (L::Both, S::Both)]
    {
        assert_eq!(format!("{lite:?}"), format!("{std:?}"));
        assert_eq!(lite, lite.clone());
    }
    assert_ne!(L::Read, L::Write);
    copy::<L>();
}

/// `connect` and `bind` report std's error when an address resolves to
/// nothing, before any socket exists.
#[test]
fn empty_address_lists_fail_like_std() {
    let none: &[SocketAddr] = &[];
    let lite = litestd::net::TcpStream::connect(none).unwrap_err();
    let std = std::net::TcpStream::connect(none).unwrap_err();
    same_error(&lite, &std);
    let lite = litestd::net::TcpListener::bind(none).unwrap_err();
    let std = std::net::TcpListener::bind(none).unwrap_err();
    same_error(&lite, &std);
    let lite = litestd::net::UdpSocket::bind(none).unwrap_err();
    let std = std::net::UdpSocket::bind(none).unwrap_err();
    same_error(&lite, &std);
}

/// The first failure is the resolution itself, which Miri can run.
#[test]
fn unresolvable_strings_fail_connect_like_std() {
    let lite = litestd::net::TcpStream::connect("127.0.0.1").unwrap_err();
    let std = std::net::TcpStream::connect("127.0.0.1").unwrap_err();
    same_error(&lite, &std);
    let lite = litestd::net::UdpSocket::bind("[::1]:x").unwrap_err();
    let std = std::net::UdpSocket::bind("[::1]:x").unwrap_err();
    same_error(&lite, &std);
}
