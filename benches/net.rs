//! litestd's sockets, pipes and child processes against std's. Each side
//! uses its own library at both ends of a connection; every socket, thread
//! and child is closed, joined or waited for before the next benchmark.

#![allow(clippy::unwrap_used, reason = "a failure ends the benchmark")]
// On WebAssembly, which has no sockets or pipes, the benchmark only builds,
// and its handles cannot exist to be dropped.
#![cfg_attr(target_family = "wasm", allow(clippy::drop_non_drop))]

mod common;

use core::{
    hint::black_box,
    sync::atomic::{AtomicBool, Ordering},
};
use std::{
    io::{Read as _, Write as _},
    sync::mpsc,
    thread,
};

use common::compare;
use litestd::io::{Read as _, Write as _};

/// The message of the ping-pong benchmarks.
const PING: [u8; 16] = *b"ping-pong-bytes!";

/// The bytes that one bulk transfer moves.
const BULK: usize = 4 << 20;

fn main() {
    udp();
    tcp_connect();
    tcp_ping_pong();
    tcp_bulk();
    #[cfg(unix)]
    unix_ping_pong();
    resolve();
    pipe();
    #[cfg(unix)]
    commands();
}

fn udp() {
    let lite = litestd::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let lite_addr = lite.local_addr().unwrap();
    let std = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let std_addr = std.local_addr().unwrap();
    let (mut lite_buf, mut std_buf) = ([0; 64], [0; 64]);
    compare(
        "udp loopback datagram to self",
        || {
            lite.send_to(black_box(b"ping"), lite_addr).unwrap();
            black_box(lite.recv_from(&mut lite_buf).unwrap());
        },
        || {
            std.send_to(black_box(b"ping"), std_addr).unwrap();
            black_box(std.recv_from(&mut std_buf).unwrap());
        },
    );
}

/// A connection and its accept on loopback; the server side closes first,
/// so that no client port stays in `TIME_WAIT`.
fn tcp_connect() {
    let lite = litestd::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let lite_addr = lite.local_addr().unwrap();
    let std = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let std_addr = std.local_addr().unwrap();
    compare(
        "tcp connect and accept",
        || {
            let client = litestd::net::TcpStream::connect(lite_addr).unwrap();
            drop(black_box(lite.accept().unwrap()));
            drop(black_box(client));
        },
        || {
            let client = std::net::TcpStream::connect(std_addr).unwrap();
            drop(black_box(std.accept().unwrap()));
            drop(black_box(client));
        },
    );
}

/// A connected litestd pair: client and accepted server stream.
fn lite_tcp_pair() -> (litestd::net::TcpStream, litestd::net::TcpStream) {
    let listener = litestd::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let client =
        litestd::net::TcpStream::connect(listener.local_addr().unwrap())
            .unwrap();
    let server = listener.accept().unwrap().0;
    client.set_nodelay(true).unwrap();
    server.set_nodelay(true).unwrap();
    (client, server)
}

/// A connected std pair: client and accepted server stream.
fn std_tcp_pair() -> (std::net::TcpStream, std::net::TcpStream) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let client =
        std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let server = listener.accept().unwrap().0;
    client.set_nodelay(true).unwrap();
    server.set_nodelay(true).unwrap();
    (client, server)
}

/// A small message there and back on one thread: four system calls, with
/// no scheduler wakeup in the measurement.
fn tcp_ping_pong() {
    let (mut lite_client, mut lite_server) = lite_tcp_pair();
    let (mut std_client, mut std_server) = std_tcp_pair();
    let (mut lite_buf, mut std_buf) = ([0; PING.len()], [0; PING.len()]);
    compare(
        "tcp ping-pong 16 bytes",
        || {
            lite_client.write_all(black_box(&PING)).unwrap();
            lite_server.read_exact(&mut lite_buf).unwrap();
            lite_server.write_all(&lite_buf).unwrap();
            lite_client.read_exact(&mut lite_buf).unwrap();
        },
        || {
            std_client.write_all(black_box(&PING)).unwrap();
            std_server.read_exact(&mut std_buf).unwrap();
            std_server.write_all(&std_buf).unwrap();
            std_client.read_exact(&mut std_buf).unwrap();
        },
    );
}

/// `BULK` bytes from a server thread, read with `read_to_end` into a vector
/// whose capacity is reused, as a loop that reads responses does.
fn tcp_bulk() {
    let data = vec![0x5a_u8; BULK];
    let lite = litestd::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let lite_addr = lite.local_addr().unwrap();
    let std = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let std_addr = std.local_addr().unwrap();
    let stop = AtomicBool::new(false);
    let (mut lite_buf, mut std_buf) = (Vec::new(), Vec::new());
    thread::scope(|s| {
        s.spawn(|| {
            // Serves until `stop` is set; one more connection wakes it.
            for stream in lite.incoming() {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                stream.unwrap().write_all(&data).unwrap();
            }
        });
        s.spawn(|| {
            // As above.
            for stream in std.incoming() {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                stream.unwrap().write_all(&data).unwrap();
            }
        });
        compare(
            "tcp read_to_end 4 MiB",
            || {
                let mut stream =
                    litestd::net::TcpStream::connect(lite_addr).unwrap();
                lite_buf.clear();
                stream.read_to_end(&mut lite_buf).unwrap();
                assert_eq!(black_box(&lite_buf).len(), BULK);
            },
            || {
                let mut stream =
                    std::net::TcpStream::connect(std_addr).unwrap();
                std_buf.clear();
                stream.read_to_end(&mut std_buf).unwrap();
                assert_eq!(black_box(&std_buf).len(), BULK);
            },
        );
        stop.store(true, Ordering::Relaxed);
        drop(litestd::net::TcpStream::connect(lite_addr).unwrap());
        drop(std::net::TcpStream::connect(std_addr).unwrap());
    });
}

/// As `tcp_ping_pong`, over a Unix stream socket pair.
#[cfg(unix)]
fn unix_ping_pong() {
    let (mut lite_a, mut lite_b) =
        litestd::os::unix::net::UnixStream::pair().unwrap();
    let (mut std_a, mut std_b) =
        std::os::unix::net::UnixStream::pair().unwrap();
    let (mut lite_buf, mut std_buf) = ([0; PING.len()], [0; PING.len()]);
    compare(
        "unix stream ping-pong 16 bytes",
        || {
            lite_a.write_all(black_box(&PING)).unwrap();
            lite_b.read_exact(&mut lite_buf).unwrap();
            lite_b.write_all(&lite_buf).unwrap();
            lite_a.read_exact(&mut lite_buf).unwrap();
        },
        || {
            std_a.write_all(black_box(&PING)).unwrap();
            std_b.read_exact(&mut std_buf).unwrap();
            std_b.write_all(&std_buf).unwrap();
            std_a.read_exact(&mut std_buf).unwrap();
        },
    );
}

fn resolve() {
    use std::net::ToSocketAddrs as Std;

    use litestd::net::ToSocketAddrs as Lite;

    compare(
        "resolve localhost:80",
        || drop(black_box(Lite::to_socket_addrs(black_box("localhost:80")))),
        || drop(black_box(Std::to_socket_addrs(black_box("localhost:80")))),
    );
}

/// `io::pipe` throughput: 32 KiB through a pipe on one thread, which fits
/// its buffer, and `read_to_end` of 1 MiB that a writer thread sends through
/// a new pipe each time.
#[allow(clippy::incompatible_msrv, reason = "std's pipes need Rust 1.87")]
fn pipe() {
    let chunk = vec![0x5a_u8; 32 << 10];
    let (mut lite_rx, mut lite_tx) = litestd::io::pipe().unwrap();
    let (mut std_rx, mut std_tx) = std::io::pipe().unwrap();
    let (mut lite_buf, mut std_buf) = (chunk.clone(), chunk.clone());
    compare(
        "pipe write and read 32 KiB",
        || {
            lite_tx.write_all(black_box(&chunk)).unwrap();
            lite_rx.read_exact(&mut lite_buf).unwrap();
        },
        || {
            std_tx.write_all(black_box(&chunk)).unwrap();
            std_rx.read_exact(&mut std_buf).unwrap();
        },
    );
    let data = &vec![0x5a_u8; 1 << 20];
    let (lite_send, lite_recv) = mpsc::channel::<litestd::io::PipeWriter>();
    let (std_send, std_recv) = mpsc::channel::<std::io::PipeWriter>();
    thread::scope(|s| {
        // Each serves until its sender is dropped.
        s.spawn(move || {
            for mut writer in lite_recv {
                writer.write_all(data).unwrap();
            }
        });
        s.spawn(move || {
            for mut writer in std_recv {
                writer.write_all(data).unwrap();
            }
        });
        compare(
            "pipe read_to_end 1 MiB",
            || {
                let (mut reader, writer) = litestd::io::pipe().unwrap();
                lite_send.send(writer).unwrap();
                lite_buf.clear();
                reader.read_to_end(&mut lite_buf).unwrap();
                assert_eq!(black_box(&lite_buf).len(), data.len());
            },
            || {
                let (mut reader, writer) = std::io::pipe().unwrap();
                std_send.send(writer).unwrap();
                std_buf.clear();
                reader.read_to_end(&mut std_buf).unwrap();
                assert_eq!(black_box(&std_buf).len(), data.len());
            },
        );
        drop((lite_send, std_send));
    });
}

/// Child processes of programs every Unix system has.
#[cfg(unix)]
fn commands() {
    use std::process::{Command, Stdio};

    use litestd::process::{Command as LiteCommand, Stdio as LiteStdio};

    compare(
        "command status /bin/true",
        || assert!(LiteCommand::new("/bin/true").status().unwrap().success()),
        || assert!(Command::new("/bin/true").status().unwrap().success()),
    );
    compare(
        "command output /bin/echo",
        || {
            let out = LiteCommand::new("/bin/echo").arg("hi").output();
            assert_eq!(black_box(out.unwrap()).stdout, b"hi\n");
        },
        || {
            let out = Command::new("/bin/echo").arg("hi").output();
            assert_eq!(black_box(out.unwrap()).stdout, b"hi\n");
        },
    );
    compare(
        "command status with env changes",
        || {
            let mut cmd = LiteCommand::new("/bin/true");
            cmd.env("LITESTD_BENCH", "1").env_remove("LITESTD_UNSET");
            assert!(cmd.status().unwrap().success());
        },
        || {
            let mut cmd = Command::new("/bin/true");
            cmd.env("LITESTD_BENCH", "1").env_remove("LITESTD_UNSET");
            assert!(cmd.status().unwrap().success());
        },
    );
    // `cat` runs until `wait` closes its stdin.
    compare(
        "child wait closing piped stdin",
        || {
            let mut cmd = LiteCommand::new("/bin/cat");
            cmd.stdin(LiteStdio::piped()).stdout(LiteStdio::null());
            assert!(cmd.spawn().unwrap().wait().unwrap().success());
        },
        || {
            let mut cmd = Command::new("/bin/cat");
            cmd.stdin(Stdio::piped()).stdout(Stdio::null());
            assert!(cmd.spawn().unwrap().wait().unwrap().success());
        },
    );
}
