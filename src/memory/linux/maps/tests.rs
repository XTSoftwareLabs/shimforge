use super::*;

#[test]
fn retries_interrupted_reads_and_reports_other_errors() {
    let mut calls = 0;
    assert_eq!(
        retry(&mut || {
            calls += 1;
            // SAFETY: errno is writable storage for this thread.
            unsafe { *libc::__errno_location() = libc::EINTR };
            if calls == 1 { -1 } else { 2 }
        })
        .unwrap(),
        2
    );
    // SAFETY: errno is writable storage for this thread.
    unsafe { *libc::__errno_location() = libc::EIO };
    assert!(retry(&mut || -1).is_err());
    assert!(read().unwrap().contains("r-x"));
}
