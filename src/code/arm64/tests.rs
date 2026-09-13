use super::*;

fn bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}

#[test]
fn jumps_and_entry_boundaries_are_checked() {
    assert_eq!(MAX_PREFIX, 32);
    let source = 0x1000_0000;
    assert_eq!(jump(source, source + 4).unwrap(), bytes(&[0x14000001]));
    assert_eq!(
        jump(source, source - (1 << 27)).unwrap(),
        bytes(&[0x16000000])
    );
    assert_eq!(jump(source, source + (1 << 27)).unwrap().len(), 16);
    assert_eq!(jump(0, 4), Err(Error::InvalidAddress));
    assert_eq!(jump(4, 0), Err(Error::InvalidAddress));
    assert_eq!(jump(1, 4), Err(Error::InvalidRange));
    assert_eq!(relative(4, 5, 26), Err(Error::InvalidRange));
    assert_eq!(plan(4, 4, &bytes(&[NOP])).err(), Some(Error::SameAddress));
    assert_eq!(plan(4, 8, &[]).err(), Some(Error::InsufficientSpace));
    assert_eq!(
        plan(4, 8, &bytes(&[0])).err(),
        Some(Error::InvalidInstruction)
    );
    assert_eq!(
        plan(4, 1 << 28, &bytes(&[0xd65f03c0, NOP, NOP, NOP])).err(),
        Some(Error::InsufficientSpace)
    );
    let plan = plan(4, 8, &bytes(&[BTI, NOP])).unwrap();
    assert_eq!(plan.offset, 4);
    assert_eq!(plan.original, bytes(&[NOP]));
    assert_eq!(plan.replacement.len(), 4);
}

#[test]
fn plain_instructions_and_addresses_are_relocated() {
    let source = 0x1000_0000;
    let destination = source + 4096;
    for instruction in [
        NOP, 0xd503233f, 0xd65f03c0, 0xd65f0bff, 0xd65f0fff, 0xd61f0200,
    ] {
        let moved = trampoline(source, destination, &bytes(&[instruction])).unwrap();
        assert_eq!(word(&moved[4..]), Ok(instruction));
    }
    for instruction in [0x10000000, 0x90000001, 0x90ffffe2] {
        let moved = trampoline(source, destination, &bytes(&[instruction])).unwrap();
        assert_eq!(moved.len(), 24);
    }
    assert_eq!(
        trampoline(0, destination, &bytes(&[NOP])),
        Err(Error::InvalidAddress)
    );
    assert_eq!(
        trampoline(source, destination, &[]),
        Err(Error::InvalidRange)
    );
    assert_eq!(
        trampoline(source, destination, &[1]),
        Err(Error::InvalidRange)
    );
    assert_eq!(
        trampoline(source, destination, &bytes(&[0])),
        Err(Error::InvalidInstruction)
    );
    assert_eq!(
        trampoline(source, destination, &bytes(&[0xd73f0800])),
        Err(Error::InvalidInstruction)
    );
    assert_eq!(
        trampoline(usize::MAX - 3, destination, &bytes(&[NOP])),
        Err(Error::InvalidRange)
    );
    assert_eq!(add(4, -8), Err(Error::InvalidRange));
}

#[test]
fn branches_and_calls_keep_their_targets() {
    let source = 0x1000_0000;
    let destination = source + 4096;
    for instruction in [
        0x14000000, 0x14000001, 0x54000000, 0x5400000e, 0x34000000, 0xb5000021, 0x36000000,
        0xb7000021,
    ] {
        assert!(trampoline(source, destination, &bytes(&[instruction])).is_ok());
    }
    for instruction in [0x94000010, 0xd63f0200] {
        let moved = trampoline(source, destination, &bytes(&[instruction])).unwrap();
        assert_eq!(moved.len(), 28);
        assert_eq!(word(&moved[4..]).unwrap() & 31, 30);
        assert_eq!(
            trampoline(source, destination, &bytes(&[instruction, NOP])),
            Err(Error::InvalidInstruction)
        );
    }
    assert_eq!(
        trampoline(source, destination, &bytes(&[0xd63f03c0])),
        Err(Error::InvalidInstruction)
    );
    // Consistent conditional branches set bit 4, and relocating them needs a decoder.
    assert_eq!(
        trampoline(source, destination, &bytes(&[0x54000010])),
        Err(Error::InvalidInstruction)
    );
    // A distant page returns through the 16-byte jump.
    let far = source + (1 << 28);
    let moved = trampoline(source, far, &bytes(&[NOP])).unwrap();
    assert_eq!(moved.len(), 24);
    assert_eq!(moved[8..], jump(far + 8, source + 4).unwrap());
    // A relocated branch to code outside the prefix still needs a near page.
    assert_eq!(
        trampoline(source, far, &bytes(&[0x14000100])),
        Err(Error::InvalidRange)
    );
}

#[test]
fn literal_loads_preserve_live_memory_reads() {
    let source = 0x1000_0000;
    for instruction in [0x18000000, 0x58000001, 0x98000002, 0xd8000003, 0x1c000004] {
        assert!(trampoline(source, source + 4096, &bytes(&[instruction])).is_ok());
    }
    for instruction in [0x18000000, 0x58000001, 0x98000002, 0xd8000003] {
        assert!(trampoline(source, source + (1 << 22), &bytes(&[instruction])).is_ok());
    }
    for instruction in [0x1c000004, 0x1800001f] {
        assert_eq!(
            trampoline(source, source + (1 << 22), &bytes(&[instruction])),
            Err(Error::InvalidRange)
        );
    }
}
