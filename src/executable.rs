//! Owned pages for saved function entries.

use crate::Error;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use windows as platform;

const CAPACITY: usize = 16384;
#[cfg(target_arch = "x86_64")]
const REACH: usize = 1 << 30;
#[cfg(target_arch = "aarch64")]
const REACH: usize = (1 << 27) - CAPACITY;

pub(crate) fn check_call_bridge() -> Result<(), Error> {
    check_shadow_stack(platform::shadow_stack()?)
}

fn check_shadow_stack(enabled: bool) -> Result<(), Error> {
    if enabled {
        Err(Error::ShadowStack)
    } else {
        Ok(())
    }
}

pub(crate) struct Executable {
    address: usize,
    sealed: bool,
}

impl Executable {
    pub(crate) fn near(source: usize) -> Result<Self, Error> {
        if source == 0 || source > isize::MAX as usize - CAPACITY {
            return Err(Error::InvalidAddress);
        }
        Ok(Self {
            address: platform::allocate(source)?,
            sealed: false,
        })
    }

    pub(crate) fn address(&self) -> usize {
        self.address
    }

    pub(crate) fn publish(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if self.sealed || bytes.is_empty() || bytes.len() > CAPACITY {
            return Err(Error::InvalidRange);
        }
        // SAFETY: This page is owned, writable, and large enough for the input.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.address as *mut u8, bytes.len())
        };
        // A failed flush may leave the page read-only. Never write it again.
        self.sealed = true;
        platform::seal(self.address)
    }
}

impl Drop for Executable {
    fn drop(&mut self) {
        crate::finish(platform::release(self.address), std::process::abort);
    }
}

fn os_error(operation: &'static str) -> Error {
    Error::Os {
        operation,
        code: std::io::Error::last_os_error().raw_os_error().unwrap_or(-1),
    }
}

fn syscall<T>(_operation: &'static str, _failure: T, call: impl FnOnce() -> T) -> T {
    #[cfg(test)]
    if tests::should_fail(_operation) {
        return _failure;
    }
    call()
}

#[cfg(test)]
mod tests;
