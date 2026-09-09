//! Code reads and writes with rollback on failure.

use crate::Error;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use windows as platform;

#[derive(Clone, Copy, Debug)]
struct Region {
    start: usize,
    end: usize,
    protection: u32,
    readable: bool,
    executable: bool,
}

#[derive(Clone, Copy, Debug)]
struct PageRange {
    address: usize,
    length: usize,
    protection: u32,
}

fn checked_end(address: usize, length: usize) -> Result<usize, Error> {
    if address == 0 {
        return Err(Error::InvalidAddress);
    }
    if length == 0 {
        return Err(Error::InvalidRange);
    }
    address.checked_add(length).ok_or(Error::InvalidRange)
}

fn executable_regions(address: usize, length: usize) -> Result<Vec<Region>, Error> {
    let end = checked_end(address, length)?;
    let mut regions = Vec::new();
    let mut cursor = address;
    while cursor < end {
        let region = platform::region_at(cursor)?;
        let error = match region {
            Some(region) if region.readable && region.executable => {
                cursor = region.end.min(end);
                regions.push(region);
                continue;
            }
            Some(region) if region.readable => Error::NotExecutable,
            _ => Error::InvalidAddress,
        };
        if regions.is_empty() {
            return Err(error);
        }
        break;
    }
    Ok(regions)
}

pub(crate) fn read(address: usize, limit: usize) -> Result<Vec<u8>, Error> {
    let regions = executable_regions(address, limit)?;
    let available = (regions.last().unwrap().end - address).min(limit);
    platform::read_bytes(address, available)
}

/// Keep the memory mapped. No other code may run or change these bytes or their
/// page permissions during the write. Input slices must not borrow the target bytes.
pub(crate) unsafe fn write(
    address: usize,
    expected: &[u8],
    replacement: &[u8],
) -> Result<(), Error> {
    // SAFETY: The caller meets the mapping and access rules above.
    unsafe { replace(address, expected, replacement, std::process::abort) }
}

unsafe fn replace(
    address: usize,
    expected: &[u8],
    replacement: &[u8],
    fatal: fn() -> !,
) -> Result<(), Error> {
    if expected.len() != replacement.len() {
        return Err(Error::InvalidRange);
    }
    let end = checked_end(address, expected.len())?;
    let regions = executable_regions(address, expected.len())?;
    if regions.last().unwrap().end < end {
        return Err(Error::InvalidRange);
    }
    let original = platform::read_bytes(address, expected.len())?;
    if original != expected {
        return Err(Error::MemoryChanged);
    }

    let page_size = platform::page_size()?;
    let last_page_end =
        end.checked_add(page_size - 1).ok_or(Error::InvalidRange)? / page_size * page_size;
    let pages: Vec<_> = regions
        .iter()
        .map(|region| {
            let start = region.start.max(address / page_size * page_size);
            let end = region.end.min(last_page_end);
            PageRange {
                address: start,
                length: end - start,
                protection: region.protection,
            }
        })
        .collect();

    for (index, page) in pages.iter().enumerate() {
        if let Err(error) = platform::protect(*page, true) {
            restore_permissions(&pages[..index], fatal);
            return Err(error);
        }
    }

    // SAFETY: The pages are writable and only this call can change them.
    unsafe { std::ptr::copy(replacement.as_ptr(), address as *mut u8, replacement.len()) };
    let result = platform::flush(address, replacement.len()).and_then(|()| {
        pages
            .iter()
            .try_for_each(|page| platform::protect(*page, false))
    });
    if let Err(error) = result {
        for page in &pages {
            if platform::protect(*page, true).is_err() {
                fatal();
            }
        }
        // SAFETY: The pages are writable again. The saved bytes are in a separate buffer.
        unsafe {
            std::ptr::copy_nonoverlapping(original.as_ptr(), address as *mut u8, original.len())
        };
        if platform::flush(address, original.len()).is_err() {
            fatal();
        }
        restore_permissions(&pages, fatal);
        return Err(error);
    }
    Ok(())
}

fn restore_permissions(pages: &[PageRange], fatal: fn() -> !) {
    for page in pages {
        if platform::protect(*page, false).is_err() {
            fatal();
        }
    }
}

fn syscall<T>(_operation: &'static str, _failure: T, call: impl FnOnce() -> T) -> T {
    #[cfg(test)]
    if tests::should_fail(_operation) {
        return _failure;
    }
    call()
}

fn os_error(operation: &'static str) -> Error {
    Error::Os {
        operation,
        code: std::io::Error::last_os_error().raw_os_error().unwrap_or(-1),
    }
}

#[cfg(test)]
mod tests;
