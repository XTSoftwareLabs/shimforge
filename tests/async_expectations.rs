#![forbid(unsafe_code)]

use shimforge::{Error, Sequence, Session, mock, replace};
use std::future::Future;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::{Pin, pin};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Wake, Waker};

static TEST_LOCK: Mutex<()> = Mutex::new(());
static BODY_CALLS: AtomicUsize = AtomicUsize::new(0);

fn serial_test() -> MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner())
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

async fn fetch(id: u32) -> String {
    BODY_CALLS.fetch_add(1, Ordering::SeqCst);
    format!("remote {id}")
}

async fn other_fetch(id: u32) -> String {
    format!("other {id}")
}

#[test]
fn failed_async_installation_clears_the_registry_for_retry() {
    fn replace_poll<F: Future<Output = String>>(session: &mut Session, _: &F) {
        replace!(
            session, F::poll => |_, _| Poll::Ready("replacement".to_owned()),
            fn(Pin<&mut F>, &mut Context<'_>) -> Poll<String>
        )
        .unwrap();
    }
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    let witness = fetch(0);
    replace_poll(&mut session, &witness);
    assert!(matches!(session.mock_async(witness), Err(Error::Overlap)));
    assert_eq!(block_on(fetch(1)), "replacement");
    session.restore().unwrap();
    let mock = session.mock_async(fetch(0)).unwrap();
    mock.expect().once().returns("retry".to_owned()).unwrap();
    assert_eq!(block_on(fetch(2)), "retry");
}

async fn local_fetch(value: Rc<String>) -> String {
    format!("local {value}")
}

fn save(value: &str) -> usize {
    value.len() + 1
}

async fn read_config(path: &str) -> io::Result<String> {
    BODY_CALLS.fetch_add(1, Ordering::SeqCst);
    std::fs::read_to_string(path)
}

async fn load_message(id: u32) -> String {
    format!("message: {}", fetch(id).await)
}

struct Client {
    prefix: String,
}

impl Client {
    async fn load(&self, key: &str) -> String {
        format!("{}:{key}", self.prefix)
    }
}

struct DropCount(Arc<AtomicUsize>);

impl Drop for DropCount {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

async fn consume(value: DropCount) -> usize {
    let _keep = value;
    BODY_CALLS.fetch_add(1, Ordering::SeqCst);
    91
}

#[derive(Debug, PartialEq)]
struct Ticket(u32);

async fn issue(id: u32) -> Ticket {
    Ticket(id + 1)
}

#[test]
fn native_async_functions_return_mock_values_without_running_the_body() {
    let _serial = serial_test();
    BODY_CALLS.store(0, Ordering::SeqCst);
    let mut session = Session::new().unwrap();
    let fetches = session.mock_async(fetch(0)).unwrap();
    let expected = fetches
        .expect()
        .times(2)
        .returns(String::from("cached"))
        .unwrap();
    assert_eq!(block_on(load_message(5)), "message: cached");
    assert_eq!(block_on(fetch(6)), "cached");
    assert_eq!(expected.calls(), 2);
    assert_eq!(BODY_CALLS.load(Ordering::SeqCst), 0);
    fetches.verify().unwrap();
    session.restore().unwrap();
    assert_eq!(block_on(fetch(7)), "remote 7");
    assert_eq!(BODY_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn construction_and_cancellation_do_not_count_as_calls() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    let fetches = session.mock_async(fetch(0)).unwrap();
    let expected = fetches
        .expect()
        .once()
        .returns(String::from("ready"))
        .unwrap();
    drop(fetch(1));
    assert_eq!(expected.calls(), 0);
    assert_eq!(block_on(fetch(2)), "ready");
    assert_eq!(expected.calls(), 1);
}

#[test]
fn callbacks_capture_state_and_produce_owned_outputs() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    let fetches = session.mock_async(fetch(0)).unwrap();
    let prefix = String::from("page");
    let mut count = 0;
    fetches
        .expect()
        .times(2)
        .returning(move || {
            count += 1;
            format!("{prefix} {count}")
        })
        .unwrap();
    assert_eq!(block_on(fetch(1)), "page 1");
    assert_eq!(block_on(fetch(2)), "page 2");
}

#[test]
fn once_responses_move_non_clone_outputs() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    let issues = session.mock_async(issue(0)).unwrap();
    issues.expect().return_once(Ticket(80)).unwrap();
    let ticket = Ticket(81);
    issues.expect().returning_once(move || ticket).unwrap();
    assert_eq!(block_on(issue(1)), Ticket(80));
    assert_eq!(block_on(issue(2)), Ticket(81));
}

#[test]
fn default_outputs_and_checkpoints_work() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    let fetches = session.mock_async(fetch(0)).unwrap();
    fetches.expect().once().returns_default().unwrap();
    assert_eq!(block_on(fetch(1)), "");
    fetches.checkpoint().unwrap();
    fetches
        .expect()
        .times(1..=2)
        .returns(String::from("next"))
        .unwrap();
    assert_eq!(block_on(fetch(2)), "next");
}

#[test]
fn async_io_can_return_errors_without_touching_disk() {
    let _serial = serial_test();
    BODY_CALLS.store(0, Ordering::SeqCst);
    let mut session = Session::new().unwrap();
    let reads = session.mock_async(read_config("witness.conf")).unwrap();
    reads
        .expect()
        .once()
        .returning(|| Err(io::ErrorKind::PermissionDenied.into()))
        .unwrap();
    reads
        .expect()
        .once()
        .returning(|| Ok(String::from("port=8080")))
        .unwrap();
    assert_eq!(
        block_on(read_config("private.conf")).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(block_on(read_config("service.conf")).unwrap(), "port=8080");
    assert_eq!(BODY_CALLS.load(Ordering::SeqCst), 0);
}

#[test]
fn borrowed_async_methods_need_no_static_receiver() {
    let _serial = serial_test();
    let client = Client {
        prefix: String::from("service"),
    };
    let key = String::from("settings");
    let mut session = Session::new().unwrap();
    let loads = session.mock_async(client.load(&key)).unwrap();
    loads
        .expect()
        .once()
        .returns(String::from("cached"))
        .unwrap();
    assert_eq!(block_on(client.load(&key)), "cached");
    session.restore().unwrap();
    assert_eq!(block_on(client.load(&key)), "service:settings");
}

#[test]
fn witness_and_mocked_futures_drop_their_inputs_once() {
    let _serial = serial_test();
    BODY_CALLS.store(0, Ordering::SeqCst);
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut session = Session::new().unwrap();
    let consumes = session
        .mock_async(consume(DropCount(Arc::clone(&dropped))))
        .unwrap();
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    consumes.expect().once().returns(7).unwrap();
    drop(consume(DropCount(Arc::clone(&dropped))));
    assert_eq!(dropped.load(Ordering::SeqCst), 2);
    assert_eq!(block_on(consume(DropCount(Arc::clone(&dropped)))), 7);
    assert_eq!(dropped.load(Ordering::SeqCst), 3);
    assert_eq!(BODY_CALLS.load(Ordering::SeqCst), 0);
}

#[test]
fn other_future_types_with_the_same_output_stay_unchanged() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    let fetches = session.mock_async(fetch(0)).unwrap();
    fetches
        .expect()
        .once()
        .returns(String::from("cached"))
        .unwrap();
    assert_eq!(block_on(other_fetch(4)), "other 4");
    assert_eq!(block_on(fetch(4)), "cached");
}

#[test]
fn sequences_check_the_order_of_awaited_calls() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    let fetches = session.mock_async(fetch(0)).unwrap();
    let issues = session.mock_async(issue(0)).unwrap();
    let order = Sequence::new();
    fetches
        .expect()
        .once()
        .in_sequence(&order)
        .returns(String::from("first"))
        .unwrap();
    issues
        .expect()
        .once()
        .in_sequence(&order)
        .return_once(Ticket(9))
        .unwrap();
    assert_eq!(block_on(fetch(1)), "first");
    assert_eq!(block_on(issue(1)), Ticket(9));
}

#[test]
fn missing_calls_fail_and_restore_still_restores_the_original() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    let fetches = session.mock_async(fetch(0)).unwrap();
    fetches
        .expect()
        .once()
        .returns(String::from("missing"))
        .unwrap();
    assert!(fetches.verify().is_err());
    assert!(matches!(session.restore(), Err(Error::Expectation(_))));
    assert_eq!(block_on(fetch(1)), "remote 1");
}

#[test]
fn extra_calls_fail_even_when_the_panic_is_caught() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    let fetches = session.mock_async(fetch(0)).unwrap();
    let expected = fetches
        .expect()
        .once()
        .returns(String::from("one"))
        .unwrap();
    assert_eq!(block_on(fetch(1)), "one");
    assert!(catch_unwind(|| block_on(fetch(2))).is_err());
    assert_eq!(expected.calls(), 1);
    assert!(fetches.verify().is_err());
    assert!(session.restore().is_err());
}

#[test]
fn never_rules_reject_awaited_calls() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    let fetches = session.mock_async(fetch(0)).unwrap();
    fetches.expect().never().unwrap();
    fetches.verify().unwrap();
    assert!(catch_unwind(|| block_on(fetch(1))).is_err());
    assert!(session.restore().is_err());
}

#[test]
fn configured_async_panics_count_as_calls() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    let fetches = session.mock_async(fetch(0)).unwrap();
    let expected = fetches.expect().once().panics("offline").unwrap();
    assert!(catch_unwind(|| block_on(fetch(1))).is_err());
    assert_eq!(expected.calls(), 1);
    fetches.verify().unwrap();
}

#[test]
fn unwind_restores_async_code_without_a_second_panic() {
    let _serial = serial_test();
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut session = Session::new().unwrap();
        let fetches = session.mock_async(fetch(0)).unwrap();
        fetches
            .expect()
            .once()
            .returns(String::from("unused"))
            .unwrap();
        panic!("test failed first");
    }));
    assert!(result.is_err());
    assert_eq!(block_on(fetch(2)), "remote 2");
}

#[test]
fn worker_threads_share_async_expectations() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    let fetches = session.mock_async(fetch(0)).unwrap();
    let expected = fetches
        .expect()
        .times(4)
        .returns(String::from("shared"))
        .unwrap();
    let workers: Vec<_> = (0..4)
        .map(|id| std::thread::spawn(move || block_on(fetch(id))))
        .collect();
    for worker in workers {
        assert_eq!(worker.join().unwrap(), "shared");
    }
    assert_eq!(expected.calls(), 4);
}

#[test]
fn restore_drops_callbacks_and_invalidates_old_handles() {
    let _serial = serial_test();
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut session = Session::new().unwrap();
    let fetches = session.mock_async(fetch(0)).unwrap();
    let marker = DropCount(Arc::clone(&dropped));
    fetches
        .expect()
        .returning(move || {
            let _keep = &marker;
            String::from("ready")
        })
        .unwrap();
    session.restore().unwrap();
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert!(fetches.expect().returns(String::from("late")).is_err());
}

#[test]
fn the_same_async_function_can_be_mocked_in_later_sessions() {
    let _serial = serial_test();
    for value in ["first", "second"] {
        let mut session = Session::new().unwrap();
        let fetches = session.mock_async(fetch(0)).unwrap();
        fetches.expect().once().returns(value.to_owned()).unwrap();
        assert_eq!(block_on(fetch(1)), value);
    }
}

#[test]
fn duplicate_async_mocks_leave_the_first_mock_usable() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    let fetches = session.mock_async(fetch(0)).unwrap();
    fetches
        .expect()
        .once()
        .returns(String::from("first"))
        .unwrap();
    assert!(session.mock_async(fetch(1)).is_err());
    assert_eq!(block_on(fetch(2)), "first");
    session.restore().unwrap();
    assert_eq!(block_on(fetch(2)), "remote 2");
}

#[test]
fn futures_with_non_send_inputs_can_be_mocked() {
    let _serial = serial_test();
    let value = Rc::new(String::from("input"));
    let mut session = Session::new().unwrap();
    let fetches = session.mock_async(local_fetch(Rc::clone(&value))).unwrap();
    assert_eq!(Rc::strong_count(&value), 1);
    fetches
        .expect()
        .once()
        .returns(String::from("cached"))
        .unwrap();
    assert_eq!(block_on(local_fetch(Rc::clone(&value))), "cached");
    assert_eq!(Rc::strong_count(&value), 1);
}

#[test]
fn sync_and_async_expectations_share_sequences() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    let fetches = session.mock_async(fetch(0)).unwrap();
    let saves = mock!(session, save, fn(&str) -> usize).unwrap();
    let order = Sequence::new();
    fetches
        .expect()
        .once()
        .in_sequence(&order)
        .returns(String::from("value"))
        .unwrap();
    saves
        .expect()
        .with(|value| **value == *"value")
        .once()
        .in_sequence(&order)
        .returns(5)
        .unwrap();
    assert_eq!(save(&block_on(fetch(1))), 5);
}

#[test]
fn recursive_async_callbacks_fail_without_deadlocking() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    let fetches = session.mock_async(fetch(0)).unwrap();
    fetches.expect().returning(|| block_on(fetch(1))).unwrap();
    assert!(catch_unwind(|| block_on(fetch(2))).is_err());
    assert!(session.restore().is_err());
}
