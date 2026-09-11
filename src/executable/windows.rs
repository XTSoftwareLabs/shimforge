use super::{CAPACITY, REACH, os_error, syscall};
use crate::Error;
use std::ffi::c_void;

const GRANULARITY: usize = 65536;

#[repr(C)]
#[derive(Default)]
struct MemoryInfo {
    base: *mut c_void,
    allocation_base: *mut c_void,
    allocation_protection: u32,
    partition: u16,
    size: usize,
    state: u32,
    protection: u32,
    kind: u32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn VirtualQuery(address: *const c_void, info: *mut MemoryInfo, size: usize) -> usize;
    fn VirtualAlloc(address: *const c_void, size: usize, kind: u32, protection: u32)
    -> *mut c_void;
    fn VirtualFree(address: *mut c_void, size: usize, kind: u32) -> i32;
    fn VirtualProtect(address: *const c_void, size: usize, protection: u32, old: *mut u32) -> i32;
    fn FlushInstructionCache(process: *mut c_void, address: *const c_void, size: usize) -> i32;
    fn GetCurrentProcess() -> *mut c_void;
    fn GetProcessMitigationPolicy(
        process: *mut c_void,
        policy: u32,
        buffer: *mut u32,
        size: usize,
    ) -> i32;
}

pub(super) fn shadow_stack() -> Result<bool, Error> {
    let mut flags = 0u32;
    // SAFETY: the current-process handle and four-byte policy buffer are valid.
    let result = syscall("query shadow stack", 0, || unsafe {
        GetProcessMitigationPolicy(GetCurrentProcess(), 15, &mut flags, 4)
    });
    if result == 0 {
        let error = os_error("query shadow stack");
        if matches!(error, Error::Os { code: 87, .. }) {
            return Ok(false);
        }
        return Err(error);
    }
    Ok(flags & 1 != 0)
}

pub(super) fn allocate(source: usize) -> Result<usize, Error> {
    search(source, &mut region, &mut reserve)
}

fn search(
    source: usize,
    region: &mut dyn FnMut(usize) -> Result<(usize, bool), Error>,
    reserve: &mut dyn FnMut(usize) -> Option<usize>,
) -> Result<usize, Error> {
    let lower = source.saturating_sub(REACH).max(GRANULARITY);
    let upper = source.saturating_add(REACH).min(isize::MAX as usize);
    let mut cursor = lower.div_ceil(GRANULARITY) * GRANULARITY;
    while cursor <= upper.saturating_sub(CAPACITY) {
        let (end, free) = region(cursor)?;
        if end <= cursor {
            return Err(Error::InvalidRange);
        }
        if free && end - cursor >= CAPACITY {
            if let Some(address) = reserve(cursor) {
                return Ok(address);
            }
            // Another thread may have reserved part of this region.
            cursor += GRANULARITY;
        } else {
            cursor = end.div_ceil(GRANULARITY) * GRANULARITY;
        }
    }
    Err(Error::InsufficientSpace)
}

fn region(address: usize) -> Result<(usize, bool), Error> {
    let mut info = MemoryInfo::default();
    // SAFETY: The OS checks the address and fills the correctly sized output.
    let result = syscall("query trampoline memory", 0, || unsafe {
        VirtualQuery(
            address as *const _,
            &mut info,
            std::mem::size_of::<MemoryInfo>(),
        )
    });
    if result == 0 {
        return Err(os_error("query trampoline memory"));
    }
    Ok((
        (info.base as usize).saturating_add(info.size),
        info.state == 0x10000,
    ))
}

fn reserve(address: usize) -> Option<usize> {
    // SAFETY: VirtualAlloc reserves only free pages at an aligned address.
    let page = syscall("allocate trampoline", std::ptr::null_mut(), || unsafe {
        VirtualAlloc(address as *const _, CAPACITY, 0x3000, 0x04)
    });
    (!page.is_null()).then_some(page as usize)
}

pub(super) fn seal(address: usize) -> Result<(), Error> {
    let mut old = 0;
    // SAFETY: This changes only the owned page; the OS checks its address.
    let result = syscall("protect trampoline", 0, || unsafe {
        VirtualProtect(address as *const _, CAPACITY, 0x20, &mut old)
    });
    if result == 0 {
        return Err(os_error("protect trampoline"));
    }
    // SAFETY: The page is owned and GetCurrentProcess returns a valid handle.
    let result = syscall("flush trampoline", 0, || unsafe {
        FlushInstructionCache(GetCurrentProcess(), address as *const _, CAPACITY)
    });
    if result == 0 {
        return Err(os_error("flush trampoline"));
    }
    Ok(())
}

pub(super) fn release(address: usize) -> Result<(), Error> {
    // SAFETY: This address is the base of the owned allocation.
    let result = syscall("free trampoline", 0, || unsafe {
        VirtualFree(address as *mut _, 0, 0x8000)
    });
    if result == 0 {
        return Err(os_error("free trampoline"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
