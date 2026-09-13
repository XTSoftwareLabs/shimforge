#![forbid(unsafe_code)]

use shimforge::{Session, replace};

fn original(value: u64) -> u64 {
    value + 1
}

#[test]
fn public_api_works_when_the_calling_crate_forbids_unsafe_code() {
    let mut session = Session::new_global();
    replace!(session, original => |value| value + 10, fn(u64) -> u64);
    assert_eq!(original(2), 12);
    session.restore();
    assert_eq!(original(2), 3);
}
