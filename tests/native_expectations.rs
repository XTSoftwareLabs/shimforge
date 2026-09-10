use shimforge::{Session, mock};
use std::sync::{Mutex, MutexGuard};

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

extern "C" fn native_sum(a: i64, b: i64) -> i64 {
    a + b
}

unsafe fn unchecked_sum(a: i64, b: i64) -> i64 {
    a + b
}

extern "system" fn system_sum(a: i64, b: i64) -> i64 {
    a + b
}

extern "C-unwind" fn unwind_sum(a: i64, b: i64) -> i64 {
    a + b
}

#[test]
fn native_calls_match_arguments_and_count_captured_responses() {
    let _serial = serial();
    let mut session = Session::new().unwrap();
    let sum = mock!(session, native_sum, extern "C" fn(i64, i64) -> i64).unwrap();
    let mut results = vec![42, 24].into_iter();
    let count = sum
        .expect()
        .with(|a, b| *a == 6 && *b == 7)
        .times(2)
        .returning(move |_, _| results.next().unwrap())
        .unwrap();
    assert_eq!(native_sum(6, 7), 42);
    assert_eq!(native_sum(6, 7), 24);
    assert_eq!(count.calls(), 2);
    session.restore().unwrap();
    assert_eq!(native_sum(6, 7), 13);
}

#[test]
fn unsafe_and_system_functions_accept_expectations() {
    let _serial = serial();
    let mut session = Session::new().unwrap();
    let sum = mock!(session, unchecked_sum, unsafe fn(i64, i64) -> i64).unwrap();
    sum.expect().once().returns(42).unwrap();
    // SAFETY: the function accepts all i64 values.
    assert_eq!(unsafe { unchecked_sum(6, 7) }, 42);
    let system = mock!(session, system_sum, extern "system" fn(i64, i64) -> i64).unwrap();
    system.expect().once().returning(|a, b| a * b).unwrap();
    assert_eq!(system_sum(6, 7), 42);
    session.verify().unwrap();
}

#[test]
fn unwind_abi_keeps_rust_panic_behavior() {
    let _serial = serial();
    let mut session = Session::new().unwrap();
    let sum = mock!(session, unwind_sum, extern "C-unwind" fn(i64, i64) -> i64).unwrap();
    sum.expect().once().panics("chosen failure").unwrap();
    assert!(std::panic::catch_unwind(|| unwind_sum(6, 7)).is_err());
    session.restore().unwrap();
    assert_eq!(unwind_sum(6, 7), 13);
}

#[test]
fn native_panic_does_not_cross_the_abi_boundary() {
    const CHILD: &str = "SHIMFORGE_NATIVE_PANIC_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let mut session = Session::new().unwrap();
        let sum = mock!(session, native_sum, extern "C" fn(i64, i64) -> i64).unwrap();
        sum.expect().panics("native callback failed").unwrap();
        native_sum(1, 2);
        std::process::exit(99);
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "native_panic_does_not_cross_the_abi_boundary",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_ne!(output.status.code(), Some(99));
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("native callback failed"), "{error}");
    assert!(error.contains("cannot unwind"), "{error}");
}

#[cfg(target_os = "linux")]
#[test]
fn imported_system_function_is_mocked_without_a_wrapper() {
    let _serial = serial();
    let mut session = Session::new().unwrap();
    let hostname = mock!(
        session,
        libc::gethostname,
        unsafe extern "C" fn(*mut libc::c_char, usize) -> libc::c_int
    )
    .unwrap();
    hostname
        .expect()
        .with(|_, size| *size == 64)
        .once()
        .returning(|buffer, size| {
            let name = b"test-host\0";
            assert!(size >= name.len());
            // SAFETY: the caller provides a writable buffer of this size.
            unsafe { std::ptr::copy_nonoverlapping(name.as_ptr().cast(), buffer, name.len()) };
            0
        })
        .unwrap();
    let mut buffer = [0u8; 64];
    // SAFETY: buffer is writable for all 64 bytes.
    let result = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    assert_eq!(result, 0);
    assert_eq!(&buffer[..10], b"test-host\0");
    session.restore().unwrap();
}

#[cfg(target_os = "windows")]
#[test]
fn imported_system_function_is_mocked_without_a_wrapper() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetComputerNameW(buffer: *mut u16, size: *mut u32) -> i32;
    }
    let _serial = serial();
    let mut session = Session::new().unwrap();
    let hostname = mock!(
        session,
        GetComputerNameW,
        unsafe extern "system" fn(*mut u16, *mut u32) -> i32
    )
    .unwrap();
    hostname
        .expect()
        .once()
        .returning(|buffer, size| {
            let name: Vec<_> = "test-host\0".encode_utf16().collect();
            // SAFETY: the caller supplies a valid size and a writable buffer.
            unsafe {
                assert!(*size as usize >= name.len());
                std::ptr::copy_nonoverlapping(name.as_ptr(), buffer, name.len());
                *size = (name.len() - 1) as u32;
            }
            1
        })
        .unwrap();
    let mut buffer = [0u16; 64];
    let mut size = buffer.len() as u32;
    // SAFETY: both pointers are valid; size describes the writable buffer.
    let result = unsafe { GetComputerNameW(buffer.as_mut_ptr(), &mut size) };
    assert_eq!(result, 1);
    assert_eq!(
        String::from_utf16(&buffer[..size as usize]).unwrap(),
        "test-host"
    );
    session.restore().unwrap();
}
