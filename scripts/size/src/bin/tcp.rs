//! Echoes a message over a loopback TCP connection.

#![cfg_attr(feature = "lite", no_std, no_main)]

#[cfg(feature = "lite")]
#[macro_use]
extern crate litestd as std;

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
};

size_probe::main! {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut server, peer) = listener.accept().unwrap();
    client.write_all(b"ping").unwrap();
    let mut buf = [0; 4];
    server.read_exact(&mut buf).unwrap();
    server.write_all(&buf).unwrap();
    client.read_exact(&mut buf).unwrap();
    assert!(&buf == b"ping");
    println!("echo from {peer}");
}
