#[cfg(target_arch = "x86_64")]
use super::{os_error, syscall};
#[cfg(target_arch = "x86_64")]
use crate::Error;

pub(super) use super::unix::{allocate, release, seal};

#[cfg(target_arch = "x86_64")]
pub(super) fn shadow_stack() -> Result<bool, Error> {
    query_shadow_stack(&mut |flags| {
        // SAFETY: ARCH_SHSTK_STATUS writes one unsigned long to this valid pointer.
        syscall("query shadow stack", -1, || unsafe {
            libc::syscall(libc::SYS_arch_prctl, 0x5005, flags as *mut usize)
        })
    })
}

#[cfg(target_arch = "x86_64")]
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

#[cfg(all(test, target_arch = "x86_64"))]
mod tests;
