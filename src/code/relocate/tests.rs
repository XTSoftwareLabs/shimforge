use super::*;

fn displacement(bytes: &[u8], offset: usize) -> i128 {
    i32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as i128
}

#[test]
fn copies_plain_instructions_and_returns_with_a_direct_jump() {
    let original = [0x55, 0x48, 0x89, 0xe5, 0x48, 0x83, 0xec, 0x28];
    let bytes = trampoline(0x1000, 0x2000, &original).unwrap();
    assert_eq!(&bytes[..4], &ENDBR64);
    assert_eq!(&bytes[4..12], &original);
    assert_eq!(bytes[12], 0xe9);
    assert_eq!(
        0x2000 + bytes.len() as i128 + displacement(&bytes, 13),
        0x1008
    );
    assert_eq!(trampoline(0x2000, 0x1000, &[]).unwrap().len(), 9);
}

#[test]
fn relocates_memory_operands_using_the_whole_instruction() {
    for original in [
        vec![0x55, 0xf0, 0x48, 0xff, 0x05, 0x20, 0, 0, 0],
        vec![0x55, 0x48, 0x8b, 0x05, 0x20, 0, 0, 0],
        vec![0x55, 0x83, 0x3d, 0x20, 0, 0, 0, 9],
        vec![0x55, 0x48, 0x8d, 0x05, 0xe0, 0xff, 0xff, 0xff],
    ] {
        let field = match original[1] {
            0x83 => 3,
            0xf0 => 5,
            _ => 4,
        };
        for destination in [0x1000, 0x3000] {
            let bytes = trampoline(0x2000, destination, &original).unwrap();
            let target = 0x2000 + original.len() as i128 + displacement(&original, field);
            assert_eq!(
                destination as i128 + 4 + original.len() as i128 + displacement(&bytes, field + 4),
                target
            );
        }
    }
}

#[test]
fn absolute_sib_addresses_are_not_relocated() {
    let original = [0x48, 0x8b, 0x04, 0x25, 0x20, 0, 0, 0];
    let bytes = trampoline(0x1000, 0x2000, &original).unwrap();
    assert_eq!(&bytes[4..12], &original);
}

#[test]
fn branches_keep_internal_and_external_targets() {
    for instruction in [
        vec![0xeb, 6],
        vec![0xe9, 6, 0, 0, 0],
        vec![0x75, 6],
        vec![0x0f, 0x85, 6, 0, 0, 0],
        vec![0x2e, 0x75, 6],
    ] {
        let bytes = trampoline(0x1000, 0x2000, &instruction).unwrap();
        let end = bytes.len() - 5;
        assert_eq!(
            0x2000 + end as i128 + displacement(&bytes, end - 4),
            0x1000 + instruction.len() as i128 + 6
        );
    }
    let bytes = trampoline(0x1000, 0x2000, &[0x75, 1, 0x90, 0xeb, 0xfb]).unwrap();
    assert_eq!(0x2000 + 10 + displacement(&bytes, 6), 0x200b);
    assert_eq!(0x2000 + 16 + displacement(&bytes, 12), 0x2004);
    assert_eq!(
        trampoline(0x1000, 0x2000, &[0xeb, 0xff]),
        Err(Error::InvalidInstruction)
    );
    assert_eq!(
        trampoline(1, 0x2000, &[0xeb, 0x80]),
        Err(Error::InvalidRange)
    );
    assert_eq!(
        trampoline(0x1000, usize::MAX - 5, &[0xeb, 0xfe]),
        Err(Error::InvalidRange)
    );
    assert_eq!(
        trampoline(0x1000, 0x1_0000_0000, &[0xeb, 10]),
        Err(Error::InvalidRange)
    );
    let indirect = [0xff, 0x25, 0x20, 0, 0, 0];
    let bytes = trampoline(0x1000, 0x2000, &indirect).unwrap();
    assert_eq!(0x200a + displacement(&bytes, 6), 0x1026);
    assert_eq!(
        &trampoline(0x1000, 0x2000, &[0xff, 0xe0]).unwrap()[4..6],
        &[0xff, 0xe0]
    );
}

#[test]
fn rejects_nonfinal_calls_stack_targets_loops_and_unknown_instructions() {
    let cases: &[&[u8]] = &[
        &[0xe8, 0, 0, 0, 0, 0x90],
        &[0xff, 0xd4],
        &[0xff, 0x54, 0x24, 0x08],
        &[0xe3, 0],
        &[0xcd, 0x80],
        &[0x67, 0x8b, 0x05, 0, 0, 0, 0],
        &[0x06],
        &[0x48, 0x8b, 0x05, 0],
    ];
    for bytes in cases {
        assert_eq!(
            trampoline(0x1000, 0x2000, bytes),
            Err(Error::InvalidInstruction)
        );
    }
}

#[test]
fn final_calls_keep_the_original_return_address() {
    for code in [
        vec![0xe8, 10, 0, 0, 0],
        vec![0xff, 0xd0],
        vec![0xff, 0x15, 0x20, 0, 0, 0],
    ] {
        let bytes = trampoline(0x1000, 0x2000, &code).unwrap();
        assert_eq!(&bytes[4..6], &[0xff, 0x35]);
        let literal = (10 + displacement(&bytes, 6)) as usize;
        assert_eq!(
            u64::from_le_bytes(bytes[literal..literal + 8].try_into().unwrap()),
            0x1000 + code.len() as u64
        );
        assert!(needs_call_bridge(&code).unwrap());
        if code[0] == 0xe8 {
            assert_eq!(bytes[10], 0xe9);
            assert_eq!(0x200f + displacement(&bytes, 11), 0x100f);
        } else if code[1] == 0xd0 {
            assert_eq!(&bytes[10..12], &[0xff, 0xe0]);
        } else {
            assert_eq!(&bytes[10..12], &[0xff, 0x25]);
            assert_eq!(0x2010 + displacement(&bytes, 12), 0x1026);
        }
    }
    assert!(!needs_call_bridge(&[0x90]).unwrap());
    assert!(needs_call_bridge(&[0x06]).is_err());
}

#[test]
fn checks_addresses_and_relative_displacement_ranges() {
    assert_eq!(trampoline(0, 0x1000, &[]), Err(Error::InvalidAddress));
    assert_eq!(trampoline(0x1000, 0, &[]), Err(Error::InvalidAddress));
    assert_eq!(
        trampoline(usize::MAX, 0x1000, &[0x90]),
        Err(Error::InvalidRange)
    );
    assert_eq!(
        trampoline(0x1000, usize::MAX, &[0x90]),
        Err(Error::InvalidRange)
    );
    assert_eq!(
        trampoline(0x1000, usize::MAX - 4, &[]),
        Err(Error::InvalidRange)
    );
    assert_eq!(
        trampoline(0x1000, 0x1_0000_0000, &[]),
        Err(Error::InvalidRange)
    );

    let positive = [0x8b, 0x05, 0xff, 0xff, 0xff, 0x7f];
    let negative = [0x8b, 0x05, 0, 0, 0, 0x80];
    assert_eq!(
        trampoline(usize::MAX - 6, 0x1000, &positive),
        Err(Error::InvalidRange)
    );
    assert_eq!(trampoline(1, 0x1000, &negative), Err(Error::InvalidRange));
    assert_eq!(
        trampoline(0x1000, usize::MAX, &positive),
        Err(Error::InvalidRange)
    );
    assert_eq!(
        trampoline(0x2000, 0x1000, &positive),
        Err(Error::InvalidRange)
    );
    assert_eq!(
        trampoline(0x8000_0000, 0x8000_1000, &negative),
        Err(Error::InvalidRange)
    );
}
