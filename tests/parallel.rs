#![forbid(unsafe_code)]

use shimforge::{Session, mock};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

fn value(seed: u64) -> u64 {
    seed.wrapping_add(1)
}

fn label(seed: u64) -> String {
    format!("live {seed}")
}

fn discard(seed: u64) {
    assert_ne!(seed, 0, "the original body runs for every unmocked thread");
}

struct Meter {
    offset: u64,
}

impl Meter {
    fn reading(&self, seed: u64) -> u64 {
        self.offset + seed
    }
}

fn isolated(seed: u64) {
    for _ in 0..100 {
        let mut session = Session::new();
        let mock = mock!(session, value, fn(u64) -> u64);
        mock.expect()
            .with(move |arg| *arg == seed)
            .times(3)
            .returns(seed + 20);
        for _ in 0..3 {
            assert_eq!(value(seed), seed + 20);
        }
        session.restore();
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

#[test]
fn calls_from_another_thread_are_safe_during_setup_and_teardown() {
    // Only the first mock for a function patches its code, and a thread must not
    // execute the entry while those bytes are written. Install once here so the
    // reader below never races that write; later installs reuse the same entry.
    {
        let mut session = Session::new();
        let mock = mock!(session, value, fn(u64) -> u64);
        mock.expect().once().returns(0);
        assert_eq!(value(41), 0);
        session.restore();
    }
    let stop = Arc::new(AtomicBool::new(false));
    let reader = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut calls = 0u64;
            while !stop.load(Ordering::Relaxed) {
                // This thread holds no session, so it always sees the original.
                assert_eq!(value(41), 42);
                calls += 1;
            }
            calls
        })
    };
    for round in 0..200 {
        let mut session = Session::new();
        let mock = mock!(session, value, fn(u64) -> u64);
        mock.expect().once().returns(round);
        assert_eq!(value(41), round);
        session.restore();
    }
    stop.store(true, Ordering::Relaxed);
    assert!(reader.join().unwrap() > 0);
    assert_eq!(value(41), 42);
}

#[test]
fn struct_methods_keep_one_result_per_thread() {
    let meter = Meter { offset: 10 };
    let start = Arc::new(Barrier::new(3));
    let workers: Vec<_> = [100, 200]
        .into_iter()
        .map(|result| {
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                let mut session = Session::new();
                let readings = mock!(session, Meter::reading, fn(&Meter, u64) -> u64);
                readings.expect().times(50).returns(result);
                start.wait();
                let local = Meter { offset: 10 };
                for _ in 0..50 {
                    assert_eq!(local.reading(5), result);
                }
                session.restore();
            })
        })
        .collect();
    start.wait();
    for _ in 0..50 {
        assert_eq!(meter.reading(5), 15);
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(meter.reading(5), 15);
}

#[test]
fn unit_functions_keep_one_behavior_per_thread() {
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    let start = Arc::new(Barrier::new(2));
    let observer = {
        let start = Arc::clone(&start);
        std::thread::spawn(move || {
            let mut session = Session::new();
            let discards = mock!(session, discard, fn(u64));
            discards.expect().times(20).returning(|_| {
                CALLS.fetch_add(1, Ordering::SeqCst);
            });
            start.wait();
            for _ in 0..20 {
                discard(0);
            }
            session.restore();
        })
    };
    start.wait();
    // The original body asserts on this argument, so the mock never leaked here.
    for seed in 1..=20 {
        discard(seed);
    }
    observer.join().unwrap();
    assert_eq!(CALLS.load(Ordering::SeqCst), 20);
}

#[test]
fn owned_results_keep_one_value_per_thread() {
    let start = Arc::new(Barrier::new(2));
    let worker = {
        let start = Arc::clone(&start);
        std::thread::spawn(move || {
            let mut session = Session::new();
            let labels = mock!(session, label, fn(u64) -> String);
            labels
                .expect()
                .times(30)
                .returning(|seed| format!("worker {seed}"));
            start.wait();
            for _ in 0..30 {
                assert_eq!(label(8), "worker 8");
            }
            session.restore();
        })
    };
    start.wait();
    for _ in 0..30 {
        assert_eq!(label(8), "live 8");
    }
    worker.join().unwrap();
}
