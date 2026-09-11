use super::*;
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Default)]
struct Faults {
    failures: Vec<(&'static str, usize)>,
    calls: HashMap<&'static str, usize>,
}

thread_local! {
    static FAULTS: RefCell<Faults> = RefCell::new(Faults::default());
}

pub(super) fn should_fail(operation: &'static str) -> bool {
    FAULTS.with(|faults| {
        let mut faults = faults.borrow_mut();
        let occurrence = faults.calls.entry(operation).or_default();
        *occurrence += 1;
        let occurrence = *occurrence;
        faults.failures.contains(&(operation, occurrence))
    })
}

struct Inject;

#[cfg(target_os = "linux")]
#[test]
fn map_read_errors_close_the_owned_descriptor() {
    let descriptor_count = || std::fs::read_dir("/proc/self/fd").unwrap().count();
    let before = descriptor_count();
    for operation in ["open maps", "read maps"] {
        let _injection = Inject::new(&[(operation, 1)]);
        // SAFETY: errno is writable storage for this thread.
        unsafe { *libc::__errno_location() = libc::EIO };
        assert!(matches!(platform::region_at(1), Err(Error::Mapping(_))));
    }
    assert_eq!(descriptor_count(), before);
}

#[test]
fn local_installation_errors_can_be_retried_and_cleanup_does_not_write_code() {
    let _serial = crate::tests::serial();
    fn value(seed: u64) -> u64 {
        seed.wrapping_add(1)
    }
    let mut session = crate::Session::new_local().unwrap();
    for failed in [true, false] {
        let injection = failed.then(|| Inject::new(&[("protect memory", 1)]));
        let result = crate::mock!(session, value, fn(u64) -> u64);
        drop(injection);
        if failed {
            assert!(result.is_err());
            assert_eq!(value(1), 2);
        } else {
            result.unwrap().expect().once().returns(40).unwrap();
        }
    }
    assert_eq!(value(1), 40);
    {
        let _injection = Inject::new(&[("protect memory", 1)]);
        session.restore().unwrap();
    }
    session.restore().unwrap();
    assert_eq!(value(1), 2);
}

impl Inject {
    fn new(failures: &[(&'static str, usize)]) -> Self {
        FAULTS.with(|faults| {
            *faults.borrow_mut() = Faults {
                failures: failures.to_vec(),
                calls: HashMap::new(),
            };
        });
        Self
    }
}

impl Drop for Inject {
    fn drop(&mut self) {
        FAULTS.with(|faults| *faults.borrow_mut() = Faults::default());
    }
}

struct Allocation {
    address: usize,
    page_size: usize,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
const RX: u32 = (libc::PROT_READ | libc::PROT_EXEC) as u32;
#[cfg(any(target_os = "linux", target_os = "macos"))]
const RWX: u32 = (libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC) as u32;
#[cfg(any(target_os = "linux", target_os = "macos"))]
const RO: u32 = libc::PROT_READ as u32;
#[cfg(any(target_os = "linux", target_os = "macos"))]
const NONE: u32 = libc::PROT_NONE as u32;
#[cfg(target_os = "windows")]
const RX: u32 = 0x20;
#[cfg(target_os = "windows")]
const RWX: u32 = 0x40;
#[cfg(target_os = "windows")]
const RO: u32 = 0x02;
#[cfg(target_os = "windows")]
const NONE: u32 = 0x01;

impl Allocation {
    fn new() -> Self {
        let page_size = platform::page_size().unwrap();
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        // SAFETY: This test owns the new allocation.
        let address = unsafe {
            let pointer = libc::mmap(
                std::ptr::null_mut(),
                page_size * 3,
                RWX as i32,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            );
            assert_ne!(pointer, libc::MAP_FAILED);
            pointer as usize
        };
        #[cfg(target_os = "windows")]
        // SAFETY: This test owns the new allocation.
        let address = unsafe {
            let pointer =
                platform::VirtualAlloc(std::ptr::null(), page_size * 3, 0x1000 | 0x2000, RWX);
            assert!(!pointer.is_null());
            pointer as usize
        };
        // SAFETY: These three writable pages contain no Rust objects.
        unsafe { std::ptr::write_bytes(address as *mut u8, 0x90, page_size * 3) };
        let allocation = Self { address, page_size };
        allocation.protect(0, RX);
        allocation.protect(2, NONE);
        allocation
    }

    fn protect(&self, page: usize, protection: u32) {
        let address = self.address + page * self.page_size;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        // SAFETY: This test owns the whole aligned page.
        unsafe {
            assert_eq!(
                libc::mprotect(address as *mut _, self.page_size, protection as i32),
                0
            );
        }
        #[cfg(target_os = "windows")]
        // SAFETY: This test owns the aligned page; old is valid output storage.
        unsafe {
            let mut old = 0;
            assert_ne!(
                platform::VirtualProtect(address as *const _, self.page_size, protection, &mut old),
                0
            );
        }
    }

    fn hole(&self, page: usize) {
        let address = self.address + page * self.page_size;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        // SAFETY: Only this test owns the page.
        unsafe {
            assert_eq!(libc::munmap(address as *mut _, self.page_size), 0);
        }
        #[cfg(target_os = "windows")]
        // SAFETY: Only this test owns the page.
        unsafe {
            assert_ne!(
                platform::VirtualFree(address as *mut _, self.page_size, 0x4000),
                0
            );
        }
    }

    fn assert_unchanged(&self, address: usize, length: usize) {
        assert_eq!(read(address, length).unwrap(), vec![0x90; length]);
        assert_eq!(
            platform::region_at(self.address)
                .unwrap()
                .unwrap()
                .protection,
            RX
        );
        assert_eq!(
            platform::region_at(self.address + self.page_size)
                .unwrap()
                .unwrap()
                .protection,
            RWX
        );
    }
}

impl Drop for Allocation {
    fn drop(&mut self) {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        // SAFETY: This test owns the allocation. munmap accepts holes.
        unsafe {
            assert_eq!(libc::munmap(self.address as *mut _, self.page_size * 3), 0);
        }
        #[cfg(target_os = "windows")]
        // SAFETY: This test owns the allocation; no references remain.
        unsafe {
            assert_ne!(platform::VirtualFree(self.address as *mut _, 0, 0x8000), 0);
        }
    }
}

#[test]
fn reads_stop_at_permission_boundaries_and_reject_invalid_ranges() {
    let memory = Allocation::new();
    let last = memory.address + memory.page_size * 2 - 2;
    assert_eq!(read(last, 16).unwrap(), [0x90; 2]);
    assert!(matches!(read(0, 1), Err(Error::InvalidAddress)));
    assert!(matches!(read(1, 1), Err(Error::InvalidAddress)));
    assert!(matches!(read(memory.address, 0), Err(Error::InvalidRange)));
    assert!(matches!(read(usize::MAX, 2), Err(Error::InvalidRange)));
    assert!(matches!(
        read(memory.address + memory.page_size * 2, 1),
        Err(Error::InvalidAddress)
    ));
    memory.protect(0, RO);
    assert!(matches!(read(memory.address, 1), Err(Error::NotExecutable)));
}

#[test]
fn reads_and_writes_respect_unmapped_holes() {
    let memory = Allocation::new();
    memory.hole(1);
    let address = memory.address + memory.page_size - 2;
    assert_eq!(read(address, 4).unwrap(), [0x90; 2]);
    assert!(matches!(
        read(memory.address + memory.page_size, 1),
        Err(Error::InvalidAddress)
    ));
    assert!(matches!(
        // SAFETY: This test owns the remaining pages; no code runs in them.
        unsafe { write(address, &[0x90; 4], &[0xcc; 4]) },
        Err(Error::InvalidRange)
    ));
}

#[test]
fn replacing_across_pages_restores_each_original_protection() {
    let memory = Allocation::new();
    let address = memory.address + memory.page_size - 2;
    // SAFETY: Only this test uses these pages; no code runs in them.
    unsafe { write(address, &[0x90; 6], &[1, 2, 3, 4, 5, 6]) }.unwrap();
    assert_eq!(read(address, 6).unwrap(), [1, 2, 3, 4, 5, 6]);
    assert_eq!(
        platform::region_at(memory.address)
            .unwrap()
            .unwrap()
            .protection,
        RX
    );
    assert_eq!(
        platform::region_at(memory.address + memory.page_size)
            .unwrap()
            .unwrap()
            .protection,
        RWX
    );
    // SAFETY: The pages are still mapped and unused by other code.
    unsafe { write(address, &[1, 2, 3, 4, 5, 6], &[0x90; 6]) }.unwrap();
    memory.assert_unchanged(address, 6);
}

#[test]
fn failed_validation_never_changes_memory() {
    let memory = Allocation::new();
    // SAFETY: Only this test uses the pages. Invalid ranges are rejected before access.
    unsafe {
        assert!(matches!(
            write(memory.address, &[0x90], &[]),
            Err(Error::InvalidRange)
        ));
        assert!(matches!(
            write(memory.address, &[], &[]),
            Err(Error::InvalidRange)
        ));
        assert!(matches!(write(0, &[0], &[1]), Err(Error::InvalidAddress)));
        assert!(matches!(
            write(memory.address, &[0x91], &[0xcc]),
            Err(Error::MemoryChanged)
        ));
        assert!(matches!(
            write(
                memory.address + memory.page_size * 2 - 1,
                &[0x90; 2],
                &[0xcc; 2]
            ),
            Err(Error::InvalidRange)
        ));
    }
    memory.assert_unchanged(memory.address, 8);
}

#[test]
fn errors_before_modification_leave_bytes_and_permissions_unchanged() {
    let memory = Allocation::new();
    for operation in ["query memory", "read memory", "protect memory"] {
        let injection = Inject::new(&[(operation, 1)]);
        assert!(
            // SAFETY: This test owns the pages; the injected error occurs before writing.
            unsafe { write(memory.address, &[0x90; 4], &[0xcc; 4]) }.is_err(),
            "{operation}"
        );
        drop(injection);
        memory.assert_unchanged(memory.address, 4);
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let injection = Inject::new(&[("page size", 1)]);
        assert!(matches!(
            // SAFETY: This test owns the pages; the injected error occurs before writing.
            unsafe { write(memory.address, &[0x90], &[0xcc]) },
            Err(Error::Os {
                operation: "page size",
                ..
            })
        ));
        drop(injection);
        memory.assert_unchanged(memory.address, 4);
    }
}

#[test]
fn partial_permission_change_failure_rolls_back_changed_pages() {
    let memory = Allocation::new();
    let address = memory.address + memory.page_size - 2;
    let injection = Inject::new(&[("protect memory", 2)]);
    assert!(matches!(
        // SAFETY: This test owns both pages; no code runs in them.
        unsafe { write(address, &[0x90; 4], &[0xcc; 4]) },
        Err(Error::Os {
            operation: "protect memory",
            ..
        })
    ));
    drop(injection);
    memory.assert_unchanged(address, 4);
}

#[test]
fn failures_after_copy_roll_back_bytes_and_permissions() {
    let memory = Allocation::new();
    let address = memory.address + memory.page_size - 2;
    for failure in [
        ("flush instruction cache", 1),
        ("protect memory", 3),
        ("protect memory", 4),
    ] {
        let injection = Inject::new(&[failure]);
        assert!(
            // SAFETY: This test owns both pages; no code runs in them.
            unsafe { write(address, &[0x90; 4], &[0xcc; 4]) }.is_err(),
            "{failure:?}"
        );
        drop(injection);
        memory.assert_unchanged(address, 4);
    }
}

fn fatal_for_test() -> ! {
    panic!("irrecoverable memory rollback");
}

#[test]
fn installing_a_prefix_that_extends_into_an_active_patch_is_rejected() {
    let _serial = crate::tests::serial();
    let memory = Allocation::new();
    memory.protect(0, RWX);
    // SAFETY: This test owns the writable page; no code runs in it.
    unsafe {
        for offset in [64, 128, 256] {
            *((memory.address + offset) as *mut u8) = 0xc3;
        }
    }
    memory.protect(0, RX);
    platform::flush(memory.address, 257).unwrap();
    let mut session = crate::Session::new_global().unwrap();
    // SAFETY: Both NOP/RET functions use the same void ABI. This test owns their
    // memory and keeps calls stopped while patching.
    unsafe {
        session.replace_raw(
            (memory.address + 4) as *const (),
            (memory.address + 128) as *const (),
        )
    }
    .unwrap();
    let installed = read(memory.address, 32).unwrap();
    assert!(matches!(
        // SAFETY: Both entries stay mapped and idle. The overlap is rejected before writing.
        unsafe {
            session.replace_raw(
                memory.address as *const (),
                (memory.address + 256) as *const (),
            )
        },
        Err(Error::Overlap)
    ));
    assert_eq!(read(memory.address, 32).unwrap(), installed);
    session.restore().unwrap();
    memory.assert_unchanged(memory.address, 32);
}

extern "C" fn native_replacement() -> u32 {
    93
}

#[test]
fn a_generated_cet_function_can_execute_replace_and_restore() {
    let _serial = crate::tests::serial();
    let memory = Allocation::new();
    let mut code = [0x90; 26];
    code[..4].copy_from_slice(&[0xf3, 0x0f, 0x1e, 0xfa]);
    code[20..].copy_from_slice(&[0xb8, 7, 0, 0, 0, 0xc3]);
    // SAFETY: This test owns the page and keeps it mapped through the session.
    // No code runs there during this write.
    unsafe { write(memory.address, &[0x90; 26], &code) }.unwrap();
    // SAFETY: ENDBR64, NOPs, MOV EAX,7, RET form an extern C fn() -> u32.
    let source: extern "C" fn() -> u32 = unsafe { std::mem::transmute(memory.address) };
    assert_eq!(source(), 7);
    let mut session = crate::Session::new_global().unwrap();
    // SAFETY: Both functions match in ABI and lifetime. Only this thread can call
    // the generated function, and calls are stopped while patching.
    unsafe { session.replace_raw(source as *const (), native_replacement as *const ()) }.unwrap();
    assert_eq!(source(), 93);
    assert_eq!(read(memory.address, 4).unwrap(), [0xf3, 0x0f, 0x1e, 0xfa]);
    session.restore().unwrap();
    assert_eq!(source(), 7);
    assert_eq!(read(memory.address, 26).unwrap(), code);
}

#[test]
fn irrecoverable_rollbacks_invoke_the_fatal_policy() {
    for failures in [
        vec![("protect memory", 2), ("protect memory", 3)],
        vec![("flush instruction cache", 1), ("protect memory", 3)],
        vec![
            ("flush instruction cache", 1),
            ("flush instruction cache", 2),
        ],
        vec![("flush instruction cache", 1), ("protect memory", 5)],
    ] {
        let memory = Allocation::new();
        let injection = Inject::new(&failures);
        let result = std::panic::catch_unwind(|| {
            // SAFETY: Only this test uses the mapped pages. The test abort handler
            // panics so the allocation can be freed afterward.
            unsafe {
                replace(
                    memory.address + memory.page_size - 2,
                    &[0x90; 4],
                    &[0xcc; 4],
                    fatal_for_test,
                )
            }
        });
        drop(injection);
        assert!(result.is_err(), "{failures:?}");
    }
}

#[cfg(target_os = "windows")]
#[test]
fn windows_kernel_errors_are_reported() {
    assert!(matches!(
        platform::region_at(usize::MAX),
        Err(Error::Os {
            operation: "query memory",
            ..
        })
    ));
    assert!(matches!(
        platform::read_bytes(1, 1),
        Err(Error::Os {
            operation: "read memory",
            ..
        })
    ));
    assert!(matches!(
        platform::protect(
            PageRange {
                address: 1,
                length: 1,
                protection: RX
            },
            true
        ),
        Err(Error::Os {
            operation: "protect memory",
            ..
        })
    ));
}
