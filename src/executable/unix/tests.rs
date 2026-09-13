use super::*;
use crate::executable::tests::fail_next;

#[test]
fn search_rejects_far_pages_and_stops_at_its_limit() {
    let mut freed = Vec::new();
    let mut attempts = 0;
    let source = REACH * 4;
    assert_eq!(
        search(
            source,
            &mut |_| {
                attempts += 1;
                Some(if attempts == 1 { CAPACITY } else { source })
            },
            &mut |address| {
                freed.push(address);
                Ok(())
            },
        ),
        Ok(source)
    );
    assert_eq!(freed, [CAPACITY]);
    assert_eq!(
        search(source, &mut |_| Some(CAPACITY), &mut |_| Err(
            Error::InvalidAddress
        )),
        Err(Error::InvalidAddress)
    );
    assert_eq!(
        search(1, &mut |_| None, &mut |_| Ok(())),
        Err(Error::InsufficientSpace)
    );
    assert_eq!(
        search(isize::MAX as usize, &mut |_| None, &mut |_| Ok(())),
        Err(Error::InsufficientSpace)
    );
}

#[test]
fn native_allocation_failure_is_reported() {
    fail_next("allocate trampoline");
    assert!(reserve(CAPACITY).is_none());
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn distant_pages_are_used_only_when_no_near_page_exists() {
    assert_eq!(
        fallback(Ok(CAPACITY), &mut |_| unreachable!()),
        Ok(CAPACITY)
    );
    assert_eq!(
        fallback(Err(Error::InvalidAddress), &mut |_| unreachable!()),
        Err(Error::InvalidAddress)
    );
    let mut hints = Vec::new();
    assert_eq!(
        fallback(Err(Error::InsufficientSpace), &mut |hint| {
            hints.push(hint);
            Some(REACH * 64)
        }),
        Ok(REACH * 64)
    );
    assert_eq!(hints, [0]);
    assert_eq!(
        fallback(Err(Error::InsufficientSpace), &mut |_| None),
        Err(Error::InsufficientSpace)
    );
}
