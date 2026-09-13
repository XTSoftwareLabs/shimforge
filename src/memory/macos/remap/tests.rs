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

/// Two page ranges over one mapping, so a write can span both.
fn two_ranges() -> (Image, [PageRange; 2], usize) {
    let size = super::super::page_size().unwrap();
    let protection = (libc::PROT_READ | libc::PROT_EXEC) as u32;
    let target = Image::new(&vec![0x90; size * 2], protection).unwrap();
    let ranges = [
        PageRange {
            address: target.address,
            length: size,
            protection,
        },
        PageRange {
            address: target.address + size,
            length: size,
            protection,
        },
    ];
    (target, ranges, size)
}

#[test]
fn a_failure_on_a_later_range_rolls_back_every_installed_range() {
    let (target, ranges, size) = two_ranges();
    let address = target.address + size - 2;
    // The first range is installed and flushed before the second one fails.
    for failure in [("protect memory", 2), ("flush instruction cache", 2)] {
        let injection = Inject::new(&[failure]);
        let result = replace_pages(address, &[1, 2, 3, 4], &ranges, || {
            panic!("rollback failed")
        });
        drop(injection);
        assert!(result.is_err(), "{failure:?}");
        assert_eq!(read_bytes(address, 4).unwrap(), [0x90; 4], "{failure:?}");
    }
    replace_pages(address, &[1, 2, 3, 4], &ranges, || {
        panic!("rollback failed")
    })
    .unwrap();
    assert_eq!(read_bytes(address, 4).unwrap(), [1, 2, 3, 4]);
}

#[test]
fn failed_rollback_steps_use_the_fatal_policy() {
    let (target, ranges, size) = two_ranges();
    let address = target.address + size - 2;
    for failures in [
        // The rollback reinstall succeeds, but its flush fails.
        vec![
            ("flush instruction cache", 1),
            ("flush instruction cache", 2),
        ],
        // The second range fails; the rollback reaches the first range and fails there.
        vec![("protect memory", 2), ("protect memory", 4)],
    ] {
        let injection = Inject::new(&failures);
        let result = std::panic::catch_unwind(|| {
            replace_pages(address, &[1, 2, 3, 4], &ranges, || panic!("fatal rollback"))
        });
        drop(injection);
        assert!(result.is_err(), "{failures:?}");
    }
}

#[test]
fn a_short_read_never_stages_a_partial_page() {
    let size = super::super::page_size().unwrap();
    let protection = (libc::PROT_READ | libc::PROT_EXEC) as u32;
    let target = Image::new(&vec![0x90; size], protection).unwrap();
    let page = PageRange {
        address: target.address,
        length: size,
        protection,
    };
    let result = stage_and_install(
        target.address,
        &[1, 2, 3, 4],
        &[page],
        || panic!("rollback failed"),
        &mut |address, length| {
            let mut bytes = read_bytes(address, length)?;
            bytes.pop();
            Ok(bytes)
        },
    );
    assert_eq!(result, Err(Error::InvalidRange));
    assert_eq!(read_bytes(target.address, 4).unwrap(), [0x90; 4]);
}

#[test]
fn a_failed_release_is_reported_and_retried_on_drop() {
    let image = Image::new(&[0x90; 4], (libc::PROT_READ | libc::PROT_EXEC) as u32).unwrap();
    let injection = Inject::new(&[("release patch", 1)]);
    assert!(matches!(
        image.release(),
        Err(Error::Os {
            operation: "release patch",
            ..
        })
    ));
    drop(injection);
    // The failed call left the mapping in place. Drop releases it without a fault.
}
