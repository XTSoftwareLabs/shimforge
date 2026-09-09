use iced_x86::{Decoder, DecoderOptions, FlowControl, Mnemonic};

use crate::Error;

pub(crate) const MAX_PREFIX: usize = 32;
const ENDBR64: [u8; 4] = [0xf3, 0x0f, 0x1e, 0xfa];

pub(crate) struct Plan {
    pub offset: usize,
    pub original: Vec<u8>,
    pub replacement: Vec<u8>,
}

pub(crate) fn plan(source: usize, target: usize, bytes: &[u8]) -> Result<Plan, Error> {
    if source == 0 || target == 0 {
        return Err(Error::InvalidAddress);
    }
    if source == target {
        return Err(Error::SameAddress);
    }
    // Preserve the ENDBR64 entry used by CET.
    let offset = if bytes.starts_with(&ENDBR64) { 4 } else { 0 };
    let address = source.checked_add(offset).ok_or(Error::InvalidRange)?;
    let mut replacement = jump(address, target)?;
    let prefix = &bytes[offset..];
    let mut decoder = Decoder::with_ip(64, prefix, address as u64, DecoderOptions::NONE);
    let mut length = 0;
    while length < replacement.len() {
        if !decoder.can_decode() {
            return Err(Error::InsufficientSpace);
        }
        let instruction = decoder.decode();
        if instruction.is_invalid() {
            return Err(Error::InvalidInstruction);
        }
        length += instruction.len();
        // Stop at a return, tail jump, or trap to avoid the next function.
        let terminal = matches!(
            instruction.flow_control(),
            FlowControl::Return
                | FlowControl::UnconditionalBranch
                | FlowControl::IndirectBranch
                | FlowControl::Exception
        ) || matches!(
            instruction.mnemonic(),
            Mnemonic::Int | Mnemonic::Int1 | Mnemonic::Int3
        );
        if terminal && length < replacement.len() {
            return Err(Error::InsufficientSpace);
        }
    }
    replacement.resize(length, 0x90);
    Ok(Plan {
        offset,
        original: prefix[..length].to_vec(),
        replacement,
    })
}

fn jump(source: usize, target: usize) -> Result<Vec<u8>, Error> {
    let next = source.checked_add(5).ok_or(Error::InvalidRange)?;
    let displacement = target as i128 - next as i128;
    if let Ok(relative) = i32::try_from(displacement) {
        let mut bytes = vec![0xe9];
        bytes.extend_from_slice(&relative.to_le_bytes());
        Ok(bytes)
    } else {
        // RIP-relative jump leaves all registers unchanged.
        let mut bytes = vec![0xff, 0x25, 0, 0, 0, 0];
        bytes.extend_from_slice(&(target as u64).to_le_bytes());
        Ok(bytes)
    }
}
