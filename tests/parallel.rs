#![forbid(unsafe_code)]

use shimforge::{Session, mock};
use std::sync::{Arc, Barrier};

fn value(seed: u64) -> u64 {
    seed.wrapping_add(1)
}

fn isolated(seed: u64) {
    for _ in 0..100 {
        let mut session = Session::new().unwrap();
        let mock = mock!(session, value, fn(u64) -> u64).unwrap();
        mock.expect()
            .with(move |arg| *arg == seed)
            .times(3)
            .returns(seed + 20)
            .unwrap();
        for _ in 0..3 {
            assert_eq!(value(seed), seed + 20);
        }
        session.restore().unwrap();
        assert_eq!(value(seed), seed + 1);
    }
}

#[test]
fn first_parallel_test() {
    isolated(1);
}

#[test]
fn second_parallel_test() {
    isolated(2);
}

#[test]
fn third_parallel_test() {
    isolated(3);
}

#[test]
fn fourth_parallel_test() {
    isolated(4);
}

#[test]
fn repeated_cleanup_does_not_interrupt_other_threads() {
    let start = Arc::new(Barrier::new(8));
    let workers: Vec<_> = (0..8)
        .map(|index| {
            let start = start.clone();
            std::thread::spawn(move || {
                start.wait();
                isolated(10 + index);
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
}
