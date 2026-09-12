#![forbid(unsafe_code)]

use shimforge::{Error, Sequence, Session, mock, replace};
use std::fs;
use std::future::Future;
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Wake, Waker};

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn serial_test() -> MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

fn price(item: u32) -> u64 {
    u64::from(item) + 100
}

fn tax(amount: u64) -> u64 {
    amount / 10
}

fn fixed_price(item: u32) -> u64 {
    u64::from(item) + 20
}

fn title(id: u32) -> String {
    format!("item {id}")
}

fn first_word(text: &str) -> &str {
    text.split_whitespace().next().unwrap_or("")
}

fn fill(buffer: &mut [u8]) -> usize {
    buffer.fill(0);
    buffer.len()
}

#[derive(Debug, PartialEq)]
struct Ticket(u64);

fn issue_ticket(id: u64) -> Ticket {
    Ticket(id + 1)
}

struct Store {
    prefix: String,
}

impl Store {
    fn lookup(&self, key: &str) -> String {
        format!("{}:{key}", self.prefix)
    }

    fn measure(&self, key: &str, length: &mut usize) {
        *length = self.prefix.len() + key.len();
    }
}

fn purge(generation: u32) {
    panic!("generation {generation} must never be purged in a test");
}

struct DropCount(Arc<AtomicUsize>);

impl Drop for DropCount {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn boxed_read(id: u32) -> Pin<Box<dyn Future<Output = usize> + Send>> {
    Box::pin(async move { id as usize + 1 })
}

struct YieldValue {
    value: usize,
    ready: bool,
}

impl Future for YieldValue {
    type Output = usize;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let state = self.get_mut();
        if state.ready {
            Poll::Ready(state.value)
        } else {
            state.ready = true;
            context.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

struct ThreadWake(std::thread::Thread);

impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park(),
        }
    }
}

#[test]
fn matches_arguments_and_checks_exact_counts() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let prices = mock!(session, price, fn(u32) -> u64).unwrap();
    let first = prices
        .expect()
        .with(|item| *item == 7)
        .times(2)
        .returns(50)
        .unwrap();
    let second = prices
        .expect()
        .with(|item| *item == 8)
        .once()
        .returns(75)
        .unwrap();

    assert_eq!(price(7), 50);
    assert_eq!(price(8), 75);
    assert_eq!(price(7), 50);
    assert_eq!(first.calls(), 2);
    assert_eq!(second.calls(), 1);
    first.verify().unwrap();
    second.verify().unwrap();
    prices.verify().unwrap();
    session.verify().unwrap();
    session.restore().unwrap();
    assert_eq!(price(7), 107);
}

#[test]
fn callbacks_capture_values_and_keep_mutable_state() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let prices = mock!(session, price, fn(u32) -> u64).unwrap();
    let base = 20;
    let mut calls = 0;
    prices
        .expect()
        .times(3)
        .returning(move |item| {
            calls += 1;
            base + u64::from(item) + calls
        })
        .unwrap();
    assert_eq!(price(4), 25);
    assert_eq!(price(4), 26);
    assert_eq!(price(4), 27);
}

#[test]
fn constant_owned_values_are_cloned_for_each_call() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let titles = mock!(session, title, fn(u32) -> String).unwrap();
    titles
        .expect()
        .times(2)
        .returns(String::from("saved"))
        .unwrap();
    let mut first = title(1);
    first.push('!');
    assert_eq!(first, "saved!");
    assert_eq!(title(2), "saved");
}

#[test]
fn returns_non_clone_values_once() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let tickets = mock!(session, issue_ticket, fn(u64) -> Ticket).unwrap();
    let expected = tickets.expect().return_once(Ticket(90)).unwrap();
    assert_eq!(issue_ticket(1), Ticket(90));
    assert_eq!(expected.calls(), 1);
    expected.verify().unwrap();
}

#[test]
fn once_callbacks_move_captured_values() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let titles = mock!(session, title, fn(u32) -> String).unwrap();
    let value = String::from("one owner");
    titles
        .expect()
        .returning_once(move |id| {
            assert_eq!(id, 9);
            value
        })
        .unwrap();
    assert_eq!(title(9), "one owner");
}

#[test]
fn default_values_and_unbounded_counts_work() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let titles = mock!(session, title, fn(u32) -> String).unwrap();
    let optional = titles.expect().returns_default().unwrap();
    optional.verify().unwrap();
    assert_eq!(title(1), "");
    assert_eq!(title(2), "");
    assert_eq!(optional.calls(), 2);
}

#[test]
fn count_ranges_accept_their_bounds() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let prices = mock!(session, price, fn(u32) -> u64).unwrap();
    prices
        .expect()
        .with(|id| *id == 1)
        .times(1..3)
        .returns(11)
        .unwrap();
    prices
        .expect()
        .with(|id| *id == 2)
        .times(1..=2)
        .returns(22)
        .unwrap();
    prices
        .expect()
        .with(|id| *id == 3)
        .times(1..)
        .returns(33)
        .unwrap();
    prices
        .expect()
        .with(|id| *id == 4)
        .times(..2)
        .returns(44)
        .unwrap();
    prices
        .expect()
        .with(|id| *id == 5)
        .times(..=1)
        .returns(55)
        .unwrap();
    prices
        .expect()
        .with(|id| *id == 6)
        .times(..)
        .returns(66)
        .unwrap();
    for id in 1..=6 {
        assert_eq!(price(id), u64::from(id) * 11);
    }
    assert_eq!(price(1), 11);
    assert_eq!(price(2), 22);
    assert_eq!(price(3), 33);
    assert_eq!(price(6), 66);
}

#[test]
fn invalid_count_ranges_are_rejected_without_adding_rules() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let prices = mock!(session, price, fn(u32) -> u64).unwrap();
    let start = 3;
    let end = 2;
    assert!(prices.expect().times(start..end).returns(1).is_err());
    assert!(prices.expect().times(2).return_once(1).is_err());
    prices.expect().once().returns(6).unwrap();
    assert_eq!(price(1), 6);
}

#[test]
fn exhausted_rules_yield_to_later_matching_rules() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let prices = mock!(session, price, fn(u32) -> u64).unwrap();
    prices.expect().once().returns(10).unwrap();
    prices.expect().times(2).returns(20).unwrap();
    prices.expect().returns(30).unwrap();
    assert_eq!(price(1), 10);
    assert_eq!(price(1), 20);
    assert_eq!(price(1), 20);
    assert_eq!(price(1), 30);
}

#[test]
fn borrowed_arguments_and_returns_keep_their_lifetimes() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let words = mock!(session, first_word, for<'a> fn(&'a str) -> &'a str).unwrap();
    words
        .expect()
        .with(|text| text.contains(' '))
        .once()
        .returning(|text| text.split_once(' ').unwrap().1)
        .unwrap();
    let value = String::from("first second");
    assert_eq!(first_word(&value), "second");
}

#[test]
fn callbacks_can_write_output_parameters() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let reads = mock!(session, fill, fn(&mut [u8]) -> usize).unwrap();
    reads
        .expect()
        .with(|buffer| buffer.len() == 4)
        .once()
        .returning(|buffer| {
            buffer[..3].copy_from_slice(b"abc");
            3
        })
        .unwrap();
    let mut buffer = [9; 4];
    assert_eq!(fill(&mut buffer), 3);
    assert_eq!(buffer, [b'a', b'b', b'c', 9]);
}

#[test]
fn unit_results_skip_the_original_body_and_still_count_calls() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let purges = mock!(session, purge, fn(u32)).unwrap();
    let expected = purges
        .expect()
        .with(|generation| *generation == 4)
        .times(2)
        .returns_default()
        .unwrap();
    purge(4);
    purge(4);
    assert_eq!(expected.calls(), 2);
    session.verify().unwrap();
}

#[test]
fn an_extra_call_to_a_unit_function_is_rejected() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let purges = mock!(session, purge, fn(u32)).unwrap();
    let expected = purges.expect().once().returns_default().unwrap();
    purge(4);
    assert!(catch_unwind(|| purge(4)).is_err());
    assert_eq!(expected.calls(), 1);
    assert!(session.restore().is_err());
}

#[test]
fn a_missing_call_to_a_unit_function_fails_verification() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let purges = mock!(session, purge, fn(u32)).unwrap();
    purges.expect().times(3).returns_default().unwrap();
    purge(1);
    purge(2);
    assert!(purges.verify().is_err());
    assert!(session.restore().is_err());
}

#[test]
fn a_method_can_fill_an_output_parameter_without_returning() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let measures = mock!(session, Store::measure, fn(&Store, &str, &mut usize)).unwrap();
    measures
        .expect()
        .with(|store, key, _| store.prefix == "cache" && **key == *"port")
        .once()
        .returning(|store, key, length| *length = store.prefix.len() * key.len())
        .unwrap();
    let store = Store {
        prefix: String::from("cache"),
    };
    let mut length = 0;
    store.measure("port", &mut length);
    assert_eq!(length, 20);
    session.restore().unwrap();
    store.measure("port", &mut length);
    assert_eq!(length, 9);
}

#[test]
fn methods_match_the_receiver_and_borrowed_parameters() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let lookups = mock!(session, Store::lookup, fn(&Store, &str) -> String).unwrap();
    lookups
        .expect()
        .with(|store, key| store.prefix == "cache" && **key == *"port")
        .once()
        .returning(|_, key| format!("mock {key}"))
        .unwrap();
    let store = Store {
        prefix: String::from("cache"),
    };
    assert_eq!(store.lookup("port"), "mock port");
}

#[test]
fn file_reads_match_paths_and_return_io_errors() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let reads = mock!(
        session,
        fs::read_to_string::<&Path>,
        fn(&Path) -> io::Result<String>
    )
    .unwrap();
    reads
        .expect()
        .with(|path| **path == *Path::new("settings.conf"))
        .once()
        .returning(|_| Ok(String::from("port=8080")))
        .unwrap();
    reads
        .expect()
        .with(|path| **path == *Path::new("secret.conf"))
        .once()
        .returning(|_| Err(io::ErrorKind::PermissionDenied.into()))
        .unwrap();
    assert_eq!(
        fs::read_to_string(Path::new("settings.conf")).unwrap(),
        "port=8080"
    );
    assert_eq!(
        fs::read_to_string(Path::new("secret.conf"))
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
}

#[test]
fn network_expectations_check_payloads_without_sending_packets() {
    let _serial = serial_test();
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    receiver.set_nonblocking(true).unwrap();
    let address = receiver.local_addr().unwrap();
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut session = Session::new_global().unwrap();
    let sends = mock!(
        session,
        UdpSocket::send_to::<SocketAddr>,
        fn(&UdpSocket, &[u8], SocketAddr) -> io::Result<usize>
    )
    .unwrap();
    sends
        .expect()
        .with(move |_, bytes, target| **bytes == *b"orders:3|c" && *target == address)
        .once()
        .returning(|_, bytes, _| Ok(bytes.len()))
        .unwrap();
    assert_eq!(sender.send_to(b"orders:3|c", address).unwrap(), 10);
    let mut buffer = [0; 32];
    assert_eq!(
        receiver.recv_from(&mut buffer).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
}

#[test]
fn a_sequence_checks_order_across_functions() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let prices = mock!(session, price, fn(u32) -> u64).unwrap();
    let taxes = mock!(session, tax, fn(u64) -> u64).unwrap();
    let order = Sequence::new();
    prices
        .expect()
        .times(2)
        .in_sequence(&order)
        .returns(10)
        .unwrap();
    taxes
        .expect()
        .once()
        .in_sequence(&order)
        .returns(3)
        .unwrap();
    assert_eq!(price(1), 10);
    assert_eq!(price(2), 10);
    assert_eq!(tax(20), 3);
}

#[test]
fn a_sequence_rejects_out_of_order_calls() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let prices = mock!(session, price, fn(u32) -> u64).unwrap();
    let taxes = mock!(session, tax, fn(u64) -> u64).unwrap();
    let order = Sequence::new();
    prices
        .expect()
        .once()
        .in_sequence(&order)
        .returns(10)
        .unwrap();
    taxes
        .expect()
        .once()
        .in_sequence(&order)
        .returns(3)
        .unwrap();
    assert!(catch_unwind(|| tax(20)).is_err());
    assert!(taxes.verify().is_err());
    assert!(matches!(session.restore(), Err(Error::Expectation(_))));
    assert_eq!(tax(20), 2);
}

#[test]
fn checkpoint_verifies_and_clears_finished_expectations() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let prices = mock!(session, price, fn(u32) -> u64).unwrap();
    prices.expect().once().returns(10).unwrap();
    assert!(prices.checkpoint().is_err());
    assert_eq!(price(1), 10);
    prices.checkpoint().unwrap();
    prices.expect().once().returns(20).unwrap();
    assert_eq!(price(1), 20);
}

#[test]
fn missing_calls_fail_verification_and_restore_still_removes_the_patch() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let prices = mock!(session, price, fn(u32) -> u64).unwrap();
    let expected = prices.expect().times(2).returns(10).unwrap();
    assert_eq!(price(1), 10);
    assert!(expected.verify().is_err());
    assert!(prices.verify().is_err());
    assert!(session.verify().is_err());
    assert!(session.restore().is_err());
    assert_eq!(price(1), 101);
    session.restore().unwrap();
}

#[test]
fn unexpected_arguments_are_reported_even_if_the_panic_is_caught() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let prices = mock!(session, price, fn(u32) -> u64).unwrap();
    prices.expect().with(|id| *id == 1).returns(10).unwrap();
    assert!(catch_unwind(|| price(2)).is_err());
    assert!(prices.verify().is_err());
    assert!(session.restore().is_err());
}

#[test]
fn calls_beyond_the_limit_fail() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let prices = mock!(session, price, fn(u32) -> u64).unwrap();
    let expected = prices.expect().once().returns(10).unwrap();
    assert_eq!(price(1), 10);
    assert!(catch_unwind(|| price(1)).is_err());
    assert_eq!(expected.calls(), 1);
    assert!(session.restore().is_err());
}

#[test]
fn never_rules_allow_no_calls_and_reject_matching_calls() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let prices = mock!(session, price, fn(u32) -> u64).unwrap();
    prices.expect().with(|id| *id == 9).never().unwrap();
    prices.expect().with(|id| *id != 9).returns(10).unwrap();
    assert_eq!(price(1), 10);
    prices.verify().unwrap();
    assert!(catch_unwind(|| price(9)).is_err());
    assert!(session.restore().is_err());
}

#[test]
fn configured_panics_count_as_calls() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let prices = mock!(session, price, fn(u32) -> u64).unwrap();
    let expected = prices.expect().once().panics("price unavailable").unwrap();
    let panic = catch_unwind(|| price(1)).unwrap_err();
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .unwrap();
    assert!(message.contains("price unavailable"));
    assert_eq!(expected.calls(), 1);
    prices.verify().unwrap();
}

#[test]
fn drop_checks_counts_and_restores_code_before_panicking() {
    let _serial = serial_test();
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut session = Session::new_global().unwrap();
        let prices = mock!(session, price, fn(u32) -> u64).unwrap();
        prices.expect().once().returns(10).unwrap();
    }));
    assert!(result.is_err());
    assert_eq!(price(1), 101);
    assert!(Session::new_global().is_ok());
}

#[test]
fn unwinding_does_not_panic_again_for_missing_calls() {
    let _serial = serial_test();
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut session = Session::new_global().unwrap();
        let prices = mock!(session, price, fn(u32) -> u64).unwrap();
        prices.expect().once().returns(10).unwrap();
        panic!("test failed first");
    }));
    assert!(result.is_err());
    assert_eq!(price(1), 101);
}

#[test]
fn callbacks_are_serialized_across_threads() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let prices = mock!(session, price, fn(u32) -> u64).unwrap();
    let mut total = 0;
    let expected = prices
        .expect()
        .times(40)
        .returning(move |_| {
            total += 1;
            total
        })
        .unwrap();
    let workers: Vec<_> = (0..4)
        .map(|_| std::thread::spawn(|| (0..10).map(price).collect::<Vec<_>>()))
        .collect();
    let mut values: Vec<_> = workers
        .into_iter()
        .flat_map(|worker| worker.join().unwrap())
        .collect();
    values.sort_unstable();
    assert_eq!(values, (1..=40).collect::<Vec<_>>());
    assert_eq!(expected.calls(), 40);
}

#[test]
fn recursive_calls_fail_without_deadlocking() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let prices = mock!(session, price, fn(u32) -> u64).unwrap();
    prices.expect().returning(|id| price(id + 1)).unwrap();
    assert!(catch_unwind(|| price(1)).is_err());
    assert!(session.restore().is_err());
}

#[test]
fn dropping_the_handle_keeps_the_mock_until_session_restore() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let prices = mock!(session, price, fn(u32) -> u64).unwrap();
    prices.expect().once().returns(42).unwrap();
    drop(prices);
    assert_eq!(price(1), 42);
    session.restore().unwrap();
    assert_eq!(price(1), 101);
}

#[test]
fn restore_drops_captures_and_rejects_new_rules_on_old_handles() {
    let _serial = serial_test();
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut session = Session::new_global().unwrap();
    let prices = mock!(session, price, fn(u32) -> u64).unwrap();
    let marker = DropCount(Arc::clone(&dropped));
    prices
        .expect()
        .returning(move |id| {
            let _keep = &marker;
            u64::from(id)
        })
        .unwrap();
    assert_eq!(price(3), 3);
    session.restore().unwrap();
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert!(prices.expect().returns(10).is_err());
    drop(prices);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn the_same_macro_call_site_can_be_used_in_later_sessions() {
    let _serial = serial_test();
    for value in [10, 20] {
        let mut session = Session::new_global().unwrap();
        let prices = mock!(session, price, fn(u32) -> u64).unwrap();
        prices.expect().once().returns(value).unwrap();
        assert_eq!(price(1), value);
    }
    assert_eq!(price(1), 101);
}

#[test]
fn duplicate_mocks_leave_the_first_mock_usable() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let prices = mock!(session, price, fn(u32) -> u64).unwrap();
    prices.expect().once().returns(42).unwrap();
    let duplicate = mock!(session, price, fn(u32) -> u64);
    assert!(matches!(duplicate, Err(Error::Overlap)));
    assert_eq!(price(1), 42);
    session.restore().unwrap();
    assert_eq!(price(1), 101);
}

#[test]
fn failed_installation_clears_the_slot_for_the_next_attempt() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    replace!(session, price => fixed_price, fn(u32) -> u64).unwrap();
    for attempt in 0..2 {
        let result = mock!(session, price, fn(u32) -> u64);
        if attempt == 0 {
            assert!(matches!(result, Err(Error::Overlap)));
            assert_eq!(price(1), 21);
        } else {
            let prices = result.unwrap();
            prices.expect().once().returns(42).unwrap();
            assert_eq!(price(1), 42);
        }
        session.restore().unwrap();
    }
    assert_eq!(price(1), 101);
}

#[test]
fn boxed_future_callbacks_match_arguments_and_can_return_pending() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let reads = mock!(
        session,
        boxed_read,
        fn(u32) -> Pin<Box<dyn Future<Output = usize> + Send>>
    )
    .unwrap();
    let offset = 20;
    let expected = reads
        .expect()
        .with(|id| *id == 7)
        .once()
        .returning(move |id| {
            Box::pin(YieldValue {
                value: id as usize + offset,
                ready: false,
            }) as Pin<Box<dyn Future<Output = usize> + Send>>
        })
        .unwrap();
    let future = boxed_read(7);
    assert_eq!(expected.calls(), 1);
    assert_eq!(block_on(future), 27);
    assert_eq!(expected.calls(), 1);
    session.restore().unwrap();
    assert_eq!(block_on(boxed_read(7)), 8);
}

#[test]
fn macro_names_do_not_shadow_the_callers_session_or_types() {
    let _serial = serial_test();
    type Result = ();
    type Box = ();
    type Option = ();
    type String = ();
    let _: (Result, Box, Option, String) = ((), (), (), ());
    let mut state = Session::new_global().unwrap();
    let prices = mock!(state, price, fn(u32) -> u64).unwrap();
    prices.expect().once().returns(42).unwrap();
    assert_eq!(price(1), 42);
    state.restore().unwrap();
    assert_eq!(price(1), 101);
}

fn static_length(value: &'static str) -> usize {
    value.len() + 1
}

fn static_result(value: &str) -> &'static str {
    if value.is_empty() { "empty" } else { "present" }
}

fn trim_left<'a>(left: &'a str, right: &str) -> &'a str {
    &left[..right.len().min(left.len())]
}

#[test]
fn independent_input_lifetimes_keep_their_output_link() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let trim = mock!(
        session,
        trim_left,
        for<'a, 'b> fn(&'a str, &'b str) -> &'a str
    )
    .unwrap();
    trim.expect().once().returning(|left, _| left).unwrap();
    let left = String::from("left");
    let result;
    {
        let right = String::from("r");
        result = trim_left(&left, &right);
    }
    assert_eq!(result, "left");
    session.restore().unwrap();
}

#[test]
fn genuine_static_inputs_and_results_stay_supported() {
    let _serial = serial_test();
    let mut session = Session::new_global().unwrap();
    let length = mock!(session, static_length, fn(&'static str) -> usize).unwrap();
    let seen = Arc::new(Mutex::new(None));
    let captured = seen.clone();
    length
        .expect()
        .once()
        .returning(move |value| {
            *captured.lock().unwrap() = Some(value);
            42
        })
        .unwrap();
    let result = mock!(session, static_result, fn(&str) -> &'static str).unwrap();
    result.expect().once().returns("mock").unwrap();
    assert_eq!(static_length("saved"), 42);
    assert_eq!(*seen.lock().unwrap(), Some("saved"));
    assert_eq!(static_result(&String::from("input")), "mock");
    session.restore().unwrap();
}

#[test]
fn signature_checks_do_not_run_source_or_target_expressions() {
    let _serial = serial_test();
    let mut evaluated = 0;
    let mut session = Session::new_global().unwrap();
    let prices = mock!(
        session,
        {
            evaluated += 1;
            price
        },
        fn(u32) -> u64
    )
    .unwrap();
    assert_eq!(evaluated, 1);
    prices.expect().once().returns(7).unwrap();
    assert_eq!(price(1), 7);
    session.restore().unwrap();
    replace!(session, { evaluated += 1; price } => { evaluated += 1; fixed_price },
        fn(u32) -> u64)
    .unwrap();
    assert_eq!(evaluated, 3);
    assert_eq!(price(1), 21);
    session.restore().unwrap();
}

#[test]
fn source_and_target_expressions_can_move_values_and_use_question_mark() -> Result<(), Error> {
    let _serial = serial_test();
    let mut session = Session::new_global()?;
    let owned = String::from("source");
    let prices = mock!(
        session,
        {
            drop(owned);
            price
        },
        fn(u32) -> u64
    )?;
    prices.expect().once().returns(9)?;
    assert_eq!(price(1), 9);
    session.restore()?;
    let source: Result<fn(u32) -> u64, Error> = Ok(price);
    let prices = mock!(session, source?, fn(u32) -> u64)?;
    prices.expect().once().returns(10)?;
    assert_eq!(price(1), 10);
    session.restore()?;
    let owned = String::from("replacement");
    let target: Result<fn(u32) -> u64, Error> = Ok(fixed_price);
    replace!(session, price => { drop(owned); target? }, fn(u32) -> u64)?;
    assert_eq!(price(1), 21);
    session.restore()?;
    Ok(())
}
