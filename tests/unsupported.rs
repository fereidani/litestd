//! What litestd does on wasm32-unknown-unknown, which has no OS, against
//! what std does there: operations fail with the same errors, the
//! environment and standard input are empty, output goes nowhere, no path
//! is absolute, and no time precedes the epoch.

#![cfg(target_os = "unknown")]
#![allow(
    clippy::incompatible_msrv,
    reason = "the tests compare against the std of the toolchain that runs them"
)]
#![allow(clippy::unwrap_used, reason = "a failure fails the test")]

use core::fmt::Debug;

/// Compares an error of litestd with one of std.
#[cfg(feature = "io")]
fn same(lite: &litestd::io::Error, std: &std::io::Error) {
    assert_eq!(lite.to_string(), std.to_string());
    assert_eq!(format!("{lite:?}"), format!("{std:?}"));
    assert_eq!(format!("{:?}", lite.kind()), format!("{:?}", std.kind()));
    assert_eq!(lite.raw_os_error(), std.raw_os_error());
}

/// Compares two calls that must both fail.
#[cfg(feature = "io")]
fn both_fail<A: Debug, B: Debug>(
    lite: litestd::io::Result<A>,
    std: std::io::Result<B>,
) {
    same(&lite.unwrap_err(), &std.unwrap_err());
}

#[cfg(feature = "fs")]
#[test]
fn files_fail_like_std() {
    use std::fs as sfs;

    use litestd::fs as lfs;

    both_fail(lfs::File::open("a"), sfs::File::open("a"));
    both_fail(lfs::File::create("a"), sfs::File::create("a"));
    both_fail(lfs::read("a"), sfs::read("a"));
    both_fail(lfs::read_to_string("a"), sfs::read_to_string("a"));
    both_fail(lfs::write("a", b"x"), sfs::write("a", b"x"));
    both_fail(lfs::read_dir("."), sfs::read_dir("."));
    both_fail(lfs::metadata("a"), sfs::metadata("a"));
    both_fail(lfs::symlink_metadata("a"), sfs::symlink_metadata("a"));
    both_fail(lfs::canonicalize("a"), sfs::canonicalize("a"));
    both_fail(lfs::read_link("a"), sfs::read_link("a"));
    both_fail(lfs::create_dir("a"), sfs::create_dir("a"));
    both_fail(lfs::create_dir_all("a/b"), sfs::create_dir_all("a/b"));
    both_fail(lfs::remove_file("a"), sfs::remove_file("a"));
    both_fail(lfs::remove_dir("a"), sfs::remove_dir("a"));
    both_fail(lfs::remove_dir_all("a"), sfs::remove_dir_all("a"));
    both_fail(lfs::rename("a", "b"), sfs::rename("a", "b"));
    both_fail(lfs::hard_link("a", "b"), sfs::hard_link("a", "b"));
    both_fail(lfs::copy("a", "b"), sfs::copy("a", "b"));
    let (lite, std) = (lfs::exists("a"), sfs::exists("a"));
    assert_eq!(
        lite.map_err(|e| e.to_string()),
        std.map_err(|e| e.to_string())
    );
}

#[cfg(feature = "env")]
#[test]
fn environment_is_empty_like_std() {
    use std::env as senv;

    use litestd::env as lenv;

    assert_eq!(lenv::args_os().len(), senv::args_os().len());
    assert_eq!(format!("{:?}", lenv::args()), format!("{:?}", senv::args()));
    assert_eq!(
        format!("{:?}", lenv::var("PATH")),
        format!("{:?}", senv::var("PATH"))
    );
    assert_eq!(lenv::home_dir(), None);
    assert_eq!(senv::home_dir(), None);
    both_fail(lenv::current_dir(), senv::current_dir());
    both_fail(lenv::set_current_dir("a"), senv::set_current_dir("a"));
    both_fail(lenv::current_exe(), senv::current_exe());
    let (lite, std) = (lenv::join_paths(["a"]), senv::join_paths(["a"]));
    assert_eq!(lite.unwrap_err().to_string(), std.unwrap_err().to_string());
}

#[cfg(all(feature = "command", feature = "io"))]
#[test]
fn processes_and_pipes_fail_like_std() {
    use std::process::Command as Std;

    use litestd::process::Command as Lite;

    both_fail(Lite::new("a").spawn(), Std::new("a").spawn());
    both_fail(Lite::new("a").output(), Std::new("a").output());
    both_fail(Lite::new("a").status(), Std::new("a").status());
    both_fail(litestd::io::pipe(), std::io::pipe());
}

#[cfg(feature = "thread")]
#[test]
fn threads_do_not_spawn_like_std() {
    use std::thread as sthread;

    use litestd::thread as lthread;

    let lite = lthread::Builder::new().spawn(|| ());
    let std = sthread::Builder::new().spawn(|| ());
    both_fail(lite, std);
    both_fail(
        lthread::available_parallelism(),
        sthread::available_parallelism(),
    );
    assert_eq!(lthread::current().name(), sthread::current().name());
}

#[cfg(all(feature = "stdio", feature = "io"))]
#[test]
fn standard_streams_are_sinks_like_std() {
    use std::io::{IsTerminal as _, Read as _, Write as _};

    use litestd::io::{IsTerminal as _, Read as _, Write as _};

    let mut buf = [0u8; 8];
    let lite = litestd::io::stdin().read(&mut buf).unwrap();
    assert_eq!(lite, std::io::stdin().read(&mut buf).unwrap());
    let lite = litestd::io::stdout().write(b"abc").unwrap();
    assert_eq!(lite, std::io::stdout().write(b"abc").unwrap());
    let lite = litestd::io::stderr().write(b"abc").unwrap();
    assert_eq!(lite, std::io::stderr().write(b"abc").unwrap());
    let lite = litestd::io::stdout().is_terminal();
    assert_eq!(lite, std::io::stdout().is_terminal());
}

#[cfg(feature = "net")]
#[test]
fn sockets_fail_like_std() {
    use std::net as snet;

    use litestd::net as lnet;

    let any = "127.0.0.1:0";
    both_fail(lnet::TcpListener::bind(any), snet::TcpListener::bind(any));
    both_fail(lnet::UdpSocket::bind(any), snet::UdpSocket::bind(any));
    let port = "127.0.0.1:1";
    both_fail(
        lnet::TcpStream::connect(port),
        snet::TcpStream::connect(port),
    );
    let host = ("localhost", 80);
    both_fail(
        lnet::ToSocketAddrs::to_socket_addrs(&host),
        snet::ToSocketAddrs::to_socket_addrs(&host),
    );
}

#[cfg(feature = "io")]
#[test]
fn os_errors_match_std() {
    use std::io::Error as Std;

    use litestd::io::Error as Lite;

    same(&Lite::last_os_error(), &Std::last_os_error());
    for code in [0, 1, 2, 5, 32, -1, i32::MAX] {
        same(
            &Lite::from_raw_os_error(code),
            &Std::from_raw_os_error(code),
        );
    }
}

#[cfg(feature = "path")]
#[test]
fn no_path_is_absolute_like_std() {
    for p in ["/", "/a", "//a", "a", "./a", "../a"] {
        let (lite, std) =
            (litestd::path::Path::new(p), std::path::Path::new(p));
        assert_eq!(lite.is_absolute(), std.is_absolute(), "{p}");
        assert_eq!(lite.has_root(), std.has_root(), "{p}");
    }
}

#[cfg(all(feature = "path", feature = "fs"))]
#[test]
fn absolute_fails_like_std() {
    for p in ["", "/", "/a", "a"] {
        both_fail(litestd::path::absolute(p), std::path::absolute(p));
    }
}

#[cfg(feature = "time")]
#[test]
fn times_match_std() {
    use core::time::Duration;

    let (lite, std) = (litestd::time::UNIX_EPOCH, std::time::UNIX_EPOCH);
    assert_eq!(format!("{lite:?}"), format!("{std:?}"));
    for d in [Duration::ZERO, Duration::new(1, 5), Duration::MAX] {
        let (a, b) = (lite.checked_add(d), std.checked_add(d));
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
        let (a, b) = (lite.checked_sub(d), std.checked_sub(d));
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
    }
    let five = Duration::from_secs(5);
    let (later, std_later) = (lite + five, std + five);
    assert_eq!(
        later.duration_since(lite).ok(),
        std_later.duration_since(std).ok()
    );
    let (a, b) = (lite.duration_since(later), std.duration_since(std_later));
    assert_eq!(a.unwrap_err().duration(), b.unwrap_err().duration());
}
