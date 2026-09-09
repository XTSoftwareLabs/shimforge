# shimforge

Scoped function replacement for Rust tests, without adding traits or dependency
injection. Supports Windows x86-64 and Linux x86-64.

```rust
use shimforge::{replace, Session};

#[inline(never)]
fn read_value() -> u32 {
    std::hint::black_box(7)
}

let mut session = Session::new()?;
replace!(session, read_value => || 42, fn() -> u32)?;
assert_eq!(read_value(), 42);
session.restore()?;
assert_eq!(read_value(), 7);
# Ok::<(), shimforge::Error>(())
```

Keep the session alive while exercising the mock. Dropping it restores the original
code, including during a panic. Replacements affect all threads. Only one session
can be active; another attempt returns `Error::Busy` immediately.

Use an explicit function pointer signature with `replace!`. Free functions,
inherent methods, individual generic instantiations, and non-capturing closures
are supported. For a method, include `&Self` or `&mut Self` as the first argument.
ABI-qualified function pointers can replace matching native functions.

## Test configuration

Add shimforge as a development dependency. In the workspace root manifest:

```toml
[profile.test]
opt-level = 0
lto = false
codegen-units = 1
incremental = false
```

Run tests that share process-wide state serially:

```text
cargo test -- --test-threads=1
```

## Safety and limits

The public macro encapsulates unsafe operations and checks signatures. Runtime
patching is best effort; it cannot provide Rust's usual memory-safety guarantees
when the following requirements are violated. All calls must be quiescent during
installation and restoration, including drop; join worker threads before ending
the session. The session lock does not stop ordinary calls. Source and replacement
must have identical calling conventions, argument and return layouts, and lifetime
requirements for every original caller. Keep their code loaded for the full scope.
Do not replace allocator, synchronization, or OS functions used by shimforge itself.

Inlining, function merging, and optimized-away calls can bypass or broaden a mock.
Use unoptimized tests and distinct function bodies; `#[inline(never)]` helps for
functions you own. Very short function entries are rejected. Import thunks are not
followed: only calls reaching the supplied address are affected. Capturing closures
and replacing distinct opaque `async fn` future types are not supported.

Memory permissions are restored after each write. If restoration fails, `restore()`
returns an error for retry. An unrecoverable rollback or a failed restoration during
drop aborts the process rather than leaving an unowned replacement active.

MIT licensed.

## Contributing

Run `./scripts/verify.ps1` on Windows or `bash scripts/verify.sh` on Linux.
The checks require rustfmt, clippy, llvm-tools-preview, and cargo-llvm-cov 0.8.7.
`./scripts/validate-docker.ps1` builds and runs a reusable Linux test environment.
Both platforms enforce 100% line and function coverage of library source; test
fixtures and dependencies are excluded. Reports are written under `coverage/`.
