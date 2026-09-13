# shimforge

[![CI](https://github.com/XTSoftwareLabs/shimforge/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/XTSoftwareLabs/shimforge/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/shimforge.svg)](https://crates.io/crates/shimforge)
[![docs.rs](https://img.shields.io/docsrs/shimforge)](https://docs.rs/shimforge)
[![License: PolyForm](https://img.shields.io/badge/license-PolyForm-blue.svg)](#licensing)
[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://github.com/XTSoftwareLabs/shimforge/blob/main/Cargo.toml)

Test Rust code without test-only traits or changes to production code.

## Why shimforge

In a large codebase the hard part of unit testing is usually the dependencies.
Disk access, sockets, clocks, and free functions in other crates all make
otherwise clean code impossible to test in isolation.

The usual answer is to change the production code first: introduce a trait,
thread it through every caller, and keep it forever even though only one
implementation ever ships. Take this function:

```rust
use std::fs;

fn claim_slot() -> Result<(), String> {
    if let Err(error) = fs::create_dir_all("/var/run/dispatcher") {
        // Failure path.
        return Err(format!("cannot claim a slot: {error}"));
    }

    // Success path.
    Ok(())
}
```

It is readable and it is not unit testable, because `fs::create_dir_all` needs a
real directory. With shimforge you test both paths without editing it and
without preparing an environment:

```rust
use shimforge::{Session, mock};
use std::io;

#[test]
fn claim_slot_succeeds_without_a_real_directory() {
    let mut session = Session::new();
    let create = mock!(
        session,
        fs::create_dir_all::<&str>,
        fn(&str) -> io::Result<()>
    );
    create
        .expect()
        .with(|path| *path == "/var/run/dispatcher")
        .once()
        .returning(|_| Ok(()));

    assert!(claim_slot().is_ok());
    session.verify();
}
```

`claim_slot` is unchanged, `fs::create_dir_all` never reaches the disk, and the
mock fails the test unless it is called exactly once with that path.

## Supported targets

| OS | Architectures |
| --- | --- |
| Linux | x86-64, ARM64 |
| macOS | x86-64, ARM64 |
| Windows | x86-64, ARM64 |

## Usage

Add shimforge as a dev dependency:

```toml
[dev-dependencies]
shimforge = "0.1"
```

Keep your production code as it is, and add these settings to the workspace root
`Cargo.toml`. They reduce inlining so that calls still reach a patchable entry
point:

```toml
[profile.test]
opt-level = 0
debug = true
lto = false
codegen-units = 1
incremental = false
```

Import the two macros and the session type, then run `cargo test` as usual:

```rust
use shimforge::{Session, mock, replace};
```

Every example below is a complete test. shimforge panics when a mock cannot be
installed or an expectation is not met, so there is no `Result` to handle.

## Thread-local and global sessions

`Session::new()` mocks functions **for the current thread only**, so tests run in
parallel without interfering with each other. Other threads keep calling the
original function.

Use `Session::new_global()` when the code under test hands work to threads you do
not control — background workers, timers, thread pools, or async tasks that move
between threads. Global sessions take an exclusive lock, so they run one at a time.

```rust
use shimforge::{Session, mock};

fn worker_count() -> usize {
    2
}

#[test]
fn other_threads_keep_the_original_function() {
    let mut session = Session::new();
    let count = mock!(session, worker_count, fn() -> usize);
    count.expect().returns(16);

    assert_eq!(worker_count(), 16);
    // A thread with no session of its own still calls the original.
    assert_eq!(std::thread::spawn(worker_count).join().unwrap(), 2);
}
```

## Constant results

The shortest useful mock returns the same value every time. This is often all a
boolean predicate needs:

```rust
use shimforge::{Session, mock};
use std::path::Path;

fn export_state(marker: &Path) -> &'static str {
    if marker.exists() {
        "finished"
    } else {
        "running"
    }
}

#[test]
fn a_constant_result_answers_every_call() {
    let mut session = Session::new();
    let exists = mock!(session, Path::exists, fn(&Path) -> bool);
    exists.expect().returns(true);

    assert_eq!(export_state(Path::new("virtual/export.done")), "finished");
    session.restore();
    assert_eq!(export_state(Path::new("virtual/export.done")), "running");
}
```

## Matching arguments and counting calls

`with` inspects the borrowed arguments, the count methods say how often the rule
may run, and the return methods say what comes back. A rule is complete once it
has a return behaviour or `never()`.

```rust
use shimforge::{Session, mock};
use std::{fs, io, path::Path};

fn load_port(path: &Path) -> io::Result<u16> {
    fs::read_to_string(path)?
        .trim()
        .parse()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[test]
fn matching_calls_are_counted() {
    let mut session = Session::new();
    let read = mock!(
        session,
        fs::read_to_string::<&Path>,
        fn(&Path) -> io::Result<String>
    );
    read.expect()
        .with(|path| *path == Path::new("service.port"))
        .times(2)
        .returning(|_| Ok("8080\n".to_owned()));

    assert_eq!(load_port(Path::new("service.port")).unwrap(), 8080);
    assert_eq!(load_port(Path::new("service.port")).unwrap(), 8080);
    session.verify();
}
```

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

The first matching rule with calls left wins, so put specific rules before
fallbacks. An unmatched call, an extra call, or a wrong order panics and makes
verification fail. The default count allows any number of calls; one-time
responses default to exactly one. The returned handle has `calls()` and
`verify()`, and dropping it leaves the expectation in place.

## Return values and call order

`returns` clones a value, `return_once` moves it, and `returning` runs a closure
that may capture state and update it between calls. A `Sequence` checks the order
of calls across different mocks.

```rust
use shimforge::{Sequence, Session, mock};

fn next_id() -> u64 {
    1
}

fn save_id(id: u64) -> bool {
    id > 0
}

#[test]
fn results_follow_the_call_order() {
    let mut session = Session::new();
    let next = mock!(session, next_id, fn() -> u64);
    let save = mock!(session, save_id, fn(u64) -> bool);
    let order = Sequence::new();
    let mut id = 40;
    next.expect()
        .times(2)
        .in_sequence(&order)
        .returning(move || {
            id += 1;
            id
        });
    save.expect()
        .with(|id| *id == 42)
        .once()
        .in_sequence(&order)
        .returns(true);

    assert_eq!(next_id(), 41);
    assert_eq!(next_id(), 42);
    assert!(save_id(42));
    session.verify();
}
```

## Writing through reference parameters

A `returning` closure receives the arguments, so it can fill output parameters
and mutable buffers as well as produce a result.

```rust
use shimforge::{Session, mock};

fn split_amount(total: u64, whole: &mut u64, cents: &mut u64) {
    *whole = total / 100;
    *cents = total % 100;
}

fn fill(buffer: &mut [u8]) -> usize {
    buffer.fill(0);
    buffer.len()
}

#[test]
fn a_mock_fills_output_parameters() {
    let mut session = Session::new();
    let split = mock!(session, split_amount, fn(u64, &mut u64, &mut u64));
    split
        .expect()
        .with(|total, _, _| *total == 1234)
        .once()
        .returning(|_, whole, cents| {
            *whole = 99;
            *cents = 5;
        });
    let write = mock!(session, fill, fn(&mut [u8]) -> usize);
    write.expect().once().returning(|buffer| {
        buffer[..2].copy_from_slice(b"ok");
        2
    });

    let (mut whole, mut cents) = (0, 0);
    split_amount(1234, &mut whole, &mut cents);
    assert_eq!((whole, cents), (99, 5));

    let mut buffer = [0; 8];
    assert_eq!(fill(&mut buffer), 2);
    assert_eq!(&buffer[..2], b"ok");
}
```

## Methods and generic functions

Methods are mocked through their path, with the receiver as the first argument.
A generic function compiles to one function per set of type arguments, so name
the instantiation you want with a turbofish; the others keep their original body.

```rust
use shimforge::{Session, mock};
use std::fmt::Display;

struct Cache {
    region: String,
}

impl Cache {
    fn hit_rate(&self, key: &str) -> f32 {
        // Reads live counters in production.
        (self.region.len() + key.len()) as f32 / 100.0
    }
}

fn render<T: Display>(value: T) -> String {
    format!("live {value}")
}

#[test]
fn a_method_and_one_generic_instance_are_mocked() {
    let mut session = Session::new();
    let rates = mock!(session, Cache::hit_rate, fn(&Cache, &str) -> f32);
    rates
        .expect()
        .with(|cache, key| cache.region == "eu" && *key == "sessions")
        .once()
        .returns(0.75);
    let rendered = mock!(session, render::<u8>, fn(u8) -> String);
    rendered.expect().once().returns("mocked".to_owned());

    let cache = Cache {
        region: "eu".to_owned(),
    };
    assert_eq!(cache.hit_rate("sessions"), 0.75);
    assert_eq!(render(7u8), "mocked");
    // A different type argument is a different function.
    assert_eq!(render("7"), "live 7");
}
```

## Replacing a whole function

When you do not need argument matching or call counts, `replace!` swaps in
another function or a closure with no captures. It checks both signatures at
compile time and needs no `unsafe` block.

```rust
use shimforge::{Session, replace};

fn checksum(bytes: &[u8]) -> u32 {
    bytes.iter().map(|byte| u32::from(*byte)).sum()
}

fn fixed_checksum(_bytes: &[u8]) -> u32 {
    7
}

#[test]
fn a_function_or_closure_replaces_the_original() {
    let mut session = Session::new();
    replace!(session, checksum => fixed_checksum, fn(&[u8]) -> u32);
    assert_eq!(checksum(b"abc"), 7);
    session.restore();

    replace!(session, checksum => |_| 9, fn(&[u8]) -> u32);
    assert_eq!(checksum(b"abc"), 9);
}
```

## Async functions

Pass an unpolled future to `mock_async`. Its concrete future type is mocked; the
witness is dropped without running its body, and its arguments do not matter.
Every poll returns a ready result, so a normal `await` counts once. The example
polls by hand so that it needs no executor.

```rust
use shimforge::Session;
use std::{
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};

async fn exchange_rate(_pair: &str) -> f64 {
    // Calls a pricing service in production.
    0.0
}

struct Ledger {
    name: String,
}

impl Ledger {
    async fn balance(&self) -> u64 {
        // Reads this ledger from a database in production.
        self.name.len() as u64
    }
}

fn ready<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("a mocked future is always ready"),
    }
}

#[test]
fn async_functions_return_mocked_results() {
    let mut session = Session::new();
    let rates = session.mock_async(exchange_rate(""));
    rates.expect().once().returns(1.25);
    // A throwaway receiver is enough to name the future type.
    let balances = session.mock_async(
        Ledger {
            name: String::new(),
        }
        .balance(),
    );
    balances.expect().once().returns(4_200);

    assert_eq!(ready(exchange_rate("EURUSD")), 1.25);
    let ledger = Ledger {
        name: "payroll".to_owned(),
    };
    assert_eq!(ready(ledger.balance()), 4_200);
    session.verify();
}
```

Async results must be `Send + 'static`, though the future itself may borrow its
arguments. Arguments stored inside a native future cannot be matched, and the
mock always reports `Ready`.

## Client libraries that return boxed futures

Client libraries usually expose a method that builds a boxed future. Mock that
method with `mock!` instead of `mock_async`: you keep argument matching, and you
avoid mocking the shared poll wrapper that every boxed future shares.

```rust
use shimforge::{Session, mock};
use std::{
    future::Future,
    io,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};

type Call<'a> = Pin<Box<dyn Future<Output = io::Result<String>> + Send + 'a>>;

struct Client {
    endpoint: String,
}

impl Client {
    fn get<'a>(&'a self, path: &'a str) -> Call<'a> {
        Box::pin(async move {
            // Opens a connection in production.
            Ok(format!("live {}{path}", self.endpoint))
        })
    }
}

#[test]
fn a_client_method_that_returns_a_boxed_future_is_mocked() {
    let mut session = Session::new();
    let get = mock!(
        session,
        Client::get,
        for<'a> fn(&'a Client, &'a str) -> Call<'a>
    );
    get.expect()
        .with(|_, path| *path == "/health")
        .once()
        .returning(|_, _| Box::pin(async { Ok("healthy".to_owned()) }));

    let client = Client {
        endpoint: "https://inventory.invalid".to_owned(),
    };
    let mut response = pin!(client.get("/health"));
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(
        response.as_mut().poll(&mut context),
        Poll::Ready(Ok(body)) if body == "healthy"
    ));
    session.verify();
}
```

## System and C runtime functions

`mock!` also accepts `unsafe fn`, `extern "C" fn`, and `extern "system" fn`
signatures, including functions imported from the operating system or the C
runtime. Installing the mock needs no `unsafe` block; calling an unsafe function
still does.

```rust
use shimforge::{Session, mock};
use std::ffi::{CStr, CString, c_char};

unsafe extern "C" {
    fn getenv(name: *const c_char) -> *mut c_char;
}

#[test]
fn getenv_reports_a_mocked_variable() {
    let mut session = Session::new();
    let lookup = mock!(
        session,
        getenv,
        unsafe extern "C" fn(*const c_char) -> *mut c_char
    );
    lookup
        .expect()
        .with(|name| {
            // SAFETY: callers of getenv always pass a valid C string.
            let name = unsafe { CStr::from_ptr(*name) };
            name == c"DEPLOY_SLOT"
        })
        .once()
        .returning(|_| c"canary".as_ptr().cast_mut());
    // Any other variable keeps reporting that it is unset.
    lookup.expect().returning(|_| std::ptr::null_mut());

    let key = CString::new("DEPLOY_SLOT").unwrap();
    // SAFETY: the key is a valid C string and the result is only read.
    let slot = unsafe { CStr::from_ptr(getenv(key.as_ptr())) };
    assert_eq!(slot.to_str().unwrap(), "canary");
}
```

A panic aborts the process when the declared ABI does not allow unwinding, so
prefer returning error values from `extern "C"` mocks.

## Raw replacement without signature checks

`replace_raw` takes two function pointers and installs one over the other with no
type checking at all. It is the escape hatch for cases the macros cannot express,
and it requires a global session and an `unsafe` block.

```rust
use shimforge::Session;

fn slot_count() -> usize {
    4
}

fn fake_slot_count() -> usize {
    64
}

#[test]
fn a_raw_replacement_swaps_one_function_for_another() {
    let mut session = Session::new_global();
    // SAFETY: both functions are live, share a signature, and stay loaded until the
    // session restores them. No thread calls them while the patch is installed.
    unsafe { session.replace_raw(slot_count as *const (), fake_slot_count as *const ()) };

    assert_eq!(slot_count(), 64);
    session.restore();
    assert_eq!(slot_count(), 4);
}
```

## Session lifetime

Keep the session alive while you use the mocks. Drop restores the original
behaviour, including during a panic, and checks expectations without raising a
second panic. `session.restore()` removes the mocks early and checks them, and
`session.verify()` checks them without removing anything.

Each thread may hold one session. Local sessions may coexist on different threads,
even for the same function. Global sessions wait for other sessions to finish, and
local sessions wait for an active global session. Opening a nested session panics;
`try_new_local()` and `try_new_global()` return `Err(Error::Busy)` instead of
waiting. Both modes support `mock!`, `replace!`, and `mock_async`; `replace_raw`
requires a global session.

Install a local mock before calls to that target start. Only the first local
installation changes code; later installs and cleanup leave the entry in place so
other threads keep working. One code page per target is kept until the process
exits, while mock state is released when the session ends. Loop instructions and
unsupported relocation forms are rejected in local mode. Some entries need a call
bridge, which requires shadow stacks to stay off on every calling thread;
shimforge checks the setting but never changes it.

Configure mocks and run checkpoints while their calls are stopped, and join
workers that use global mocks before verification and cleanup. In local mode each
compiled macro site stays bound to one target function, so do not reuse a site for
different function pointers.

Every Rust example in this README is also an integration test in
[`tests/readme.rs`](tests/readme.rs), which fails if the two stop matching.
More cases are in
[`tests/expectations.rs`](tests/expectations.rs),
[`tests/async_expectations.rs`](tests/async_expectations.rs),
[`tests/filesystem.rs`](tests/filesystem.rs), [`tests/generics.rs`](tests/generics.rs),
and [`tests/io.rs`](tests/io.rs). Imported OS and C runtime calls are covered by
[`tests/native_expectations.rs`](tests/native_expectations.rs) and
[`tests/cruntime.rs`](tests/cruntime.rs), and network clients by
[`tests/async_network.rs`](tests/async_network.rs) and
[`tests/http_client.rs`](tests/http_client.rs).

## Licensing

shimforge is copyright XT SOFTWARE LABS LLC and source-available under either of
two licenses, at your option. You need to satisfy only one of them.

- [PolyForm Small Business 1.0.0](LICENSE-POLYFORM-SMALL-BUSINESS), for a company
  with fewer than 100 people and less than 1,000,000 USD (2019, inflation
  adjusted) of revenue in the prior tax year. Parent companies, subsidiaries, and
  entities under common control count together.
- [PolyForm Noncommercial 1.0.0](LICENSE-POLYFORM-NONCOMMERCIAL), for any
  noncommercial purpose. This covers personal study, hobby projects, and
  research, and it covers charitable organizations, educational institutions,
  public research organizations, and government institutions whatever their size.

Under either one, use, modification, and redistribution are free of charge. Keep
the license text with any copy you pass on.

Commercial use by a company above the Small Business thresholds is not covered by
either license. See [COMMERCIAL.md](COMMERCIAL.md) or write to
info@xtsoftwarelabs.com.

Neither license is approved by the Open Source Initiative. If your dependency
policy allows only OSI licenses, treat shimforge as commercial software and ask
for a license rather than assuming it is blocked.
