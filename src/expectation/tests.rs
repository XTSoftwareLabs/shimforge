use super::*;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};

struct TestRule {
    meta: Arc<Meta>,
    value: usize,
    dropped: Arc<AtomicUsize>,
}

impl Rule for TestRule {
    fn meta(&self) -> &Arc<Meta> {
        &self.meta
    }
}

impl Drop for TestRule {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

fn add(state: &State<TestRule>, config: Config, value: usize) -> Expectation {
    state
        .add(config, |meta| TestRule {
            meta,
            value,
            dropped: Arc::default(),
        })
        .unwrap()
}

fn panics(action: impl FnOnce()) -> String {
    let panic = catch_unwind(AssertUnwindSafe(action)).unwrap_err();
    match panic.downcast::<String>() {
        Ok(message) => *message,
        Err(panic) => panic.downcast_ref::<&str>().unwrap().to_string(),
    }
}

#[test]
fn counts_accept_exact_and_range_bounds() {
    let cases: [(CallCount, usize, Option<usize>); 8] = [
        (3.into(), 3, Some(3)),
        ((2..5).into(), 2, Some(4)),
        ((2..=5).into(), 2, Some(5)),
        ((2..).into(), 2, None),
        ((..5).into(), 0, Some(4)),
        ((..=5).into(), 0, Some(5)),
        ((..).into(), 0, None),
        (0.into(), 0, Some(0)),
    ];
    for (count, min, max) in cases {
        assert_eq!(count.min, min);
        assert_eq!(count.max, max);
        assert!(count.validate().is_ok());
        assert!(!format!("{count:?}").is_empty());
    }
    let empty_start = 4;
    for count in [
        CallCount::from(0..0),
        (empty_start..2).into(),
        (empty_start..=2).into(),
        (..0).into(),
    ] {
        assert!(count.validate().is_err());
    }
    assert_eq!(CallCount::from(..=usize::MAX).max, Some(usize::MAX));
}

#[test]
fn one_shot_counts_are_checked() {
    let default = Config::default().for_once().unwrap().validate().unwrap();
    assert_eq!((default.min, default.max), (1, Some(1)));
    for count in [CallCount::from(0), 1.into(), (0..=1).into()] {
        assert!(Config::default().times(count).for_once().is_ok());
    }
    for count in [CallCount::from(2), (..).into(), (0..0).into()] {
        assert!(Config::default().times(count).for_once().is_err());
    }
}

#[test]
fn response_chains_skip_exhausted_rules() {
    let state = State::new("lookup");
    let first = add(&state, Config::default().once(), 10);
    let next = add(&state, Config::default().times(1..=2), 20);
    let other = add(&state, Config::default(), 30);
    assert!(first.meta.verify().is_err());
    assert!(state.verify().is_err());
    assert_eq!(state.select(&|rule| rule.value != 30).value, 10);
    first.verify();
    assert_eq!(first.clone().calls(), 1);
    for _ in 0..2 {
        assert_eq!(state.select(&|rule| rule.value != 30).value, 20);
    }
    assert_eq!(next.calls(), 2);
    assert_eq!(other.calls(), 0);
    state.verify().unwrap();
    assert!(panics(|| drop(state.select(&|rule| rule.value != 30))).contains("exhausted"));
    assert!(
        state
            .verify()
            .unwrap_err()
            .to_string()
            .contains("exhausted")
    );
    assert!(state.checkpoint().is_err());
}

#[test]
fn missing_and_forbidden_calls_stay_failed() {
    let state = State::new("write");
    assert!(panics(|| drop(state.select(&|_| true))).contains("no expectation"));
    let never = add(&state, Config::default().times(0), 1);
    never.verify();
    assert!(panics(|| drop(state.select(&|_| true))).contains("forbidden"));
    assert_eq!(never.calls(), 0);
    assert!(
        never
            .meta
            .verify()
            .unwrap_err()
            .to_string()
            .contains("forbidden")
    );
    assert!(
        state
            .verify()
            .unwrap_err()
            .to_string()
            .contains("no expectation")
    );
    state.deactivate();
    assert!(state.verify().is_err());
}

#[test]
fn sequence_tracks_multiple_calls_and_mocks() {
    let sequence = Sequence::new();
    let first = State::new("open");
    let second = State::new("close");
    let first_count = add(&first, Config::default().times(2).in_sequence(&sequence), 1);
    let second_count = add(&second, Config::default().once().in_sequence(&sequence), 2);
    first.select(&|_| true);
    assert_eq!(lock(&sequence.progress).cursor, 0);
    first.select(&|_| true);
    assert_eq!(lock(&sequence.progress).cursor, 1);
    second.select(&|_| true);
    assert_eq!(lock(&sequence.progress).cursor, 2);
    assert_eq!(first_count.calls(), 2);
    assert_eq!(second_count.calls(), 1);
    first.verify().unwrap();
    second.verify().unwrap();
}

#[test]
fn out_of_order_calls_stay_failed() {
    let sequence = Sequence::default();
    let state = State::new("steps");
    add(&state, Config::default().once().in_sequence(&sequence), 1);
    let later = add(&state, Config::default().once().in_sequence(&sequence), 2);
    assert!(panics(|| drop(state.select(&|rule| rule.value == 2))).contains("out of order"));
    assert_eq!(later.calls(), 0);
    state.select(&|rule| rule.value == 1);
    state.select(&|rule| rule.value == 2);
    assert!(
        later
            .meta
            .verify()
            .unwrap_err()
            .to_string()
            .contains("out of order")
    );
    assert!(state.verify().is_err());
}

#[test]
fn invalid_adds_do_not_use_sequence_slots() {
    let state = State::new("read");
    let sequence = Sequence::new();
    for config in [
        Config::default(),
        Config::default().times(0),
        Config::default().times(1..=2),
        Config::default().times(0..0),
    ] {
        assert!(
            state
                .add(config.in_sequence(&sequence), |_| -> TestRule {
                    panic!("invalid rule must not be built")
                })
                .is_err()
        );
    }
    assert_eq!(lock(&sequence.progress).next, 0);
    state.deactivate();
    assert!(
        state
            .add(
                Config::default().once().in_sequence(&sequence),
                |_| -> TestRule { panic!("inactive rule must not be built") }
            )
            .is_err()
    );
    assert_eq!(lock(&sequence.progress).next, 0);
}

#[test]
fn sequence_overflow_leaves_registration_unchanged() {
    let state = State::new("overflow");
    let sequence = Sequence::new();
    lock(&sequence.progress).next = usize::MAX;
    let dropped = Arc::new(AtomicUsize::new(0));
    assert!(
        state
            .add(Config::default().once().in_sequence(&sequence), |meta| {
                TestRule {
                    meta,
                    value: 1,
                    dropped: dropped.clone(),
                }
            })
            .is_err()
    );
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(lock(&sequence.progress).next, usize::MAX);
    assert!(lock(&state.inner).rules.is_empty());
    state.verify().unwrap();
}

#[test]
fn count_overflow_is_reported_without_wrapping() {
    let state = State::new("count");
    let expectation = add(&state, Config::default(), 1);
    lock(&expectation.meta.progress).calls = usize::MAX;
    assert!(panics(|| drop(state.select(&|_| true))).contains("overflow"));
    assert_eq!(expectation.calls(), usize::MAX);
    assert!(expectation.meta.verify().is_err());
}

#[test]
fn checkpoint_clears_only_completed_expectations() {
    let state = State::new("checkpoint");
    let dropped = Arc::new(AtomicUsize::new(0));
    let expectation = state
        .add(Config::default().once(), |meta| TestRule {
            meta,
            value: 1,
            dropped: dropped.clone(),
        })
        .unwrap();
    assert!(state.checkpoint().is_err());
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    state.select(&|_| true);
    state.checkpoint().unwrap();
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(expectation.calls(), 1);
    expectation.verify();
    state.checkpoint().unwrap();
    add(&state, Config::default(), 2);
    assert_eq!(state.select(&|_| true).value, 2);
    let control: &dyn Control = state.as_ref();
    control.verify().unwrap();
    control.deactivate();
    assert!(state.checkpoint().is_err());
    assert!(panics(|| drop(state.select(&|_| true))).contains("no longer active"));
    assert!(control.verify().is_err());
}

#[test]
fn registration_and_matchers_run_without_the_rule_lock() {
    let state = State::new("reentry");
    let sequence = Sequence::new();
    let dropped = Arc::new(AtomicUsize::new(0));
    assert!(
        state
            .add(Config::default().once().in_sequence(&sequence), |meta| {
                state.deactivate();
                TestRule {
                    meta,
                    value: 1,
                    dropped: dropped.clone(),
                }
            })
            .is_err()
    );
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(lock(&sequence.progress).next, 0);
    let state = State::new("matcher");
    add(&state, Config::default(), 1);
    state.select(&|_| {
        state.verify().unwrap();
        true
    });
}

#[test]
fn concurrent_calls_claim_each_slot_once() {
    let state = State::new("workers");
    let first = add(&state, Config::default().times(20), 1);
    let second = add(&state, Config::default().times(20), 2);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let state = state.clone();
            scope.spawn(move || {
                for _ in 0..10 {
                    state.select(&|_| true);
                }
            });
        }
    });
    assert_eq!(first.calls(), 20);
    assert_eq!(second.calls(), 20);
    state.verify().unwrap();
}

#[test]
fn recursive_guards_recover_after_panics() {
    let state = State::<TestRule>::new("recursive");
    let other = State::<TestRule>::new("other");
    let outer = state.enter();
    let inner = other.enter();
    assert!(panics(|| drop(state.enter())).contains("recursive: a mock called itself recursively"));
    assert!(state.verify().is_err());
    other.verify().unwrap();
    drop(outer);
    drop(state.enter());
    drop(inner);
    assert!(
        panics(|| {
            let _guard = state.enter();
            panic!("callback failed");
        })
        .contains("callback failed")
    );
    drop(state.enter());
    let _guard = state.enter();
    std::thread::spawn(move || drop(state.enter()))
        .join()
        .unwrap();
}

struct DropRule {
    meta: Arc<Meta>,
    state: std::sync::Weak<State<DropRule>>,
    dropped: Arc<AtomicUsize>,
}

impl Rule for DropRule {
    fn meta(&self) -> &Arc<Meta> {
        &self.meta
    }
}

impl Drop for DropRule {
    fn drop(&mut self) {
        self.state.upgrade().unwrap().verify().unwrap();
        self.dropped.fetch_add(1, Ordering::SeqCst);
        panic!("response drop failed");
    }
}

#[test]
fn panicking_response_drops_leave_the_rule_list_clear() {
    for checkpoint in [true, false] {
        let state = State::new("drop");
        let dropped = Arc::new(AtomicUsize::new(0));
        state
            .add(Config::default(), |meta| DropRule {
                meta,
                state: Arc::downgrade(&state),
                dropped: dropped.clone(),
            })
            .unwrap();
        assert!(
            panics(|| {
                if checkpoint {
                    state.checkpoint().unwrap();
                } else {
                    state.deactivate();
                }
            })
            .contains("response drop failed")
        );
        state.verify().unwrap();
        assert!(lock(&state.inner).rules.is_empty());
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        state.deactivate();
    }
}

#[test]
fn poisoned_locks_keep_their_contents() {
    let mutex = Mutex::new(3);
    panics(|| {
        let mut value = lock(&mutex);
        *value = 4;
        panic!("poison");
    });
    assert_eq!(*lock(&mutex), 4);
}
