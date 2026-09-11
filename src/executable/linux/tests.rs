use super::*;
use crate::executable::tests::fail_next;

#[test]
fn shadow_stack_query_errors_are_reported() {
    for enabled in [false, true] {
        assert_eq!(
            query_shadow_stack(&mut |flags| {
                *flags = usize::from(enabled);
                0
            }),
            Ok(enabled)
        );
    }
    for code in [libc::EINVAL, libc::ENOSYS, libc::ENOTSUP, libc::EPERM] {
        fail_next("query shadow stack");
        // SAFETY: errno is writable storage owned by this thread.
        unsafe { *libc::__errno_location() = code };
        let result = shadow_stack();
        if code == libc::EPERM {
            assert!(result.is_err());
        } else {
            assert_eq!(result, Ok(false));
        }
    }
}
