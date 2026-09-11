use super::{PageRange, Region, syscall};
use crate::Error;

const SUCCESS: i32 = 0;
const INVALID_ADDRESS: i32 = 1;
const FAILURE: i32 = 5;
const COPY: i32 = 0x10;

#[repr(C, packed(4))]
#[derive(Default)]
struct RegionInfo {
    protection: i32,
    max_protection: i32,
    inheritance: u32,
    shared: u32,
    reserved: u32,
    offset: u64,
    behavior: i32,
    wired: u16,
}

unsafe extern "C" {
    static mach_task_self_: u32;
    fn mach_vm_region(
        task: u32,
        address: *mut u64,
        size: *mut u64,
        flavor: i32,
        info: *mut i32,
        count: *mut u32,
        object: *mut u32,
    ) -> i32;
    fn mach_vm_read_overwrite(
        task: u32,
        address: u64,
        size: u64,
        output: u64,
        copied: *mut u64,
    ) -> i32;
    fn mach_vm_protect(task: u32, address: u64, size: u64, maximum: u32, protection: i32) -> i32;
    fn mach_port_deallocate(task: u32, name: u32) -> i32;
}

pub(super) fn region_at(address: usize) -> Result<Option<Region>, Error> {
    let mut start = address as u64;
    let mut size = 0;
    let mut info = RegionInfo::default();
    let mut count = (size_of::<RegionInfo>() / size_of::<i32>()) as u32;
    let mut object = 0;
    // SAFETY: all output pointers are valid and match VM_REGION_BASIC_INFO_64.
    let result = syscall("query memory", FAILURE, || unsafe {
        mach_vm_region(
            mach_task_self_,
            &mut start,
            &mut size,
            9,
            (&raw mut info).cast(),
            &mut count,
            &mut object,
        )
    });
    // SAFETY: this releases the returned right; a null name owns no right.
    unsafe { mach_port_deallocate(mach_task_self_, object) };
    if result == INVALID_ADDRESS {
        return Ok(None);
    }
    check("query memory", result)?;
    let end = (start as usize)
        .checked_add(size as usize)
        .ok_or(Error::InvalidRange)?;
    if start as usize > address || address >= end {
        return Ok(None);
    }
    Ok(Some(Region {
        start: start as usize,
        end,
        protection: info.protection as u32,
        readable: info.protection & libc::PROT_READ != 0,
        executable: info.protection & libc::PROT_EXEC != 0,
    }))
}

pub(super) fn read_bytes(address: usize, length: usize) -> Result<Vec<u8>, Error> {
    let mut bytes = vec![0; length];
    let mut copied = 0;
    // SAFETY: the output buffer is writable; Mach checks the source mapping.
    let result = syscall("read memory", FAILURE, || unsafe {
        mach_vm_read_overwrite(
            mach_task_self_,
            address as u64,
            length as u64,
            bytes.as_mut_ptr() as u64,
            &mut copied,
        )
    });
    check("read memory", result)?;
    bytes.truncate(copied as usize);
    Ok(bytes)
}

pub(super) fn page_size() -> Result<usize, Error> {
    // SAFETY: sysconf reads a setting and takes no pointers.
    let size = syscall("page size", -1, || unsafe {
        libc::sysconf(libc::_SC_PAGESIZE)
    });
    usize::try_from(size).map_err(|_| Error::Os {
        operation: "page size",
        code: std::io::Error::last_os_error().raw_os_error().unwrap_or(-1),
    })
}

pub(super) fn protect(page: PageRange, writable: bool) -> Result<(), Error> {
    let protection = page.protection as i32 | if writable { libc::PROT_WRITE | COPY } else { 0 };
    // SAFETY: the caller keeps these pages mapped. COPY makes text changes private.
    let result = syscall("protect memory", FAILURE, || unsafe {
        mach_vm_protect(
            mach_task_self_,
            page.address as u64,
            page.length as u64,
            0,
            protection,
        )
    });
    check("protect memory", result)
}

pub(super) fn flush(_address: usize, _length: usize) -> Result<(), Error> {
    let result = syscall("flush instruction cache", FAILURE, || {
        // x86-64 keeps code and data caches in sync. Target calls are stopped.
        std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
        SUCCESS
    });
    check("flush instruction cache", result)
}

fn check(operation: &'static str, code: i32) -> Result<(), Error> {
    if code == SUCCESS {
        Ok(())
    } else {
        Err(Error::Os { operation, code })
    }
}

#[cfg(test)]
mod tests;
