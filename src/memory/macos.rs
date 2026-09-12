use super::{PageRange, Region, syscall};
use crate::Error;

#[cfg(target_arch = "aarch64")]
mod remap;
#[cfg(target_arch = "aarch64")]
pub(super) use remap::replace_pages;

const SUCCESS: i32 = 0;
const INVALID_ADDRESS: i32 = 1;
const FAILURE: i32 = 5;
#[cfg(target_arch = "x86_64")]
const COPY: i32 = 0x10;

#[repr(C, packed(4))]
#[derive(Default)]
struct RegionInfo {
    protection: i32,
    max_protection: i32,
    inheritance: u32,
    offset: u64,
    user_tag: u32,
    pages_resident: u32,
    pages_shared: u32,
    pages_swapped: u32,
    pages_dirtied: u32,
    references: u32,
    shadow_depth: u16,
    external_pager: u8,
    share_mode: u8,
    is_submap: u32,
    behavior: i32,
    object_id: u32,
    wired: u16,
    flags: u16,
}

unsafe extern "C" {
    static mach_task_self_: u32;
    fn mach_vm_region_recurse(
        task: u32,
        address: *mut u64,
        size: *mut u64,
        depth: *mut u32,
        info: *mut i32,
        count: *mut u32,
    ) -> i32;
    fn mach_vm_read_overwrite(
        task: u32,
        address: u64,
        size: u64,
        output: u64,
        copied: *mut u64,
    ) -> i32;
    #[cfg(target_arch = "x86_64")]
    fn mach_vm_protect(task: u32, address: u64, size: u64, maximum: u32, protection: i32) -> i32;
}

pub(super) fn region_at(address: usize) -> Result<Option<Region>, Error> {
    let mut depth = 0u32;
    loop {
        let mut start = address as u64;
        let mut size = 0;
        let mut info = RegionInfo::default();
        let mut count = (size_of::<RegionInfo>() / size_of::<i32>()) as u32;
        // SAFETY: outputs match VM_REGION_SUBMAP_INFO_V0_COUNT_64 (16 words).
        let result = syscall("query memory", FAILURE, || unsafe {
            mach_vm_region_recurse(
                mach_task_self_,
                &mut start,
                &mut size,
                &mut depth,
                (&raw mut info).cast(),
                &mut count,
            )
        });
        if result == INVALID_ADDRESS {
            return Ok(None);
        }
        check("query memory", result)?;
        if info.is_submap != 0 {
            depth = depth.checked_add(1).ok_or(Error::InvalidRange)?;
            continue;
        }
        let end = (start as usize)
            .checked_add(size as usize)
            .ok_or(Error::InvalidRange)?;
        if start as usize > address || address >= end {
            return Ok(None);
        }
        return Ok(Some(Region {
            start: start as usize,
            end,
            protection: info.protection as u32,
            readable: info.protection & libc::PROT_READ != 0,
            executable: info.protection & libc::PROT_EXEC != 0,
        }));
    }
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

#[cfg(target_arch = "x86_64")]
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
        crate::cache::flush(_address, _length);
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
