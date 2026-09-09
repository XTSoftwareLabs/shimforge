#![forbid(unsafe_code)]

use shimforge::{Session, replace};
use std::hint::black_box;

#[inline(never)]
fn original(value: u64) -> u64 {
    black_box(value) + black_box(1)
}

#[test]
fn public_api_works_when_the_calling_crate_forbids_unsafe_code() {
    let mut session = Session::new().unwrap();
    replace!(session, original => |value| black_box(value) + 10, fn(u64) -> u64).unwrap();
    assert_eq!(original(black_box(2)), 12);
    session.restore().unwrap();
    assert_eq!(original(black_box(2)), 3);
}
