//! Every Rust example in README.md, compiled and run as integration tests.
//!
//! Each module holds the examples of one README section, indented one level.
//! `readme_examples_match_this_file` fails when this file and README.md differ, so
//! change both together.

mod why_shimforge {
    use std::fs;

    fn claim_slot() -> Result<(), String> {
        if let Err(error) = fs::create_dir_all("/var/run/dispatcher") {
            // Failure path.
            return Err(format!("cannot claim a slot: {error}"));
        }

        // Success path.
        Ok(())
    }

    use shimforge::{Session, mock};
    use std::io;

    #[test]
    fn claim_slot_succeeds_without_a_real_directory() {
        let mut session = Session::new();
        let create = mock!(
            session,
            fs::create_dir_all::<&str>,
            fn(&str) -> io::Result<()>
        );
        create
            .expect()
            .with(|path| *path == "/var/run/dispatcher")
            .once()
            .returning(|_| Ok(()));

        assert!(claim_slot().is_ok());
    }
}

#[allow(unused_imports)]
mod usage {
    use shimforge::{Session, mock, replace};
}

mod thread_local_and_global_sessions {
    use shimforge::{Session, mock};

    fn worker_count() -> usize {
        2
    }

    #[test]
    fn other_threads_keep_the_original_function() {
        let mut session = Session::new();
        let count = mock!(session, worker_count, fn() -> usize);
        count.expect().returns(16);

        assert_eq!(worker_count(), 16);
        // A thread with no session of its own still calls the original.
        assert_eq!(std::thread::spawn(worker_count).join().unwrap(), 2);
    }
}

mod constant_results {
    use shimforge::{Session, mock};
    use std::path::Path;

    fn export_state(marker: &Path) -> &'static str {
        if marker.exists() {
            "finished"
        } else {
            "running"
        }
    }

    #[test]
    fn a_constant_result_answers_every_call() {
        let mut session = Session::new();
        let exists = mock!(session, Path::exists, fn(&Path) -> bool);
        exists.expect().returns(true);

        assert_eq!(export_state(Path::new("virtual/export.done")), "finished");
        session.restore();
        assert_eq!(export_state(Path::new("virtual/export.done")), "running");
    }
}

mod matching_arguments_and_counting_calls {
    use shimforge::{Session, mock};
    use std::{fs, io, path::Path};

    fn load_port(path: &Path) -> io::Result<u16> {
        fs::read_to_string(path)?
            .trim()
            .parse()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    #[test]
    fn matching_calls_are_counted() {
        let mut session = Session::new();
        let read = mock!(
            session,
            fs::read_to_string::<&Path>,
            fn(&Path) -> io::Result<String>
        );
        read.expect()
            .with(|path| *path == Path::new("service.port"))
            .times(2)
            .returning(|_| Ok("8080\n".to_owned()));

        assert_eq!(load_port(Path::new("service.port")).unwrap(), 8080);
        assert_eq!(load_port(Path::new("service.port")).unwrap(), 8080);
    }
}

mod checking_expectations_during_a_test {
    use shimforge::{Session, mock};

    fn fetch_config(name: &str) -> String {
        // Reads a configuration service in production.
        format!("live {name}")
    }

    fn start_worker() -> String {
        fetch_config("worker")
    }

    #[test]
    fn expectations_are_checked_before_the_next_step() {
        let mut session = Session::new();
        let fetch = mock!(session, fetch_config, fn(&str) -> String);
        fetch
            .expect()
            .with(|name| *name == "worker")
            .once()
            .returns("threads=4".to_owned());

        assert_eq!(start_worker(), "threads=4");
        // Fails here instead of at the end of the test if the fetch was skipped.
        session.verify();

        fetch.expect().returns("threads=8".to_owned());
        assert_eq!(start_worker(), "threads=8");
    }
}

mod clearing_rules_between_steps {
    use shimforge::{Session, mock};

    fn read_status(service: &str) -> String {
        // Queries a health endpoint in production.
        format!("{service}: live")
    }

    fn is_ready(service: &str) -> bool {
        read_status(service).ends_with("ready")
    }

    #[test]
    fn each_step_starts_with_its_own_rules() {
        let mut session = Session::new();
        let status = mock!(session, read_status, fn(&str) -> String);

        // Step 1: the service is still starting, however often it is asked.
        status.expect().returns("billing: starting".to_owned());
        assert!(!is_ready("billing"));
        assert!(!is_ready("billing"));
        // Checks step 1 and removes its rule, which would otherwise keep answering.
        status.checkpoint();

        // Step 2: the service is ready.
        status.expect().once().returns("billing: ready".to_owned());
        assert!(is_ready("billing"));
    }
}

mod return_values_and_call_order {
    use shimforge::{Sequence, Session, mock};

    fn next_id() -> u64 {
        1
    }

    fn save_id(id: u64) -> bool {
        id > 0
    }

    #[test]
    fn results_follow_the_call_order() {
        let mut session = Session::new();
        let next = mock!(session, next_id, fn() -> u64);
        let save = mock!(session, save_id, fn(u64) -> bool);
        let order = Sequence::new();
        let mut id = 40;
        next.expect()
            .times(2)
            .in_sequence(&order)
            .returning(move || {
                id += 1;
                id
            });
        save.expect()
            .with(|id| *id == 42)
            .once()
            .in_sequence(&order)
            .returns(true);

        assert_eq!(next_id(), 41);
        assert_eq!(next_id(), 42);
        assert!(save_id(42));
    }
}

mod writing_through_reference_parameters {
    use shimforge::{Session, mock};

    fn split_amount(total: u64, whole: &mut u64, cents: &mut u64) {
        *whole = total / 100;
        *cents = total % 100;
    }

    fn fill(buffer: &mut [u8]) -> usize {
        buffer.fill(0);
        buffer.len()
    }

    #[test]
    fn a_mock_fills_output_parameters() {
        let mut session = Session::new();
        let split = mock!(session, split_amount, fn(u64, &mut u64, &mut u64));
        split
            .expect()
            .with(|total, _, _| *total == 1234)
            .once()
            .returning(|_, whole, cents| {
                *whole = 99;
                *cents = 5;
            });
        let write = mock!(session, fill, fn(&mut [u8]) -> usize);
        write.expect().once().returning(|buffer| {
            buffer[..2].copy_from_slice(b"ok");
            2
        });

        let (mut whole, mut cents) = (0, 0);
        split_amount(1234, &mut whole, &mut cents);
        assert_eq!((whole, cents), (99, 5));

        let mut buffer = [0; 8];
        assert_eq!(fill(&mut buffer), 2);
        assert_eq!(&buffer[..2], b"ok");
    }
}

mod methods_and_generic_functions {
    use shimforge::{Session, mock};
    use std::fmt::Display;

    struct Cache {
        region: String,
    }

    impl Cache {
        fn hit_rate(&self, key: &str) -> f32 {
            // Reads live counters in production.
            (self.region.len() + key.len()) as f32 / 100.0
        }
    }

    fn render<T: Display>(value: T) -> String {
        format!("live {value}")
    }

    #[test]
    fn a_method_and_one_generic_instance_are_mocked() {
        let mut session = Session::new();
        let rates = mock!(session, Cache::hit_rate, fn(&Cache, &str) -> f32);
        rates
            .expect()
            .with(|cache, key| cache.region == "eu" && *key == "sessions")
            .once()
            .returns(0.75);
        let rendered = mock!(session, render::<u8>, fn(u8) -> String);
        rendered.expect().once().returns("mocked".to_owned());

        let cache = Cache {
            region: "eu".to_owned(),
        };
        assert_eq!(cache.hit_rate("sessions"), 0.75);
        assert_eq!(render(7u8), "mocked");
        // A different type argument is a different function.
        assert_eq!(render("7"), "live 7");
    }
}

mod replacing_a_whole_function {
    use shimforge::{Session, replace};

    fn checksum(bytes: &[u8]) -> u32 {
        bytes.iter().map(|byte| u32::from(*byte)).sum()
    }

    fn fixed_checksum(_bytes: &[u8]) -> u32 {
        7
    }

    #[test]
    fn a_function_or_closure_replaces_the_original() {
        let mut session = Session::new();
        replace!(session, checksum => fixed_checksum, fn(&[u8]) -> u32);
        assert_eq!(checksum(b"abc"), 7);
        session.restore();

        replace!(session, checksum => |_| 9, fn(&[u8]) -> u32);
        assert_eq!(checksum(b"abc"), 9);
    }
}

mod async_functions {
    use shimforge::Session;
    use std::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };

    async fn exchange_rate(_pair: &str) -> f64 {
        // Calls a pricing service in production.
        0.0
    }

    struct Ledger {
        name: String,
    }

    impl Ledger {
        async fn balance(&self) -> u64 {
            // Reads this ledger from a database in production.
            self.name.len() as u64
        }
    }

    fn ready<F: Future>(future: F) -> F::Output {
        let mut future = pin!(future);
        let mut context = Context::from_waker(Waker::noop());
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("a mocked future is always ready"),
        }
    }

    #[test]
    fn async_functions_return_mocked_results() {
        let mut session = Session::new();
        let rates = session.mock_async(exchange_rate(""));
        rates.expect().once().returns(1.25);
        // A throwaway receiver is enough to name the future type.
        let balances = session.mock_async(
            Ledger {
                name: String::new(),
            }
            .balance(),
        );
        balances.expect().once().returns(4_200);

        assert_eq!(ready(exchange_rate("EURUSD")), 1.25);
        let ledger = Ledger {
            name: "payroll".to_owned(),
        };
        assert_eq!(ready(ledger.balance()), 4_200);
    }
}

mod client_libraries_that_return_boxed_futures {
    use shimforge::{Session, mock};
    use std::{
        future::Future,
        io,
        pin::{Pin, pin},
        task::{Context, Poll, Waker},
    };

    type Call<'a> = Pin<Box<dyn Future<Output = io::Result<String>> + Send + 'a>>;

    struct Client {
        endpoint: String,
    }

    impl Client {
        fn get<'a>(&'a self, path: &'a str) -> Call<'a> {
            Box::pin(async move {
                // Opens a connection in production.
                Ok(format!("live {}{path}", self.endpoint))
            })
        }
    }

    #[test]
    fn a_client_method_that_returns_a_boxed_future_is_mocked() {
        let mut session = Session::new();
        let get = mock!(
            session,
            Client::get,
            for<'a> fn(&'a Client, &'a str) -> Call<'a>
        );
        get.expect()
            .with(|_, path| *path == "/health")
            .once()
            .returning(|_, _| Box::pin(async { Ok("healthy".to_owned()) }));

        let client = Client {
            endpoint: "https://inventory.invalid".to_owned(),
        };
        let mut response = pin!(client.get("/health"));
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(
            response.as_mut().poll(&mut context),
            Poll::Ready(Ok(body)) if body == "healthy"
        ));
    }
}

mod system_and_c_runtime_functions {
    use shimforge::{Session, mock};
    use std::ffi::{CStr, CString, c_char};

    unsafe extern "C" {
        fn getenv(name: *const c_char) -> *mut c_char;
    }

    #[test]
    fn getenv_reports_a_mocked_variable() {
        let mut session = Session::new();
        let lookup = mock!(
            session,
            getenv,
            unsafe extern "C" fn(*const c_char) -> *mut c_char
        );
        lookup
            .expect()
            .with(|name| {
                // SAFETY: callers of getenv always pass a valid C string.
                let name = unsafe { CStr::from_ptr(*name) };
                name == c"DEPLOY_SLOT"
            })
            .once()
            .returning(|_| c"canary".as_ptr().cast_mut());
        // Any other variable keeps reporting that it is unset.
        lookup.expect().returning(|_| std::ptr::null_mut());

        let key = CString::new("DEPLOY_SLOT").unwrap();
        // SAFETY: the key is a valid C string and the result is only read.
        let slot = unsafe { CStr::from_ptr(getenv(key.as_ptr())) };
        assert_eq!(slot.to_str().unwrap(), "canary");
    }
}

mod raw_replacement_without_signature_checks {
    use shimforge::Session;

    fn slot_count() -> usize {
        4
    }

    fn fake_slot_count() -> usize {
        64
    }

    #[test]
    fn a_raw_replacement_swaps_one_function_for_another() {
        let mut session = Session::new_global();
        // SAFETY: both functions are live, share a signature, and stay loaded until the
        // session restores them. No thread calls them while the patch is installed.
        unsafe { session.replace_raw(slot_count as *const (), fake_slot_count as *const ()) };

        assert_eq!(slot_count(), 64);
        session.restore();
        assert_eq!(slot_count(), 4);
    }
}

/// A line of example code and its line number in the file it came from.
type Lines<'a> = Vec<(usize, &'a str)>;

#[test]
fn readme_examples_match_this_file() {
    let readme = block_lines(
        include_str!("../README.md"),
        |line| line.starts_with("```rust"),
        "```",
        "",
    );
    let modules = block_lines(
        include_str!("readme.rs"),
        |line| line.starts_with("mod ") && line.ends_with(" {"),
        "}",
        "    ",
    );
    for (&(readme_line, expected), &(test_line, actual)) in readme.iter().zip(&modules) {
        assert_eq!(
            actual, expected,
            "tests/readme.rs:{test_line} does not match README.md:{readme_line}"
        );
    }
    assert_eq!(
        modules.len(),
        readme.len(),
        "tests/readme.rs and README.md have a different number of example lines"
    );
}

/// Collects the lines between each line that `opens` a block and the next `close` line,
/// with `indent` removed, and puts a blank line between blocks.
fn block_lines<'a>(text: &'a str, opens: fn(&str) -> bool, close: &str, indent: &str) -> Lines<'a> {
    let mut lines = Vec::new();
    let mut in_block = false;
    for (number, line) in (1..).zip(text.lines()) {
        if in_block {
            if line == close {
                in_block = false;
            } else {
                lines.push((number, line.strip_prefix(indent).unwrap_or(line)));
            }
        } else if opens(line) {
            if !lines.is_empty() {
                lines.push((number, ""));
            }
            in_block = true;
        }
    }
    lines
}
