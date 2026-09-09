use crate::Error;

// Only size known x86-64 entry instructions. Unknown forms are rejected.
pub(super) fn instruction(bytes: &[u8]) -> Result<(usize, bool), Error> {
    let mut reader = Reader { bytes, offset: 0 };
    let mut word = false;
    let mut rex = 0;
    let mut address32 = false;
    let opcode = loop {
        let byte = reader.byte()?;
        match byte {
            0x66 => {
                word = true;
                rex = 0;
            }
            0x67 => {
                address32 = true;
                rex = 0;
            }
            0x26 | 0x2e | 0x36 | 0x3e | 0x64 | 0x65 | 0xf2 | 0xf3 => rex = 0,
            0x40..=0x4f => rex = byte,
            _ => break byte,
        }
    };
    let immediate = if word && rex & 8 == 0 { 2 } else { 4 };
    let mut terminal = false;
    match opcode {
        0x50..=0x5f | 0x90..=0x99 | 0x9c | 0x9d | 0xc9 | 0xfc | 0xfd => {}
        0xc3 | 0xcb | 0xcc | 0xcf | 0xf1 | 0xf4 => terminal = true,
        0xc2 | 0xca => {
            reader.skip(2)?;
            terminal = true;
        }
        0xcd | 0xeb => {
            reader.skip(1)?;
            terminal = true;
        }
        0xe9 => {
            reader.skip(4)?;
            terminal = true;
        }
        0xe8 => reader.skip(4)?,
        0x6a | 0x70..=0x7f | 0xb0..=0xb7 | 0xe0..=0xe3 | 0xa8 => reader.skip(1)?,
        0x68 => reader.skip(immediate)?,
        0xb8..=0xbf => reader.skip(if rex & 8 != 0 { 8 } else { immediate })?,
        0xa0..=0xa3 => reader.skip(if address32 { 4 } else { 8 })?,
        0xa9 => reader.skip(immediate)?,
        0x00..=0x3d if opcode & 7 <= 5 => match opcode & 7 {
            0..=3 => {
                reader.modrm()?;
            }
            4 => reader.skip(1)?,
            _ => reader.skip(immediate)?,
        },
        0x63 | 0x84..=0x8b | 0x8d | 0xd0..=0xd3 => {
            reader.modrm()?;
        }
        0x69 | 0x6b | 0x80 | 0x81 | 0x83 | 0xc0 | 0xc1 => {
            reader.modrm()?;
            reader.skip(if matches!(opcode, 0x69 | 0x81) {
                immediate
            } else {
                1
            })?;
        }
        0x8f | 0xc6 | 0xc7 => {
            if reader.modrm()? != 0 {
                return Err(Error::InvalidInstruction);
            }
            match opcode {
                0xc6 => reader.skip(1)?,
                0xc7 => reader.skip(immediate)?,
                _ => {}
            }
        }
        0xf6 | 0xf7 => {
            let group = reader.modrm()?;
            if group == 1 {
                return Err(Error::InvalidInstruction);
            }
            if group == 0 {
                reader.skip(if opcode == 0xf6 { 1 } else { immediate })?;
            }
        }
        0xfe | 0xff => {
            let group = reader.modrm()?;
            if (opcode == 0xfe && group > 1) || matches!(group, 3 | 5 | 7) {
                return Err(Error::InvalidInstruction);
            }
            terminal = opcode == 0xff && group == 4;
        }
        0x0f => match reader.byte()? {
            0x0b => terminal = true,
            0x1f => {
                if reader.modrm()? != 0 {
                    return Err(Error::InvalidInstruction);
                }
            }
            0x10..=0x17
            | 0x28..=0x2f
            | 0x40..=0x4f
            | 0x54..=0x59
            | 0x6e
            | 0x6f
            | 0x7e
            | 0x7f
            | 0x90..=0x9f
            | 0xaf
            | 0xb6
            | 0xb7
            | 0xbe
            | 0xbf
            | 0xef => {
                reader.modrm()?;
            }
            0x80..=0x8f => reader.skip(4)?,
            0xc8..=0xcf => {}
            _ => return Err(Error::InvalidInstruction),
        },
        _ => return Err(Error::InvalidInstruction),
    }
    Ok((reader.offset, terminal))
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8, Error> {
        self.skip(1)?;
        Ok(self.bytes[self.offset - 1])
    }

    fn skip(&mut self, count: usize) -> Result<(), Error> {
        let end = self.offset + count;
        if end > self.bytes.len() || end > 15 {
            return Err(Error::InvalidInstruction);
        }
        self.offset = end;
        Ok(())
    }

    fn modrm(&mut self) -> Result<u8, Error> {
        let byte = self.byte()?;
        let mode = byte >> 6;
        let register = (byte >> 3) & 7;
        let base = byte & 7;
        if mode != 3 {
            let sib_base = if base == 4 { self.byte()? & 7 } else { base };
            let displacement = match mode {
                0 if sib_base == 5 => 4,
                1 => 1,
                2 => 4,
                _ => 0,
            };
            self.skip(displacement)?;
        }
        Ok(register)
    }
}

#[cfg(test)]
mod tests;
