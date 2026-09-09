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

#[cfg(target_os = "linux")]
const RX: u32 = (libc::PROT_READ | libc::PROT_EXEC) as u32;
#[cfg(target_os = "linux")]
const RWX: u32 = (libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC) as u32;
#[cfg(target_os = "linux")]
const RO: u32 = libc::PROT_READ as u32;
#[cfg(target_os = "linux")]
const NONE: u32 = libc::PROT_NONE as u32;
#[cfg(target_os = "windows")]
use windows_sys::Win32::System::Memory::{
    PAGE_EXECUTE_READ as RX, PAGE_EXECUTE_READWRITE as RWX, PAGE_NOACCESS as NONE,
    PAGE_READONLY as RO,
};

impl Allocation {
    fn new() -> Self {
        let page_size = platform::page_size().unwrap();
        #[cfg(target_os = "linux")]
        // SAFETY: This creates a fresh anonymous allocation owned by this fixture.
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
        // SAFETY: This creates a fresh allocation owned by this fixture.
        let address = unsafe {
            use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, VirtualAlloc};
            let pointer = VirtualAlloc(
                std::ptr::null(),
                page_size * 3,
                MEM_COMMIT | MEM_RESERVE,
                RWX,
            );
            assert!(!pointer.is_null());
            pointer as usize
        };
        // SAFETY: All three owned pages are writable and do not alias any Rust object.
        unsafe { std::ptr::write_bytes(address as *mut u8, 0x90, page_size * 3) };
        let allocation = Self { address, page_size };
        allocation.protect(0, RX);
        allocation.protect(2, NONE);
        allocation
    }

    fn protect(&self, page: usize, protection: u32) {
        let address = self.address + page * self.page_size;
        #[cfg(target_os = "linux")]
        // SAFETY: The fixture owns this complete aligned page.
        unsafe {
            assert_eq!(
                libc::mprotect(address as *mut _, self.page_size, protection as i32),
                0
            );
        }
        #[cfg(target_os = "windows")]
        // SAFETY: The fixture owns this complete aligned page and `old` is valid output.
        unsafe {
            let mut old = 0;
            assert_ne!(
                windows_sys::Win32::System::Memory::VirtualProtect(
                    address as *const _,
                    self.page_size,
                    protection,
                    &mut old
                ),
                0
            );
        }
    }

    fn hole(&self, page: usize) {
        let address = self.address + page * self.page_size;
        #[cfg(target_os = "linux")]
        // SAFETY: The removed page belongs exclusively to this fixture.
        unsafe {
            assert_eq!(libc::munmap(address as *mut _, self.page_size), 0);
        }
        #[cfg(target_os = "windows")]
        // SAFETY: The decommitted page belongs exclusively to this fixture.
        unsafe {
            use windows_sys::Win32::System::Memory::{MEM_DECOMMIT, VirtualFree};
            assert_ne!(
                VirtualFree(address as *mut _, self.page_size, MEM_DECOMMIT),
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
        #[cfg(target_os = "linux")]
        // SAFETY: The fixture owns this allocation; munmap permits already-unmapped holes.
        unsafe {
            assert_eq!(libc::munmap(self.address as *mut _, self.page_size * 3), 0);
        }
        #[cfg(target_os = "windows")]
        // SAFETY: The fixture owns this reservation and no references survive it.
        unsafe {
            use windows_sys::Win32::System::Memory::{MEM_RELEASE, VirtualFree};
            assert_ne!(VirtualFree(self.address as *mut _, 0, MEM_RELEASE), 0);
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
        // SAFETY: This fixture owns the surviving mappings and no code executes in them.
        unsafe { write(address, &[0x90; 4], &[0xcc; 4]) },
        Err(Error::InvalidRange)
    ));
}

#[test]
fn replacing_across_pages_restores_each_original_protection() {
    let memory = Allocation::new();
    let address = memory.address + memory.page_size - 2;
    // SAFETY: These fixture mappings are exclusive and never executed concurrently.
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
    // SAFETY: The same exclusive owned mapping remains alive for this restoration.
    unsafe { write(address, &[1, 2, 3, 4, 5, 6], &[0x90; 6]) }.unwrap();
    memory.assert_unchanged(address, 6);
}

#[test]
fn failed_validation_never_changes_memory() {
    let memory = Allocation::new();
    // SAFETY: The owned mapping is exclusive; invalid values must be rejected before access.
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
            // SAFETY: The fixture exclusively owns the mapping; faults occur before mutation.
            unsafe { write(memory.address, &[0x90; 4], &[0xcc; 4]) }.is_err(),
            "{operation}"
        );
        drop(injection);
        memory.assert_unchanged(memory.address, 4);
    }
    #[cfg(target_os = "linux")]
    {
        let injection = Inject::new(&[("page size", 1)]);
        assert!(matches!(
            // SAFETY: The fixture owns this mapping and the failure precedes mutation.
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
        // SAFETY: The fixture exclusively owns both pages and does not execute their contents.
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
            // SAFETY: The fixture owns both pages and no threads execute their contents.
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
    // SAFETY: The fixture owns this writable page, and no code can execute it yet.
    unsafe {
        for offset in [64, 128, 256] {
            *((memory.address + offset) as *mut u8) = 0xc3;
        }
    }
    memory.protect(0, RX);
    platform::flush(memory.address, 257).unwrap();
    let mut session = crate::Session::new().unwrap();
    // SAFETY: These owned NOP/RET entries have the same void ABI, remain mapped,
    // and cannot execute concurrently. Only the first replacement is installed.
    unsafe {
        session.replace_raw(
            (memory.address + 4) as *const (),
            (memory.address + 128) as *const (),
        )
    }
    .unwrap();
    let installed = read(memory.address, 32).unwrap();
    assert!(matches!(
        // SAFETY: Both owned entries remain live and quiescent; the overlapping
        // source must be rejected before a second modification is made.
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

#[inline(never)]
extern "C" fn native_replacement() -> u32 {
    std::hint::black_box(93)
}

#[test]
fn a_generated_cet_function_can_execute_replace_and_restore() {
    let _serial = crate::tests::serial();
    let memory = Allocation::new();
    let mut code = [0x90; 26];
    code[..4].copy_from_slice(&[0xf3, 0x0f, 0x1e, 0xfa]);
    code[20..].copy_from_slice(&[0xb8, 7, 0, 0, 0, 0xc3]);
    // SAFETY: This fixture exclusively owns the target page and does not execute
    // its initial NOP bytes; code remains allocated through the session below.
    unsafe { write(memory.address, &[0x90; 26], &code) }.unwrap();
    // SAFETY: The bytes encode ENDBR64, NOP padding, MOV EAX,7, RET. This is a
    // complete leaf extern C function with no arguments and a u32 return value.
    let source: extern "C" fn() -> u32 = unsafe { std::mem::transmute(memory.address) };
    assert_eq!(source(), 7);
    let mut session = crate::Session::new().unwrap();
    // SAFETY: Both functions have identical ABIs and lifetimes; only this thread
    // has access to the generated entry, and it is idle during installation.
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
            // SAFETY: Fixture ownership satisfies the transaction requirements;
            // the test fatal policy unwinds only so the fixture can release its allocation.
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
