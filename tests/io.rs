use shimforge::{Session, replace};
use std::fs::{self, File};
use std::io;
use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

static TEST_LOCK: Mutex<()> = Mutex::new(());
static READ_CALLS: AtomicUsize = AtomicUsize::new(0);
static OPEN_CALLS: AtomicUsize = AtomicUsize::new(0);
static CONNECT_CALLS: AtomicUsize = AtomicUsize::new(0);
static WRITE_CALLS: AtomicUsize = AtomicUsize::new(0);
static SEND_CALLS: AtomicUsize = AtomicUsize::new(0);
static WRITES: Mutex<Vec<WrittenFile>> = Mutex::new(Vec::new());
static DATAGRAMS: Mutex<Vec<Datagram>> = Mutex::new(Vec::new());

struct WrittenFile {
    path: PathBuf,
    contents: Vec<u8>,
}

struct Datagram {
    source: SocketAddr,
    target: SocketAddr,
    contents: Vec<u8>,
}

fn serial() -> MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

// These application functions call the standard library directly.
fn load_members(path: &Path) -> io::Result<Vec<String>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let contents = String::from_utf8(bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

fn save_members(path: &Path, names: &[&str]) -> io::Result<()> {
    let contents = format!("{}\n", names.join("\n"));
    fs::write(path, contents.as_bytes())
}

fn has_saved_session(path: &str) -> io::Result<bool> {
    match File::open(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn upstream_is_available(address: SocketAddr) -> io::Result<bool> {
    match TcpStream::connect_timeout(&address, Duration::from_millis(250)) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::TimedOut => Ok(false),
        Err(error) => Err(error),
    }
}

fn emit_counter(
    socket: &UdpSocket,
    target: SocketAddr,
    name: &str,
    value: u64,
) -> io::Result<usize> {
    let payload = format!("{name}:{value}|c");
    socket.send_to(payload.as_bytes(), target)
}

fn member_data(path: &Path) -> io::Result<Vec<u8>> {
    assert_eq!(path, Path::new("virtual/members.txt"));
    READ_CALLS.fetch_add(1, Ordering::SeqCst);
    Ok(b"alice\n\n bob \n".to_vec())
}

fn missing_member_data(path: &Path) -> io::Result<Vec<u8>> {
    assert_eq!(path, Path::new("virtual/members.txt"));
    READ_CALLS.fetch_add(1, Ordering::SeqCst);
    Err(io::ErrorKind::NotFound.into())
}

fn invalid_member_data(path: &Path) -> io::Result<Vec<u8>> {
    assert_eq!(path, Path::new("virtual/members.txt"));
    READ_CALLS.fetch_add(1, Ordering::SeqCst);
    Ok(vec![0xff, 0xfe])
}

fn capture_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    WRITE_CALLS.fetch_add(1, Ordering::SeqCst);
    WRITES.lock().unwrap().push(WrittenFile {
        path: path.to_owned(),
        contents: contents.to_vec(),
    });
    Ok(())
}

fn deny_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    assert_eq!(path, Path::new("virtual/readonly/members.txt"));
    assert_eq!(contents, b"alice\nbob\n");
    WRITE_CALLS.fetch_add(1, Ordering::SeqCst);
    Err(io::ErrorKind::PermissionDenied.into())
}

fn missing_session(path: &str) -> io::Result<File> {
    assert_eq!(path, "virtual/session.bin");
    OPEN_CALLS.fetch_add(1, Ordering::SeqCst);
    Err(io::ErrorKind::NotFound.into())
}

fn deny_session(path: &str) -> io::Result<File> {
    assert_eq!(path, "virtual/session.bin");
    OPEN_CALLS.fetch_add(1, Ordering::SeqCst);
    Err(io::ErrorKind::PermissionDenied.into())
}

fn connection_timeout(address: &SocketAddr, timeout: Duration) -> io::Result<TcpStream> {
    assert_eq!(*address, SocketAddr::from(([192, 0, 2, 10], 443)));
    assert_eq!(timeout, Duration::from_millis(250));
    CONNECT_CALLS.fetch_add(1, Ordering::SeqCst);
    Err(io::ErrorKind::TimedOut.into())
}

fn connection_refused(address: &SocketAddr, timeout: Duration) -> io::Result<TcpStream> {
    assert_eq!(*address, SocketAddr::from(([192, 0, 2, 10], 443)));
    assert_eq!(timeout, Duration::from_millis(250));
    CONNECT_CALLS.fetch_add(1, Ordering::SeqCst);
    Err(io::ErrorKind::ConnectionRefused.into())
}

fn capture_datagram(socket: &UdpSocket, contents: &[u8], target: SocketAddr) -> io::Result<usize> {
    SEND_CALLS.fetch_add(1, Ordering::SeqCst);
    DATAGRAMS.lock().unwrap().push(Datagram {
        source: socket.local_addr()?,
        target,
        contents: contents.to_vec(),
    });
    Ok(contents.len())
}

fn deny_datagram(socket: &UdpSocket, contents: &[u8], target: SocketAddr) -> io::Result<usize> {
    assert!(socket.local_addr()?.ip().is_loopback());
    assert!(target.ip().is_loopback());
    assert_eq!(contents, b"jobs.completed:12|c");
    SEND_CALLS.fetch_add(1, Ordering::SeqCst);
    Err(io::ErrorKind::PermissionDenied.into())
}

#[test]
fn filesystem_read_supplies_data_to_unmodified_business_code() {
    let _serial = serial();
    READ_CALLS.store(0, Ordering::SeqCst);
    let mut session = Session::new_global();
    replace!(session, fs::read::<&Path> => member_data, fn(&Path) -> io::Result<Vec<u8>>);
    assert_eq!(
        load_members(Path::new("virtual/members.txt")).unwrap(),
        ["alice", "bob"]
    );
    assert_eq!(READ_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn missing_file_uses_the_application_default() {
    let _serial = serial();
    READ_CALLS.store(0, Ordering::SeqCst);
    let mut session = Session::new_global();
    replace!(session, fs::read::<&Path> => missing_member_data, fn(&Path) -> io::Result<Vec<u8>>);
    assert!(
        load_members(Path::new("virtual/members.txt"))
            .unwrap()
            .is_empty()
    );
    assert_eq!(READ_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn malformed_file_contents_are_rejected() {
    let _serial = serial();
    READ_CALLS.store(0, Ordering::SeqCst);
    let mut session = Session::new_global();
    replace!(session, fs::read::<&Path> => invalid_member_data, fn(&Path) -> io::Result<Vec<u8>>);
    assert_eq!(
        load_members(Path::new("virtual/members.txt"))
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(READ_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn filesystem_write_captures_the_generated_path_and_payload() {
    let _serial = serial();
    WRITE_CALLS.store(0, Ordering::SeqCst);
    WRITES.lock().unwrap().clear();
    let mut session = Session::new_global();
    replace!(session, fs::write::<&Path, &[u8]> => capture_write, fn(&Path, &[u8]) -> io::Result<()>);
    save_members(Path::new("virtual/members.txt"), &["alice", "bob"]).unwrap();
    assert_eq!(WRITE_CALLS.load(Ordering::SeqCst), 1);
    let writes = WRITES.lock().unwrap();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].path, Path::new("virtual/members.txt"));
    assert_eq!(writes[0].contents, b"alice\nbob\n");
}

#[test]
fn filesystem_write_permission_errors_reach_the_caller() {
    let _serial = serial();
    WRITE_CALLS.store(0, Ordering::SeqCst);
    let mut session = Session::new_global();
    replace!(session, fs::write::<&Path, &[u8]> => deny_write, fn(&Path, &[u8]) -> io::Result<()>);
    let error =
        save_members(Path::new("virtual/readonly/members.txt"), &["alice", "bob"]).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(WRITE_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn file_open_can_report_a_missing_session_without_touching_disk() {
    let _serial = serial();
    OPEN_CALLS.store(0, Ordering::SeqCst);
    let mut session = Session::new_global();
    replace!(session, File::open::<&str> => missing_session, fn(&str) -> io::Result<File>);
    assert!(!has_saved_session("virtual/session.bin").unwrap());
    assert_eq!(OPEN_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn file_open_permission_errors_are_not_treated_as_missing_data() {
    let _serial = serial();
    OPEN_CALLS.store(0, Ordering::SeqCst);
    let mut session = Session::new_global();
    replace!(session, File::open::<&str> => deny_session, fn(&str) -> io::Result<File>);
    assert_eq!(
        has_saved_session("virtual/session.bin").unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(OPEN_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn tcp_timeout_is_handled_without_connecting() {
    let _serial = serial();
    CONNECT_CALLS.store(0, Ordering::SeqCst);
    let mut session = Session::new_global();
    replace!(session, TcpStream::connect_timeout => connection_timeout, fn(&SocketAddr, Duration) -> io::Result<TcpStream>);
    assert!(!upstream_is_available(SocketAddr::from(([192, 0, 2, 10], 443))).unwrap());
    assert_eq!(CONNECT_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn tcp_connection_errors_reach_the_caller_without_connecting() {
    let _serial = serial();
    CONNECT_CALLS.store(0, Ordering::SeqCst);
    let mut session = Session::new_global();
    replace!(session, TcpStream::connect_timeout => connection_refused, fn(&SocketAddr, Duration) -> io::Result<TcpStream>);
    let error = upstream_is_available(SocketAddr::from(([192, 0, 2, 10], 443))).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionRefused);
    assert_eq!(CONNECT_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn udp_send_captures_the_metric_without_delivering_a_packet() {
    let _serial = serial();
    SEND_CALLS.store(0, Ordering::SeqCst);
    DATAGRAMS.lock().unwrap().clear();
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    let target = receiver.local_addr().unwrap();
    let mut session = Session::new_global();
    replace!(session, UdpSocket::send_to::<SocketAddr> => capture_datagram, fn(&UdpSocket, &[u8], SocketAddr) -> io::Result<usize>);
    assert_eq!(
        emit_counter(&sender, target, "jobs.completed", 12).unwrap(),
        19
    );
    assert_eq!(SEND_CALLS.load(Ordering::SeqCst), 1);
    let datagrams = DATAGRAMS.lock().unwrap();
    assert_eq!(datagrams.len(), 1);
    assert_eq!(datagrams[0].source, sender.local_addr().unwrap());
    assert_eq!(datagrams[0].target, target);
    assert_eq!(datagrams[0].contents, b"jobs.completed:12|c");
    receiver.set_nonblocking(true).unwrap();
    assert_eq!(
        receiver.recv_from(&mut [0; 64]).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
}

#[test]
fn udp_send_errors_reach_the_caller_without_delivering_a_packet() {
    let _serial = serial();
    SEND_CALLS.store(0, Ordering::SeqCst);
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    let target = receiver.local_addr().unwrap();
    let mut session = Session::new_global();
    replace!(session, UdpSocket::send_to::<SocketAddr> => deny_datagram, fn(&UdpSocket, &[u8], SocketAddr) -> io::Result<usize>);
    assert_eq!(
        emit_counter(&sender, target, "jobs.completed", 12)
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(SEND_CALLS.load(Ordering::SeqCst), 1);
    receiver.set_nonblocking(true).unwrap();
    assert_eq!(
        receiver.recv_from(&mut [0; 64]).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
}
