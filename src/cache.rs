pub(crate) fn flush(address: usize, length: usize) {
    #[cfg(target_arch = "x86_64")]
    {
        let _ = (address, length);
        std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
    }
    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    {
        unsafe extern "C" {
            fn sys_icache_invalidate(address: *mut u8, length: usize);
        }
        // SAFETY: the caller keeps the whole range mapped during cache maintenance.
        unsafe { sys_icache_invalidate(address as *mut u8, length) };
    }
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    // SAFETY: the caller owns a mapped range. These instructions only sync caches.
    unsafe {
        let cache_type: usize;
        core::arch::asm!("mrs {value}, ctr_el0", value = out(reg) cache_type, options(nostack, preserves_flags));
        let end = address + length;
        let data_line = 4 << ((cache_type >> 16) & 15);
        let instruction_line = 4 << (cache_type & 15);
        for line in (address & !(data_line - 1)..end).step_by(data_line) {
            core::arch::asm!("dc cvau, {line}", line = in(reg) line, options(nostack, preserves_flags));
        }
        core::arch::asm!("dsb ish", options(nostack, preserves_flags));
        for line in (address & !(instruction_line - 1)..end).step_by(instruction_line) {
            core::arch::asm!("ic ivau, {line}", line = in(reg) line, options(nostack, preserves_flags));
        }
        core::arch::asm!("dsb ish", "isb", options(nostack, preserves_flags));
    }
}
