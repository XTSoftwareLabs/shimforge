# shimforge

Mock Rust functions without traits or dependency injection.
Supports Windows, Linux, and macOS on x86-64. Apple Silicon is not supported.

The only runtime crate dependency is `libc` on Linux and macOS. Macro generation uses
`syn`, `quote`, and `proc-macro2` at build time. The library currently uses `std`.

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

Run tests normally. Mocks are thread-local by default:

```text
cargo test
```

## Disk access

This code reads a port from a file. The mock supplies its contents without creating
or reading the file. The production function needs no changes.

```rust
use shimforge::{mock, Session};
use std::{fs, io, path::Path};

fn load_port(path: &Path) -> io::Result<u16> {
    fs::read_to_string(path)?.trim().parse()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

let mut session = Session::new().unwrap();
let read = mock!(session, fs::read_to_string::<&Path>,
    fn(&Path) -> io::Result<String>).unwrap();
read.expect()
    .with(|path| *path == Path::new("service.port"))
    .times(2)
    .returning(|_| Ok("8080\n".to_owned())).unwrap();

assert_eq!(load_port(Path::new("service.port")).unwrap(), 8080);
assert_eq!(load_port(Path::new("service.port")).unwrap(), 8080);
session.verify().unwrap();
```

## Network traffic

This code sends a UDP metric. The test opens a local socket, but the mock checks
the payload and destination without sending a packet.

```rust
use shimforge::{mock, Session};
use std::{io, net::{SocketAddr, UdpSocket}};

fn send_metric(address: SocketAddr, name: &str, value: u64) -> io::Result<usize> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.send_to(format!("{name}:{value}|c").as_bytes(), address)
}

let mut session = Session::new().unwrap();
let send = mock!(session, UdpSocket::send_to::<SocketAddr>,
    fn(&UdpSocket, &[u8], SocketAddr) -> io::Result<usize>).unwrap();
send.expect()
    .with(|_, payload, address| {
        *payload == b"orders:3|c" && *address == "127.0.0.1:8125".parse().unwrap()
    })
    .once()
    .returning(|_, payload, _| Ok(payload.len())).unwrap();

assert_eq!(send_metric("127.0.0.1:8125".parse().unwrap(), "orders", 3).unwrap(), 10);
```

## Return values and call order

Use `returns` to clone a value, `return_once` to move it, or `returning` to run a
closure. Closures may capture owned values and update them between calls.

```rust
use shimforge::{mock, Sequence, Session};

fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
fn save_id(id: u64) -> bool { id > 0 }

let mut session = Session::new().unwrap();
let next = mock!(session, next_id, fn() -> u64).unwrap();
let save = mock!(session, save_id, fn(u64) -> bool).unwrap();
let order = Sequence::new();
let mut id = 40;
next.expect().times(2).in_sequence(&order).returning(move || {
    id += 1;
    id
}).unwrap();
save.expect().with(|id| *id == 42).once().in_sequence(&order)
    .returns(true).unwrap();

assert_eq!(next_id(), 41);
assert_eq!(next_id(), 42);
assert!(save_id(42));
session.verify().unwrap();
```

The first matching expectation with calls left is used. Put specific rules before
fallbacks. A matching `never()` rule rejects the call. No matching rule, too many
calls, or a wrong call order causes a panic and makes later verification fail.

| Method | Meaning |
| --- | --- |
| `with(predicate)` | Match borrowed arguments |
| `times(3)`, `once()`, `never()` | Exact count, one call, or no calls |
| `times(1..=3)`, `times(2..)` | A count range; other Rust ranges also work |
| `returns(value)` | Clone the value for each call |
| `return_once(value)` | Move a value that need not be `Clone` |
| `returning(closure)` | Compute a result or change mutable arguments |
| `returning_once(closure)` | Run a closure that moves its captures |
| `returns_default()` | Create a default result for each call |
| `panics(message)` | Raise a chosen panic |
| `in_sequence(&order)` | Check order across mocks; requires an exact positive count |
| `checkpoint()` | Verify, then clear expectations for the next phase |

Finish each expectation with a return behavior or `never()`. The default allows
any number of calls; one-time responses default to exactly one. The returned
expectation handle has `calls()` and `verify()`. Dropping that handle does not
remove the expectation.

## Mutable arguments and borrowed results

```rust
use shimforge::{mock, Session};

fn fill(buffer: &mut [u8]) -> usize {
    buffer.fill(0);
    buffer.len()
}
fn first_word(text: &str) -> &str { text.split_whitespace().next().unwrap_or("") }

let mut session = Session::new().unwrap();
let write = mock!(session, fill, fn(&mut [u8]) -> usize).unwrap();
write.expect().once().returning(|buffer| {
    buffer[..2].copy_from_slice(b"ok");
    2
}).unwrap();
let word = mock!(session, first_word, fn(&str) -> &str).unwrap();
word.expect().once().returning(|text| &text[3..]).unwrap();

let mut buffer = [0; 8];
assert_eq!(fill(&mut buffer), 2);
assert_eq!(&buffer[..2], b"ok");
assert_eq!(first_word("ok token"), "token");
```

## Async functions

Pass an unpolled future to `mock_async`. Its concrete future type is mocked; the
witness is dropped without running its body. Each poll returns a ready result,
so normal awaits count once. This example polls directly and needs no executor.

```rust
use shimforge::Session;
use std::{fs, future::Future, io, path::Path, pin::pin, task::{Context, Poll, Waker}};

async fn read_token(path: &Path) -> io::Result<String> {
    fs::read_to_string(path)
}

let mut session = Session::new().unwrap();
let read = session.mock_async(read_token(Path::new("token.txt"))).unwrap();
read.expect().return_once(Ok("test-token".to_owned())).unwrap();

let mut future = pin!(read_token(Path::new("token.txt")));
let mut context = Context::from_waker(Waker::noop());
let Poll::Ready(result) = future.as_mut().poll(&mut context) else {
    panic!("mock should be ready");
};
assert_eq!(result.unwrap(), "test-token");
read.verify().unwrap();
```

Async results must be `Send + 'static`. The future itself may borrow arguments.
Arguments stored inside a native future cannot be matched, and its mock always
returns `Ready`. For functions that already return boxed futures, `mock!` can
match arguments and return a custom future, including one that yields `Pending`.

## Boxed async methods

Mock the method that creates a boxed future. Passing a boxed trait object to
`mock_async` would mock its shared poll wrapper, including unrelated methods.
This client test checks the request without opening a connection.

```rust
use shimforge::{mock, Session};
use std::{future::Future, io::{self, Read, Write}, net::TcpStream,
    pin::Pin, task::{Context, Poll, Waker}};

type Response<'a> = Pin<Box<dyn Future<Output = io::Result<String>> + Send + 'a>>;
struct Client { address: String }
impl Client {
    fn get<'a>(&'a self, path: &'a str) -> Response<'a> {
        Box::pin(async move {
            let mut stream = TcpStream::connect(&self.address)?;
            write!(stream, "GET {path} HTTP/1.0\r\nHost: {}\r\n\r\n", self.address)?;
            let mut response = String::new();
            stream.read_to_string(&mut response)?;
            Ok(response)
        })
    }
    fn cached<'a>(&'a self, value: &'a str) -> Response<'a> {
        Box::pin(async move { Ok(value.to_owned()) })
    }
}

let client = Client { address: "127.0.0.1:9".into() };
let mut session = Session::new().unwrap();
let get = mock!(session, Client::get,
    for<'a> fn(&'a Client, &'a str) -> Response<'a>).unwrap();
get.expect().with(|_, path| *path == "/health").once()
    .returning(|_, _| Box::pin(async { Ok("healthy".to_owned()) })).unwrap();

let mut context = Context::from_waker(Waker::noop());
let mut response = client.get("/health");
assert!(matches!(response.as_mut().poll(&mut context),
    Poll::Ready(Ok(value)) if value == "healthy"));
let mut cached = client.cached("unchanged");
assert!(matches!(cached.as_mut().poll(&mut context),
    Poll::Ready(Ok(value)) if value == "unchanged"));
session.verify().unwrap();
```

## Native functions

`mock!` also accepts `unsafe fn`, `extern "C" fn`, and `extern "system" fn`
signatures. Argument matching, counts, and captured callbacks work the same way.
Installing the mock needs no `unsafe` block. Calling an unsafe target still does.
A panic aborts the process when the declared ABI does not allow unwinding.

```rust
use shimforge::{mock, Session};

extern "C" fn add(left: i64, right: i64) -> i64 { left + right }

let mut session = Session::new().unwrap();
let add_mock = mock!(session, add, extern "C" fn(i64, i64) -> i64).unwrap();
add_mock.expect().with(|left, right| *left == 6 && *right == 7)
    .once().returns(42).unwrap();
assert_eq!(add(6, 7), 42);
session.restore().unwrap();
assert_eq!(add(6, 7), 13);
```

All Rust examples above run as doctests. None uses inline attributes. More cases
are in [`tests/expectations.rs`](tests/expectations.rs),
[`tests/async_expectations.rs`](tests/async_expectations.rs), and
[`tests/io.rs`](tests/io.rs). Imported OS calls are tested in
[`tests/native_expectations.rs`](tests/native_expectations.rs).

## Session lifetime

`Session::new()` and `Session::new_local()` create mocks for the current thread.
Other threads use their own mocks or call the original function. Use
`Session::new_global()` when worker threads need the same mock, including async
tasks that may move between threads.

```rust
use shimforge::{mock, Session};

fn count(seed: u64) -> u64 { seed.wrapping_add(1) }

let mut session = Session::new().unwrap();
let counts = mock!(session, count, fn(u64) -> u64).unwrap();
counts.expect().once().returns(42).unwrap();
assert_eq!(count(3), 42);
let worker = std::thread::spawn(|| count(3));
assert_eq!(worker.join().unwrap(), 4);
session.restore().unwrap();
assert_eq!(count(3), 4);
```

Local sessions may coexist on different threads, even for the same function.
Each thread may hold one session. Global sessions wait for other sessions to
finish; local sessions wait for an active global session. Nested sessions return
`Error::Busy`. Use `try_new_local()` or `try_new_global()` to return `Error::Busy`
instead of waiting. Workers using a global mock do not need their own session.
Both modes support `mock!`, `replace!`, and `mock_async`. The untyped, unsafe
`replace_raw` API requires a global session.

Install a local mock before starting calls to that target. Only the first local
installation changes its code. Later local installs and cleanup leave the entry
in place, so other threads can keep calling it. One code page per target is kept
until process exit; mock state is released when the session ends. Keep those
functions loaded for the rest of the process. Loop instructions and unsupported
relocation forms are rejected in local mode. Some entries need a call bridge;
these require shadow stacks to stay off on every calling thread. Shimforge checks
the setting but never disables it. Do not change stack protection settings while
local patches are installed.

Keep the session alive while using the mock. Drop restores original behavior,
including during a panic, and checks expectations without raising a second panic.
`session.restore()` removes the mocks early and checks expectations. Configure
mocks and run checkpoints while their calls are stopped. Join workers using
global mocks before verification and cleanup.

Functions, methods, and concrete generic functions are supported. `replace!` is
also available for simple replacements, unsafe functions, and native calls with
matching `extern` signatures. In local mode, each compiled macro site stays bound
to one target function; do not reuse that site for different function pointers.

## Safety and limits

The public API checks signatures and handles unsafe operations inside the library.
You do not need an `unsafe` block, but you must follow these rules to avoid memory
errors. No thread may call a target during its first code patch. Global code
patches also need calls stopped during restoration, including drop. The session
lock coordinates sessions, not function calls.

Source and replacement must match in calling convention, argument and return
layout, and lifetimes for every caller. Keep global replacements loaded until
restored and locally patched functions loaded until process exit.
Do not mock memory allocation, locking, or OS functions that shimforge uses.
On macOS, use a normal test executable without Hardened Runtime. Shimforge does
not change signing settings, entitlements, or system security settings.
Lifetime checks catch common mistakes in ordinary Rust functions. Generic
instances, nested borrowed types, pre-cast pointers, and unsafe or native
functions still need manual lifetime checks.

The test profile above reduces inlining and other optimizations without source
changes. It cannot undo calls already inlined in prebuilt libraries or separate
merged functions. Short entries and unsupported instruction forms are rejected.
Only the supplied entry point is
patched; import jumps are not followed. `replace!` accepts only noncapturing
closures and cannot swap different `async fn` future types. Use `mock!` for
captured closures and `mock_async` for native futures.

Memory permissions are restored after each write. A memory error from `restore()`
keeps the failed patch for retry. An expectation error is returned after removing
the mocks. A failed rollback or failed code restoration during drop aborts the
process.

MIT licensed.

## Contributing

Run `./scripts/verify.ps1` on Windows or `bash scripts/verify.sh` on Linux or macOS.
The checks require rustfmt, clippy, llvm-tools-preview, and cargo-llvm-cov 0.8.7.
Use `./scripts/validate-docker.ps1` to run Linux checks in Docker.
All platforms require 100% line and function coverage of library code. Tests and
dependencies are excluded. Reports are saved under `coverage/`.
