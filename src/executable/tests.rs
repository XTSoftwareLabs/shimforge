use super::*;
use std::cell::RefCell;

thread_local! {
    static FAILURE: RefCell<Option<&'static str>> = const { RefCell::new(None) };
}

pub(super) fn should_fail(operation: &'static str) -> bool {
    FAILURE.with_borrow_mut(|failure| {
        if *failure == Some(operation) {
            *failure = None;
            true
        } else {
            false
        }
    })
}

pub(super) fn fail_next(operation: &'static str) {
    FAILURE.with_borrow_mut(|failure| *failure = Some(operation));
}

#[test]
fn page_is_near_owned_and_executable() {
    let source = Executable::near as *const () as usize;
    let mut page = Executable::near(source).unwrap();
    assert!(page.address().abs_diff(source) <= REACH);
    assert_eq!(page.publish(&[]), Err(Error::InvalidRange));
    assert_eq!(page.publish(&[0; CAPACITY + 1]), Err(Error::InvalidRange));
    page.publish(&[0xf3, 0x0f, 0x1e, 0xfa, 0xb8, 42, 0, 0, 0, 0xc3])
        .unwrap();
    // SAFETY: These bytes form a complete x86-64 fn() -> u32 in an RX page.
    let call: fn() -> u32 = unsafe { std::mem::transmute(page.address()) };
    assert_eq!(call(), 42);
    assert_eq!(page.publish(&[0xc3]), Err(Error::InvalidRange));
    let bytes = crate::memory::read(page.address(), 10).unwrap();
    assert_eq!(bytes, [0xf3, 0x0f, 0x1e, 0xfa, 0xb8, 42, 0, 0, 0, 0xc3]);
}

#[test]
fn bad_source_and_publish_failures_are_reported() {
    assert!(matches!(Executable::near(0), Err(Error::InvalidAddress)));
    assert!(matches!(
        Executable::near(usize::MAX),
        Err(Error::InvalidAddress)
    ));
    let mut page = Executable::near(Executable::near as *const () as usize).unwrap();
    fail_next("protect trampoline");
    assert!(matches!(page.publish(&[0xc3]), Err(Error::Os { .. })));
    assert_eq!(page.publish(&[0xc3]), Err(Error::InvalidRange));
    fail_next("free trampoline");
    assert!(matches!(
        platform::release(page.address()),
        Err(Error::Os { .. })
    ));
    // The failed call did not free the page. Drop retries without a fault.
}

#[test]
fn shadow_stack_policy_is_checked_without_changing_it() {
    assert_eq!(check_shadow_stack(false), Ok(()));
    assert_eq!(check_shadow_stack(true), Err(Error::ShadowStack));
    check_call_bridge().unwrap();
}

#[cfg(target_os = "windows")]
#[test]
fn cache_flush_failure_seals_the_page() {
    let mut page = Executable::near(Executable::near as *const () as usize).unwrap();
    fail_next("flush trampoline");
    assert!(matches!(page.publish(&[0xc3]), Err(Error::Os { .. })));
    assert_eq!(page.publish(&[0xc3]), Err(Error::InvalidRange));
}
