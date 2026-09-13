//! Async network calls answered from memory instead of a socket.

use shimforge::Session;
use std::io::{self, Write};
use std::net::{Shutdown, TcpListener, TcpStream as BlockingStream};
use std::thread;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

/// A stream that is already connected to a local peer holding `payload`.
fn stream_serving(payload: &'static [u8]) -> io::Result<TcpStream> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let address = listener.local_addr()?;
    thread::spawn(move || {
        if let Ok((mut peer, _)) = listener.accept() {
            let _ = peer.write_all(payload);
            let _ = peer.shutdown(Shutdown::Write);
        }
    });
    let client = BlockingStream::connect(address)?;
    client.set_nonblocking(true)?;
    TcpStream::from_std(client)
}

// Business code that opens its own connection and stays unchanged.
async fn fetch_banner(address: &str) -> io::Result<String> {
    let mut stream = TcpStream::connect(address).await?;
    let mut banner = String::new();
    stream.read_to_string(&mut banner).await?;
    Ok(banner.trim_end().to_owned())
}

#[tokio::test]
async fn a_connection_is_answered_without_reaching_the_address() {
    let mut session = Session::new();
    let connects = session.mock_async(TcpStream::connect(""));
    connects
        .expect()
        .once()
        .return_once(stream_serving(b"inventory-service ready\n"));

    // 203.0.113.0/24 is reserved for documentation and is never routed.
    assert_eq!(
        fetch_banner("203.0.113.9:7000").await.unwrap(),
        "inventory-service ready"
    );
    connects.verify();
}

#[tokio::test]
async fn a_refused_connection_reaches_the_caller() {
    let mut session = Session::new();
    let connects = session.mock_async(TcpStream::connect(""));
    connects
        .expect()
        .once()
        .returning(|| Err(io::ErrorKind::ConnectionRefused.into()));

    assert_eq!(
        fetch_banner("203.0.113.9:7000").await.unwrap_err().kind(),
        io::ErrorKind::ConnectionRefused
    );
    session.restore();
}

#[tokio::test]
async fn other_threads_still_open_real_connections() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let accepted = thread::spawn(move || {
        let (mut peer, _) = listener.accept().unwrap();
        peer.write_all(b"real-peer\n").unwrap();
        peer.shutdown(Shutdown::Write).unwrap();
    });

    let mut session = Session::new();
    let connects = session.mock_async(TcpStream::connect(""));
    connects
        .expect()
        .once()
        .return_once(stream_serving(b"mocked-peer\n"));

    assert_eq!(
        fetch_banner("203.0.113.9:7000").await.unwrap(),
        "mocked-peer"
    );
    let worker = thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fetch_banner(&address.to_string()))
    });
    assert_eq!(worker.join().unwrap().unwrap(), "real-peer");
    accepted.join().unwrap();
}
