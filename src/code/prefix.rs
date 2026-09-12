use crate::Error;

// Only size known x86-64 entry instructions. Unknown forms are rejected.
pub(super) fn instruction(bytes: &[u8]) -> Result<(usize, bool), Error> {
    let decoded = decode(bytes)?;
    Ok((decoded.size, decoded.terminal))
}

pub(super) struct Instruction {
    pub size: usize,
    pub terminal: bool,
    pub relative_memory: Option<usize>,
    pub cannot_move: bool,
    pub branch: Option<Branch>,
    pub call: Option<Call>,
}

#[derive(Clone, Copy)]
pub(super) enum Call {
    Direct(usize),
    Indirect(usize),
}

pub(super) struct Branch {
    pub displacement: usize,
    pub width: usize,
    pub condition: Option<u8>,
}

pub(super) fn decode(bytes: &[u8]) -> Result<Instruction, Error> {
    let mut reader = Reader {
        bytes,
        offset: 0,
        relative_memory: None,
        memory: false,
    };
    let mut word = false;
    let mut rex = 0;
    let mut address32 = false;
    let mut locked = false;
    let opcode = loop {
        let byte = reader.byte()?;
        match byte {
            0xf0 => {
                locked = true;
                rex = 0;
            }
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
    let mut cannot_move = false;
    let mut branch = None;
    let mut call = None;
    let mut lockable = false;
    match opcode {
        0x50..=0x5f | 0x90..=0x99 | 0x9c | 0x9d | 0xc9 | 0xfc | 0xfd => {}
        0xc3 | 0xcb | 0xcc | 0xcf | 0xf1 | 0xf4 => terminal = true,
        0xc2 | 0xca => {
            reader.skip(2)?;
            terminal = true;
        }
        0xcd => {
            reader.skip(1)?;
            terminal = true;
            cannot_move = true;
        }
        0xe9 | 0xeb => {
            let width = if opcode == 0xe9 { 4 } else { 1 };
            branch = Some(Branch {
                displacement: reader.offset,
                width,
                condition: None,
            });
            reader.skip(width)?;
            terminal = true;
        }
        0xe8 => {
            call = Some(Call::Direct(reader.offset));
            reader.skip(4)?;
        }
        0x70..=0x7f => {
            branch = Some(Branch {
                displacement: reader.offset,
                width: 1,
                condition: Some(opcode & 15),
            });
            reader.skip(1)?;
        }
        0xe0..=0xe3 => {
            reader.skip(1)?;
            cannot_move = true;
        }
        0x6a | 0xb0..=0xb7 | 0xa8 => reader.skip(1)?,
        0x68 => reader.skip(immediate)?,
        0xb8..=0xbf => reader.skip(if rex & 8 != 0 { 8 } else { immediate })?,
        0xa0..=0xa3 => reader.skip(if address32 { 4 } else { 8 })?,
        0xa9 => reader.skip(immediate)?,
        0x00..=0x3d if opcode & 7 <= 5 => match opcode & 7 {
            0..=3 => {
                reader.modrm()?;
                lockable = opcode < 0x38 && opcode & 7 <= 1;
            }
            4 => reader.skip(1)?,
            _ => reader.skip(immediate)?,
        },
        0x63 | 0x84..=0x8b | 0x8d | 0xd0..=0xd3 => {
            reader.modrm()?;
            lockable = matches!(opcode, 0x86 | 0x87);
        }
        0x69 | 0x6b | 0x80 | 0x81 | 0x83 | 0xc0 | 0xc1 => {
            let group = reader.modrm()?;
            lockable = matches!(opcode, 0x80 | 0x81 | 0x83) && group != 7;
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
            lockable = matches!(group, 2 | 3);
            if group == 1 {
                return Err(Error::InvalidInstruction);
            }
            if group == 0 {
                reader.skip(if opcode == 0xf6 { 1 } else { immediate })?;
            }
        }
        0xfe | 0xff => {
            let modrm = reader.offset;
            let group = reader.modrm()?;
            lockable = group <= 1;
            if (opcode == 0xfe && group > 1) || matches!(group, 3 | 5 | 7) {
                return Err(Error::InvalidInstruction);
            }
            terminal = opcode == 0xff && group == 4;
            if opcode == 0xff && group == 2 {
                // A pushed return address would change an RSP-based target.
                cannot_move = bytes[modrm] & 7 == 4;
                call = Some(Call::Indirect(modrm));
            }
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
            condition @ 0x80..=0x8f => {
                branch = Some(Branch {
                    displacement: reader.offset,
                    width: 4,
                    condition: Some(condition & 15),
                });
                reader.skip(4)?;
            }
            0xc8..=0xcf => {}
            _ => return Err(Error::InvalidInstruction),
        },
        _ => return Err(Error::InvalidInstruction),
    }
    if locked && !(lockable && reader.memory) {
        return Err(Error::InvalidInstruction);
    }
    Ok(Instruction {
        size: reader.offset,
        terminal,
        relative_memory: reader.relative_memory,
        cannot_move: cannot_move || (address32 && reader.relative_memory.is_some()),
        branch,
        call,
    })
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
    relative_memory: Option<usize>,
    memory: bool,
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
        self.memory = mode != 3;
        let register = (byte >> 3) & 7;
        let base = byte & 7;
        if mode != 3 {
            let sib_base = if base == 4 { self.byte()? & 7 } else { base };
            if mode == 0 && base == 5 {
                self.relative_memory = Some(self.offset);
            }
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
#[path = "prefix/tests.rs"]
mod tests;
