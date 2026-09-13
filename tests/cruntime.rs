//! C runtime entry points are mocked like any other native function.

use shimforge::{Session, mock};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_long};
use std::ptr;
use std::sync::{Mutex, MutexGuard};

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

unsafe extern "C" {
    fn getenv(name: *const c_char) -> *mut c_char;
    fn strtol(text: *const c_char, end: *mut *mut c_char, base: c_int) -> c_long;
}

/// Business code that reads one setting straight from the process environment.
fn deployment_slot() -> Option<String> {
    let key = CString::new("DEPLOY_SLOT").unwrap();
    // SAFETY: the key is a valid C string and the result is copied before use.
    let value = unsafe { getenv(key.as_ptr()) };
    if value.is_null() {
        return None;
    }
    // SAFETY: a non-null result points at a C string owned by the environment.
    Some(
        unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned(),
    )
}

/// Business code that parses a prefix and reports how much it consumed.
fn leading_number(text: &CStr) -> (c_long, usize) {
    let mut end: *mut c_char = ptr::null_mut();
    // SAFETY: both pointers are valid and the base is in range.
    let value = unsafe { strtol(text.as_ptr(), &mut end, 10) };
    // SAFETY: the callee reports a position inside the same string.
    let consumed = unsafe { end.offset_from(text.as_ptr()) };
    (value, consumed as usize)
}

#[test]
fn an_environment_lookup_returns_a_chosen_string() {
    let _serial = serial();
    let mut session = Session::new();
    let lookup = mock!(
        session,
        getenv,
        unsafe extern "C" fn(*const c_char) -> *mut c_char
    );
    lookup.expect().returning(|_| c"canary".as_ptr().cast_mut());

    assert_eq!(deployment_slot().as_deref(), Some("canary"));
}

#[test]
fn an_environment_lookup_can_match_the_requested_key() {
    let _serial = serial();
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
        .returning(|_| c"blue".as_ptr().cast_mut());
    // Every other key keeps reporting an unset variable.
    lookup.expect().returning(|_| ptr::null_mut());

    assert_eq!(deployment_slot().as_deref(), Some("blue"));
    let other = CString::new("PATH").unwrap();
    // SAFETY: the key is a valid C string.
    assert!(unsafe { getenv(other.as_ptr()) }.is_null());
    session.restore();
    assert_eq!(deployment_slot(), None);
}

#[test]
fn a_parser_writes_through_its_output_pointer_and_returns_a_value() {
    let _serial = serial();
    let mut session = Session::new();
    let parse = mock!(
        session,
        strtol,
        unsafe extern "C" fn(*const c_char, *mut *mut c_char, c_int) -> c_long
    );
    parse
        .expect()
        .with(|_, end, base| !end.is_null() && *base == 10)
        .once()
        .returning(|text, end, _| {
            // SAFETY: the caller supplies a writable slot and a long enough string.
            unsafe { *end = text.cast_mut().add(4) };
            815
        });

    assert_eq!(leading_number(c"1234 units"), (815, 4));
    session.restore();
    assert_eq!(leading_number(c"1234 units"), (1234, 4));
}
