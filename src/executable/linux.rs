use super::{CAPACITY, REACH, os_error, syscall};
use crate::Error;

pub(super) fn allocate(source: usize) -> Result<usize, Error> {
    search(source, &mut reserve, &mut release)
}

fn search(
    source: usize,
    reserve: &mut dyn FnMut(usize) -> Option<usize>,
    release: &mut dyn FnMut(usize) -> Result<(), Error>,
) -> Result<usize, Error> {
    // Hints never replace an existing mapping, including on older kernels.
    for step in 0usize..=128 {
        let distance = step.div_ceil(2) * (REACH / 64);
        let hint = if step % 2 == 0 {
            source.saturating_add(distance)
        } else {
            source.saturating_sub(distance)
        };
        let hint = hint / CAPACITY * CAPACITY;
        if hint < CAPACITY || hint > isize::MAX as usize - CAPACITY {
            continue;
        }
        if let Some(address) = reserve(hint) {
            if address.abs_diff(source) <= REACH - CAPACITY {
                return Ok(address);
            }
            release(address)?;
        }
    }
    Err(Error::InsufficientSpace)
}

fn reserve(hint: usize) -> Option<usize> {
    // SAFETY: A non-fixed hint cannot overwrite existing memory.
    let page = syscall("allocate trampoline", libc::MAP_FAILED, || unsafe {
        libc::mmap(
            hint as *mut _,
            CAPACITY,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    });
    (page != libc::MAP_FAILED).then_some(page as usize)
}

pub(super) fn seal(address: usize) -> Result<(), Error> {
    // SAFETY: This changes only the owned page; the OS checks its address.
    let result = syscall("protect trampoline", -1, || unsafe {
        libc::mprotect(
            address as *mut _,
            CAPACITY,
            libc::PROT_READ | libc::PROT_EXEC,
        )
    });
    if result != 0 {
        return Err(os_error("protect trampoline"));
    }
    // x86 keeps instruction and data caches coherent.
    std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

pub(super) fn release(address: usize) -> Result<(), Error> {
    // SAFETY: This address and length describe the owned allocation.
    let result = syscall("free trampoline", -1, || unsafe {
        libc::munmap(address as *mut _, CAPACITY)
    });
    if result != 0 {
        return Err(os_error("free trampoline"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
