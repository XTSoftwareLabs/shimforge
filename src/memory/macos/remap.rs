use super::{FAILURE, PageRange, check, flush, mach_task_self_, read_bytes, syscall};
use crate::Error;

unsafe extern "C" {
    fn mach_vm_remap(
        task: u32,
        target: *mut u64,
        size: u64,
        mask: u64,
        flags: i32,
        source_task: u32,
        source: u64,
        copy: u32,
        current: *mut i32,
        maximum: *mut i32,
        inheritance: u32,
    ) -> i32;
}

struct Image {
    address: usize,
    length: usize,
}

impl Image {
    fn new(bytes: &[u8], protection: u32) -> Result<Self, Error> {
        // SAFETY: a non-fixed anonymous mapping cannot replace existing memory.
        let pointer = syscall("allocate patch", libc::MAP_FAILED, || unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                bytes.len(),
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        });
        if pointer == libc::MAP_FAILED {
            return Err(os_error("allocate patch"));
        }
        let image = Self {
            address: pointer as usize,
            length: bytes.len(),
        };
        // SAFETY: this new writable allocation holds exactly bytes.len() bytes.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast(), bytes.len()) };
        // SAFETY: only this allocation changes; it is never writable and executable.
        let result = syscall("seal patch", -1, || unsafe {
            libc::mprotect(pointer, bytes.len(), protection as i32)
        });
        if result != 0 {
            return Err(os_error("seal patch"));
        }
        crate::cache::flush(image.address, image.length);
        Ok(image)
    }

    fn install(&self, address: usize) -> Result<(), Error> {
        let mut target = address as u64;
        let mut current = 0;
        let mut maximum = 0;
        // SAFETY: the caller owns this page change. The source stays mapped and RX.
        let result = syscall("protect memory", FAILURE, || unsafe {
            mach_vm_remap(
                mach_task_self_,
                &mut target,
                self.length as u64,
                0,
                0x4000,
                mach_task_self_,
                self.address as u64,
                0,
                &mut current,
                &mut maximum,
                1,
            )
        });
        check("protect memory", result)
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        // SAFETY: no references to this owned mapping remain.
        let result = unsafe { libc::munmap(self.address as *mut _, self.length) };
        if result != 0 {
            std::process::abort();
        }
    }
}

pub(crate) fn replace_pages(
    address: usize,
    replacement: &[u8],
    pages: &[PageRange],
    fatal: fn() -> !,
) -> Result<(), Error> {
    let end = address + replacement.len();
    let mut images = Vec::new();
    for page in pages {
        let mut bytes = read_bytes(page.address, page.length)?;
        if bytes.len() != page.length {
            return Err(Error::InvalidRange);
        }
        let original = Image::new(&bytes, page.protection)?;
        let start = address.max(page.address);
        let limit = end.min(page.address + page.length);
        bytes[start - page.address..limit - page.address]
            .copy_from_slice(&replacement[start - address..limit - address]);
        let updated = Image::new(&bytes, page.protection)?;
        images.push((page.address, original, updated));
    }
    for (index, (page, _, updated)) in images.iter().enumerate() {
        let result = updated
            .install(*page)
            .and_then(|()| flush(*page, updated.length));
        if let Err(error) = result {
            for (page, original, _) in images[..=index].iter().rev() {
                if original
                    .install(*page)
                    .and_then(|()| flush(*page, original.length))
                    .is_err()
                {
                    fatal();
                }
            }
            return Err(error);
        }
    }
    Ok(())
}

fn os_error(operation: &'static str) -> Error {
    Error::Os {
        operation,
        code: std::io::Error::last_os_error().raw_os_error().unwrap_or(-1),
    }
}

#[cfg(test)]
mod tests;
