#![allow(non_snake_case)]

use std::ffi::c_void;

pub(super) const MEM_COMMIT: u32 = 0x1000;
pub(super) const PAGE_READONLY: u32 = 0x02;
pub(super) const PAGE_READWRITE: u32 = 0x04;
pub(super) const PAGE_WRITECOPY: u32 = 0x08;
pub(super) const PAGE_EXECUTE_READ: u32 = 0x20;
pub(super) const PAGE_EXECUTE_READWRITE: u32 = 0x40;
pub(super) const PAGE_EXECUTE_WRITECOPY: u32 = 0x80;
pub(super) const PAGE_GUARD: u32 = 0x100;

#[repr(C)]
#[derive(Default)]
pub(super) struct MemoryInfo {
    pub BaseAddress: *mut c_void,
    pub AllocationBase: *mut c_void,
    pub AllocationProtect: u32,
    pub PartitionId: u16,
    pub RegionSize: usize,
    pub State: u32,
    pub Protect: u32,
    pub Type: u32,
}

#[repr(C)]
#[derive(Default)]
pub(super) struct SystemInfo {
    pub ProcessorArchitecture: u32,
    pub dwPageSize: u32,
    pub MinimumApplicationAddress: *mut c_void,
    pub MaximumApplicationAddress: *mut c_void,
    pub ActiveProcessorMask: usize,
    pub NumberOfProcessors: u32,
    pub ProcessorType: u32,
    pub AllocationGranularity: u32,
    pub ProcessorLevel: u16,
    pub ProcessorRevision: u16,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    pub(super) fn VirtualQuery(address: *const c_void, info: *mut MemoryInfo, size: usize)
    -> usize;
    pub(crate) fn VirtualProtect(
        address: *const c_void,
        size: usize,
        protection: u32,
        old: *mut u32,
    ) -> i32;
    pub(super) fn GetSystemInfo(info: *mut SystemInfo);
    pub(super) fn GetCurrentProcess() -> *mut c_void;
    pub(super) fn ReadProcessMemory(
        process: *mut c_void,
        source: *const c_void,
        target: *mut c_void,
        length: usize,
        copied: *mut usize,
    ) -> i32;
    pub(super) fn FlushInstructionCache(
        process: *mut c_void,
        address: *const c_void,
        length: usize,
    ) -> i32;

    #[cfg(test)]
    pub(crate) fn VirtualAlloc(
        address: *const c_void,
        size: usize,
        allocation: u32,
        protection: u32,
    ) -> *mut c_void;
    #[cfg(test)]
    pub(crate) fn VirtualFree(address: *mut c_void, size: usize, operation: u32) -> i32;
}
