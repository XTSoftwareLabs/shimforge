use crate::Error;

pub(crate) const MAX_PREFIX: usize = 32;
const NOP: u32 = 0xd503201f;
const BTI: u32 = 0xd503245f;

pub(crate) struct Plan {
    pub offset: usize,
    pub original: Vec<u8>,
    pub replacement: Vec<u8>,
}

fn aligned(address: usize) -> Result<(), Error> {
    if address == 0 {
        Err(Error::InvalidAddress)
    } else if address & 3 != 0 {
        Err(Error::InvalidRange)
    } else {
        Ok(())
    }
}

fn word(bytes: &[u8]) -> Result<u32, Error> {
    Ok(u32::from_le_bytes(
        bytes
            .get(..4)
            .ok_or(Error::InsufficientSpace)?
            .try_into()
            .unwrap(),
    ))
}

fn emit(bytes: &mut Vec<u8>, instruction: u32) {
    bytes.extend_from_slice(&instruction.to_le_bytes());
}

fn relative(source: usize, target: usize, bits: u32) -> Result<u32, Error> {
    let delta = target as i128 - source as i128;
    let value = delta / 4;
    if delta % 4 != 0 || value < -(1i128 << (bits - 1)) || value >= 1i128 << (bits - 1) {
        return Err(Error::InvalidRange);
    }
    Ok((value as u32) & ((1 << bits) - 1))
}

fn signed(value: u32, bits: u32) -> i64 {
    ((value << (32 - bits)) as i32 >> (32 - bits)) as i64
}

fn add(address: usize, delta: i64) -> Result<usize, Error> {
    usize::try_from(address as i128 + delta as i128).map_err(|_| Error::InvalidRange)
}

fn constant(bytes: &mut Vec<u8>, register: u32, value: usize) {
    for half in 0..4 {
        let opcode = if half == 0 { 0xd2800000 } else { 0xf2800000 };
        emit(
            bytes,
            opcode | (half << 21) | (((value >> (half * 16)) as u32 & 0xffff) << 5) | register,
        );
    }
}

pub(crate) fn jump(source: usize, target: usize) -> Result<Vec<u8>, Error> {
    aligned(source)?;
    aligned(target)?;
    let mut bytes = Vec::new();
    if let Ok(delta) = relative(source, target, 26) {
        emit(&mut bytes, 0x14000000 | delta);
    } else {
        // x16 is reserved for call veneers by all three platform ABIs.
        emit(&mut bytes, 0x58000050); // ldr x16, +8
        emit(&mut bytes, 0xd61f0200); // br x16
        bytes.extend_from_slice(&(target as u64).to_le_bytes());
    }
    Ok(bytes)
}

fn valid(instruction: u32) -> bool {
    // Unallocated major opcode groups are never copied.
    !matches!((instruction >> 25) & 15, 0 | 1 | 3) && instruction != u32::MAX
}

fn terminal(instruction: u32) -> bool {
    instruction & 0xfc000000 == 0x14000000
        || instruction & 0xfe000000 == 0xd6000000
        || instruction & 0xffe0001f == 0xd4200000
}

pub(crate) fn plan(source: usize, target: usize, bytes: &[u8]) -> Result<Plan, Error> {
    aligned(source)?;
    aligned(target)?;
    if source == target {
        return Err(Error::SameAddress);
    }
    let offset = if word(bytes)? & 0xffffff1f == 0xd503241f {
        4
    } else {
        0
    };
    let address = source.checked_add(offset).ok_or(Error::InvalidRange)?;
    let replacement = jump(address, target)?;
    let prefix = bytes
        .get(offset..offset + replacement.len())
        .ok_or(Error::InsufficientSpace)?;
    for (index, bytes) in prefix.chunks_exact(4).enumerate() {
        let instruction = word(bytes)?;
        if !valid(instruction) {
            return Err(Error::InvalidInstruction);
        }
        if terminal(instruction) && (index + 1) * 4 < prefix.len() {
            return Err(Error::InsufficientSpace);
        }
    }
    Ok(Plan {
        offset,
        original: prefix.to_vec(),
        replacement,
    })
}

pub(crate) fn needs_call_bridge(_original: &[u8]) -> Result<bool, Error> {
    // ARM64 calls restore x30 directly; they do not use the x86 stack bridge.
    Ok(false)
}

pub(crate) fn trampoline(
    source: usize,
    destination: usize,
    original: &[u8],
) -> Result<Vec<u8>, Error> {
    aligned(source)?;
    aligned(destination)?;
    if original.is_empty() || original.len() % 4 != 0 {
        return Err(Error::InvalidRange);
    }
    let continuation = source
        .checked_add(original.len())
        .ok_or(Error::InvalidRange)?;
    let mut output = BTI.to_le_bytes().to_vec();
    let mut positions = Vec::new();
    let mut branches = Vec::new();
    for (index, bytes) in original.chunks_exact(4).enumerate() {
        let instruction = word(bytes)?;
        if !valid(instruction) {
            return Err(Error::InvalidInstruction);
        }
        let offset = index * 4;
        let pc = source + offset;
        positions.push((offset, output.len()));
        if instruction & 0x7c000000 == 0x14000000 {
            let target = add(pc, signed(instruction & 0x03ffffff, 26) * 4)?;
            if instruction & 0x80000000 != 0 {
                if offset + 4 != original.len() {
                    return Err(Error::InvalidInstruction);
                }
                constant(&mut output, 30, continuation);
            }
            branches.push((output.len(), target));
            emit(&mut output, 0x14000000);
        } else if instruction & 0xff000010 == 0x54000000
            || instruction & 0x7e000000 == 0x34000000
            || instruction & 0x7e000000 == 0x36000000
        {
            let test_bit = instruction & 0x7e000000 == 0x36000000;
            let bits = if test_bit { 14 } else { 19 };
            let mask = ((1u32 << bits) - 1) << 5;
            let target = add(pc, signed((instruction & mask) >> 5, bits) * 4)?;
            if instruction & 0xff000010 == 0x54000000 {
                if instruction & 15 < 14 {
                    emit(&mut output, (instruction & !mask) ^ 1 | (2 << 5));
                }
            } else {
                emit(&mut output, (instruction & !mask) ^ (1 << 24) | (2 << 5));
            }
            branches.push((output.len(), target));
            emit(&mut output, 0x14000000);
        } else if instruction & 0x1f000000 == 0x10000000 {
            let immediate = ((instruction >> 5) & 0x7ffff) << 2 | ((instruction >> 29) & 3);
            let delta = signed(immediate, 21);
            let target = if instruction & 0x80000000 != 0 {
                add(pc & !4095, delta * 4096)?
            } else {
                add(pc, delta)?
            };
            constant(&mut output, instruction & 31, target);
        } else if instruction & 0x3b000000 == 0x18000000 {
            let target = add(pc, signed((instruction >> 5) & 0x7ffff, 19) * 4)?;
            let next = destination
                .checked_add(output.len())
                .ok_or(Error::InvalidRange)?;
            if let Ok(delta) = relative(next, target, 19) {
                emit(&mut output, (instruction & !0x00ffffe0) | (delta << 5));
            } else {
                let register = instruction & 31;
                let operation = instruction >> 30;
                if instruction & (1 << 26) != 0 || register == 31 {
                    return Err(Error::InvalidRange);
                }
                if operation == 3 {
                    emit(&mut output, NOP);
                } else {
                    constant(&mut output, register, target);
                    let opcode = [0xb9400000, 0xf9400000, 0xb9800000][operation as usize];
                    emit(&mut output, opcode | (register << 5) | register);
                }
            }
        } else if instruction & 0xfffffc1f == 0xd63f0000 {
            if offset + 4 != original.len() || (instruction >> 5) & 31 == 30 {
                return Err(Error::InvalidInstruction);
            }
            constant(&mut output, 30, continuation);
            emit(&mut output, instruction & !(1 << 21));
        } else {
            if instruction & 0xff000000 == 0x54000000 {
                return Err(Error::InvalidInstruction);
            }
            // Authenticated link branches and unknown branch encodings need a decoder.
            if instruction & 0xfe000000 == 0xd6000000
                && instruction & 0xfffffc1f != 0xd61f0000
                && instruction & 0xfffffc1f != 0xd65f0000
                && !matches!(instruction, 0xd65f0bff | 0xd65f0fff)
            {
                return Err(Error::InvalidInstruction);
            }
            emit(&mut output, instruction);
        }
    }
    for (field, target) in branches {
        let target = if (source..continuation).contains(&target) {
            let position = positions
                .iter()
                .find(|entry| entry.0 == target - source)
                .ok_or(Error::InvalidInstruction)?
                .1;
            destination
                .checked_add(position)
                .ok_or(Error::InvalidRange)?
        } else {
            target
        };
        let pc = destination.checked_add(field).ok_or(Error::InvalidRange)?;
        output[field..field + 4]
            .copy_from_slice(&(0x14000000 | relative(pc, target, 26)?).to_le_bytes());
    }
    let next = destination
        .checked_add(output.len())
        .ok_or(Error::InvalidRange)?;
    emit(&mut output, 0x14000000 | relative(next, continuation, 26)?);
    Ok(output)
}

#[cfg(test)]
mod tests;
