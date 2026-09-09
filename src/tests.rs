use super::*;

static TEST_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn serial() -> MutexGuard<'static, ()> {
    TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn original(value: u64) -> u64 {
    value.wrapping_add(109)
}

fn replacement(value: u64) -> u64 {
    value.wrapping_add(207)
}

#[test]
fn all_errors_are_useful() {
    let cases = [
        (Error::Busy, "another shimforge session is active"),
        (Error::InvalidAddress, "address is not readable"),
        (Error::InvalidRange, "invalid memory range"),
        (
            Error::NotExecutable,
            "address is not readable executable memory",
        ),
        (Error::MemoryChanged, "function bytes changed unexpectedly"),
        (
            Error::SameAddress,
            "source and replacement have the same address",
        ),
        (Error::Overlap, "target overlaps an active replacement"),
        (
            Error::InsufficientSpace,
            "function prefix is too short for a jump",
        ),
        (
            Error::InvalidInstruction,
            "function prefix contains an invalid instruction",
        ),
        (
            Error::Os {
                operation: "probe",
                code: 13,
            },
            "probe failed (OS error 13)",
        ),
        (
            Error::Mapping("missing".into()),
            "cannot inspect memory mapping: missing",
        ),
    ];
    for (error, message) in cases {
        assert_eq!(error.to_string(), message);
        assert_eq!(error.clone(), error);
        assert!(!format!("{error:?}").is_empty());
        assert!(std::error::Error::source(&error).is_none());
    }
}

#[test]
fn poisoned_sessions_recover_after_unwinding() {
    let _serial = serial();
    let panic = std::panic::catch_unwind(|| {
        let mut session = Session::new().unwrap();
        replace!(session, original => replacement, fn(u64) -> u64).unwrap();
        assert_eq!(original(1), 208);
        panic!("simulate failed test");
    });
    assert!(panic.is_err());
    assert_eq!(original(1), 110);
    let _session = Session::new().unwrap();
    assert!(matches!(Session::new(), Err(Error::Busy)));
}

#[test]
fn installation_errors_do_not_change_the_target() {
    let _serial = serial();
    let mut session = Session::new().unwrap();
    assert_eq!(
        replace!(session, original => original, fn(u64) -> u64),
        Err(Error::SameAddress)
    );
    // SAFETY: Address checks reject null before reading memory.
    assert!(unsafe { session.replace_raw(std::ptr::null(), replacement as *const ()) }.is_err());
    // SAFETY: The null replacement is also rejected before access.
    assert!(unsafe { session.replace_raw(original as *const (), std::ptr::null()) }.is_err());
    replace!(session, original => replacement, fn(u64) -> u64).unwrap();
    assert_eq!(
        replace!(session, original => replacement, fn(u64) -> u64),
        Err(Error::Overlap)
    );
    assert_eq!(original(4), 211);
    session.restore().unwrap();
    session.restore().unwrap();
    assert_eq!(original(4), 113);
}

#[test]
fn failed_restoration_keeps_ownership_for_retry() {
    let _serial = serial();
    let mut session = Session::new().unwrap();
    replace!(session, original => replacement, fn(u64) -> u64).unwrap();
    session.patches[0].replacement[0] ^= 1;
    assert_eq!(session.restore(), Err(Error::MemoryChanged));
    assert_eq!(session.patches.len(), 1);
    session.patches[0].replacement[0] ^= 1;
    session.restore().unwrap();
    assert_eq!(original(0), 109);
}

#[test]
fn unrecoverable_drop_selects_the_fatal_policy() {
    fn simulated_abort() -> ! {
        panic!("fatal policy selected")
    }
    finish(Ok(()), simulated_abort);
    assert!(
        std::panic::catch_unwind(|| finish(Err(Error::MemoryChanged), simulated_abort)).is_err()
    );
}

#[test]
fn decoder_preserves_complete_instructions_and_cet() {
    let bytes = [0x55, 0x48, 0x89, 0xe5, 0x48, 0x83, 0xec, 0x20, 0xc3];
    let plan = code::plan(0x1000, 0x2000, &bytes).unwrap();
    assert_eq!(plan.offset, 0);
    assert_eq!(plan.original, bytes[..8]);
    assert_eq!(plan.replacement, [0xe9, 0xfb, 0xf, 0, 0, 0x90, 0x90, 0x90]);
    let mut cet = vec![0xf3, 0x0f, 0x1e, 0xfa];
    cet.extend_from_slice(&bytes);
    let plan = code::plan(0x1000, 0x2000, &cet).unwrap();
    assert_eq!(plan.offset, 4);
    assert_eq!(plan.original, bytes[..8]);
    assert_eq!(&plan.replacement[..5], &[0xe9, 0xf7, 0xf, 0, 0]);
}

#[test]
fn decoder_handles_near_boundaries_and_far_jumps() {
    let source = 0x1_0000_0000usize;
    let bytes = [0x90; 32];
    for displacement in [i32::MIN, -1, 0, i32::MAX] {
        let target = (source as i128 + 5 + displacement as i128) as usize;
        let plan = code::plan(source, target, &bytes).unwrap();
        assert_eq!(plan.original.len(), 5);
        assert_eq!(plan.replacement[0], 0xe9);
        assert_eq!(&plan.replacement[1..], &displacement.to_le_bytes());
    }
    for target in [1, source + 5 + i32::MAX as usize + 1, usize::MAX] {
        let plan = code::plan(source, target, &bytes).unwrap();
        assert_eq!(plan.original.len(), 14);
        assert_eq!(&plan.replacement[..6], &[0xff, 0x25, 0, 0, 0, 0]);
        assert_eq!(&plan.replacement[6..], &(target as u64).to_le_bytes());
    }
}

#[test]
fn decoder_rejects_short_invalid_and_overflowing_entries() {
    for bytes in [
        &[][..],
        &[0x90, 0x90][..],
        &[0xc3][..],
        &[0xeb, 0][..],
        &[0xcc][..],
        &[0xcd, 0x80][..],
        &[0xf1][..],
        &[0x0f, 0x0b][..],
    ] {
        assert!(matches!(
            code::plan(0x1000, 0x2000, bytes),
            Err(Error::InsufficientSpace)
        ));
    }
    assert!(matches!(
        code::plan(1, 2, &[0x0f]),
        Err(Error::InvalidInstruction)
    ));
    assert!(matches!(
        code::plan(0, 2, &[0x90; 32]),
        Err(Error::InvalidAddress)
    ));
    assert!(matches!(
        code::plan(1, 0, &[0x90; 32]),
        Err(Error::InvalidAddress)
    ));
    assert!(matches!(
        code::plan(1, 1, &[0x90; 32]),
        Err(Error::SameAddress)
    ));
    assert!(matches!(
        code::plan(usize::MAX, 1, &[0x90; 32]),
        Err(Error::InvalidRange)
    ));
    assert!(matches!(
        code::plan(usize::MAX, 1, &[0xf3, 0x0f, 0x1e, 0xfa]),
        Err(Error::InvalidRange)
    ));
}
