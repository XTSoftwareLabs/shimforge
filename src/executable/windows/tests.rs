use super::*;
use crate::executable::tests::fail_next;

#[test]
fn search_skips_busy_regions_and_retries_races() {
    let mut attempts = 0;
    let found = search(
        1,
        &mut |address| Ok((address + GRANULARITY, address > GRANULARITY)),
        &mut |address| {
            attempts += 1;
            (attempts == 2).then_some(address)
        },
    )
    .unwrap();
    assert_eq!(found, 3 * GRANULARITY);
    assert_eq!(attempts, 2);
    assert_eq!(
        search(1, &mut |_| Ok((REACH + CAPACITY, false)), &mut |_| None),
        Err(Error::InsufficientSpace)
    );
    assert_eq!(
        search(1, &mut |address| Ok((address, true)), &mut |_| None),
        Err(Error::InvalidRange)
    );
    assert_eq!(
        search(1, &mut |_| Err(Error::InvalidAddress), &mut |_| None),
        Err(Error::InvalidAddress)
    );
}

#[test]
fn native_allocation_failures_are_reported() {
    fail_next("query trampoline memory");
    assert!(matches!(allocate(1), Err(Error::Os { .. })));
    fail_next("allocate trampoline");
    assert!(reserve(GRANULARITY).is_none());
    let source = allocate as *const () as usize;
    assert!(region(source).unwrap().0 > source);
}

#[test]
fn memory_layout_matches_windows_amd64() {
    assert_eq!(std::mem::size_of::<MemoryInfo>(), 48);
    assert_eq!(std::mem::offset_of!(MemoryInfo, size), 24);
    assert_eq!(std::mem::offset_of!(MemoryInfo, state), 32);
}
