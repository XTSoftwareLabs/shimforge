use super::{PageRange, Region, os_error, syscall};
use crate::Error;
use windows_sys::Win32::System::{
    Diagnostics::Debug::{FlushInstructionCache, ReadProcessMemory},
    Memory::{
        MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE,
        PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_READONLY, PAGE_READWRITE, PAGE_WRITECOPY,
        VirtualProtect, VirtualQuery,
    },
    SystemInformation::{GetSystemInfo, SYSTEM_INFO},
    Threading::GetCurrentProcess,
};

pub(super) fn region_at(address: usize) -> Result<Option<Region>, Error> {
    let mut information = MEMORY_BASIC_INFORMATION::default();
    // SAFETY: `information` points to a correctly sized, writable output object;
    // VirtualQuery inspects an address without dereferencing it in user code.
    let queried = syscall("query memory", 0, || unsafe {
        VirtualQuery(
            address as *const _,
            &mut information,
            std::mem::size_of_val(&information),
        )
    });
    if queried == 0 {
        return Err(os_error("query memory"));
    }
    let start = information.BaseAddress as usize;
    let end = start
        .checked_add(information.RegionSize)
        .ok_or(Error::InvalidRange)?;
    let accessible = information.State == MEM_COMMIT && information.Protect & PAGE_GUARD == 0;
    let protection = information.Protect & 0xff;
    Ok(Some(Region {
        start,
        end,
        protection: information.Protect,
        readable: accessible
            && matches!(
                protection,
                PAGE_READONLY
                    | PAGE_READWRITE
                    | PAGE_WRITECOPY
                    | PAGE_EXECUTE_READ
                    | PAGE_EXECUTE_READWRITE
                    | PAGE_EXECUTE_WRITECOPY
            ),
        executable: accessible
            && matches!(
                protection,
                PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY
            ),
    }))
}

pub(super) fn read_bytes(address: usize, length: usize) -> Result<Vec<u8>, Error> {
    let mut bytes = vec![0; length];
    let mut copied = 0;
    // SAFETY: The destination is an owned buffer of `length` bytes. The OS
    // validates source mappings, including concurrent unmapping, without a fault.
    let result = syscall("read memory", 0, || unsafe {
        ReadProcessMemory(
            GetCurrentProcess(),
            address as *const _,
            bytes.as_mut_ptr().cast(),
            length,
            &mut copied,
        )
    });
    if result == 0 {
        return Err(os_error("read memory"));
    }
    bytes.truncate(copied);
    Ok(bytes)
}

pub(super) fn page_size() -> Result<usize, Error> {
    let mut information = SYSTEM_INFO::default();
    // SAFETY: The supplied object is a correctly sized writable SYSTEM_INFO.
    unsafe { GetSystemInfo(&mut information) };
    Ok(information.dwPageSize as usize)
}

pub(super) fn protect(page: PageRange, writable: bool) -> Result<(), Error> {
    let protection = if writable {
        (page.protection & !0xff) | PAGE_EXECUTE_READWRITE
    } else {
        page.protection
    };
    let mut original = 0;
    // SAFETY: The caller holds each page range alive and serializes permission
    // changes; `original` is valid writable storage for the previous protection.
    let result = syscall("protect memory", 0, || unsafe {
        VirtualProtect(
            page.address as *const _,
            page.length,
            protection,
            &mut original,
        )
    });
    if result == 0 {
        return Err(os_error("protect memory"));
    }
    Ok(())
}

pub(super) fn flush(address: usize, length: usize) -> Result<(), Error> {
    // SAFETY: The process pseudo-handle is always valid; the caller guarantees
    // the target mapping remains alive for the requested range.
    let result = syscall("flush instruction cache", 0, || unsafe {
        FlushInstructionCache(GetCurrentProcess(), address as *const _, length)
    });
    if result == 0 {
        return Err(os_error("flush instruction cache"));
    }
    Ok(())
}
