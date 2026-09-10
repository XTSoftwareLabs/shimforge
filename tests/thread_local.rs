#![forbid(unsafe_code)]

use shimforge::{Error, Session, mock};
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::pin;
use std::sync::{Mutex, MutexGuard, mpsc};
use std::task::{Context, Poll, Waker};

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn serial_test() -> MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

fn increment(value: i64) -> i64 {
    value + 1
}

fn borrowed(value: &str) -> &str {
    value.trim()
}

fn owned(value: String) -> String {
    format!("original {value}")
}

extern "C" fn native(value: i64) -> i64 {
    value + 2
}

fn install_shared(session: &mut Session, result: i64) {
    let mock = mock!(session, increment, fn(i64) -> i64).unwrap();
    mock.expect().once().returns(result).unwrap();
}

fn two_threads(
    install: impl FnOnce(&mut Session),
    worker_install: impl FnOnce(&mut Session) + Send + 'static,
) {
    let mut session = Session::new_local().unwrap();
    install(&mut session);
    let (ready_tx, ready_rx) = mpsc::channel();
    let (run_tx, run_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let (drop_tx, drop_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut session = Session::new_local().unwrap();
        worker_install(&mut session);
        ready_tx.send(()).unwrap();
        run_rx.recv().unwrap();
        assert_eq!(increment(10), 200);
        done_tx.send(()).unwrap();
        drop_rx.recv().unwrap();
    });
    ready_rx.recv().unwrap();
    run_tx.send(()).unwrap();
    assert_eq!(increment(10), 100);
    done_rx.recv().unwrap();
    drop_tx.send(()).unwrap();
    worker.join().unwrap();
    session.restore().unwrap();
    assert_eq!(increment(10), 11);
}

#[test]
fn unmocked_threads_call_the_original() {
    let _serial = serial_test();
    let mut session = Session::new_local().unwrap();
    install_shared(&mut session, 90);
    assert_eq!(increment(3), 90);
    assert_eq!(std::thread::spawn(|| increment(3)).join().unwrap(), 4);
    session.restore().unwrap();
    assert_eq!(increment(3), 4);
}

#[test]
fn different_macro_sites_can_mock_the_same_function() {
    let _serial = serial_test();
    two_threads(
        |session| {
            let mock = mock!(session, increment, fn(i64) -> i64).unwrap();
            mock.expect().once().returns(100).unwrap();
        },
        |session| {
            let mock = mock!(session, increment, fn(i64) -> i64).unwrap();
            mock.expect().once().returns(200).unwrap();
        },
    );
}

#[test]
fn one_macro_site_can_serve_parallel_sessions() {
    let _serial = serial_test();
    two_threads(
        |session| install_shared(session, 100),
        |session| install_shared(session, 200),
    );
}

#[test]
fn dropping_the_first_session_keeps_the_second_mock_alive() {
    let _serial = serial_test();
    let mut session = Session::new_local().unwrap();
    install_shared(&mut session, 100);
    assert_eq!(increment(4), 100);
    let (ready_tx, ready_rx) = mpsc::channel();
    let (run_tx, run_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut session = Session::new_local().unwrap();
        install_shared(&mut session, 200);
        ready_tx.send(()).unwrap();
        run_rx.recv().unwrap();
        assert_eq!(increment(4), 200);
    });
    ready_rx.recv().unwrap();
    drop(session);
    assert_eq!(increment(4), 5);
    run_tx.send(()).unwrap();
    worker.join().unwrap();
    assert_eq!(increment(4), 5);
}

#[test]
fn panic_restores_the_original_and_releases_the_local_session() {
    let _serial = serial_test();
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut session = Session::new_local().unwrap();
        let mock = mock!(session, increment, fn(i64) -> i64).unwrap();
        mock.expect().once().panics("local failure").unwrap();
        increment(1);
    }));
    assert!(result.is_err());
    assert_eq!(increment(1), 2);
    let mut session = Session::new_local().unwrap();
    install_shared(&mut session, 40);
    assert_eq!(increment(1), 40);
}

#[test]
fn duplicate_installation_can_be_retried_after_restore() {
    let _serial = serial_test();
    let mut session = Session::new_local().unwrap();
    let first = mock!(session, increment, fn(i64) -> i64).unwrap();
    first.expect().once().returns(80).unwrap();
    for should_succeed in [false, true] {
        let result = mock!(session, increment, fn(i64) -> i64);
        if should_succeed {
            result.unwrap().expect().once().returns(90).unwrap();
            assert_eq!(increment(1), 90);
        } else {
            assert!(matches!(result, Err(Error::Overlap)));
            assert_eq!(increment(1), 80);
            session.restore().unwrap();
        }
    }
}

#[test]
fn global_and_local_sessions_exclude_each_other() {
    let _serial = serial_test();
    let local = Session::new_local().unwrap();
    assert!(matches!(Session::new_local(), Err(Error::Busy)));
    assert!(matches!(Session::new_global(), Err(Error::Busy)));
    std::thread::spawn(|| {
        assert!(matches!(Session::new_global(), Err(Error::Busy)));
        assert!(Session::new_local().is_ok());
    })
    .join()
    .unwrap();
    drop(local);
    let global = Session::new_global().unwrap();
    assert!(matches!(Session::new_local(), Err(Error::Busy)));
    std::thread::spawn(|| {
        assert!(matches!(Session::new_local(), Err(Error::Busy)));
    })
    .join()
    .unwrap();
    drop(global);
    assert!(Session::new_local().is_ok());
}

#[test]
fn raw_replacements_are_rejected_in_local_mode() {
    let _serial = serial_test();
    let mut session = Session::new_local().unwrap();
    let result = shimforge::replace!(session, increment => |x| x + 10, fn(i64) -> i64);
    assert!(matches!(result, Err(Error::Expectation(_))));
    assert_eq!(increment(1), 2);
}

#[test]
fn local_sessions_recover_after_a_global_panic() {
    let _serial = serial_test();
    assert!(
        catch_unwind(|| {
            let _session = Session::new_global().unwrap();
            panic!("global failure");
        })
        .is_err()
    );
    let mut session = Session::new_local().unwrap();
    install_shared(&mut session, 40);
    assert_eq!(increment(1), 40);
}

fn aggregate(seed: u64, gain: f64) -> [u64; 12] {
    [seed + gain as u64; 12]
}

#[test]
fn original_fallback_preserves_large_returns_and_float_arguments() {
    let _serial = serial_test();
    let mut session = Session::new_local().unwrap();
    let mock = mock!(session, aggregate, fn(u64, f64) -> [u64; 12]).unwrap();
    mock.expect().once().returns([40; 12]).unwrap();
    assert_eq!(aggregate(3, 4.0), [40; 12]);
    assert_eq!(
        std::thread::spawn(|| aggregate(3, 4.0)).join().unwrap(),
        [7; 12]
    );
}

fn may_panic(value: i64) -> i64 {
    assert!(value > 0, "original panic");
    value + 1
}

#[test]
fn original_fallback_can_unwind() {
    let _serial = serial_test();
    let mut session = Session::new_local().unwrap();
    let mock = mock!(session, may_panic, fn(i64) -> i64).unwrap();
    mock.expect().once().returns(10).unwrap();
    std::thread::spawn(|| assert!(catch_unwind(|| may_panic(0)).is_err()))
        .join()
        .unwrap();
    assert_eq!(may_panic(0), 10);
}

#[test]
fn borrowed_and_owned_results_keep_their_original_fallback() {
    let _serial = serial_test();
    let mut session = Session::new_local().unwrap();
    let mock = mock!(session, borrowed, fn(&str) -> &str).unwrap();
    mock.expect().once().returning(|value| value).unwrap();
    let mock = mock!(session, owned, fn(String) -> String).unwrap();
    mock.expect()
        .once()
        .returning(|value| format!("mock {value}"))
        .unwrap();
    assert_eq!(borrowed(" value "), " value ");
    assert_eq!(owned("value".to_owned()), "mock value");
    std::thread::spawn(|| {
        let value = " value ".to_owned();
        assert_eq!(borrowed(&value), "value");
        assert_eq!(owned(value), "original  value ");
    })
    .join()
    .unwrap();
}

#[test]
fn native_abi_mocks_are_thread_local() {
    let _serial = serial_test();
    let mut session = Session::new_local().unwrap();
    let mock = mock!(session, native, extern "C" fn(i64) -> i64).unwrap();
    mock.expect().once().returns(70).unwrap();
    assert_eq!(native(5), 70);
    assert_eq!(std::thread::spawn(|| native(5)).join().unwrap(), 7);
}

fn ready<F: Future>(future: F) -> F::Output {
    let mut context = Context::from_waker(Waker::noop());
    match pin!(future).as_mut().poll(&mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("expected a ready future"),
    }
}

async fn fetch(value: i64) -> i64 {
    value + 1
}

#[test]
fn async_mocks_leave_other_threads_unchanged() {
    let _serial = serial_test();
    let mut session = Session::new_local().unwrap();
    let mock = session.mock_async(fetch(0)).unwrap();
    mock.expect().once().returns(60).unwrap();
    assert_eq!(ready(fetch(5)), 60);
    assert_eq!(std::thread::spawn(|| ready(fetch(5))).join().unwrap(), 6);
    session.restore().unwrap();
    assert_eq!(ready(fetch(5)), 6);
}

#[test]
fn async_sessions_can_share_a_poll_function_and_drop_in_either_order() {
    let _serial = serial_test();
    let mut session = Session::new_local().unwrap();
    let mock = session.mock_async(fetch(0)).unwrap();
    mock.expect().once().returns(100).unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (run_tx, run_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut session = Session::new_local().unwrap();
        let mock = session.mock_async(fetch(0)).unwrap();
        mock.expect().once().returns(200).unwrap();
        ready_tx.send(()).unwrap();
        run_rx.recv().unwrap();
        assert_eq!(ready(fetch(4)), 200);
    });
    ready_rx.recv().unwrap();
    assert_eq!(ready(fetch(4)), 100);
    drop(session);
    assert_eq!(ready(fetch(4)), 5);
    run_tx.send(()).unwrap();
    worker.join().unwrap();
    assert_eq!(ready(fetch(4)), 5);
}

#[test]
fn file_reads_are_mocked_only_on_the_current_thread() {
    let _serial = serial_test();
    use std::io::Write;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "shimforge-local-{}-{stamp}.txt",
        std::process::id()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    file.write_all(b"disk content").unwrap();
    drop(file);
    let mut session = Session::new_local().unwrap();
    let mock = mock!(
        session,
        std::fs::read_to_string::<&std::path::Path>,
        fn(&std::path::Path) -> std::io::Result<String>
    )
    .unwrap();
    mock.expect()
        .once()
        .return_once(Ok("mock content".to_owned()))
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(path.as_path()).unwrap(),
        "mock content"
    );
    let worker_path = path.clone();
    assert_eq!(
        std::thread::spawn(move || std::fs::read_to_string(worker_path.as_path()).unwrap())
            .join()
            .unwrap(),
        "disk content"
    );
    session.restore().unwrap();
    std::fs::remove_file(path).unwrap();
}
