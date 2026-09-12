use super::*;
use crate::memory::tests::Inject;

#[test]
fn staging_failures_leave_the_target_unchanged() {
    let page_size = super::super::page_size().unwrap();
    let bytes = vec![0x90; page_size];
    let target = Image::new(&bytes, (libc::PROT_READ | libc::PROT_EXEC) as u32).unwrap();
    let page = PageRange {
        address: target.address,
        length: page_size,
        protection: (libc::PROT_READ | libc::PROT_EXEC) as u32,
    };
    for operation in [
        "allocate patch",
        "seal patch",
        "protect memory",
        "flush instruction cache",
    ] {
        let injection = Inject::new(&[(operation, 1)]);
        assert!(
            replace_pages(target.address, &[1, 2, 3, 4], &[page], || panic!(
                "rollback failed"
            ))
            .is_err()
        );
        drop(injection);
        assert_eq!(read_bytes(target.address, 4).unwrap(), [0x90; 4]);
    }
    replace_pages(target.address, &[1, 2, 3, 4], &[page], || {
        panic!("rollback failed")
    })
    .unwrap();
    assert_eq!(read_bytes(target.address, 4).unwrap(), [1, 2, 3, 4]);
}

#[test]
fn failed_rollback_uses_the_fatal_policy() {
    let size = super::super::page_size().unwrap();
    let target = Image::new(&vec![0x90; size], 5).unwrap();
    let page = PageRange {
        address: target.address,
        length: size,
        protection: 5,
    };
    let injection = Inject::new(&[("flush instruction cache", 1), ("protect memory", 2)]);
    let result = std::panic::catch_unwind(|| {
        replace_pages(target.address, &[1], &[page], || panic!("fatal rollback"))
    });
    drop(injection);
    assert!(result.is_err());
}
