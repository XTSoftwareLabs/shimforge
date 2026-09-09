# shimforge

Mock Rust functions without traits or dependency injection.
Supports Windows x86-64 and Linux x86-64.

```rust
use shimforge::{replace, Session};

#[inline(never)]
fn read_value() -> u32 {
    std::hint::black_box(7)
}

let mut session = Session::new().unwrap();
replace!(session, read_value => || 42, fn() -> u32).unwrap();
assert_eq!(read_value(), 42);
session.restore().unwrap();
assert_eq!(read_value(), 7);
```

Keep the session alive while using the mock. Drop restores the original code,
including during a panic. Mocks affect all threads. Only one session can be active;
another attempt returns `Error::Busy` without waiting.

Pass a function signature to `replace!`. It accepts functions, methods, concrete
generic functions, and closures without captures. For methods, include the receiver
as the first argument. Use matching `extern` signatures for native functions.

## Test configuration

Add shimforge as a dev dependency. Use these settings in the root `Cargo.toml`:

```toml
[profile.test]
opt-level = 0
lto = false
codegen-units = 1
incremental = false
```

Run tests that share mocks one at a time:

```text
cargo test -- --test-threads=1
```

## Safety and limits

`replace!` checks signatures and handles unsafe operations inside the library.
You do not need an `unsafe` block, but you must follow these rules to avoid memory
errors. No thread may call the target while installing or restoring a mock,
including during drop. Join workers before ending the session; the session lock
does not stop function calls.

Source and replacement must match in calling convention, argument and return
layout, and lifetimes for every caller. Keep both functions loaded until restored.
Do not mock memory allocation, locking, or OS functions that shimforge uses.

The compiler can inline, merge, or remove functions, which can skip a mock or affect
other calls. Use unoptimized tests and distinct function bodies. `#[inline(never)]`
helps for functions you own. Short entries are rejected. Only the supplied entry
point is patched; import jumps are not followed. Capturing closures and replacements
between different `async fn` future types are not supported.

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
