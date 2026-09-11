use super::*;

#[test]
fn lock_prefixes_require_a_supported_memory_update() {
    for bytes in [
        &[0xf0, 0x48, 0xff, 0x05, 0, 0, 0, 0][..],
        &[0xf0, 0x83, 0x00, 1],
        &[0xf0, 0x01, 0x00],
        &[0xf0, 0x87, 0x00],
        &[0xf0, 0xf7, 0x18],
    ] {
        assert_eq!(instruction(bytes).unwrap().0, bytes.len());
    }
    for bytes in [
        &[0xf0, 0x48, 0xff, 0xc0][..],
        &[0xf0, 0x90],
        &[0xf0, 0x83, 0x38, 1],
        &[0xf0, 0xff, 0x10],
    ] {
        assert!(instruction(bytes).is_err());
    }
}

#[test]
fn known_entry_instructions_have_exact_sizes() {
    let cases: &[&[u8]] = &[
        &[0x55],
        &[0x41, 0x54],
        &[0x90],
        &[0x98],
        &[0xc9],
        &[0x48, 0x89, 0xe5],
        &[0x48, 0x83, 0xec, 0x28],
        &[0x48, 0x89, 0x4c, 0x24, 0x08],
        &[0x48, 0x8d, 0x05, 1, 2, 3, 4],
        &[0x48, 0x8b, 0x84, 0x24, 1, 2, 3, 4],
        &[0x48, 0x8b, 0x04, 0x25, 1, 2, 3, 4],
        &[0x48, 0x8b, 0x00],
        &[0x48, 0x8b, 0x04, 0x24],
        &[0x66, 0x81, 0xc0, 1, 2],
        &[0x66, 0x48, 0x81, 0xc0, 1, 2, 3, 4],
        &[0x48, 0x66, 0x81, 0xc0, 1, 2],
        &[0x48, 0xf3, 0x81, 0xc0, 1, 2, 3, 4],
        &[0x48, 0x67, 0x81, 0xc0, 1, 2, 3, 4],
        &[0xb0, 1],
        &[0xb8, 1, 2, 3, 4],
        &[0x66, 0xb8, 1, 2],
        &[0x48, 0xb8, 1, 2, 3, 4, 5, 6, 7, 8],
        &[0xa1, 1, 2, 3, 4, 5, 6, 7, 8],
        &[0x67, 0xa1, 1, 2, 3, 4],
        &[0x68, 1, 2, 3, 4],
        &[0x66, 0x68, 1, 2],
        &[0x66, 0x48, 0x68, 1, 2, 3, 4],
        &[0x6a, 1],
        &[0x04, 1],
        &[0x05, 1, 2, 3, 4],
        &[0x31, 0xc0],
        &[0xa8, 1],
        &[0xa9, 1, 2, 3, 4],
        &[0x69, 0xc0, 1, 2, 3, 4],
        &[0x6b, 0xc0, 1],
        &[0x80, 0xc0, 1],
        &[0xc1, 0xe0, 1],
        &[0xd1, 0xe0],
        &[0x8f, 0x00],
        &[0xc6, 0x00, 1],
        &[0xc7, 0x00, 1, 2, 3, 4],
        &[0xf6, 0xc0, 1],
        &[0xf7, 0xc0, 1, 2, 3, 4],
        &[0xf7, 0xd0],
        &[0xfe, 0xc0],
        &[0xff, 0xc0],
        &[0xff, 0xd0],
        &[0xff, 0xf0],
        &[0xe8, 1, 2, 3, 4],
        &[0x75, 1],
        &[0xe3, 1],
        &[0x0f, 0x1f, 0x00],
        &[0x0f, 0x85, 1, 2, 3, 4],
        &[0x0f, 0xc8],
        &[0xf3, 0x0f, 0x10, 0x00],
        &[0x0f, 0x28, 0x44, 0x24, 0x10],
        &[0x0f, 0xb6, 0x00],
    ];
    for bytes in cases {
        assert_eq!(instruction(bytes), Ok((bytes.len(), false)), "{bytes:x?}");
        for end in 0..bytes.len() {
            assert_eq!(instruction(&bytes[..end]), Err(Error::InvalidInstruction));
        }
    }
}

#[test]
fn control_flow_stops_before_the_next_entry() {
    let cases: &[&[u8]] = &[
        &[0xc3],
        &[0xcb],
        &[0xcc],
        &[0xcf],
        &[0xf1],
        &[0xf4],
        &[0xc2, 1, 2],
        &[0xca, 1, 2],
        &[0xcd, 0x80],
        &[0xeb, 0],
        &[0xe9, 1, 2, 3, 4],
        &[0xff, 0xe0],
        &[0xff, 0x25, 1, 2, 3, 4],
        &[0x0f, 0x0b],
    ];
    for bytes in cases {
        assert_eq!(instruction(bytes), Ok((bytes.len(), true)), "{bytes:x?}");
    }
}

#[test]
fn unsupported_groups_and_prefixes_are_rejected() {
    let cases: &[&[u8]] = &[
        &[0x06],
        &[0xf0, 0x90],
        &[0xc5, 0xf8, 0x77],
        &[0x62, 0, 0, 0],
        &[0xc7, 0xf8, 1, 2, 3, 4],
        &[0x8f, 0xc8],
        &[0xc6, 0xc8, 0],
        &[0xf6, 0xc8, 1],
        &[0xff, 0xd8],
        &[0xff, 0xe8],
        &[0xff, 0xf8],
        &[0xfe, 0xd0],
        &[0x0f, 0xff],
        &[0x0f, 0x1f, 0xc8],
    ];
    for bytes in cases {
        assert_eq!(
            instruction(bytes),
            Err(Error::InvalidInstruction),
            "{bytes:x?}"
        );
    }
    assert_eq!(instruction(&[0x66; 16]), Err(Error::InvalidInstruction));
    let mut longest = [0x66; 15];
    longest[14] = 0x90;
    assert_eq!(instruction(&longest), Ok((15, false)));
}

#[test]
fn every_modrm_form_respects_displacements() {
    for mode in 0..4 {
        for base in 0..8 {
            for sib in 0..8 {
                let mut bytes = vec![0x8b, (mode << 6) | base];
                if mode != 3 && base == 4 {
                    bytes.push(sib);
                }
                let displacement = match mode {
                    0 if base == 5 || (base == 4 && sib == 5) => 4,
                    1 => 1,
                    2 => 4,
                    _ => 0,
                };
                bytes.extend(std::iter::repeat_n(0, displacement));
                assert_eq!(instruction(&bytes), Ok((bytes.len(), false)));
            }
        }
    }
}

#[test]
fn arbitrary_inputs_never_read_past_their_bounds() {
    let mut seed = 7349u64;
    for size in 0..=32 {
        for _ in 0..1000 {
            let mut bytes = vec![0; size];
            for byte in &mut bytes {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                *byte = seed as u8;
            }
            if let Ok((length, _)) = instruction(&bytes) {
                assert!((1..=15.min(size)).contains(&length));
            }
        }
    }
}
