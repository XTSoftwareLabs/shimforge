//! Installing a mock must not disturb the calling thread's stack.
//!
//! Shimforge publishes its trampolines on pages allocated near the patched
//! function. A page placed inside a thread's stack reservation would break the
//! guard page and turn ordinary recursion into a stack overflow, so these tests
//! patch first and then use a large amount of stack.

#![forbid(unsafe_code)]

use shimforge::{Session, mock};
use std::sync::{Arc, Barrier};
use std::thread;

#[inline(never)]
fn retry_limit() -> u32 {
    std::hint::black_box(3)
}

#[inline(never)]
fn batch_size() -> u32 {
    std::hint::black_box(64)
}

#[inline(never)]
fn tracing_enabled() -> bool {
    std::hint::black_box(false)
}

/// Uses roughly 256 bytes of stack per frame.
#[inline(never)]
fn descend(depth: u32) -> u64 {
    if depth == 0 {
        return 1;
    }
    let frame = std::hint::black_box([0u8; 256]);
    let total = descend(depth - 1);
    u64::from(std::hint::black_box(frame[0])) + total
}

#[test]
fn deep_recursion_still_works_after_a_mock_is_installed() {
    let mut session = Session::new().unwrap();
    let limit = mock!(session, retry_limit, fn() -> u32).unwrap();
    limit.expect().returns(9).unwrap();

    assert_eq!(retry_limit(), 9);
    assert_eq!(descend(1024), 1);
    assert_eq!(retry_limit(), 9);
}

#[test]
fn threads_that_patch_at_the_same_time_keep_their_stacks() {
    let workers = 8;
    let start = Arc::new(Barrier::new(workers));
    let handles: Vec<_> = (0..workers)
        .map(|index| {
            let start = Arc::clone(&start);
            thread::Builder::new()
                .stack_size(1 << 20)
                .spawn(move || {
                    let mut session = Session::new().unwrap();
                    match index % 3 {
                        0 => {
                            let limit = mock!(session, retry_limit, fn() -> u32).unwrap();
                            limit.expect().returns(11).unwrap();
                            assert_eq!(retry_limit(), 11);
                        }
                        1 => {
                            let size = mock!(session, batch_size, fn() -> u32).unwrap();
                            size.expect().returns(256).unwrap();
                            assert_eq!(batch_size(), 256);
                        }
                        _ => {
                            let tracing = mock!(session, tracing_enabled, fn() -> bool).unwrap();
                            tracing.expect().returns(true).unwrap();
                            assert!(tracing_enabled());
                        }
                    }
                    // Every thread has patched before any of them uses the stack.
                    start.wait();
                    assert_eq!(descend(768), 1);
                })
                .unwrap()
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(retry_limit(), 3);
    assert_eq!(batch_size(), 64);
    assert!(!tracing_enabled());
}
