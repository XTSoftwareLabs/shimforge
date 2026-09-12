use super::{ENDBR64, prefix};
use crate::Error;

pub(crate) fn trampoline(
    source: usize,
    destination: usize,
    original: &[u8],
) -> Result<Vec<u8>, Error> {
    if source == 0 || destination == 0 {
        return Err(Error::InvalidAddress);
    }
    let continuation = source
        .checked_add(original.len())
        .ok_or(Error::InvalidRange)?;
    let mut output = ENDBR64.to_vec();
    let mut offset = 0;
    let mut positions = Vec::new();
    let mut branches = Vec::new();
    while offset < original.len() {
        let instruction = prefix::decode(&original[offset..])?;
        if instruction.cannot_move {
            return Err(Error::InvalidInstruction);
        }
        let end = offset + instruction.size;
        let start = output.len();
        positions.push((offset, start));
        if instruction.call.is_some() {
            if end != original.len() {
                return Err(Error::InvalidInstruction);
            }
            // Keep the real return address so the original unwind data applies.
            output.extend_from_slice(&[0xff, 0x35, 0, 0, 0, 0]);
        }
        let copied_start = output.len();
        if let Some(branch) = instruction.branch {
            let field = offset + branch.displacement;
            let value = if branch.width == 1 {
                original[field] as i8 as i128
            } else {
                i32::from_le_bytes(original[field..field + 4].try_into().unwrap()) as i128
            };
            let target = source as i128 + end as i128 + value;
            let prefix = branch.displacement
                - if branch.width == 4 && branch.condition.is_some() {
                    2
                } else {
                    1
                };
            output.extend_from_slice(&original[offset..offset + prefix]);
            if let Some(condition) = branch.condition {
                output.extend_from_slice(&[0x0f, 0x80 | condition]);
            } else {
                output.push(0xe9);
            }
            branches.push((output.len(), target));
            output.extend_from_slice(&[0; 4]);
        } else if let Some(prefix::Call::Direct(displacement)) = instruction.call {
            let field = offset + displacement;
            let value = i32::from_le_bytes(original[field..field + 4].try_into().unwrap());
            output.push(0xe9);
            branches.push((output.len(), source as i128 + end as i128 + value as i128));
            output.extend_from_slice(&[0; 4]);
        } else {
            output.extend_from_slice(&original[offset..end]);
            if let Some(prefix::Call::Indirect(modrm)) = instruction.call {
                output[copied_start + modrm] = (output[copied_start + modrm] & !0x38) | 0x20;
            }
        }
        if let Some(displacement) = instruction.relative_memory {
            let field = offset + displacement;
            let value = i32::from_le_bytes(original[field..field + 4].try_into().unwrap());
            let target = source as i128 + end as i128 + value as i128;
            if !(0..=usize::MAX as i128).contains(&target) {
                return Err(Error::InvalidRange);
            }
            let next = destination
                .checked_add(output.len())
                .ok_or(Error::InvalidRange)?;
            let relative = i32::try_from(target - next as i128).map_err(|_| Error::InvalidRange)?;
            let field = copied_start + displacement;
            output[field..field + 4].copy_from_slice(&relative.to_le_bytes());
        }
        if instruction.call.is_some() {
            let displacement = (output.len() - start - 6) as i32;
            output[start + 2..start + 6].copy_from_slice(&displacement.to_le_bytes());
            output.extend_from_slice(&(continuation as u64).to_le_bytes());
        }
        offset = end;
    }
    for (field, target) in branches {
        let target = usize::try_from(target).map_err(|_| Error::InvalidRange)?;
        let target = if (source..continuation).contains(&target) {
            let mapped = positions
                .iter()
                .find(|position| position.0 == target - source)
                .ok_or(Error::InvalidInstruction)?
                .1;
            destination.checked_add(mapped).ok_or(Error::InvalidRange)?
        } else {
            target
        };
        let next = destination
            .checked_add(field + 4)
            .ok_or(Error::InvalidRange)?;
        let relative =
            i32::try_from(target as i128 - next as i128).map_err(|_| Error::InvalidRange)?;
        output[field..field + 4].copy_from_slice(&relative.to_le_bytes());
    }
    let next = destination
        .checked_add(output.len())
        .and_then(|address| address.checked_add(5))
        .ok_or(Error::InvalidRange)?;
    let relative =
        i32::try_from(continuation as i128 - next as i128).map_err(|_| Error::InvalidRange)?;
    output.push(0xe9);
    output.extend_from_slice(&relative.to_le_bytes());
    Ok(output)
}

pub(crate) fn needs_call_bridge(original: &[u8]) -> Result<bool, Error> {
    let mut offset = 0;
    while offset < original.len() {
        let instruction = prefix::decode(&original[offset..])?;
        if instruction.call.is_some() {
            return Ok(true);
        }
        offset += instruction.size;
    }
    Ok(false)
}

#[cfg(test)]
#[path = "relocate/tests.rs"]
mod tests;
