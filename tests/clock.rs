//! Time-dependent logic tested at chosen instants, without a clock trait.
//!
//! Code that calls `SystemTime::now()` directly usually has to be rewritten to
//! take a clock before a test can choose the time. Mocking `SystemTime::now`
//! lets the tests below pick the time, step across boundaries, and let time pass
//! without sleeping, while the code under test stays as it is.

#![forbid(unsafe_code)]

use shimforge::{Session, mock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 2026-01-01T00:00:00Z.
const NEW_YEAR: u64 = 1_767_225_600;

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

// Business code below reads the system clock directly and stays unchanged.

fn is_morning() -> bool {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_secs();
    let hour = seconds % 86_400 / 3_600;
    (7..12).contains(&hour)
}

struct Token {
    expires_at: SystemTime,
}

impl Token {
    /// A token stays valid for 30 seconds past its expiry to absorb clock skew.
    fn is_expired(&self) -> bool {
        SystemTime::now() > self.expires_at + Duration::from_secs(30)
    }
}

#[test]
fn the_morning_check_runs_at_a_chosen_time() {
    let mut session = Session::new();
    let now = mock!(session, SystemTime::now, fn() -> SystemTime);
    now.expect().once().returns(at(NEW_YEAR + 10 * 3_600));
    now.expect().once().returns(at(NEW_YEAR + 13 * 3_600));

    assert!(is_morning());
    assert!(!is_morning());
    session.verify();
}

#[test]
fn both_edges_of_the_morning_are_checked() {
    let edges = [
        (6 * 3_600 + 59 * 60 + 59, false),
        (7 * 3_600, true),
        (11 * 3_600 + 59 * 60 + 59, true),
        (12 * 3_600, false),
    ];
    let mut session = Session::new();
    let now = mock!(session, SystemTime::now, fn() -> SystemTime);
    let mut times = edges.map(|(offset, _)| at(NEW_YEAR + offset)).into_iter();
    now.expect()
        .times(4)
        .returning(move || times.next().unwrap());

    for (offset, morning) in edges {
        assert_eq!(is_morning(), morning, "{offset} seconds after midnight");
    }
    session.verify();
}

#[test]
fn a_token_expires_as_time_passes_without_sleeping() {
    let mut session = Session::new();
    let now = mock!(session, SystemTime::now, fn() -> SystemTime);
    let mut current = at(NEW_YEAR);
    // Each reading of the clock is 20 seconds after the previous one.
    now.expect().times(4).returning(move || {
        let reading = current;
        current += Duration::from_secs(20);
        reading
    });

    let token = Token {
        expires_at: at(NEW_YEAR + 10),
    };
    let checks: Vec<bool> = (0..4).map(|_| token.is_expired()).collect();
    // Expired only once the clock is more than 30 seconds past the expiry.
    assert_eq!(checks, [false, false, false, true]);
    session.verify();
}

#[test]
fn other_threads_keep_the_real_clock() {
    let mut session = Session::new();
    let now = mock!(session, SystemTime::now, fn() -> SystemTime);
    now.expect().returns(at(NEW_YEAR));
    assert_eq!(SystemTime::now(), at(NEW_YEAR));

    // A thread with no session of its own reads the machine clock.
    let real = thread::spawn(SystemTime::now).join().unwrap();
    assert!(real > at(NEW_YEAR), "{real:?}");
}
