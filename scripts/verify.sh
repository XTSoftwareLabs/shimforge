#!/usr/bin/env bash
# Requires rustfmt, clippy, llvm-tools-preview, and cargo-llvm-cov 0.8.7.
set -euo pipefail

cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."

cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked -- --test-threads=1
cargo test --test parallel --locked -- --test-threads=8

mkdir -p coverage/linux
cargo llvm-cov --workspace --all-targets --locked \
    --ignore-filename-regex '(^|[/\\])tests([/\\]|\.rs$)' \
    --fail-under-lines 100 --fail-under-functions 100 \
    --lcov --output-path coverage/linux/lcov.info \
    -- --test-threads=1
