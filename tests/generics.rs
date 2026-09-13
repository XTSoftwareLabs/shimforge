//! Mocks bind to one instantiation of a generic function.

#![forbid(unsafe_code)]

use shimforge::{Session, mock};
use std::fmt::Display;
use std::io;
use std::path::Path;

fn archive<P: AsRef<Path>>(_target: P) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::ReadOnlyFilesystem,
        "archive is sealed",
    ))
}

fn summarize<A: Display, B: Display, C: Display>(_head: A, _flag: B, _count: C) -> String {
    String::from("live summary")
}

fn is_stale<A, B, C>(_head: A, _flag: B, _count: C) -> bool {
    false
}

fn joined<S: AsRef<str>>(_separator: &str, _parts: &[S]) -> String {
    String::from("live join")
}

struct Queue {
    name: String,
}

impl Queue {
    fn publish<T: Display>(&self, payload: T) -> String {
        format!("{} <- {payload}", self.name)
    }
}

fn stale_with_borrowed_count<'a>(count: &'a str) -> bool {
    is_stale::<u16, bool, &'a str>(7, false, count)
}

#[test]
fn one_instantiation_is_mocked_and_restored_with_the_session() {
    let target = Path::new("virtual/ledger.tar");
    assert!(archive(target).is_err());
    {
        let mut session = Session::new();
        let archives = mock!(session, archive::<&Path>, fn(&Path) -> io::Result<()>);
        archives
            .expect()
            .with(|path| **path == *Path::new("virtual/ledger.tar"))
            .once()
            .returning(|_| Ok(()));
        assert!(archive(target).is_ok());
        session.verify();
    }
    assert_eq!(
        archive(target).unwrap_err().kind(),
        io::ErrorKind::ReadOnlyFilesystem
    );
}

#[test]
fn other_instantiations_keep_the_original_body() {
    let mut session = Session::new();
    let summaries = mock!(
        session,
        summarize::<&str, bool, i32>,
        fn(&str, bool, i32) -> String
    );
    let expected = summaries
        .expect()
        .with(|head, flag, count| **head == *"orders" && *flag && *count == 19)
        .once()
        .returns(String::from("mocked summary"));

    assert_eq!(summarize("orders", true, 19), "mocked summary");
    // A different set of type arguments compiles to a different function.
    assert_eq!(summarize(1u8, 2u8, 3u8), "live summary");
    assert_eq!(expected.calls(), 1);
}

#[test]
fn a_type_parameter_inside_a_slice_can_be_named_with_a_turbofish() {
    let mut session = Session::new();
    let joins = mock!(session, joined::<&str>, fn(&str, &[&str]) -> String);
    joins
        .expect()
        .with(|separator, parts| **separator == *" / " && parts.len() == 2)
        .once()
        .returning(|separator, parts| parts.join(separator));

    assert_eq!(joined(" / ", &["north", "south"]), "north / south");
    session.verify();
}

#[test]
fn a_caller_that_names_its_own_lifetime_reaches_the_same_instantiation() {
    let mut session = Session::new();
    let checks = mock!(
        session,
        is_stale::<u16, bool, &str>,
        fn(u16, bool, &str) -> bool
    );
    checks.expect().times(2).returns(true);

    assert!(is_stale(7u16, false, "12"));
    let count = String::from("12");
    assert!(stale_with_borrowed_count(&count));
    session.verify();
}

#[test]
fn generic_methods_are_mocked_per_type_argument() {
    let queue = Queue {
        name: String::from("reports"),
    };
    let mut session = Session::new();
    let publishes = mock!(session, Queue::publish::<u32>, fn(&Queue, u32) -> String);
    publishes
        .expect()
        .with(|queue, payload| queue.name == "reports" && *payload == 5)
        .once()
        .returning(|queue, payload| format!("{} dropped {payload}", queue.name));

    assert_eq!(queue.publish(5u32), "reports dropped 5");
    assert_eq!(queue.publish("5"), "reports <- 5");
    session.verify();
}
