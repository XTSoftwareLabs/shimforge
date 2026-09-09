# shimforge

Mock Rust functions without traits or dependency injection.
Supports Windows x86-64 and Linux x86-64.

## Test configuration

Keep your production code as it is. Add shimforge as a dev dependency and use these
settings in the workspace root `Cargo.toml`:

```toml
[profile.test]
opt-level = 0
debug = true
lto = false
codegen-units = 1
incremental = false
```

Run tests that share mocks one at a time:

```text
cargo test -- --test-threads=1
```

## Disk access

This code reads a port from a file. The mock supplies its contents without creating
or reading the file. The production function needs no changes.

```rust
use shimforge::{replace, Session};
use std::{fs, io, path::Path};

fn load_port(path: &Path) -> io::Result<u16> {
    fs::read_to_string(path)?.trim().parse()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

let mut session = Session::new().unwrap();
replace!(session, fs::read_to_string::<&Path> => |path| {
    assert_eq!(path, Path::new("service.port"));
    Ok("8080\n".to_owned())
}, fn(&Path) -> io::Result<String>).unwrap();

assert_eq!(load_port(Path::new("service.port")).unwrap(), 8080);
```

## Network traffic

This code sends a UDP metric. The test opens a local socket, but the mock checks
the payload and destination without sending a packet.

```rust
use shimforge::{replace, Session};
use std::{io, net::{SocketAddr, UdpSocket}};

fn send_metric(address: SocketAddr, name: &str, value: u64) -> io::Result<usize> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.send_to(format!("{name}:{value}|c").as_bytes(), address)
}

let mut session = Session::new().unwrap();
replace!(session, UdpSocket::send_to::<SocketAddr> => |_, payload, address| {
    assert_eq!(payload, b"orders:3|c");
    assert_eq!(address, "127.0.0.1:8125".parse().unwrap());
    Ok(payload.len())
}, fn(&UdpSocket, &[u8], SocketAddr) -> io::Result<usize>).unwrap();

assert_eq!(send_metric("127.0.0.1:8125".parse().unwrap(), "orders", 3).unwrap(), 10);
```

Both examples run as doctests in `cargo test`. Neither uses inline attributes.
More disk and network error cases are in [`tests/io.rs`](tests/io.rs).

Keep the session alive while using the mock. Drop restores the original code,
including during a panic. Mocks affect all threads. Only one session can be active;
another attempt returns `Error::Busy` without waiting. `session.restore()` removes
the mocks early. Functions, methods, concrete generic functions, and closures
without captures are supported. Use matching `extern` signatures for native calls.

## Safety and limits

`replace!` checks signatures and handles unsafe operations inside the library.
You do not need an `unsafe` block, but you must follow these rules to avoid memory
errors. No thread may call the target while installing or restoring a mock,
including during drop. Join workers before ending the session; the session lock
does not stop function calls.

Source and replacement must match in calling convention, argument and return
layout, and lifetimes for every caller. Keep both functions loaded until restored.
Do not mock memory allocation, locking, or OS functions that shimforge uses.

The test profile above reduces inlining and other optimizations without source
changes. It cannot undo calls already inlined in prebuilt libraries or separate
merged functions. Short entries are rejected. Only the supplied entry point is
patched; import jumps are not followed. Capturing closures and replacements between
different `async fn` future types are not supported.

Memory permissions are restored after each write. If `restore()` fails, it returns
an error so you can retry. A failed rollback or failed restoration during drop
aborts the process.

MIT licensed.

## Contributing

Run `./scripts/verify.ps1` on Windows or `bash scripts/verify.sh` on Linux.
The checks require rustfmt, clippy, llvm-tools-preview, and cargo-llvm-cov 0.8.7.
Use `./scripts/validate-docker.ps1` to run Linux checks in Docker.
Both platforms require 100% line and function coverage of library code. Tests and
dependencies are excluded. Reports are saved under `coverage/`.
