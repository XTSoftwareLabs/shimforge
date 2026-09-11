use super::{os_error, syscall};
use crate::Error;

pub(super) use super::unix::{allocate, release, seal};

pub(super) fn shadow_stack() -> Result<bool, Error> {
    query_shadow_stack(&mut |flags| {
        // SAFETY: ARCH_SHSTK_STATUS writes one unsigned long to this valid pointer.
        syscall("query shadow stack", -1, || unsafe {
            libc::syscall(libc::SYS_arch_prctl, 0x5005, flags as *mut usize)
        })
    })
}

fn query_shadow_stack(query: &mut dyn FnMut(&mut usize) -> libc::c_long) -> Result<bool, Error> {
    let mut flags = 0usize;
    let result = query(&mut flags);
    if result != 0 {
        let error = os_error("query shadow stack");
        if matches!(
            error,
            Error::Os {
                code: libc::EINVAL | libc::ENOSYS | libc::ENOTSUP,
                ..
            }
        ) {
            return Ok(false);
        }
        return Err(error);
    }
    Ok(flags & 1 != 0)
}

#[cfg(test)]
mod tests;
