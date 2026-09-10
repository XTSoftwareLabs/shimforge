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
    while offset < original.len() {
        let instruction = prefix::decode(&original[offset..])?;
        // Calls need unwind metadata; branches need a separate layout pass.
        if instruction.cannot_move {
            return Err(Error::InvalidInstruction);
        }
        let end = offset + instruction.size;
        let copied_start = output.len();
        output.extend_from_slice(&original[offset..end]);
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
        offset = end;
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

#[cfg(test)]
mod tests;
