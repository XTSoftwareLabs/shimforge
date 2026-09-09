use shimforge::{Error, Session, replace};
use std::hint::black_box;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, MutexGuard};

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn serial_test() -> MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

#[inline(never)]
fn increment(value: i64) -> i64 {
    black_box(value) + black_box(1)
}

#[inline(never)]
fn add_ten(value: i64) -> i64 {
    black_box(value) + black_box(10)
}

#[inline(never)]
fn add_hundred(value: i64) -> i64 {
    black_box(value) + black_box(100)
}

#[inline(never)]
fn multiply(value: i64) -> i64 {
    black_box(value) * black_box(2)
}

#[inline(never)]
fn triple(value: i64) -> i64 {
    black_box(value) * black_box(3)
}

#[test]
fn direct_calls_are_replaced_and_restored_on_drop() {
    let _serial = serial_test();
    assert_eq!(increment(black_box(7)), 8);
    {
        let mut session = Session::new().unwrap();
        replace!(session, increment => add_ten, fn(i64) -> i64).unwrap();
        assert_eq!(increment(black_box(7)), 17);
        assert_eq!(add_ten(black_box(7)), 17);
    }
    assert_eq!(increment(black_box(7)), 8);
}

#[test]
fn multiple_replacements_restore_together() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    replace!(session, increment => add_ten, fn(i64) -> i64).unwrap();
    replace!(session, multiply => triple, fn(i64) -> i64).unwrap();
    assert_eq!(increment(black_box(7)), 17);
    assert_eq!(multiply(black_box(7)), 21);

    session.restore().unwrap();
    assert_eq!(increment(black_box(7)), 8);
    assert_eq!(multiply(black_box(7)), 14);
}

#[test]
fn explicit_restore_is_idempotent_and_session_can_be_reused() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    session.restore().unwrap();
    replace!(session, increment => add_ten, fn(i64) -> i64).unwrap();
    assert_eq!(increment(black_box(3)), 13);
    session.restore().unwrap();
    session.restore().unwrap();
    assert_eq!(increment(black_box(3)), 4);

    replace!(session, increment => add_hundred, fn(i64) -> i64).unwrap();
    assert_eq!(increment(black_box(3)), 103);
    session.restore().unwrap();
    assert_eq!(increment(black_box(3)), 4);
}

#[test]
fn panic_unwinding_restores_and_releases_session() {
    let _serial = serial_test();
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut session = Session::new().unwrap();
        replace!(session, increment => add_ten, fn(i64) -> i64).unwrap();
        assert_eq!(increment(black_box(2)), 12);
        panic!("exercise session cleanup");
    }));
    assert!(result.is_err());
    assert_eq!(increment(black_box(2)), 3);

    let mut session = Session::new().unwrap();
    replace!(session, increment => add_hundred, fn(i64) -> i64).unwrap();
    assert_eq!(increment(black_box(2)), 102);
}

#[test]
fn nested_sessions_fail_without_blocking() {
    let _serial = serial_test();
    let session = Session::new().unwrap();
    assert!(matches!(Session::new(), Err(Error::Busy)));
    drop(session);
    assert!(Session::new().is_ok());
}

#[test]
fn session_exclusivity_extends_to_other_threads() {
    let _serial = serial_test();
    let session = Session::new().unwrap();
    assert!(
        std::thread::spawn(|| matches!(Session::new(), Err(Error::Busy)))
            .join()
            .unwrap()
    );
    drop(session);
    assert!(
        std::thread::spawn(|| Session::new().is_ok())
            .join()
            .unwrap()
    );
}

#[test]
fn installed_replacement_is_visible_to_a_worker_thread() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    replace!(session, increment => add_ten, fn(i64) -> i64).unwrap();
    // Start workers after installation and join them before restoration.
    let answer = std::thread::spawn(|| increment(black_box(31)))
        .join()
        .unwrap();
    assert_eq!(answer, 41);
    session.restore().unwrap();
    assert_eq!(increment(black_box(31)), 32);
}

#[test]
fn duplicate_replacement_fails_without_losing_original() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    replace!(session, increment => add_ten, fn(i64) -> i64).unwrap();
    assert_eq!(
        replace!(session, increment => add_hundred, fn(i64) -> i64),
        Err(Error::Overlap)
    );
    assert_eq!(increment(black_box(9)), 19);
    session.restore().unwrap();
    assert_eq!(increment(black_box(9)), 10);
}

#[test]
fn replacement_cycles_are_rejected_without_disturbing_existing_patches() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    replace!(session, increment => add_ten, fn(i64) -> i64).unwrap();
    assert_eq!(
        replace!(session, add_ten => increment, fn(i64) -> i64),
        Err(Error::Overlap)
    );
    assert_eq!(increment(black_box(9)), 19);
    assert_eq!(add_ten(black_box(9)), 19);
    session.restore().unwrap();
    assert_eq!(increment(black_box(9)), 10);
    assert_eq!(add_ten(black_box(9)), 19);
}

#[inline(never)]
fn panic_replacement(value: i64) -> i64 {
    panic!("replacement rejected {}", black_box(value));
}

#[test]
fn replacement_can_unwind_through_the_original_caller() {
    let _serial = serial_test();
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut session = Session::new().unwrap();
        replace!(session, increment => panic_replacement, fn(i64) -> i64).unwrap();
        increment(black_box(42))
    }));
    let payload = result.unwrap_err();
    assert_eq!(
        payload.downcast_ref::<String>().unwrap(),
        "replacement rejected 42"
    );
    assert_eq!(increment(black_box(42)), 43);
    assert!(Session::new().is_ok());
}

#[test]
fn replacing_a_function_with_itself_is_rejected() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    assert_eq!(
        replace!(session, increment => increment, fn(i64) -> i64),
        Err(Error::SameAddress)
    );
    assert_eq!(increment(black_box(9)), 10);
}

#[inline(never)]
fn greeting(name: &str) -> String {
    format!("hello {}", black_box(name))
}

#[inline(never)]
fn fake_greeting(name: &str) -> String {
    format!("welcome {}", black_box(name))
}

#[test]
fn owned_return_values_preserve_ownership() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    replace!(session, greeting => fake_greeting, fn(&str) -> String).unwrap();
    let answer = greeting(black_box("Rust"));
    session.restore().unwrap();
    assert_eq!(answer, "welcome Rust");
    assert_eq!(greeting(black_box("Rust")), "hello Rust");
}

#[derive(Debug, PartialEq)]
struct Aggregate {
    values: [u64; 12],
    label: String,
}

#[inline(never)]
fn aggregate(seed: u64, label: &str) -> Aggregate {
    Aggregate {
        values: [black_box(seed); 12],
        label: black_box(label).to_owned(),
    }
}

#[inline(never)]
fn fake_aggregate(seed: u64, label: &str) -> Aggregate {
    Aggregate {
        values: [black_box(seed) + 17; 12],
        label: format!("fake {}", black_box(label)),
    }
}

#[test]
fn large_aggregate_return_keeps_the_hidden_return_pointer() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    replace!(session, aggregate => fake_aggregate, fn(u64, &str) -> Aggregate).unwrap();
    assert_eq!(
        aggregate(black_box(25), black_box("payload")),
        Aggregate {
            values: [42; 12],
            label: "fake payload".to_owned(),
        }
    );
    session.restore().unwrap();
    assert_eq!(aggregate(black_box(25), "payload").values, [25; 12]);
}

#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn many_arguments(
    a: u64,
    b: u64,
    c: u64,
    d: u64,
    e: u64,
    f: u64,
    g: u64,
    h: u64,
    x: f64,
    y: f64,
    z: f64,
    u: f64,
    v: f64,
) -> f64 {
    black_box(a + b + c + d + e + f + g + h) as f64 + black_box(x + y + z + u + v)
}

#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn fake_many_arguments(
    a: u64,
    b: u64,
    c: u64,
    d: u64,
    e: u64,
    f: u64,
    g: u64,
    h: u64,
    x: f64,
    y: f64,
    z: f64,
    u: f64,
    v: f64,
) -> f64 {
    black_box(a + 2 * b + 3 * c + 4 * d + 5 * e + 6 * f + 7 * g + 8 * h) as f64
        + black_box(x + 2.0 * y + 3.0 * z + 4.0 * u + 5.0 * v)
}

#[test]
fn replacement_preserves_integer_float_and_stack_arguments() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    replace!(
        session,
        many_arguments => fake_many_arguments,
        fn(u64, u64, u64, u64, u64, u64, u64, u64, f64, f64, f64, f64, f64) -> f64
    )
    .unwrap();
    assert_eq!(
        many_arguments(black_box(1), 2, 3, 4, 5, 6, 7, 8, 0.5, 1.0, 1.5, 2.0, 2.5),
        231.5
    );
    session.restore().unwrap();
    assert_eq!(
        many_arguments(black_box(1), 2, 3, 4, 5, 6, 7, 8, 0.5, 1.0, 1.5, 2.0, 2.5),
        43.5
    );
}

#[inline(never)]
fn borrowed(value: &str) -> &str {
    let value = black_box(value);
    black_box(&value[..1])
}

#[inline(never)]
fn borrowed_identity(value: &str) -> &str {
    let value = black_box(value);
    black_box(value)
}

#[test]
fn replacement_preserves_the_borrowed_argument_lifetime() {
    let _serial = serial_test();
    let owned = String::from("borrowed data");
    let mut session = Session::new().unwrap();
    replace!(session, borrowed => borrowed_identity, for<'a> fn(&'a str) -> &'a str).unwrap();
    let answer = borrowed(black_box(owned.as_str()));
    assert_eq!(answer, owned);
    assert_eq!(answer.as_ptr(), owned.as_ptr());
    session.restore().unwrap();
    assert_eq!(answer, "borrowed data");
    assert_eq!(borrowed(black_box(owned.as_str())), "b");
}

struct Counter {
    value: i64,
}

impl Counter {
    #[inline(never)]
    fn advance(&mut self, amount: i64) -> i64 {
        self.value += black_box(amount);
        black_box(self.value)
    }

    #[inline(never)]
    fn fake_advance(&mut self, amount: i64) -> i64 {
        self.value += black_box(amount) * 10;
        black_box(self.value)
    }
}

#[test]
fn methods_with_mutable_receivers_are_supported() {
    let _serial = serial_test();
    let mut counter = Counter { value: 5 };
    let mut session = Session::new().unwrap();
    replace!(session, Counter::advance => Counter::fake_advance, fn(&mut Counter, i64) -> i64)
        .unwrap();
    assert_eq!(counter.advance(black_box(3)), 35);
    session.restore().unwrap();
    assert_eq!(counter.advance(black_box(2)), 37);
}

#[inline(never)]
fn generic_size<T>(seed: usize) -> usize {
    black_box(seed) + black_box(std::mem::size_of::<T>())
}

#[inline(never)]
fn fake_generic_size(seed: usize) -> usize {
    black_box(seed) + black_box(100)
}

#[test]
fn replacing_one_generic_instantiation_preserves_another() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    replace!(session, generic_size::<u32> => fake_generic_size, fn(usize) -> usize).unwrap();
    assert_eq!(generic_size::<u32>(black_box(1)), 101);
    assert_eq!(generic_size::<u64>(black_box(1)), 9);
    session.restore().unwrap();
    assert_eq!(generic_size::<u32>(black_box(1)), 5);
    assert_eq!(generic_size::<u64>(black_box(1)), 9);
}

#[inline(never)]
extern "C" fn native_sum(a: i64, b: i64) -> i64 {
    black_box(a) + black_box(b)
}

#[inline(never)]
extern "C" fn native_product(a: i64, b: i64) -> i64 {
    black_box(a) * black_box(b)
}

#[test]
fn native_abi_functions_are_supported() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    replace!(session, native_sum => native_product, extern "C" fn(i64, i64) -> i64).unwrap();
    assert_eq!(native_sum(black_box(6), black_box(7)), 42);
    session.restore().unwrap();
    assert_eq!(native_sum(black_box(6), black_box(7)), 13);
}

#[test]
fn noncapturing_closure_can_be_a_replacement() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    replace!(session, increment => |value| black_box(value) - black_box(2), fn(i64) -> i64)
        .unwrap();
    assert_eq!(increment(black_box(42)), 40);
    session.restore().unwrap();
    assert_eq!(increment(black_box(42)), 43);
}

#[inline(never)]
fn touch(value: &mut usize) {
    *black_box(value) += black_box(1);
}

#[inline(never)]
fn fake_touch(value: &mut usize) {
    *black_box(value) += black_box(5);
}

#[test]
fn unit_return_functions_are_supported() {
    let _serial = serial_test();
    let mut value = 0;
    let mut session = Session::new().unwrap();
    replace!(session, touch => fake_touch, fn(&mut usize)).unwrap();
    touch(black_box(&mut value));
    assert_eq!(value, 5);
    session.restore().unwrap();
    touch(black_box(&mut value));
    assert_eq!(value, 6);
}

#[inline(never)]
unsafe fn unsafe_increment(value: i64) -> i64 {
    black_box(value) + black_box(1)
}

#[inline(never)]
unsafe fn unsafe_add_ten(value: i64) -> i64 {
    black_box(value) + black_box(10)
}

#[test]
fn unsafe_function_installation_does_not_require_an_unsafe_block() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    replace!(session, unsafe_increment => unsafe_add_ten, unsafe fn(i64) -> i64).unwrap();
    // SAFETY: both test functions accept every i64 value without other preconditions.
    assert_eq!(unsafe { unsafe_increment(black_box(5)) }, 15);
    session.restore().unwrap();
    // SAFETY: both test functions accept every i64 value without other preconditions.
    assert_eq!(unsafe { unsafe_increment(black_box(5)) }, 6);
}

#[inline(never)]
extern "system" fn system_sum(a: i64, b: i64) -> i64 {
    black_box(a) + black_box(b)
}

#[inline(never)]
extern "system" fn system_product(a: i64, b: i64) -> i64 {
    black_box(a) * black_box(b)
}

#[inline(never)]
unsafe extern "C" fn unsafe_native_sum(a: i64, b: i64) -> i64 {
    black_box(a) + black_box(b)
}

#[inline(never)]
unsafe extern "C" fn unsafe_native_product(a: i64, b: i64) -> i64 {
    black_box(a) * black_box(b)
}

#[test]
fn system_abi_and_unsafe_native_functions_are_supported() {
    let _serial = serial_test();
    let mut session = Session::new().unwrap();
    replace!(session, system_sum => system_product, extern "system" fn(i64, i64) -> i64).unwrap();
    replace!(session, unsafe_native_sum => unsafe_native_product, unsafe extern "C" fn(i64, i64) -> i64)
        .unwrap();
    assert_eq!(system_sum(black_box(6), black_box(7)), 42);
    // SAFETY: both test functions accept every pair of i64 values without other preconditions.
    assert_eq!(unsafe { unsafe_native_sum(black_box(6), black_box(7)) }, 42);
    session.restore().unwrap();
    assert_eq!(system_sum(black_box(6), black_box(7)), 13);
    // SAFETY: both test functions accept every pair of i64 values without other preconditions.
    assert_eq!(unsafe { unsafe_native_sum(black_box(6), black_box(7)) }, 13);
}
